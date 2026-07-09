import { describe, expect, it, vi } from 'vitest'
import { SignJWT } from 'jose'
import { MemoryBroadcastProvider, MemoryShortTermStore, TaskEngine } from '@taskcast/core'
import type { LongTermStore, Task, TaskEvent } from '@taskcast/core'
import { createTaskcastApp } from '../src/index.js'

const JWT_SECRET = 'test-secret-that-is-long-enough'

function makeApp() {
  const engine = new TaskEngine({
    broadcast: new MemoryBroadcastProvider(),
    shortTermStore: new MemoryShortTermStore(),
  })
  const { app } = createTaskcastApp({ engine, auth: { mode: 'none' } })
  return { app, engine }
}

async function makeToken(scope: string[], taskIds: string[] | '*' = '*') {
  return new SignJWT({ scope, taskIds })
    .setProtectedHeader({ alg: 'HS256' })
    .setExpirationTime('1h')
    .sign(new TextEncoder().encode(JWT_SECRET))
}

function makeJwtApp() {
  const engine = new TaskEngine({
    broadcast: new MemoryBroadcastProvider(),
    shortTermStore: new MemoryShortTermStore(),
  })
  const { app } = createTaskcastApp({
    engine,
    auth: { mode: 'jwt', jwt: { algorithm: 'HS256', secret: JWT_SECRET } },
  })
  return { app, engine }
}

function makeAppWithEngine(engine: TaskEngine) {
  const { app } = createTaskcastApp({ engine, auth: { mode: 'none' } })
  return app
}

function makeLongTermStoreWithAccumulatedOnlyHistory(): LongTermStore {
  const task: Task = {
    id: 'task-corrupt',
    status: 'running',
    createdAt: 1000,
    updatedAt: 2000,
  }
  const event: TaskEvent = {
    id: 'event-1',
    taskId: task.id,
    index: 0,
    timestamp: 3000,
    type: 'demo.event',
    level: 'info',
    data: { delta: 'hello world' },
    seriesId: 'series-1',
    seriesMode: 'accumulate',
    seriesAccField: 'delta',
  }

  return {
    saveTask: vi.fn().mockResolvedValue(undefined),
    getTask: vi.fn().mockResolvedValue(task),
    saveEvent: vi.fn().mockResolvedValue(undefined),
    getEvents: vi.fn().mockResolvedValue([event]),
    saveWorkerEvent: vi.fn().mockResolvedValue(undefined),
    getWorkerEvents: vi.fn().mockResolvedValue([]),
  }
}

function makeArchive(taskId = 'task-1') {
  return {
    schema: 'taskcast.taskArchive',
    version: 1,
    exportedAt: 5000,
    task: { id: taskId, status: 'running', createdAt: 1000, updatedAt: 2000 },
    events: [
      {
        id: `${taskId}-event-1`,
        taskId,
        index: 0,
        timestamp: 3000,
        type: 'demo.event',
        level: 'info',
        data: { value: 'hello' },
      },
    ],
  }
}

describe('task archive routes', () => {
  it('exports a TaskArchive with the task id and events', async () => {
    const { app, engine } = makeApp()
    await engine.createTask({ id: 'task-1' })
    await engine.publishEvent('task-1', { type: 'demo.event', level: 'info', data: { value: 'hello' } })

    const res = await app.request('/tasks/task-1/archive')
    expect(res.status).toBe(200)

    const body = await res.json()
    expect(body.schema).toBe('taskcast.taskArchive')
    expect(body.version).toBe(1)
    expect(body.task.id).toBe('task-1')
    expect(body.events).toHaveLength(1)
    expect(body.events[0]).toMatchObject({ taskId: 'task-1', index: 0, type: 'demo.event' })
  })

  it('returns 404 when exporting a missing task', async () => {
    const { app } = makeApp()

    const res = await app.request('/tasks/missing/archive')

    expect(res.status).toBe(404)
  })

  it('exports compacted accumulated long-term history', async () => {
    const engine = new TaskEngine({
      broadcast: new MemoryBroadcastProvider(),
      shortTermStore: new MemoryShortTermStore(),
      longTermStore: makeLongTermStoreWithAccumulatedOnlyHistory(),
    })
    const app = makeAppWithEngine(engine)

    const res = await app.request('/tasks/task-corrupt/archive')

    expect(res.status).toBe(200)
    const body = await res.json()
    expect(body.events).toHaveLength(1)
    expect(body.events[0]).toMatchObject({
      taskId: 'task-corrupt',
      index: 0,
      data: { delta: 'hello world' },
      seriesId: 'series-1',
      seriesMode: 'accumulate',
    })
    expect(body.events[0]).not.toHaveProperty('_accumulatedData')
    expect(body.events[0]).not.toHaveProperty('seriesSnapshot')
  })

  it('imports a valid archive', async () => {
    const { app } = makeApp()

    const res = await app.request('/tasks/import', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ archive: makeArchive('task-1') }),
    })

    expect(res.status).toBe(200)
    await expect(res.json()).resolves.toEqual({
      ok: true,
      taskId: 'task-1',
      eventCount: 1,
      overwritten: false,
    })
  })

  it('returns 409 on import conflict without overwrite', async () => {
    const { app, engine } = makeApp()
    await engine.createTask({ id: 'task-1' })

    const res = await app.request('/tasks/import', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ archive: makeArchive('task-1') }),
    })

    expect(res.status).toBe(409)
  })

  it('imports over an existing task when overwrite is true', async () => {
    const { app, engine } = makeApp()
    await engine.createTask({ id: 'task-1', type: 'old-task' })

    const res = await app.request('/tasks/import', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ archive: makeArchive('task-1'), overwrite: true }),
    })

    expect(res.status).toBe(200)
    await expect(res.json()).resolves.toEqual({
      ok: true,
      taskId: 'task-1',
      eventCount: 1,
      overwritten: true,
    })
    await expect(engine.getEvents('task-1')).resolves.toHaveLength(1)
  })

  it('returns 400 for malformed archive input', async () => {
    const { app } = makeApp()

    const res = await app.request('/tasks/import', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ archive: { schema: 'wrong' } }),
    })

    expect(res.status).toBe(400)
  })

  it('returns 400 when archive events contain seriesSnapshot', async () => {
    const { app } = makeApp()
    const archive = makeArchive('task-1')
    archive.events[0] = { ...archive.events[0], seriesSnapshot: true }

    const res = await app.request('/tasks/import', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ archive }),
    })

    expect(res.status).toBe(400)
  })

  it('returns 400 when archive events contain _accumulatedData', async () => {
    const { app } = makeApp()
    const archive = makeArchive('task-1')
    archive.events[0] = { ...archive.events[0], _accumulatedData: { value: 'hello' } }

    const res = await app.request('/tasks/import', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ archive }),
    })

    expect(res.status).toBe(400)
  })

  it('requires task:manage scope to import archives', async () => {
    const { app } = makeJwtApp()
    const createOnlyToken = await makeToken(['task:create'])
    const manageToken = await makeToken(['task:manage'])

    const denied = await app.request('/tasks/import', {
      method: 'POST',
      headers: {
        Authorization: `Bearer ${createOnlyToken}`,
        'Content-Type': 'application/json',
      },
      body: JSON.stringify({ archive: makeArchive('task-auth') }),
    })
    expect(denied.status).toBe(403)

    const allowed = await app.request('/tasks/import', {
      method: 'POST',
      headers: {
        Authorization: `Bearer ${manageToken}`,
        'Content-Type': 'application/json',
      },
      body: JSON.stringify({ archive: makeArchive('task-auth') }),
    })
    expect(allowed.status).toBe(200)
  })

  it('requires event:history scope to export archives', async () => {
    const { app, engine } = makeJwtApp()
    await engine.createTask({ id: 'task-auth' })
    const subscribeToken = await makeToken(['event:subscribe'])
    const historyToken = await makeToken(['event:history'])

    const denied = await app.request('/tasks/task-auth/archive', {
      headers: { Authorization: `Bearer ${subscribeToken}` },
    })
    expect(denied.status).toBe(403)

    const allowed = await app.request('/tasks/task-auth/archive', {
      headers: { Authorization: `Bearer ${historyToken}` },
    })
    expect(allowed.status).toBe(200)
  })

  it('surfaces unknown import failures as server errors', async () => {
    const consoleError = vi.spyOn(console, 'error').mockImplementation(() => {})
    const app = makeAppWithEngine({
      async importTaskArchive() {
        throw new Error('storage failed')
      },
    } as unknown as TaskEngine)

    try {
      const res = await app.request('/tasks/import', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ archive: makeArchive('task-1') }),
      })

      expect(res.status).toBe(500)
    } finally {
      consoleError.mockRestore()
    }
  })
})
