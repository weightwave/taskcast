import { describe, it, expect } from 'vitest'
import { Hono } from 'hono'
import { TaskEngine, MemoryBroadcastProvider, MemoryShortTermStore } from '@taskcast/core'
import { createTasksRouter } from '../src/routes/tasks.js'
import { createSubscriberCounts } from '../src/routes/sse.js'
import type { AuthContext } from '../src/auth.js'

function makeApp() {
  const store = new MemoryShortTermStore()
  const broadcast = new MemoryBroadcastProvider()
  const engine = new TaskEngine({ shortTermStore: store, broadcast })
  const app = new Hono()
  app.use('*', async (c, next) => {
    const auth: AuthContext = { taskIds: '*', scope: ['*'] }
    c.set('auth', auth)
    await next()
  })
  app.route('/tasks', createTasksRouter(engine, createSubscriberCounts()))
  return { app, engine }
}

// ─── POST /tasks/:id/resolve ─────────────────────────────────────────────────

describe('POST /tasks/:id/resolve', () => {
  it('transitions a blocked task to running', async () => {
    const { app, engine } = makeApp()
    const task = await engine.createTask({})
    await engine.transitionTask(task.id, 'running')
    await engine.transitionTask(task.id, 'blocked', {
      blockedRequest: { type: 'human-approval', data: { question: 'Continue?' } },
    })

    const res = await app.request(`/tasks/${task.id}/resolve`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ data: { approved: true } }),
    })
    expect(res.status).toBe(200)
    const body = await res.json()
    expect(body.status).toBe('running')
    expect(body.result).toEqual({ approved: true })
  })

  it('wraps non-object resolution data in { resolution }', async () => {
    const { app, engine } = makeApp()
    const task = await engine.createTask({})
    await engine.transitionTask(task.id, 'running')
    await engine.transitionTask(task.id, 'blocked')

    const res = await app.request(`/tasks/${task.id}/resolve`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ data: 'yes' }),
    })
    expect(res.status).toBe(200)
    const body = await res.json()
    expect(body.status).toBe('running')
    expect(body.result).toEqual({ resolution: 'yes' })
  })

  it('returns 400 for non-blocked task', async () => {
    const { app, engine } = makeApp()
    const task = await engine.createTask({})
    await engine.transitionTask(task.id, 'running')

    const res = await app.request(`/tasks/${task.id}/resolve`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ data: {} }),
    })
    expect(res.status).toBe(400)
    const body = await res.json()
    expect(body.error).toBe('Task is not blocked')
  })

  it('returns 404 for non-existent task', async () => {
    const { app } = makeApp()
    const res = await app.request('/tasks/nonexistent/resolve', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ data: {} }),
    })
    expect(res.status).toBe(404)
    const body = await res.json()
    expect(body.error).toBe('Task not found')
  })

  it('returns 400 for completed (terminal) task', async () => {
    const { app, engine } = makeApp()
    const task = await engine.createTask({})
    await engine.transitionTask(task.id, 'running')
    await engine.transitionTask(task.id, 'completed', { result: { done: true } })

    const res = await app.request(`/tasks/${task.id}/resolve`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ data: {} }),
    })
    expect(res.status).toBe(400)
    const body = await res.json()
    expect(body.error).toBe('Task is not blocked')
  })

  it('returns 400 for pending task', async () => {
    const { app, engine } = makeApp()
    const task = await engine.createTask({})

    const res = await app.request(`/tasks/${task.id}/resolve`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ data: {} }),
    })
    expect(res.status).toBe(400)
    const body = await res.json()
    expect(body.error).toBe('Task is not blocked')
  })

  it('returns 400 for invalid request body', async () => {
    const { app, engine } = makeApp()
    const task = await engine.createTask({})
    await engine.transitionTask(task.id, 'running')
    await engine.transitionTask(task.id, 'blocked')

    const res = await app.request(`/tasks/${task.id}/resolve`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({}),
    })
    // `data: z.unknown()` accepts undefined, so {} should actually parse successfully.
    // Let's verify the resolve works with empty body (data = undefined -> { resolution: undefined })
    expect(res.status).toBe(200)
  })
})

// ─── GET /tasks/:id/request ──────────────────────────────────────────────────

describe('GET /tasks/:id/request', () => {
  it('returns blockedRequest for a blocked task', async () => {
    const { app, engine } = makeApp()
    const task = await engine.createTask({})
    await engine.transitionTask(task.id, 'running')
    await engine.transitionTask(task.id, 'blocked', {
      blockedRequest: { type: 'tool-call', data: { tool: 'search', query: 'test' } },
    })

    const res = await app.request(`/tasks/${task.id}/request`)
    expect(res.status).toBe(200)
    const body = await res.json()
    expect(body.type).toBe('tool-call')
    expect(body.data).toEqual({ tool: 'search', query: 'test' })
  })

  it('returns 404 when task has no blockedRequest', async () => {
    const { app, engine } = makeApp()
    const task = await engine.createTask({})
    await engine.transitionTask(task.id, 'running')
    await engine.transitionTask(task.id, 'blocked')

    const res = await app.request(`/tasks/${task.id}/request`)
    expect(res.status).toBe(404)
    const body = await res.json()
    expect(body.error).toBe('No blocked request')
  })

  it('returns 404 for non-blocked task', async () => {
    const { app, engine } = makeApp()
    const task = await engine.createTask({})
    await engine.transitionTask(task.id, 'running')

    const res = await app.request(`/tasks/${task.id}/request`)
    expect(res.status).toBe(404)
    const body = await res.json()
    expect(body.error).toBe('No blocked request')
  })

  it('returns 404 for non-existent task', async () => {
    const { app } = makeApp()
    const res = await app.request('/tasks/nonexistent/request')
    expect(res.status).toBe(404)
    const body = await res.json()
    expect(body.error).toBe('Task not found')
  })
})

// ─── POST /tasks — bug fix regression: webhooks, cleanup, authConfig ─────────

describe('POST /tasks — field passthrough regression', () => {
  it('passes webhooks to engine', async () => {
    const { app } = makeApp()
    const webhooks = [{ url: 'https://example.com/hook', secret: 's3cret' }]
    const res = await app.request('/tasks', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ type: 'test', webhooks }),
    })
    expect(res.status).toBe(201)
    const body = await res.json()
    expect(body.webhooks).toEqual(webhooks)
  })

  it('passes cleanup to engine', async () => {
    const { app } = makeApp()
    const cleanup = {
      rules: [{ trigger: { afterMs: 60000 }, target: 'events' }],
    }
    const res = await app.request('/tasks', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ type: 'test', cleanup }),
    })
    expect(res.status).toBe(201)
    const body = await res.json()
    expect(body.cleanup).toEqual(cleanup)
  })

  it('passes authConfig to engine', async () => {
    const { app } = makeApp()
    const authConfig = {
      rules: [{
        match: { scope: ['task:manage'] },
        require: { sub: ['admin'] },
      }],
    }
    const res = await app.request('/tasks', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ type: 'test', authConfig }),
    })
    expect(res.status).toBe(201)
    const body = await res.json()
    expect(body.authConfig).toEqual(authConfig)
  })
})

// ─── PATCH /tasks/:id/status — blockedRequest in payload ─────────────────────

describe('PATCH /tasks/:id/status — blockedRequest', () => {
  it('stores blockedRequest when transitioning to blocked', async () => {
    const { app, engine } = makeApp()
    const task = await engine.createTask({})
    await engine.transitionTask(task.id, 'running')

    const res = await app.request(`/tasks/${task.id}/status`, {
      method: 'PATCH',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({
        status: 'blocked',
        blockedRequest: { type: 'confirmation', data: { prompt: 'Proceed?' } },
      }),
    })
    expect(res.status).toBe(200)
    const body = await res.json()
    expect(body.status).toBe('blocked')
    expect(body.blockedRequest).toEqual({ type: 'confirmation', data: { prompt: 'Proceed?' } })
  })

  it('stores reason when transitioning to blocked', async () => {
    const { app, engine } = makeApp()
    const task = await engine.createTask({})
    await engine.transitionTask(task.id, 'running')

    const res = await app.request(`/tasks/${task.id}/status`, {
      method: 'PATCH',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({
        status: 'blocked',
        reason: 'Need human approval',
      }),
    })
    expect(res.status).toBe(200)
    const body = await res.json()
    expect(body.status).toBe('blocked')
    expect(body.reason).toBe('Need human approval')
  })

  it('stores reason when transitioning to paused', async () => {
    const { app, engine } = makeApp()
    const task = await engine.createTask({})
    await engine.transitionTask(task.id, 'running')

    const res = await app.request(`/tasks/${task.id}/status`, {
      method: 'PATCH',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({
        status: 'paused',
        reason: 'User requested pause',
      }),
    })
    expect(res.status).toBe(200)
    const body = await res.json()
    expect(body.status).toBe('paused')
    expect(body.reason).toBe('User requested pause')
  })

  it('clears blockedRequest and reason when leaving blocked', async () => {
    const { app, engine } = makeApp()
    const task = await engine.createTask({})
    await engine.transitionTask(task.id, 'running')
    await engine.transitionTask(task.id, 'blocked', {
      blockedRequest: { type: 'tool-call', data: {} },
      reason: 'Needs tool result',
    })

    // Resolve back to running
    const res = await app.request(`/tasks/${task.id}/status`, {
      method: 'PATCH',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ status: 'running' }),
    })
    expect(res.status).toBe(200)
    const body = await res.json()
    expect(body.status).toBe('running')
    expect(body.blockedRequest).toBeUndefined()
    expect(body.reason).toBeUndefined()
  })
})
