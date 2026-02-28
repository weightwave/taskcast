import { describe, it, expect } from 'vitest'
import { Hono } from 'hono'
import { TaskEngine, MemoryBroadcastProvider, MemoryShortTermStore } from '@taskcast/core'
import { createTasksRouter } from '../src/routes/tasks.js'
import type { AuthContext } from '../src/auth.js'

function makeApp() {
  const store = new MemoryShortTermStore()
  const broadcast = new MemoryBroadcastProvider()
  const engine = new TaskEngine({ shortTerm: store, broadcast })
  const app = new Hono()
  app.use('*', async (c, next) => {
    const auth: AuthContext = { taskIds: '*', scope: ['*'] }
    c.set('auth', auth)
    await next()
  })
  app.route('/tasks', createTasksRouter(engine))
  return { app, engine }
}

describe('POST /tasks', () => {
  it('creates a task and returns 201', async () => {
    const { app } = makeApp()
    const res = await app.request('/tasks', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ params: { prompt: 'hello' }, type: 'llm.chat' }),
    })
    expect(res.status).toBe(201)
    const body = await res.json()
    expect(body.status).toBe('pending')
    expect(body.type).toBe('llm.chat')
    expect(body.params).toEqual({ prompt: 'hello' })
    expect(body.id).toBeTruthy()
  })

  it('creates a task with user-supplied id', async () => {
    const { app } = makeApp()
    const res = await app.request('/tasks', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ id: 'my-custom-id' }),
    })
    expect(res.status).toBe(201)
    const body = await res.json()
    expect(body.id).toBe('my-custom-id')
  })
})

describe('GET /tasks/:taskId', () => {
  it('returns task by id', async () => {
    const { app, engine } = makeApp()
    const task = await engine.createTask({ type: 'test' })
    const res = await app.request(`/tasks/${task.id}`)
    expect(res.status).toBe(200)
    const body = await res.json()
    expect(body.id).toBe(task.id)
    expect(body.status).toBe('pending')
  })

  it('returns 404 for unknown task', async () => {
    const { app } = makeApp()
    const res = await app.request('/tasks/nonexistent')
    expect(res.status).toBe(404)
  })
})

describe('PATCH /tasks/:taskId/status', () => {
  it('transitions task to running', async () => {
    const { app, engine } = makeApp()
    const task = await engine.createTask({})
    const res = await app.request(`/tasks/${task.id}/status`, {
      method: 'PATCH',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ status: 'running' }),
    })
    expect(res.status).toBe(200)
    const body = await res.json()
    expect(body.status).toBe('running')
  })

  it('returns 400 on invalid transition', async () => {
    const { app, engine } = makeApp()
    const task = await engine.createTask({})
    const res = await app.request(`/tasks/${task.id}/status`, {
      method: 'PATCH',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ status: 'completed' }),
    })
    expect(res.status).toBe(400)
  })
})

describe('POST /tasks/:taskId/events', () => {
  it('publishes a single event', async () => {
    const { app, engine } = makeApp()
    const task = await engine.createTask({})
    await engine.transitionTask(task.id, 'running')
    const res = await app.request(`/tasks/${task.id}/events`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ type: 'llm.delta', level: 'info', data: { text: 'hi' } }),
    })
    expect(res.status).toBe(201)
    const body = await res.json()
    expect(body.type).toBe('llm.delta')
  })

  it('publishes a batch of events', async () => {
    const { app, engine } = makeApp()
    const task = await engine.createTask({})
    await engine.transitionTask(task.id, 'running')
    const res = await app.request(`/tasks/${task.id}/events`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify([
        { type: 'a', level: 'info', data: null },
        { type: 'b', level: 'info', data: null },
      ]),
    })
    expect(res.status).toBe(201)
    const body = await res.json()
    expect(body).toHaveLength(2)
  })
})

describe('GET /tasks/:taskId/events/history', () => {
  it('returns all events by default', async () => {
    const { app, engine } = makeApp()
    const task = await engine.createTask({})
    await engine.transitionTask(task.id, 'running')
    await engine.publishEvent(task.id, { type: 'llm.delta', level: 'info', data: { text: 'test' } })

    const res = await app.request(`/tasks/${task.id}/events/history`)
    expect(res.status).toBe(200)
    const body = await res.json()
    expect(Array.isArray(body)).toBe(true)
    expect(body.length).toBeGreaterThan(0)
  })

  it('returns 404 for unknown task', async () => {
    const { app } = makeApp()
    const res = await app.request('/tasks/nonexistent/events/history')
    expect(res.status).toBe(404)
  })

  it('filters by since.index query param', async () => {
    const { app, engine } = makeApp()
    const task = await engine.createTask({})
    await engine.transitionTask(task.id, 'running')
    await engine.publishEvent(task.id, { type: 'a', level: 'info', data: null })
    await engine.publishEvent(task.id, { type: 'b', level: 'info', data: null })

    // since.index=1 means return events with raw index > 1
    // running transition emits taskcast:status (index=0), then 'a' (index=1), then 'b' (index=2)
    const res = await app.request(`/tasks/${task.id}/events/history?since.index=1`)
    expect(res.status).toBe(200)
    const body = await res.json()
    const types = body.map((e: { type: string }) => e.type)
    expect(types).toContain('b')
    expect(types).not.toContain('a')
  })

  it('filters by since.timestamp query param', async () => {
    const { app, engine } = makeApp()
    const task = await engine.createTask({})
    await engine.transitionTask(task.id, 'running')
    await engine.publishEvent(task.id, { type: 'early', level: 'info', data: null })
    await engine.publishEvent(task.id, { type: 'late', level: 'info', data: null })

    // Use timestamp=0 to get all events (all events have timestamp > 0)
    const res = await app.request(`/tasks/${task.id}/events/history?since.timestamp=0`)
    expect(res.status).toBe(200)
    const body = await res.json()
    const types = body.map((e: { type: string }) => e.type)
    // All events should be included since their timestamps are > 0
    expect(types).toContain('early')
    expect(types).toContain('late')
  })

  it('filters by since.id query param', async () => {
    const { app, engine } = makeApp()
    const task = await engine.createTask({})
    await engine.transitionTask(task.id, 'running')
    const evt = await engine.publishEvent(task.id, { type: 'first', level: 'info', data: null })
    await engine.publishEvent(task.id, { type: 'second', level: 'info', data: null })

    const res = await app.request(`/tasks/${task.id}/events/history?since.id=${evt.id}`)
    expect(res.status).toBe(200)
    const body = await res.json()
    const types = body.map((e: { type: string }) => e.type)
    expect(types).toContain('second')
    expect(types).not.toContain('first')
  })
})

describe('PATCH /tasks/:taskId/status - error payload', () => {
  it('transitions task with error payload including code and details', async () => {
    const { app, engine } = makeApp()
    const task = await engine.createTask({})
    await engine.transitionTask(task.id, 'running')
    const res = await app.request(`/tasks/${task.id}/status`, {
      method: 'PATCH',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({
        status: 'failed',
        error: { message: 'Something went wrong', code: 'ERR_OOPS', details: { retryable: true } },
      }),
    })
    expect(res.status).toBe(200)
    const body = await res.json()
    expect(body.status).toBe('failed')
    expect(body.error?.code).toBe('ERR_OOPS')
    expect(body.error?.details).toEqual({ retryable: true })
  })

  it('returns 404 when task not found in transition', async () => {
    const { app } = makeApp()
    const res = await app.request('/tasks/no-such-task/status', {
      method: 'PATCH',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ status: 'running' }),
    })
    expect(res.status).toBe(404)
  })

  it('returns 400 for invalid status schema', async () => {
    const { app, engine } = makeApp()
    const task = await engine.createTask({})
    const res = await app.request(`/tasks/${task.id}/status`, {
      method: 'PATCH',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ status: 'invalid-state' }),
    })
    expect(res.status).toBe(400)
  })
})

describe('POST /tasks/:taskId/events - error handling', () => {
  it('returns 404 when publishing event to nonexistent task', async () => {
    const { app } = makeApp()
    const res = await app.request('/tasks/no-such-task/events', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ type: 'test', level: 'info', data: null }),
    })
    expect(res.status).toBe(404)
  })

  it('returns 400 for invalid event schema', async () => {
    const { app, engine } = makeApp()
    const task = await engine.createTask({})
    const res = await app.request(`/tasks/${task.id}/events`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ type: 'test', level: 'not-a-level', data: null }),
    })
    expect(res.status).toBe(400)
  })

  it('returns 400 when publishing event to task in terminal state', async () => {
    const { app, engine } = makeApp()
    const task = await engine.createTask({})
    await engine.transitionTask(task.id, 'running')
    await engine.transitionTask(task.id, 'completed')
    // Task is completed (terminal), publishing should fail with 400
    const res = await app.request(`/tasks/${task.id}/events`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ type: 'test', level: 'info', data: null }),
    })
    expect(res.status).toBe(400)
  })

  it('publishes event with seriesId and seriesMode', async () => {
    const { app, engine } = makeApp()
    const task = await engine.createTask({})
    await engine.transitionTask(task.id, 'running')
    const res = await app.request(`/tasks/${task.id}/events`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({
        type: 'llm.chunk',
        level: 'info',
        data: { text: 'hello' },
        seriesId: 'series-1',
        seriesMode: 'accumulate',
      }),
    })
    expect(res.status).toBe(201)
    const body = await res.json()
    expect(body.seriesId).toBe('series-1')
    expect(body.seriesMode).toBe('accumulate')
  })
})
