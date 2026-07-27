import { describe, expect, it } from 'vitest'
import { SignJWT } from 'jose'
import {
  MemoryBroadcastProvider,
  MemoryLongTermStore,
  MemoryShortTermStore,
  TaskEngine,
} from '@taskcast/core'
import { createTaskcastApp } from '../src/index.js'

const JWT_SECRET = 'storage-release-route-test-secret'

function makeReleaseApp(auth: 'none' | 'jwt' = 'none') {
  const shortTermStore = new MemoryShortTermStore()
  const longTermStore = new MemoryLongTermStore()
  const engine = new TaskEngine({
    broadcast: new MemoryBroadcastProvider(),
    shortTermStore,
    longTermStore,
  })
  const taskcast = createTaskcastApp({
    engine,
    shortTermStore,
    auth: auth === 'none'
      ? { mode: 'none' }
      : { mode: 'jwt', jwt: { algorithm: 'HS256', secret: JWT_SECRET } },
  })
  return { ...taskcast, engine, shortTermStore, longTermStore }
}

async function token(scope: string[]) {
  return new SignJWT({ scope, taskIds: '*' })
    .setProtectedHeader({ alg: 'HS256' })
    .setExpirationTime('1h')
    .sign(new TextEncoder().encode(JWT_SECRET))
}

describe('POST /tasks/:taskId/storage/release', () => {
  it('releases hot storage and is idempotent after the task is cold', async () => {
    const { app, engine, stop } = makeReleaseApp()
    await engine.createTask({ id: 'release-me' })
    const event = await engine.publishEvent('release-me', {
      type: 'demo.event',
      level: 'info',
      data: { value: 1 },
    })

    const first = await app.request('/tasks/release-me/storage/release', {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({
        expectedLastEventIndex: event.index,
        inactiveSince: Date.now() + 1_000,
      }),
    })
    expect(first.status).toBe(200)
    expect(await first.json()).toEqual({
      taskId: 'release-me',
      storageState: 'cold',
      archiveWatermark: event.index,
      released: true,
    })

    const second = await app.request('/tasks/release-me/storage/release', {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({
        expectedLastEventIndex: event.index,
        inactiveSince: Date.now() + 1_000,
      }),
    })
    expect(second.status).toBe(200)
    expect(await second.json()).toEqual({
      taskId: 'release-me',
      storageState: 'cold',
      archiveWatermark: event.index,
      released: false,
    })
    stop()
  })

  it('returns stable status and error codes for missing, stale, and unsupported release', async () => {
    const supported = makeReleaseApp()
    const missing = await supported.app.request('/tasks/missing/storage/release', {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ expectedLastEventIndex: -1, inactiveSince: Date.now() }),
    })
    expect(missing.status).toBe(404)

    await supported.engine.createTask({ id: 'stale' })
    await supported.engine.publishEvent('stale', {
      type: 'demo.event',
      level: 'info',
      data: null,
    })
    const stale = await supported.app.request('/tasks/stale/storage/release', {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ expectedLastEventIndex: -1, inactiveSince: Date.now() + 1_000 }),
    })
    expect(stale.status).toBe(409)
    expect(await stale.json()).toMatchObject({ code: 'storage_precondition_failed' })
    expect(await supported.longTermStore.listStorageReleaseRequests(10)).toEqual([])
    supported.stop()

    const unsupportedEngine = new TaskEngine({
      broadcast: new MemoryBroadcastProvider(),
      shortTermStore: new MemoryShortTermStore(),
    })
    const unsupported = createTaskcastApp({
      engine: unsupportedEngine,
      auth: { mode: 'none' },
    })
    await unsupportedEngine.createTask({ id: 'unsupported' })
    const unavailable = await unsupported.app.request('/tasks/unsupported/storage/release', {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ expectedLastEventIndex: -1, inactiveSince: Date.now() + 1_000 }),
    })
    expect(unavailable.status).toBe(503)
    expect(await unavailable.json()).toMatchObject({ code: 'storage_release_unsupported' })
    unsupported.stop()
  })

  it('retains busy requests and blocks release while an old writer is live', async () => {
    const busyApp = makeReleaseApp()
    await busyApp.engine.createTask({ id: 'busy' })
    const lease = await busyApp.shortTermStore.acquireStorageLock(
      'busy',
      'other-lock',
      'other-generation',
      30_000,
    )
    expect(lease).not.toBeNull()
    const busy = await busyApp.app.request('/tasks/busy/storage/release', {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({
        expectedLastEventIndex: -1,
        inactiveSince: Date.now() + 1_000,
      }),
    })
    expect(busy.status).toBe(409)
    expect(await busy.json()).toMatchObject({ code: 'storage_busy' })
    expect(await busyApp.longTermStore.listStorageReleaseRequests(10)).toHaveLength(1)
    await busyApp.shortTermStore.releaseStorageLock(lease!)
    busyApp.stop()

    const oldWriterApp = makeReleaseApp()
    await oldWriterApp.engine.createTask({ id: 'old-writer' })
    await oldWriterApp.shortTermStore.registerStorageWriter({
      instanceId: 'old-writer-instance',
      storageProtocolVersion: 1,
      build: 'legacy',
      expiresAt: 0,
    }, 30_000)
    const blocked = await oldWriterApp.app.request('/tasks/old-writer/storage/release', {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({
        expectedLastEventIndex: -1,
        inactiveSince: Date.now() + 1_000,
      }),
    })
    expect(blocked.status).toBe(503)
    expect(await blocked.json()).toMatchObject({ code: 'storage_unavailable' })
    expect(await oldWriterApp.longTermStore.listStorageReleaseRequests(10)).toEqual([])
    oldWriterApp.stop()
  })

  it('requires task:manage scope and exposes the route in OpenAPI', async () => {
    const { app, engine, stop } = makeReleaseApp('jwt')
    await engine.createTask({ id: 'managed' })
    const deniedToken = await token(['event:subscribe'])
    const denied = await app.request('/tasks/managed/storage/release', {
      method: 'POST',
      headers: {
        authorization: `Bearer ${deniedToken}`,
        'content-type': 'application/json',
      },
      body: JSON.stringify({ expectedLastEventIndex: -1, inactiveSince: Date.now() }),
    })
    expect(denied.status).toBe(403)

    const spec = await (await app.request('/openapi.json')).json()
    expect(spec.paths['/tasks/{taskId}/storage/release']?.post).toBeDefined()
    expect(
      spec.paths['/tasks/{taskId}/storage/release'].post.responses['200']
        .content['application/json'].schema,
    ).toBeDefined()
    stop()
  })
})
