import { describe, it, expect } from 'vitest'
import { Hono } from 'hono'
import { TaskEngine, MemoryBroadcastProvider, MemoryShortTermStore } from '@taskcast/core'
import { createTasksRouter } from '../../src/routes/tasks.js'
import { createSSERouter } from '../../src/routes/sse.js'
import type { AuthContext } from '../../src/auth.js'

function makeApp(authOverride?: Partial<AuthContext>) {
  const store = new MemoryShortTermStore()
  const broadcast = new MemoryBroadcastProvider()
  const engine = new TaskEngine({ shortTermStore: store, broadcast })
  const app = new Hono()
  app.use('*', async (c, next) => {
    c.set('auth', { taskIds: '*', scope: ['*'], ...authOverride } as AuthContext)
    await next()
  })
  app.route('/tasks', createTasksRouter(engine))
  app.route('/tasks', createSSERouter(engine))
  return { app, engine }
}

describe('GET /tasks', () => {
  it('returns empty task list when no tasks exist', async () => {
    const { app } = makeApp()
    const res = await app.request('/tasks')
    expect(res.status).toBe(200)
    const body = await res.json()
    expect(body.tasks).toEqual([])
  })

  it('lists all tasks', async () => {
    const { app, engine } = makeApp()
    await engine.createTask({ type: 'llm.chat' })
    await engine.createTask({ type: 'llm.embed' })

    const res = await app.request('/tasks')
    expect(res.status).toBe(200)
    const body = await res.json()
    expect(body.tasks).toHaveLength(2)
  })

  it('filters tasks by status', async () => {
    const { app, engine } = makeApp()
    const t1 = await engine.createTask({ type: 'a' })
    await engine.createTask({ type: 'b' })
    await engine.transitionTask(t1.id, 'running')

    const res = await app.request('/tasks?status=running')
    expect(res.status).toBe(200)
    const body = await res.json()
    expect(body.tasks).toHaveLength(1)
    expect(body.tasks[0].status).toBe('running')
    expect(body.tasks[0].type).toBe('a')
  })

  it('filters tasks by multiple statuses', async () => {
    const { app, engine } = makeApp()
    const t1 = await engine.createTask({ type: 'a' })
    const t2 = await engine.createTask({ type: 'b' })
    await engine.createTask({ type: 'c' })
    await engine.transitionTask(t1.id, 'running')
    await engine.transitionTask(t2.id, 'running')
    await engine.transitionTask(t2.id, 'completed')

    const res = await app.request('/tasks?status=running,completed')
    expect(res.status).toBe(200)
    const body = await res.json()
    expect(body.tasks).toHaveLength(2)
    const statuses = body.tasks.map((t: { status: string }) => t.status)
    expect(statuses).toContain('running')
    expect(statuses).toContain('completed')
  })

  it('filters tasks by type', async () => {
    const { app, engine } = makeApp()
    await engine.createTask({ type: 'llm.chat' })
    await engine.createTask({ type: 'llm.embed' })

    const res = await app.request('/tasks?type=llm.chat')
    expect(res.status).toBe(200)
    const body = await res.json()
    expect(body.tasks).toHaveLength(1)
    expect(body.tasks[0].type).toBe('llm.chat')
  })

  it('includes hot and subscriberCount in enriched response', async () => {
    const { app, engine } = makeApp()
    await engine.createTask({ type: 'test' })

    const res = await app.request('/tasks')
    expect(res.status).toBe(200)
    const body = await res.json()
    expect(body.tasks).toHaveLength(1)
    expect(body.tasks[0].hot).toBe(true)
    expect(body.tasks[0].subscriberCount).toBe(0)
  })

  it('requires event:subscribe scope', async () => {
    const { app, engine } = makeApp({ scope: ['task:create'] })
    await engine.createTask({ type: 'test' })

    const res = await app.request('/tasks')
    expect(res.status).toBe(403)
  })

  it('returns empty array when filter matches nothing', async () => {
    const { app, engine } = makeApp()
    await engine.createTask({ type: 'test' })

    const res = await app.request('/tasks?status=completed')
    expect(res.status).toBe(200)
    const body = await res.json()
    expect(body.tasks).toEqual([])
  })
})
