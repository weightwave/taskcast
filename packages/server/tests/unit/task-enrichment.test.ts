import { describe, it, expect } from 'vitest'
import { Hono } from 'hono'
import { TaskEngine, MemoryBroadcastProvider, MemoryShortTermStore } from '@taskcast/core'
import { createTasksRouter } from '../../src/routes/tasks.js'
import { createSSERouter, getSubscriberCount, createSubscriberCounts } from '../../src/routes/sse.js'
import type { AuthContext } from '../../src/auth.js'

function makeApp() {
  const store = new MemoryShortTermStore()
  const broadcast = new MemoryBroadcastProvider()
  const engine = new TaskEngine({ shortTermStore: store, broadcast })
  const subscriberCounts = createSubscriberCounts()
  const app = new Hono()
  app.use('*', async (c, next) => {
    const auth: AuthContext = { taskIds: '*', scope: ['*'] }
    c.set('auth', auth)
    await next()
  })
  app.route('/tasks', createTasksRouter(engine, subscriberCounts))
  app.route('/tasks', createSSERouter(engine, subscriberCounts))
  return { app, engine, subscriberCounts }
}

describe('GET /tasks/:id — hot and subscriberCount enrichment', () => {
  it('returns hot: true and subscriberCount: 0 for a task with no subscribers', async () => {
    const { app, engine } = makeApp()
    const task = await engine.createTask({ type: 'test' })
    const res = await app.request(`/tasks/${task.id}`)
    expect(res.status).toBe(200)
    const body = await res.json()
    expect(body.hot).toBe(true)
    expect(body.subscriberCount).toBe(0)
  })

  it('still includes original task fields alongside hot/subscriberCount', async () => {
    const { app, engine } = makeApp()
    const task = await engine.createTask({ type: 'enrichment-test', params: { x: 1 } })
    const res = await app.request(`/tasks/${task.id}`)
    expect(res.status).toBe(200)
    const body = await res.json()
    expect(body.id).toBe(task.id)
    expect(body.status).toBe('pending')
    expect(body.type).toBe('enrichment-test')
    expect(body.params).toEqual({ x: 1 })
    expect(body.hot).toBe(true)
    expect(body.subscriberCount).toBe(0)
  })

  it('returns 404 with no enrichment fields for nonexistent task', async () => {
    const { app } = makeApp()
    const res = await app.request('/tasks/nonexistent')
    expect(res.status).toBe(404)
    const body = await res.json()
    expect(body.hot).toBeUndefined()
    expect(body.subscriberCount).toBeUndefined()
  })
})

describe('getSubscriberCount — direct function', () => {
  it('returns 0 for an unknown taskId', () => {
    const counts = createSubscriberCounts()
    expect(getSubscriberCount(counts, 'unknown-task-id')).toBe(0)
  })
})

describe('SSE subscriber tracking — increment/decrement', () => {
  it('subscriberCount increments when SSE is connected and decrements when closed', async () => {
    const { app, engine } = makeApp()
    const task = await engine.createTask({ type: 'sse-test' })
    await engine.transitionTask(task.id, 'running')

    // Before any SSE, subscriberCount should be 0
    const before = await app.request(`/tasks/${task.id}`)
    expect((await before.json()).subscriberCount).toBe(0)

    // Start SSE connection (it will block waiting for events or terminal)
    const ssePromise = app.request(`/tasks/${task.id}/events`)

    // Give the stream a tick to start
    await new Promise((r) => setTimeout(r, 50))

    // Check subscriberCount is now 1
    const during = await app.request(`/tasks/${task.id}`)
    expect((await during.json()).subscriberCount).toBe(1)

    // Transition to terminal to close the SSE stream
    await engine.transitionTask(task.id, 'completed')

    // Wait for the SSE response to complete
    const sseRes = await ssePromise

    // Consume the SSE body to allow cleanup
    await sseRes.text()

    // Give a tick for the decrement to happen
    await new Promise((r) => setTimeout(r, 50))

    // Check subscriberCount is back to 0
    const after = await app.request(`/tasks/${task.id}`)
    expect((await after.json()).subscriberCount).toBe(0)
  })
})
