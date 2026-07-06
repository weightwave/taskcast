import { describe, expect, it, vi } from 'vitest'
import { MemoryBroadcastProvider, MemoryShortTermStore, TaskEngine } from '@taskcast/core'
import { createTaskcastApp } from '../src/index.js'

function makeApp() {
  const engine = new TaskEngine({
    broadcast: new MemoryBroadcastProvider(),
    shortTermStore: new MemoryShortTermStore(),
  })
  const { app } = createTaskcastApp({ engine, auth: { mode: 'none' } })
  return { app, engine }
}

function makeAppWithEngine(engine: TaskEngine) {
  const { app } = createTaskcastApp({ engine, auth: { mode: 'none' } })
  return app
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
