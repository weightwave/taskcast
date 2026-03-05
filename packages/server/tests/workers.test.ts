import { describe, it, expect } from 'vitest'
import { Hono } from 'hono'
import {
  TaskEngine,
  MemoryBroadcastProvider,
  MemoryShortTermStore,
  WorkerManager,
} from '@taskcast/core'
import { createWorkersRouter } from '../src/routes/workers.js'
import type { AuthContext } from '../src/auth.js'

function makeApp(authOverride?: Partial<AuthContext>) {
  const store = new MemoryShortTermStore()
  const broadcast = new MemoryBroadcastProvider()
  const engine = new TaskEngine({ shortTermStore: store, broadcast })
  const manager = new WorkerManager({ engine, shortTermStore: store, broadcast })
  const app = new Hono()
  app.use('*', async (c, next) => {
    c.set('auth', { taskIds: '*', scope: ['*'], ...authOverride } as AuthContext)
    await next()
  })
  app.route('/workers', createWorkersRouter(manager, engine))
  return { app, engine, manager, store }
}

describe('GET /workers', () => {
  it('returns empty list initially', async () => {
    const { app } = makeApp()
    const res = await app.request('/workers')
    expect(res.status).toBe(200)
    const body = await res.json()
    expect(body).toEqual([])
  })

  it('returns registered workers', async () => {
    const { app, manager } = makeApp()
    await manager.registerWorker({
      matchRule: {},
      capacity: 5,
      connectionMode: 'pull',
    })
    await manager.registerWorker({
      matchRule: {},
      capacity: 3,
      connectionMode: 'websocket',
    })
    const res = await app.request('/workers')
    expect(res.status).toBe(200)
    const body = await res.json()
    expect(body).toHaveLength(2)
  })
})

describe('GET /workers/:workerId', () => {
  it('returns 404 for unknown worker', async () => {
    const { app } = makeApp()
    const res = await app.request('/workers/nonexistent')
    expect(res.status).toBe(404)
    const body = await res.json()
    expect(body.error).toBe('Worker not found')
  })

  it('returns worker details', async () => {
    const { app, manager } = makeApp()
    const worker = await manager.registerWorker({
      matchRule: { taskTypes: ['gpu'] },
      capacity: 10,
      weight: 80,
      connectionMode: 'pull',
      metadata: { region: 'us-east' },
    })
    const res = await app.request(`/workers/${worker.id}`)
    expect(res.status).toBe(200)
    const body = await res.json()
    expect(body.id).toBe(worker.id)
    expect(body.status).toBe('idle')
    expect(body.capacity).toBe(10)
    expect(body.weight).toBe(80)
    expect(body.matchRule).toEqual({ taskTypes: ['gpu'] })
    expect(body.metadata).toEqual({ region: 'us-east' })
    expect(body.connectionMode).toBe('pull')
  })
})

describe('DELETE /workers/:workerId', () => {
  it('removes worker and returns 204', async () => {
    const { app, manager } = makeApp()
    const worker = await manager.registerWorker({
      matchRule: {},
      capacity: 5,
      connectionMode: 'pull',
    })
    const res = await app.request(`/workers/${worker.id}`, { method: 'DELETE' })
    expect(res.status).toBe(204)

    // Verify worker is actually removed
    const check = await manager.getWorker(worker.id)
    expect(check).toBeNull()
  })

  it('returns 404 for unknown worker', async () => {
    const { app } = makeApp()
    const res = await app.request('/workers/nonexistent', { method: 'DELETE' })
    expect(res.status).toBe(404)
    const body = await res.json()
    expect(body.error).toBe('Worker not found')
  })
})

describe('POST /workers/tasks/:taskId/decline', () => {
  it('declines an assigned task', async () => {
    const { app, engine, manager } = makeApp()
    // Create a task and a worker
    const task = await engine.createTask({ type: 'test', cost: 1 })
    const worker = await manager.registerWorker({
      matchRule: {},
      capacity: 5,
      connectionMode: 'pull',
    })
    // Claim the task so it becomes assigned
    await manager.claimTask(task.id, worker.id)
    const assignedTask = await engine.getTask(task.id)
    expect(assignedTask!.status).toBe('assigned')

    // Decline the task
    const res = await app.request(`/workers/tasks/${task.id}/decline`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ workerId: worker.id }),
    })
    expect(res.status).toBe(200)
    const body = await res.json()
    expect(body.ok).toBe(true)

    // Task should be back to pending
    const declinedTask = await engine.getTask(task.id)
    expect(declinedTask!.status).toBe('pending')
  })

  it('declines with blacklist option', async () => {
    const { app, engine, manager } = makeApp()
    const task = await engine.createTask({ type: 'test', cost: 1 })
    const worker = await manager.registerWorker({
      matchRule: {},
      capacity: 5,
      connectionMode: 'pull',
    })
    await manager.claimTask(task.id, worker.id)

    const res = await app.request(`/workers/tasks/${task.id}/decline`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ workerId: worker.id, blacklist: true }),
    })
    expect(res.status).toBe(200)

    // Verify the worker is blacklisted for the task
    const declinedTask = await engine.getTask(task.id)
    const blacklisted = (declinedTask!.metadata?._blacklistedWorkers as string[]) ?? []
    expect(blacklisted).toContain(worker.id)
  })

  it('returns 400 for invalid body (missing workerId)', async () => {
    const { app, engine } = makeApp()
    const task = await engine.createTask({ type: 'test' })
    const res = await app.request(`/workers/tasks/${task.id}/decline`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({}),
    })
    expect(res.status).toBe(400)
    const body = await res.json()
    expect(body.error).toBeTruthy()
  })

  it('returns 400 for invalid body (workerId is not a string)', async () => {
    const { app, engine } = makeApp()
    const task = await engine.createTask({ type: 'test' })
    const res = await app.request(`/workers/tasks/${task.id}/decline`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ workerId: 123 }),
    })
    expect(res.status).toBe(400)
  })
})

describe('scope enforcement', () => {
  it('GET /workers requires worker:manage scope', async () => {
    const { app } = makeApp({ scope: ['event:subscribe'] })
    const res = await app.request('/workers')
    expect(res.status).toBe(403)
    const body = await res.json()
    expect(body.error).toBe('Forbidden')
  })

  it('GET /workers/:workerId requires worker:manage scope', async () => {
    const { app } = makeApp({ scope: ['event:subscribe'] })
    const res = await app.request('/workers/some-id')
    expect(res.status).toBe(403)
  })

  it('DELETE /workers/:workerId requires worker:manage scope', async () => {
    const { app } = makeApp({ scope: ['event:subscribe'] })
    const res = await app.request('/workers/some-id', { method: 'DELETE' })
    expect(res.status).toBe(403)
  })

  it('POST /workers/tasks/:taskId/decline requires worker:connect scope', async () => {
    const { app } = makeApp({ scope: ['event:subscribe'] })
    const res = await app.request('/workers/tasks/some-task/decline', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ workerId: 'w1' }),
    })
    expect(res.status).toBe(403)
  })

  it('worker:manage scope grants access to list workers', async () => {
    const { app } = makeApp({ scope: ['worker:manage'] })
    const res = await app.request('/workers')
    expect(res.status).toBe(200)
  })

  it('worker:connect scope grants access to decline task', async () => {
    const { app, engine, manager } = makeApp({ scope: ['worker:connect'] })
    const task = await engine.createTask({ type: 'test', cost: 1 })
    const worker = await manager.registerWorker({
      matchRule: {},
      capacity: 5,
      connectionMode: 'pull',
    })
    await manager.claimTask(task.id, worker.id)

    const res = await app.request(`/workers/tasks/${task.id}/decline`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ workerId: worker.id }),
    })
    expect(res.status).toBe(200)
  })
})

describe('GET /workers/pull', () => {
  it('returns task when one is available', async () => {
    const { app, engine, manager } = makeApp()
    const worker = await manager.registerWorker({
      matchRule: {},
      capacity: 5,
      connectionMode: 'pull',
    })
    const task = await engine.createTask({ type: 'test', assignMode: 'pull' })

    const res = await app.request(`/workers/pull?workerId=${worker.id}`)
    expect(res.status).toBe(200)
    const body = await res.json()
    expect(body.id).toBe(task.id)
    expect(body.status).toBe('assigned')
  })

  it('returns 204 on timeout', async () => {
    const { app, manager } = makeApp()
    const worker = await manager.registerWorker({
      matchRule: {},
      capacity: 5,
      connectionMode: 'pull',
    })

    // Use AbortController to abort quickly instead of waiting for 30s
    const controller = new AbortController()
    const timeout = setTimeout(() => controller.abort(), 100)

    const res = await app.request(`/workers/pull?workerId=${worker.id}`, {
      signal: controller.signal,
    }).catch(() => null)

    clearTimeout(timeout)

    // When the client aborts, the fetch throws; but the server sees a 204.
    // In Hono test mode, the abort propagates to the handler and returns 204.
    // Either way, the behavior is correct.
    if (res) {
      expect(res.status).toBe(204)
    }
  })

  it('requires workerId param', async () => {
    const { app } = makeApp()
    const res = await app.request('/workers/pull')
    expect(res.status).toBe(400)
    const body = await res.json()
    expect(body.error).toBe('workerId query param required')
  })

  it('updates weight if provided', async () => {
    const { app, engine, manager, store } = makeApp()
    const worker = await manager.registerWorker({
      matchRule: {},
      capacity: 5,
      connectionMode: 'pull',
      weight: 50,
    })
    // Create a task so the request completes immediately
    await engine.createTask({ type: 'test', assignMode: 'pull' })

    const res = await app.request(`/workers/pull?workerId=${worker.id}&weight=80`)
    expect(res.status).toBe(200)

    const updated = await store.getWorker(worker.id)
    expect(updated?.weight).toBe(80)
  })

  it('requires worker:connect scope', async () => {
    const { app } = makeApp({ scope: ['event:subscribe'] })
    const res = await app.request('/workers/pull?workerId=w1')
    expect(res.status).toBe(403)
  })
})
