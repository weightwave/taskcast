import { describe, it, expect } from 'vitest'
import { Hono } from 'hono'
import {
  TaskEngine,
  MemoryBroadcastProvider,
  MemoryShortTermStore,
} from '@taskcast/core'
import { createTasksRouter } from '../../src/routes/tasks.js'
import { createSubscriberCounts } from '../../src/routes/sse.js'
import type { AuthContext } from '../../src/auth.js'

function authMiddleware() {
  return async (c: any, next: any) => {
    c.set('auth', { taskIds: '*', scope: ['*'] } as AuthContext)
    await next()
  }
}

function makeApp(opts?: { seqHoldTimeout?: number }) {
  const store = new MemoryShortTermStore()
  const broadcast = new MemoryBroadcastProvider()
  const engine = new TaskEngine({ shortTermStore: store, broadcast, seqHoldTimeout: opts?.seqHoldTimeout })
  const subscriberCounts = createSubscriberCounts()
  const app = new Hono()
  app.use('*', authMiddleware())
  app.route('/tasks', createTasksRouter(engine, subscriberCounts))
  return { app, engine, store }
}

async function createRunningTask(app: Hono): Promise<string> {
  const createRes = await app.request('/tasks', {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({}),
  })
  const task = await createRes.json() as any
  await app.request(`/tasks/${task.id}/status`, {
    method: 'PATCH',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ status: 'running' }),
  })
  return task.id
}

async function publishEvent(app: Hono, taskId: string, body: Record<string, unknown>) {
  return app.request(`/tasks/${taskId}/events`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(body),
  })
}

describe('HTTP seq ordering', () => {
  it('publishes event with clientId/clientSeq and returns them', async () => {
    const { app } = makeApp()
    const taskId = await createRunningTask(app)

    const res = await publishEvent(app, taskId, {
      type: 'test', level: 'info', data: {}, clientId: 'c1', clientSeq: 0,
    })
    expect(res.status).toBe(201)
    const event = await res.json() as any
    expect(event.clientId).toBe('c1')
    expect(event.clientSeq).toBe(0)
  })

  it('rejects when clientId provided without clientSeq', async () => {
    const { app } = makeApp()
    const taskId = await createRunningTask(app)

    const res = await publishEvent(app, taskId, {
      type: 'test', level: 'info', data: {}, clientId: 'c1',
    })
    expect(res.status).toBe(400)
  })

  it('rejects when clientSeq provided without clientId', async () => {
    const { app } = makeApp()
    const taskId = await createRunningTask(app)

    const res = await publishEvent(app, taskId, {
      type: 'test', level: 'info', data: {}, clientSeq: 0,
    })
    expect(res.status).toBe(400)
  })

  it('returns 409 for stale seq', async () => {
    const { app } = makeApp()
    const taskId = await createRunningTask(app)

    await publishEvent(app, taskId, { type: 'a', level: 'info', data: {}, clientId: 'c1', clientSeq: 0 })
    await publishEvent(app, taskId, { type: 'b', level: 'info', data: {}, clientId: 'c1', clientSeq: 1 })

    const res = await publishEvent(app, taskId, { type: 'c', level: 'info', data: {}, clientId: 'c1', clientSeq: 0 })
    expect(res.status).toBe(409)
    const body = await res.json() as any
    expect(body.error).toBe('seq_stale')
    expect(body.expectedSeq).toBe(2)
    expect(body.receivedSeq).toBe(0)
  })

  it('returns 409 for fast-fail with gap', async () => {
    const { app } = makeApp()
    const taskId = await createRunningTask(app)

    await publishEvent(app, taskId, { type: 'a', level: 'info', data: {}, clientId: 'c1', clientSeq: 0 })

    const res = await publishEvent(app, taskId, {
      type: 'b', level: 'info', data: {}, clientId: 'c1', clientSeq: 5, seqMode: 'fast-fail',
    })
    expect(res.status).toBe(409)
    const body = await res.json() as any
    expect(body.error).toBe('seq_gap')
  })

  it('returns 408 for hold timeout', async () => {
    const { app } = makeApp({ seqHoldTimeout: 50 })
    const taskId = await createRunningTask(app)

    await publishEvent(app, taskId, { type: 'a', level: 'info', data: {}, clientId: 'c1', clientSeq: 0 })

    const res = await publishEvent(app, taskId, { type: 'b', level: 'info', data: {}, clientId: 'c1', clientSeq: 5 })
    expect(res.status).toBe(408)
    const body = await res.json() as any
    expect(body.error).toBe('seq_timeout')
  }, 10_000)

  it('GET /tasks/:taskId/seq/:clientId returns expected seq', async () => {
    const { app } = makeApp()
    const taskId = await createRunningTask(app)

    await publishEvent(app, taskId, { type: 'a', level: 'info', data: {}, clientId: 'c1', clientSeq: 0 })

    const res = await app.request(`/tasks/${taskId}/seq/c1`)
    expect(res.status).toBe(200)
    const body = await res.json() as any
    expect(body.clientId).toBe('c1')
    expect(body.expectedSeq).toBe(1)
  })

  it('GET /tasks/:taskId/seq/:clientId returns 404 for unknown client', async () => {
    const { app } = makeApp()
    const taskId = await createRunningTask(app)

    const res = await app.request(`/tasks/${taskId}/seq/unknown`)
    expect(res.status).toBe(404)
  })

  it('returns 409 for stale duplicate (already-consumed seq)', async () => {
    const { app } = makeApp()
    const taskId = await createRunningTask(app)

    await publishEvent(app, taskId, { type: 'a', level: 'info', data: {}, clientId: 'c1', clientSeq: 0 })

    // Publish same seq again — it's now below expected, so it's stale
    const res = await publishEvent(app, taskId, { type: 'a', level: 'info', data: {}, clientId: 'c1', clientSeq: 0 })
    expect(res.status).toBe(409)
    const body = await res.json() as any
    expect(body.error).toBe('seq_stale')
    expect(body.expectedSeq).toBe(1)
  })


  it('holds out-of-order events and emits them in order', async () => {
    const { app } = makeApp()
    const taskId = await createRunningTask(app)

    // Publish seq 0 first
    await publishEvent(app, taskId, { type: 'a', level: 'info', data: { n: 0 }, clientId: 'c1', clientSeq: 0 })

    // Publish seq 2 (will be held), then seq 1 (fills gap, triggers 2)
    const holdPromise = publishEvent(app, taskId, { type: 'c', level: 'info', data: { n: 2 }, clientId: 'c1', clientSeq: 2 })
    // Small delay to ensure seq 2 registers its slot
    await new Promise((r) => setTimeout(r, 50))
    const res1 = await publishEvent(app, taskId, { type: 'b', level: 'info', data: { n: 1 }, clientId: 'c1', clientSeq: 1 })
    expect(res1.status).toBe(201)

    const res2 = await holdPromise
    expect(res2.status).toBe(201)

    // Verify event ordering via history
    const historyRes = await app.request(`/tasks/${taskId}/events/history`)
    expect(historyRes.status).toBe(200)
    const allEvents = await historyRes.json() as any[]
    // Filter to only user-published events (exclude taskcast:status_changed etc.)
    const userEvents = allEvents.filter((e: any) => e.clientSeq !== undefined)
    const seqs = userEvents.map((e: any) => e.clientSeq)
    expect(seqs).toEqual([0, 1, 2])
    // Index ordering should also be ascending
    const indices = userEvents.map((e: any) => e.index)
    expect(indices).toEqual([...indices].sort((a: number, b: number) => a - b))
  })

  it('publishes events without clientId/clientSeq (backward compatible)', async () => {
    const { app } = makeApp()
    const taskId = await createRunningTask(app)

    const res = await publishEvent(app, taskId, { type: 'a', level: 'info', data: {} })
    expect(res.status).toBe(201)
    const event = await res.json() as any
    expect(event.clientId).toBeUndefined()
    expect(event.clientSeq).toBeUndefined()
  })

  it('independent clientIds do not interfere', async () => {
    const { app } = makeApp()
    const taskId = await createRunningTask(app)

    // Both clients publish seq 0
    const r1 = await publishEvent(app, taskId, { type: 'a', level: 'info', data: {}, clientId: 'c1', clientSeq: 0 })
    const r2 = await publishEvent(app, taskId, { type: 'b', level: 'info', data: {}, clientId: 'c2', clientSeq: 0 })
    expect(r1.status).toBe(201)
    expect(r2.status).toBe(201)

    // Both clients can independently advance
    const r3 = await publishEvent(app, taskId, { type: 'c', level: 'info', data: {}, clientId: 'c1', clientSeq: 1 })
    const r4 = await publishEvent(app, taskId, { type: 'd', level: 'info', data: {}, clientId: 'c2', clientSeq: 1 })
    expect(r3.status).toBe(201)
    expect(r4.status).toBe(201)
  })

  it('rejects negative clientSeq', async () => {
    const { app } = makeApp()
    const taskId = await createRunningTask(app)

    const res = await publishEvent(app, taskId, { type: 'a', level: 'info', data: {}, clientId: 'c1', clientSeq: -1 })
    expect(res.status).toBe(400)
  })

  it('seq state resets after task reaches terminal state', async () => {
    const { app, engine } = makeApp()
    const taskId = await createRunningTask(app)

    await publishEvent(app, taskId, { type: 'a', level: 'info', data: {}, clientId: 'c1', clientSeq: 0 })

    // Verify seq state exists
    const seqBefore = await engine.getExpectedSeq(taskId, 'c1')
    expect(seqBefore).toBe(1)

    // Complete the task
    const transitionRes = await app.request(`/tasks/${taskId}/status`, {
      method: 'PATCH',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ status: 'completed' }),
    })
    expect(transitionRes.status).toBe(200)

    // cleanupSeq is fire-and-forget — flush microtasks
    await new Promise((r) => setTimeout(r, 100))

    // Seq state should be cleaned up (best-effort, fire-and-forget)
    const seqAfter = await engine.getExpectedSeq(taskId, 'c1')
    expect(seqAfter).toBeNull()
  })
})
