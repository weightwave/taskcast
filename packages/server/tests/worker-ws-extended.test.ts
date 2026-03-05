import { describe, it, expect, vi } from 'vitest'
import { Hono } from 'hono'
import {
  TaskEngine,
  MemoryBroadcastProvider,
  MemoryShortTermStore,
  WorkerManager,
} from '@taskcast/core'
import { WorkerWSHandler } from '../src/routes/worker-ws.js'
import type { WSLike } from '../src/routes/worker-ws.js'
import { createWorkersRouter } from '../src/routes/workers.js'
import type { AuthContext } from '../src/auth.js'

// ─── Test Helpers ────────────────────────────────────────────────────────────

function mockWS(): WSLike & { sent: unknown[]; close: ReturnType<typeof vi.fn> } {
  const sent: unknown[] = []
  return {
    send: vi.fn((data: string) => sent.push(JSON.parse(data))),
    close: vi.fn(),
    sent,
  }
}

function makeEnv() {
  const store = new MemoryShortTermStore()
  const broadcast = new MemoryBroadcastProvider()
  const engine = new TaskEngine({ shortTermStore: store, broadcast })
  const manager = new WorkerManager({ engine, shortTermStore: store, broadcast })
  return { store, broadcast, engine, manager }
}

function makeHandler(env: ReturnType<typeof makeEnv>, auth?: AuthContext) {
  const ws = mockWS()
  const handler = new WorkerWSHandler(env.manager, ws, auth)
  return { ws, handler }
}

function makeApp(authOverride?: Partial<AuthContext>) {
  const env = makeEnv()
  const app = new Hono()
  app.use('*', async (c, next) => {
    c.set('auth', { taskIds: '*', scope: ['*'], ...authOverride } as AuthContext)
    await next()
  })
  app.route('/workers', createWorkersRouter(env.manager, env.engine))
  return { app, ...env }
}

// ─── ws-offer Flow ──────────────────────────────────────────────────────────

describe('ws-offer flow', () => {
  it('complete flow: register -> dispatch -> offer -> accept -> assigned', async () => {
    const env = makeEnv()
    const { ws, handler } = makeHandler(env)

    // 1. Register the worker
    await handler.handleMessage(JSON.stringify({
      type: 'register',
      capacity: 5,
      matchRule: {},
    }))
    const workerId = handler.registeredWorkerId!
    expect(workerId).toBeDefined()

    // 2. Create a pending task
    const task = await env.engine.createTask({ type: 'compute', cost: 1 })
    expect(task.status).toBe('pending')

    // 3. Dispatch finds this worker as best candidate
    const dispatch = await env.manager.dispatchTask(task.id)
    expect(dispatch.matched).toBe(true)
    expect(dispatch.workerId).toBe(workerId)

    // 4. Server offers task to the worker via WS
    ws.sent.length = 0
    handler.offerTask(task)
    expect(ws.sent).toHaveLength(1)
    const offerMsg = ws.sent[0] as Record<string, unknown>
    expect(offerMsg.type).toBe('offer')
    expect(offerMsg.taskId).toBe(task.id)

    // 5. Worker accepts
    ws.sent.length = 0
    await handler.handleMessage(JSON.stringify({ type: 'accept', taskId: task.id }))
    expect(ws.sent).toHaveLength(1)
    const assignedMsg = ws.sent[0] as Record<string, unknown>
    expect(assignedMsg.type).toBe('assigned')
    expect(assignedMsg.taskId).toBe(task.id)

    // 6. Task is now assigned to the worker
    const updatedTask = await env.engine.getTask(task.id)
    expect(updatedTask!.status).toBe('assigned')
    expect(updatedTask!.assignedWorker).toBe(workerId)
  })

  it('decline: worker declines offered task -> task returns to pending -> re-dispatched to another worker', async () => {
    const env = makeEnv()

    // Register worker A (lower weight)
    const { ws: wsA, handler: handlerA } = makeHandler(env)
    await handlerA.handleMessage(JSON.stringify({
      type: 'register',
      workerId: 'worker-a',
      capacity: 5,
      weight: 60,
    }))

    // Register worker B (higher weight)
    const { ws: wsB, handler: handlerB } = makeHandler(env)
    await handlerB.handleMessage(JSON.stringify({
      type: 'register',
      workerId: 'worker-b',
      capacity: 5,
      weight: 80,
    }))

    // Create a task
    const task = await env.engine.createTask({ type: 'test', cost: 1 })

    // Dispatch should pick worker-b (highest weight)
    const dispatch1 = await env.manager.dispatchTask(task.id)
    expect(dispatch1.matched).toBe(true)
    expect(dispatch1.workerId).toBe('worker-b')

    // Offer to worker-b and it accepts (to create an assignment for decline)
    handlerB.offerTask(task)
    await handlerB.handleMessage(JSON.stringify({ type: 'accept', taskId: task.id }))

    // Verify task is assigned to worker-b
    let currentTask = await env.engine.getTask(task.id)
    expect(currentTask!.status).toBe('assigned')
    expect(currentTask!.assignedWorker).toBe('worker-b')

    // Worker-b declines with blacklist so it won't be dispatched again
    wsB.sent.length = 0
    await handlerB.handleMessage(JSON.stringify({
      type: 'decline',
      taskId: task.id,
      blacklist: true,
    }))
    const declineMsg = wsB.sent[0] as Record<string, unknown>
    expect(declineMsg.type).toBe('declined')

    // Task should be back to pending
    currentTask = await env.engine.getTask(task.id)
    expect(currentTask!.status).toBe('pending')

    // Re-dispatch should now pick worker-a (worker-b is blacklisted)
    const dispatch2 = await env.manager.dispatchTask(task.id)
    expect(dispatch2.matched).toBe(true)
    expect(dispatch2.workerId).toBe('worker-a')
  })

  it('multiple workers: best worker (highest weight) gets offer first via dispatch', async () => {
    const env = makeEnv()

    // Register 3 workers with different weights
    const { handler: h1 } = makeHandler(env)
    await h1.handleMessage(JSON.stringify({
      type: 'register',
      workerId: 'low-weight',
      capacity: 10,
      weight: 20,
    }))

    const { handler: h2 } = makeHandler(env)
    await h2.handleMessage(JSON.stringify({
      type: 'register',
      workerId: 'high-weight',
      capacity: 10,
      weight: 90,
    }))

    const { handler: h3 } = makeHandler(env)
    await h3.handleMessage(JSON.stringify({
      type: 'register',
      workerId: 'mid-weight',
      capacity: 10,
      weight: 50,
    }))

    const task = await env.engine.createTask({ type: 'test', cost: 1 })

    // Dispatch should pick highest weight worker
    const dispatch = await env.manager.dispatchTask(task.id)
    expect(dispatch.matched).toBe(true)
    expect(dispatch.workerId).toBe('high-weight')
  })

  it('dispatch prefers more available capacity when weights are equal', async () => {
    const env = makeEnv()

    // Register two workers with same weight but different capacity usage
    const { handler: hFull } = makeHandler(env)
    await hFull.handleMessage(JSON.stringify({
      type: 'register',
      workerId: 'nearly-full',
      capacity: 3,
      weight: 50,
    }))

    const { handler: hEmpty } = makeHandler(env)
    await hEmpty.handleMessage(JSON.stringify({
      type: 'register',
      workerId: 'mostly-empty',
      capacity: 10,
      weight: 50,
    }))

    // Occupy the nearly-full worker with a task
    const preTask = await env.engine.createTask({ type: 'test', cost: 2 })
    await env.manager.claimTask(preTask.id, 'nearly-full')

    // Now dispatch a new task
    const task = await env.engine.createTask({ type: 'test', cost: 1 })
    const dispatch = await env.manager.dispatchTask(task.id)
    expect(dispatch.matched).toBe(true)
    // mostly-empty has 10 available slots, nearly-full has 1, so mostly-empty should win
    expect(dispatch.workerId).toBe('mostly-empty')
  })
})

// ─── ws-race Flow ───────────────────────────────────────────────────────────

describe('ws-race flow', () => {
  it('multiple workers: first to claim wins, others get claimed:false', async () => {
    const env = makeEnv()

    // Register three workers
    const { ws: ws1, handler: h1 } = makeHandler(env)
    await h1.handleMessage(JSON.stringify({
      type: 'register',
      workerId: 'racer-1',
      capacity: 5,
    }))

    const { ws: ws2, handler: h2 } = makeHandler(env)
    await h2.handleMessage(JSON.stringify({
      type: 'register',
      workerId: 'racer-2',
      capacity: 5,
    }))

    const { ws: ws3, handler: h3 } = makeHandler(env)
    await h3.handleMessage(JSON.stringify({
      type: 'register',
      workerId: 'racer-3',
      capacity: 5,
    }))

    // Create a task
    const task = await env.engine.createTask({ type: 'race-test', cost: 1, assignMode: 'ws-race' })

    // Broadcast available to all workers
    h1.broadcastAvailable(task)
    h2.broadcastAvailable(task)
    h3.broadcastAvailable(task)

    // Clear sent messages from broadcast
    ws1.sent.length = 0
    ws2.sent.length = 0
    ws3.sent.length = 0

    // Worker 1 claims first
    await h1.handleMessage(JSON.stringify({ type: 'claim', taskId: task.id }))
    const claim1 = ws1.sent[0] as Record<string, unknown>
    expect(claim1.type).toBe('claimed')
    expect(claim1.taskId).toBe(task.id)
    expect(claim1.success).toBe(true)

    // Worker 2 claims second -- should fail
    await h2.handleMessage(JSON.stringify({ type: 'claim', taskId: task.id }))
    const claim2 = ws2.sent[0] as Record<string, unknown>
    expect(claim2.type).toBe('claimed')
    expect(claim2.taskId).toBe(task.id)
    expect(claim2.success).toBe(false)

    // Worker 3 claims third -- should fail
    await h3.handleMessage(JSON.stringify({ type: 'claim', taskId: task.id }))
    const claim3 = ws3.sent[0] as Record<string, unknown>
    expect(claim3.type).toBe('claimed')
    expect(claim3.taskId).toBe(task.id)
    expect(claim3.success).toBe(false)

    // Verify task is assigned to racer-1
    const updatedTask = await env.engine.getTask(task.id)
    expect(updatedTask!.status).toBe('assigned')
    expect(updatedTask!.assignedWorker).toBe('racer-1')
  })

  it('concurrent claims: all workers race simultaneously, exactly one wins', async () => {
    const env = makeEnv()

    const handlers: { ws: ReturnType<typeof mockWS>; handler: WorkerWSHandler }[] = []
    for (let i = 0; i < 10; i++) {
      const { ws, handler } = makeHandler(env)
      await handler.handleMessage(JSON.stringify({
        type: 'register',
        workerId: `racer-${i}`,
        capacity: 5,
      }))
      handlers.push({ ws, handler })
    }

    const task = await env.engine.createTask({ type: 'race-test', cost: 1, assignMode: 'ws-race' })

    // Clear registration messages
    handlers.forEach(({ ws }) => { ws.sent.length = 0 })

    // All workers claim concurrently
    await Promise.all(
      handlers.map(({ handler }) =>
        handler.handleMessage(JSON.stringify({ type: 'claim', taskId: task.id })),
      ),
    )

    // Exactly one should succeed
    const results = handlers.map(({ ws }) => {
      const msg = ws.sent[0] as Record<string, unknown>
      return msg.success as boolean
    })

    const winners = results.filter((s) => s === true)
    const losers = results.filter((s) => s === false)
    expect(winners).toHaveLength(1)
    expect(losers).toHaveLength(9)

    // Task should be assigned
    const updatedTask = await env.engine.getTask(task.id)
    expect(updatedTask!.status).toBe('assigned')
  })

  it('all workers declining after race -- no one claims, task stays pending', async () => {
    const env = makeEnv()

    // Register two workers, claim with one, then decline
    const { ws: ws1, handler: h1 } = makeHandler(env)
    await h1.handleMessage(JSON.stringify({
      type: 'register',
      workerId: 'decliner-1',
      capacity: 5,
    }))

    const { ws: ws2, handler: h2 } = makeHandler(env)
    await h2.handleMessage(JSON.stringify({
      type: 'register',
      workerId: 'decliner-2',
      capacity: 5,
    }))

    const task = await env.engine.createTask({ type: 'test', cost: 1, assignMode: 'ws-race' })

    // Neither worker claims the task
    // In ws-race, if no one sends a 'claim' message, the task simply stays pending
    const currentTask = await env.engine.getTask(task.id)
    expect(currentTask!.status).toBe('pending')
    expect(currentTask!.assignedWorker).toBeUndefined()
  })
})

// ─── Protocol Edge Cases ────────────────────────────────────────────────────

describe('protocol edge cases', () => {
  it('double register on same connection re-registers with new worker ID', async () => {
    const env = makeEnv()
    const { ws, handler } = makeHandler(env)

    // First registration
    await handler.handleMessage(JSON.stringify({
      type: 'register',
      capacity: 5,
    }))
    const firstId = handler.registeredWorkerId!
    expect(firstId).toBeDefined()
    const firstMsg = ws.sent[0] as Record<string, unknown>
    expect(firstMsg.type).toBe('registered')

    // Second registration (same connection)
    ws.sent.length = 0
    await handler.handleMessage(JSON.stringify({
      type: 'register',
      capacity: 10,
    }))
    const secondId = handler.registeredWorkerId!
    const secondMsg = ws.sent[0] as Record<string, unknown>
    expect(secondMsg.type).toBe('registered')
    // The handler updates its workerId to the new one
    expect(secondId).toBeDefined()
  })

  it('double register with same workerId succeeds', async () => {
    const env = makeEnv()
    const { ws, handler } = makeHandler(env)

    await handler.handleMessage(JSON.stringify({
      type: 'register',
      workerId: 'same-worker',
      capacity: 5,
    }))
    expect(handler.registeredWorkerId).toBe('same-worker')

    ws.sent.length = 0
    await handler.handleMessage(JSON.stringify({
      type: 'register',
      workerId: 'same-worker',
      capacity: 10,
    }))
    const msg = ws.sent[0] as Record<string, unknown>
    expect(msg.type).toBe('registered')
    expect(msg.workerId).toBe('same-worker')
  })

  it('operations after disconnect/cleanup do not crash', async () => {
    const env = makeEnv()
    const { ws, handler } = makeHandler(env)

    // Register and then disconnect
    await handler.handleMessage(JSON.stringify({
      type: 'register',
      workerId: 'temp-worker',
      capacity: 5,
    }))
    await handler.handleDisconnect()

    // Worker should be gone
    const worker = await env.manager.getWorker('temp-worker')
    expect(worker).toBeNull()

    // Try to send messages after disconnect -- handler still has workerId set
    // but operations against the manager should not crash
    ws.sent.length = 0

    // update should silently handle missing worker
    await handler.handleMessage(JSON.stringify({ type: 'update', weight: 99 }))

    // pong (heartbeat) on missing worker should not crash
    await handler.handleMessage(JSON.stringify({ type: 'pong' }))

    // drain on missing worker -- updateWorker returns null but doesn't throw
    await handler.handleMessage(JSON.stringify({ type: 'drain' }))

    // Second disconnect should be safe
    await expect(handler.handleDisconnect()).resolves.toBeUndefined()
  })

  it('accept for non-existent task sends error', async () => {
    const env = makeEnv()
    const { ws, handler } = makeHandler(env)

    await handler.handleMessage(JSON.stringify({ type: 'register', capacity: 5 }))
    ws.sent.length = 0

    await handler.handleMessage(JSON.stringify({
      type: 'accept',
      taskId: 'nonexistent-task-id',
    }))

    expect(ws.sent).toHaveLength(1)
    const msg = ws.sent[0] as Record<string, unknown>
    expect(msg.type).toBe('error')
    expect(msg.taskId).toBe('nonexistent-task-id')
    expect(msg.message).toBeDefined()
  })

  it('claim for non-existent task sends claimed with success:false', async () => {
    const env = makeEnv()
    const { ws, handler } = makeHandler(env)

    await handler.handleMessage(JSON.stringify({ type: 'register', capacity: 5 }))
    ws.sent.length = 0

    await handler.handleMessage(JSON.stringify({
      type: 'claim',
      taskId: 'does-not-exist',
    }))

    expect(ws.sent).toHaveLength(1)
    const msg = ws.sent[0] as Record<string, unknown>
    expect(msg.type).toBe('claimed')
    expect(msg.taskId).toBe('does-not-exist')
    expect(msg.success).toBe(false)
  })

  it('accept for already-completed task sends error', async () => {
    const env = makeEnv()
    const { ws, handler } = makeHandler(env)

    await handler.handleMessage(JSON.stringify({ type: 'register', capacity: 5 }))

    // Create a task, transition to running then completed
    const task = await env.engine.createTask({ type: 'test', cost: 1 })
    await env.engine.transitionTask(task.id, 'running')
    await env.engine.transitionTask(task.id, 'completed', { result: { done: true } })

    ws.sent.length = 0

    await handler.handleMessage(JSON.stringify({ type: 'accept', taskId: task.id }))

    expect(ws.sent).toHaveLength(1)
    const msg = ws.sent[0] as Record<string, unknown>
    expect(msg.type).toBe('error')
    expect(msg.taskId).toBe(task.id)
    expect(msg.message).toContain('not pending')
  })

  it('claim for already-completed task sends claimed with success:false', async () => {
    const env = makeEnv()
    const { ws, handler } = makeHandler(env)

    await handler.handleMessage(JSON.stringify({ type: 'register', capacity: 5 }))

    const task = await env.engine.createTask({ type: 'test', cost: 1 })
    await env.engine.transitionTask(task.id, 'running')
    await env.engine.transitionTask(task.id, 'completed', { result: { done: true } })

    ws.sent.length = 0

    await handler.handleMessage(JSON.stringify({ type: 'claim', taskId: task.id }))

    expect(ws.sent).toHaveLength(1)
    const msg = ws.sent[0] as Record<string, unknown>
    expect(msg.type).toBe('claimed')
    expect(msg.success).toBe(false)
  })

  it('accept without taskId sends error', async () => {
    const env = makeEnv()
    const { ws, handler } = makeHandler(env)

    await handler.handleMessage(JSON.stringify({ type: 'register', capacity: 5 }))
    ws.sent.length = 0

    await handler.handleMessage(JSON.stringify({ type: 'accept' }))

    expect(ws.sent).toHaveLength(1)
    const msg = ws.sent[0] as Record<string, unknown>
    expect(msg.type).toBe('error')
    expect(msg.code).toBe('INVALID_MESSAGE')
  })

  it('claim without taskId sends error', async () => {
    const env = makeEnv()
    const { ws, handler } = makeHandler(env)

    await handler.handleMessage(JSON.stringify({ type: 'register', capacity: 5 }))
    ws.sent.length = 0

    await handler.handleMessage(JSON.stringify({ type: 'claim' }))

    expect(ws.sent).toHaveLength(1)
    const msg = ws.sent[0] as Record<string, unknown>
    expect(msg.type).toBe('error')
    expect(msg.code).toBe('INVALID_MESSAGE')
  })

  it('decline without taskId sends error', async () => {
    const env = makeEnv()
    const { ws, handler } = makeHandler(env)

    await handler.handleMessage(JSON.stringify({ type: 'register', capacity: 5 }))
    ws.sent.length = 0

    await handler.handleMessage(JSON.stringify({ type: 'decline' }))

    expect(ws.sent).toHaveLength(1)
    const msg = ws.sent[0] as Record<string, unknown>
    expect(msg.type).toBe('error')
    expect(msg.code).toBe('INVALID_MESSAGE')
  })

  it('update weight/capacity/matchRule after registration changes take effect', async () => {
    const env = makeEnv()
    const { handler } = makeHandler(env)

    // Register with initial values
    await handler.handleMessage(JSON.stringify({
      type: 'register',
      workerId: 'updatable-worker',
      capacity: 5,
      weight: 30,
      matchRule: { taskTypes: ['cpu'] },
    }))

    // Verify initial state
    let worker = await env.manager.getWorker('updatable-worker')
    expect(worker!.weight).toBe(30)
    expect(worker!.capacity).toBe(5)
    expect(worker!.matchRule).toEqual({ taskTypes: ['cpu'] })

    // Update weight
    await handler.handleMessage(JSON.stringify({ type: 'update', weight: 95 }))
    worker = await env.manager.getWorker('updatable-worker')
    expect(worker!.weight).toBe(95)
    expect(worker!.capacity).toBe(5) // unchanged
    expect(worker!.matchRule).toEqual({ taskTypes: ['cpu'] }) // unchanged

    // Update capacity
    await handler.handleMessage(JSON.stringify({ type: 'update', capacity: 20 }))
    worker = await env.manager.getWorker('updatable-worker')
    expect(worker!.weight).toBe(95) // unchanged
    expect(worker!.capacity).toBe(20)

    // Update matchRule
    await handler.handleMessage(JSON.stringify({
      type: 'update',
      matchRule: { taskTypes: ['gpu', 'tpu'] },
    }))
    worker = await env.manager.getWorker('updatable-worker')
    expect(worker!.matchRule).toEqual({ taskTypes: ['gpu', 'tpu'] })

    // Verify the updated matchRule actually affects dispatch
    // Task with type 'gpu' should now match
    const gpuTask = await env.engine.createTask({ type: 'gpu', cost: 1 })
    const dispatch1 = await env.manager.dispatchTask(gpuTask.id)
    expect(dispatch1.matched).toBe(true)
    expect(dispatch1.workerId).toBe('updatable-worker')

    // Task with type 'cpu' should NOT match anymore
    const cpuTask = await env.engine.createTask({ type: 'cpu', cost: 1 })
    const dispatch2 = await env.manager.dispatchTask(cpuTask.id)
    expect(dispatch2.matched).toBe(false)
  })

  it('update all fields at once', async () => {
    const env = makeEnv()
    const { handler } = makeHandler(env)

    await handler.handleMessage(JSON.stringify({
      type: 'register',
      workerId: 'bulk-update',
      capacity: 5,
      weight: 30,
      matchRule: {},
    }))

    await handler.handleMessage(JSON.stringify({
      type: 'update',
      weight: 100,
      capacity: 50,
      matchRule: { taskTypes: ['ml'] },
    }))

    const worker = await env.manager.getWorker('bulk-update')
    expect(worker!.weight).toBe(100)
    expect(worker!.capacity).toBe(50)
    expect(worker!.matchRule).toEqual({ taskTypes: ['ml'] })
  })
})

// ─── Auth Edge Cases ────────────────────────────────────────────────────────

describe('auth edge cases', () => {
  it('register with workerId mismatch in auth context sends FORBIDDEN error', async () => {
    const env = makeEnv()
    const auth: AuthContext = {
      taskIds: '*',
      scope: ['*'],
      workerId: 'auth-worker-123',
    }
    const { ws, handler } = makeHandler(env, auth)

    await handler.handleMessage(JSON.stringify({
      type: 'register',
      workerId: 'different-worker-id',
      capacity: 5,
    }))

    expect(ws.sent).toHaveLength(1)
    const msg = ws.sent[0] as Record<string, unknown>
    expect(msg.type).toBe('error')
    expect(msg.code).toBe('FORBIDDEN')
    expect(handler.registeredWorkerId).toBeNull()
  })

  it('register with matching workerId in auth context succeeds', async () => {
    const env = makeEnv()
    const auth: AuthContext = {
      taskIds: '*',
      scope: ['*'],
      workerId: 'auth-worker-123',
    }
    const { ws, handler } = makeHandler(env, auth)

    await handler.handleMessage(JSON.stringify({
      type: 'register',
      workerId: 'auth-worker-123',
      capacity: 5,
    }))

    expect(ws.sent).toHaveLength(1)
    const msg = ws.sent[0] as Record<string, unknown>
    expect(msg.type).toBe('registered')
    expect(msg.workerId).toBe('auth-worker-123')
  })

  it('register without workerId when auth has workerId still succeeds (no mismatch check)', async () => {
    const env = makeEnv()
    const auth: AuthContext = {
      taskIds: '*',
      scope: ['*'],
      workerId: 'auth-worker-123',
    }
    const { ws, handler } = makeHandler(env, auth)

    // No workerId in the register message -- the auth check only triggers
    // when msg.workerId is a string AND it doesn't match
    await handler.handleMessage(JSON.stringify({
      type: 'register',
      capacity: 5,
    }))

    expect(ws.sent).toHaveLength(1)
    const msg = ws.sent[0] as Record<string, unknown>
    expect(msg.type).toBe('registered')
  })
})

// ─── REST Endpoint Edge Cases ───────────────────────────────────────────────

describe('REST endpoint edge cases', () => {
  it('DELETE worker that has active assignments removes worker', async () => {
    const { app, engine, manager } = makeApp()

    // Register a worker and assign it a task
    const worker = await manager.registerWorker({
      matchRule: {},
      capacity: 5,
      connectionMode: 'pull',
    })

    const task = await engine.createTask({ type: 'test', cost: 1 })
    const claimResult = await manager.claimTask(task.id, worker.id)
    expect(claimResult.success).toBe(true)

    // Verify assignment exists
    const assignments = await manager.getWorkerTasks(worker.id)
    expect(assignments).toHaveLength(1)
    expect(assignments[0]!.taskId).toBe(task.id)

    // Delete the worker
    const res = await app.request(`/workers/${worker.id}`, { method: 'DELETE' })
    expect(res.status).toBe(204)

    // Worker should be gone
    const check = await manager.getWorker(worker.id)
    expect(check).toBeNull()
  })

  it('DELETE worker that has multiple active assignments removes worker', async () => {
    const { app, engine, manager } = makeApp()

    const worker = await manager.registerWorker({
      matchRule: {},
      capacity: 10,
      connectionMode: 'pull',
    })

    // Assign multiple tasks
    const task1 = await engine.createTask({ type: 'test', cost: 1 })
    const task2 = await engine.createTask({ type: 'test', cost: 2 })
    await manager.claimTask(task1.id, worker.id)
    await manager.claimTask(task2.id, worker.id)

    const assignments = await manager.getWorkerTasks(worker.id)
    expect(assignments).toHaveLength(2)

    // Delete
    const res = await app.request(`/workers/${worker.id}`, { method: 'DELETE' })
    expect(res.status).toBe(204)

    const check = await manager.getWorker(worker.id)
    expect(check).toBeNull()
  })

  it('pull with very short timeout returns 204 quickly when no task available', async () => {
    const { app, manager } = makeApp()

    const worker = await manager.registerWorker({
      matchRule: {},
      capacity: 5,
      connectionMode: 'pull',
    })

    const start = Date.now()
    const res = await app.request(`/workers/pull?workerId=${worker.id}&timeout=50`)
    const elapsed = Date.now() - start

    // Should return 204 (no content) because no task was available
    expect(res.status).toBe(204)
    // Should resolve relatively quickly (timeout was 50ms, allow generous margin)
    expect(elapsed).toBeLessThan(5000)
  })

  it('pull with worker_id mismatch in auth returns 403', async () => {
    const { app, manager } = makeApp({
      scope: ['worker:connect'],
      workerId: 'auth-bound-worker',
    })

    const worker = await manager.registerWorker({
      id: 'different-worker',
      matchRule: {},
      capacity: 5,
      connectionMode: 'pull',
    })

    const res = await app.request(`/workers/pull?workerId=${worker.id}`)
    expect(res.status).toBe(403)
    const body = await res.json()
    expect(body.error).toContain('Forbidden')
  })

  it('pull with matching worker_id in auth succeeds', async () => {
    const { app, engine, manager } = makeApp({
      scope: ['worker:connect'],
      workerId: 'my-worker',
    })

    const worker = await manager.registerWorker({
      id: 'my-worker',
      matchRule: {},
      capacity: 5,
      connectionMode: 'pull',
    })

    // Create a pull-mode task so it completes immediately
    await engine.createTask({ type: 'test', assignMode: 'pull' })

    const res = await app.request(`/workers/pull?workerId=${worker.id}`)
    expect(res.status).toBe(200)
  })

  it('decline with worker_id mismatch in auth returns 403', async () => {
    const { app, engine, manager } = makeApp({
      scope: ['worker:connect'],
      workerId: 'auth-worker',
    })

    const task = await engine.createTask({ type: 'test', cost: 1 })
    const worker = await manager.registerWorker({
      id: 'other-worker',
      matchRule: {},
      capacity: 5,
      connectionMode: 'pull',
    })
    await manager.claimTask(task.id, worker.id)

    const res = await app.request(`/workers/tasks/${task.id}/decline`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ workerId: 'other-worker' }),
    })
    expect(res.status).toBe(403)
    const body = await res.json()
    expect(body.error).toContain('Forbidden')
  })
})

// ─── Ping Timer ─────────────────────────────────────────────────────────────

describe('ping timer', () => {
  it('starts sending pings after registration', async () => {
    const env = makeEnv()
    // Override heartbeat interval to be very short for testing
    const manager = new WorkerManager({
      engine: env.engine,
      shortTermStore: env.store,
      broadcast: env.broadcast,
      defaults: { heartbeatIntervalMs: 50 },
    })
    const ws = mockWS()
    const handler = new WorkerWSHandler(manager, ws)

    await handler.handleMessage(JSON.stringify({ type: 'register', capacity: 5 }))
    ws.sent.length = 0

    // Wait for at least one ping
    await new Promise((r) => setTimeout(r, 120))

    handler.stopPingTimer()

    const pings = ws.sent.filter((m) => (m as Record<string, unknown>).type === 'ping')
    expect(pings.length).toBeGreaterThanOrEqual(1)
  })

  it('stopPingTimer stops sending pings', async () => {
    const env = makeEnv()
    const manager = new WorkerManager({
      engine: env.engine,
      shortTermStore: env.store,
      broadcast: env.broadcast,
      defaults: { heartbeatIntervalMs: 30 },
    })
    const ws = mockWS()
    const handler = new WorkerWSHandler(manager, ws)

    await handler.handleMessage(JSON.stringify({ type: 'register', capacity: 5 }))
    handler.stopPingTimer()
    ws.sent.length = 0

    // Wait and verify no pings are sent
    await new Promise((r) => setTimeout(r, 100))

    const pings = ws.sent.filter((m) => (m as Record<string, unknown>).type === 'ping')
    expect(pings).toHaveLength(0)
  })

  it('handleDisconnect stops ping timer', async () => {
    const env = makeEnv()
    const manager = new WorkerManager({
      engine: env.engine,
      shortTermStore: env.store,
      broadcast: env.broadcast,
      defaults: { heartbeatIntervalMs: 30 },
    })
    const ws = mockWS()
    const handler = new WorkerWSHandler(manager, ws)

    await handler.handleMessage(JSON.stringify({ type: 'register', capacity: 5 }))
    await handler.handleDisconnect()
    ws.sent.length = 0

    // Wait and verify no pings after disconnect
    await new Promise((r) => setTimeout(r, 100))

    const pings = ws.sent.filter((m) => (m as Record<string, unknown>).type === 'ping')
    expect(pings).toHaveLength(0)
  })
})

// ─── Dispatch Matching ──────────────────────────────────────────────────────

describe('dispatch with matchRule filtering', () => {
  it('does not dispatch to worker whose matchRule excludes the task type', async () => {
    const env = makeEnv()
    const { handler } = makeHandler(env)

    await handler.handleMessage(JSON.stringify({
      type: 'register',
      workerId: 'gpu-only',
      capacity: 10,
      matchRule: { taskTypes: ['gpu'] },
    }))

    // Create a CPU task
    const task = await env.engine.createTask({ type: 'cpu', cost: 1 })
    const dispatch = await env.manager.dispatchTask(task.id)
    expect(dispatch.matched).toBe(false)
  })

  it('dispatches to worker whose matchRule includes the task type', async () => {
    const env = makeEnv()
    const { handler } = makeHandler(env)

    await handler.handleMessage(JSON.stringify({
      type: 'register',
      workerId: 'gpu-worker',
      capacity: 10,
      matchRule: { taskTypes: ['gpu'] },
    }))

    const task = await env.engine.createTask({ type: 'gpu', cost: 1 })
    const dispatch = await env.manager.dispatchTask(task.id)
    expect(dispatch.matched).toBe(true)
    expect(dispatch.workerId).toBe('gpu-worker')
  })

  it('does not dispatch to worker at full capacity', async () => {
    const env = makeEnv()
    const { handler } = makeHandler(env)

    await handler.handleMessage(JSON.stringify({
      type: 'register',
      workerId: 'small-worker',
      capacity: 1,
      matchRule: {},
    }))

    // Fill up the worker
    const task1 = await env.engine.createTask({ type: 'test', cost: 1 })
    await env.manager.claimTask(task1.id, 'small-worker')

    // Try to dispatch another task
    const task2 = await env.engine.createTask({ type: 'test', cost: 1 })
    const dispatch = await env.manager.dispatchTask(task2.id)
    expect(dispatch.matched).toBe(false)
  })

  it('does not dispatch to blacklisted worker', async () => {
    const env = makeEnv()

    // Register two workers
    const { handler: h1 } = makeHandler(env)
    await h1.handleMessage(JSON.stringify({
      type: 'register',
      workerId: 'blacklisted-worker',
      capacity: 10,
      weight: 90,
    }))

    const { handler: h2 } = makeHandler(env)
    await h2.handleMessage(JSON.stringify({
      type: 'register',
      workerId: 'normal-worker',
      capacity: 10,
      weight: 10,
    }))

    const task = await env.engine.createTask({ type: 'test', cost: 1 })

    // Claim and decline with blacklist from the higher-weight worker
    await env.manager.claimTask(task.id, 'blacklisted-worker')
    await env.manager.declineTask(task.id, 'blacklisted-worker', { blacklist: true })

    // Re-dispatch should skip blacklisted worker
    const dispatch = await env.manager.dispatchTask(task.id)
    expect(dispatch.matched).toBe(true)
    expect(dispatch.workerId).toBe('normal-worker')
  })

  it('does not dispatch non-pending tasks', async () => {
    const env = makeEnv()
    const { handler } = makeHandler(env)

    await handler.handleMessage(JSON.stringify({
      type: 'register',
      workerId: 'worker-1',
      capacity: 10,
    }))

    const task = await env.engine.createTask({ type: 'test', cost: 1 })
    await env.engine.transitionTask(task.id, 'running')

    const dispatch = await env.manager.dispatchTask(task.id)
    expect(dispatch.matched).toBe(false)
  })

  it('dispatches to draining worker is excluded (draining workers are not idle/busy)', async () => {
    const env = makeEnv()
    const { handler } = makeHandler(env)

    await handler.handleMessage(JSON.stringify({
      type: 'register',
      workerId: 'draining-worker',
      capacity: 10,
    }))

    // Set to draining
    await handler.handleMessage(JSON.stringify({ type: 'drain' }))

    const task = await env.engine.createTask({ type: 'test', cost: 1 })
    const dispatch = await env.manager.dispatchTask(task.id)
    // Draining workers have status 'draining', not 'idle' or 'busy'
    // so listWorkers({ status: ['idle', 'busy'] }) should exclude them
    expect(dispatch.matched).toBe(false)
  })
})
