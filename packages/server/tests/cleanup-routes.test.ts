import { describe, expect, it } from 'vitest'
import { Hono } from 'hono'
import { TaskEngine, MemoryShortTermStore, MemoryBroadcastProvider } from '@taskcast/core'
import { createTasksRouter } from '../src/routes/tasks.js'
import { createSubscriberCounts } from '../src/routes/sse.js'

function fixture() {
  const engine = new TaskEngine({ shortTermStore: new MemoryShortTermStore(), broadcast: new MemoryBroadcastProvider(), cleanup: { enabled: true, rules: [] } })
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
