import { describe, expect, it, vi } from 'vitest'
import { Hono } from 'hono'
import { TaskEngine, MemoryShortTermStore, MemoryLongTermStore, MemoryBroadcastProvider } from '@taskcast/core'
import { createTasksRouter } from '../src/routes/tasks.js'
import { createSubscriberCounts } from '../src/routes/sse.js'

function fixture() {
  const engine = new TaskEngine({ shortTermStore: new MemoryShortTermStore(), longTermStore: new MemoryLongTermStore(), broadcast: new MemoryBroadcastProvider(), cleanup: { enabled: true, rules: [] } })
  const app = new Hono()
  app.use('*', async (c, next) => { c.set('auth', { taskIds: '*', scope: ['*'] }); await next() })
  app.route('/tasks', createTasksRouter(engine, createSubscriberCounts()))
  return app
}

describe('cleanup creation boundary', () => {
  it('returns 400 for an unsupported policy without creating a task', async () => {
    const app = fixture()
    const response = await app.request('/tasks', { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ id: 'unsupported', cleanup: { rules: [{ target: 'task', trigger: {} }] } }) })
    expect(response.status).toBe(400)
    expect((await app.request('/tasks/unsupported')).status).toBe(404)
  })

  it('generates enrollment markers itself and preserves explicit empty policy', async () => {
    const app = fixture()
    const response = await app.request('/tasks', { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ id: 'empty', cleanup: { rules: [] }, cleanupPolicyVersion: 99, cleanupResolvedAt: 1, historyExpiredAt: 1 }) })
    expect(response.status).toBe(201)
    const task = await response.json()
    expect(task.cleanup).toEqual({ rules: [] })
    expect(task.cleanupPolicyVersion).toBe(1)
    expect(task.cleanupResolvedAt).toBeGreaterThan(1)
    expect(task.historyExpiredAt).toBeUndefined()
  })
})


import { createTaskcastApp } from '../src/index.js'

async function expiredFixture(target: 'events' | 'all' = 'events') {
  const hot = new MemoryShortTermStore()
  const durable = new MemoryLongTermStore()
  const engine = new TaskEngine({ shortTermStore: hot, longTermStore: durable, broadcast: new MemoryBroadcastProvider(), cleanup: { enabled: true, rules: [{ target, trigger: {} }] } })
  await engine.createTask({ id: 'expired' })
  await engine.transitionTask('expired', 'running')
  const original = await engine.transitionTask('expired', 'failed', { error: { code: 'EXPECTED', message: 'test' } })
  const events = await hot.getEvents('expired')
  const archive = await engine.exportTaskArchive('expired')
  expect(await engine.sweepCleanup(100, 1)).toMatchObject({ failed: 0, deletedEvents: 1 })
  // A stale cache cannot override durable expiry or expose the remaining chunk.
  await hot.saveTask(original)
  for (const event of events) await hot.appendEvent('expired', event)
  const { app } = createTaskcastApp({ engine, auth: { mode: 'none' }, cors: true })
  return { app, engine, hot, durable, archive }
}

describe('expired history boundary', () => {
  it('keeps task facts but hides partially deleted history for every old cursor', async () => {
    const { app, engine } = await expiredFixture()
    const task = await (await app.request('/tasks/expired')).json()
    expect(task.status).toBe('failed')
    expect(task.historyExpiredAt).toBeGreaterThan(0)
    for (const query of ['', '?since.id=old', '?since.index=0', '?since.timestamp=1', '?seriesFormat=accumulated']) {
      const response = await app.request('/tasks/expired/events/history' + query)
      expect(response.status).toBe(200)
      expect(response.headers.get('X-Taskcast-History-Expired')).toBe('true')
      expect(await response.json()).toEqual([])
    }
    expect(await engine.getSeriesLatest('expired', 'stale')).toBeNull()
  })

  it('sends expiry before the original terminal done even with an old cursor', async () => {
    const { app } = await expiredFixture()
    for (const query of ['', '?since.id=old', '?since.index=0', '?since.timestamp=1']) {
      const response = await app.request('/tasks/expired/events' + query)
      const body = await response.text()
      expect(body).toContain('event: taskcast.history_expired')
      expect(body).toContain('"taskId":"expired"')
      expect(body).toContain('"reason":"failed"')
      expect(body.indexOf('taskcast.history_expired')).toBeLessThan(body.indexOf('taskcast.done'))
      expect(body).not.toContain('event: taskcast.event')
    }
  })

  it('rejects incomplete archive exports with a stable conflict code', async () => {
    const { app } = await expiredFixture()
    const response = await app.request('/tasks/expired/archive')
    expect(response.status).toBe(409)
    expect(await response.json()).toMatchObject({ code: 'TASKCAST_HISTORY_EXPIRED' })
  })
})


it('checks authentication before revealing retention and exposes the history header to browsers', async () => {
  const { engine, app } = await expiredFixture()
  const { app: secured } = createTaskcastApp({ engine, auth: { mode: 'jwt', jwt: { algorithm: 'HS256', secret: 'cleanup-test-secret' } } })
  const { SignJWT } = await import('jose')
  const token = await new SignJWT({ scope: [], taskIds: '*' }).setProtectedHeader({ alg: 'HS256' }).setExpirationTime('1h').sign(new TextEncoder().encode('cleanup-test-secret'))
  for (const suffix of ['', '/events/history', '/events', '/archive']) {
    expect((await secured.request('/tasks/expired' + suffix)).status).toBe(401)
    expect((await secured.request('/tasks/expired' + suffix, { headers: { Authorization: `Bearer ${token}` } })).status).toBe(403)
  }
  const cors = await app.request('/tasks/expired/events/history', { headers: { Origin: 'https://client.example' } })
  expect(cors.headers.get('Access-Control-Expose-Headers')?.toLowerCase()).toContain('x-taskcast-history-expired')
})

it('returns 404 after whole-task cleanup', async () => {
  const hot = new MemoryShortTermStore()
  const durable = new MemoryLongTermStore()
  const engine = new TaskEngine({ shortTermStore: hot, longTermStore: durable, broadcast: new MemoryBroadcastProvider(), cleanup: { enabled: true, rules: [{ target: 'all', trigger: {} }] } })
  await engine.createTask({ id: 'gone' })
  await engine.transitionTask('gone', 'cancelled')
  expect(await engine.sweepCleanup()).toMatchObject({ completed: 1 })
  const { app } = createTaskcastApp({ engine })
  for (const suffix of ['', '/events/history', '/events', '/archive']) expect((await app.request('/tasks/gone' + suffix)).status).toBe(404)
})

it('discards a history snapshot if expiry begins while it is being read', async () => {
  const hot = new MemoryShortTermStore()
  const durable = new MemoryLongTermStore()
  const engine = new TaskEngine({ shortTermStore: hot, longTermStore: durable, broadcast: new MemoryBroadcastProvider(), cleanup: { enabled: true, rules: [{ target: 'events', trigger: {} }] } })
  await engine.createTask({ id: 'racing' })
  await engine.transitionTask('racing', 'cancelled')
  const read = durable.getEvents.bind(durable)
  let cleanup = true
  vi.spyOn(durable, 'getEvents').mockImplementation(async (...args) => {
    const events = await read(...args)
    if (cleanup) { cleanup = false; await engine.sweepCleanup() }
    return events
  })
  expect(await engine.getEvents('racing')).toEqual([])
  expect((await engine.getTask('racing'))?.historyExpiredAt).toBeGreaterThan(0)
})

it.each(['/events/history', '/events'])('handles a task deleted during replay at %s', async suffix => {
  const engine = new TaskEngine({ shortTermStore: new MemoryShortTermStore(), broadcast: new MemoryBroadcastProvider() })
  await engine.createTask({ id: 'removed-during-read' })
  await engine.transitionTask('removed-during-read', 'cancelled')
  vi.spyOn(engine, 'getEvents').mockImplementation(async () => {
    vi.spyOn(engine, 'getTask').mockResolvedValue(null)
    return []
  })
  const { app } = createTaskcastApp({ engine })
  const response = await app.request('/tasks/removed-during-read' + suffix)
  if (suffix === '/events/history') {
    expect(response.status).toBe(404)
    expect(await response.json()).toEqual({ error: 'Task not found' })
  } else {
    const body = await response.text()
    expect(body).toBe('')
  }
})
