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
