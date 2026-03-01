import { describe, it, expect, vi } from 'vitest'
import {
  TaskEngine,
  MemoryBroadcastProvider,
  MemoryShortTermStore,
  WorkerManager,
} from '@taskcast/core'
import { WorkerWSHandler } from '../src/routes/worker-ws.js'
import type { WSLike } from '../src/routes/worker-ws.js'

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
  const engine = new TaskEngine({ shortTerm: store, broadcast })
  const manager = new WorkerManager({ engine, shortTerm: store, broadcast })
  const ws = mockWS()
  const handler = new WorkerWSHandler(manager, ws)
  return { store, broadcast, engine, manager, ws, handler }
}

describe('WorkerWSHandler', () => {
  describe('register', () => {
    it('sends registered with workerId', async () => {
      const { ws, handler } = makeEnv()
      await handler.handleMessage(JSON.stringify({
        type: 'register',
        capacity: 5,
      }))
      expect(ws.sent).toHaveLength(1)
      const msg = ws.sent[0] as Record<string, unknown>
      expect(msg.type).toBe('registered')
      expect(msg.workerId).toBeDefined()
      expect(typeof msg.workerId).toBe('string')
      expect(handler.registeredWorkerId).toBe(msg.workerId)
    })

    it('uses provided workerId', async () => {
      const { ws, handler } = makeEnv()
      await handler.handleMessage(JSON.stringify({
        type: 'register',
        workerId: 'my-worker-42',
        capacity: 3,
      }))
      expect(ws.sent).toHaveLength(1)
      const msg = ws.sent[0] as Record<string, unknown>
      expect(msg.type).toBe('registered')
      expect(msg.workerId).toBe('my-worker-42')
      expect(handler.registeredWorkerId).toBe('my-worker-42')
    })

    it('defaults capacity to 10 and matchRule to {}', async () => {
      const { handler, manager } = makeEnv()
      await handler.handleMessage(JSON.stringify({ type: 'register' }))
      const worker = await manager.getWorker(handler.registeredWorkerId!)
      expect(worker).not.toBeNull()
      expect(worker!.capacity).toBe(10)
      expect(worker!.matchRule).toEqual({})
    })

    it('sets connectionMode to websocket', async () => {
      const { handler, manager } = makeEnv()
      await handler.handleMessage(JSON.stringify({ type: 'register' }))
      const worker = await manager.getWorker(handler.registeredWorkerId!)
      expect(worker!.connectionMode).toBe('websocket')
    })

    it('passes weight and matchRule to registerWorker', async () => {
      const { handler, manager } = makeEnv()
      await handler.handleMessage(JSON.stringify({
        type: 'register',
        weight: 80,
        matchRule: { taskTypes: ['gpu'] },
        capacity: 20,
      }))
      const worker = await manager.getWorker(handler.registeredWorkerId!)
      expect(worker!.weight).toBe(80)
      expect(worker!.matchRule).toEqual({ taskTypes: ['gpu'] })
      expect(worker!.capacity).toBe(20)
    })
  })

  describe('accept (ws-offer mode)', () => {
    it('claims task and sends assigned', async () => {
      const { ws, handler, engine, manager } = makeEnv()
      // Register worker
      await handler.handleMessage(JSON.stringify({ type: 'register', capacity: 5 }))
      const workerId = handler.registeredWorkerId!

      // Create a pending task
      const task = await engine.createTask({ type: 'test', cost: 1 })

      // Offer the task to the handler (simulate ws-offer dispatch)
      handler.offerTask(task)

      // Clear sent messages so far
      ws.sent.length = 0

      // Accept the task
      await handler.handleMessage(JSON.stringify({ type: 'accept', taskId: task.id }))

      expect(ws.sent).toHaveLength(1)
      const msg = ws.sent[0] as Record<string, unknown>
      expect(msg.type).toBe('assigned')
      expect(msg.taskId).toBe(task.id)

      // Verify the task was actually claimed
      const updatedTask = await engine.getTask(task.id)
      expect(updatedTask!.status).toBe('assigned')
      expect(updatedTask!.assignedWorker).toBe(workerId)
    })

    it('sends error when task is already claimed', async () => {
      const { ws, handler, engine, manager } = makeEnv()
      // Register two workers
      await handler.handleMessage(JSON.stringify({ type: 'register', capacity: 5 }))

      const otherWorker = await manager.registerWorker({
        matchRule: {},
        capacity: 5,
        connectionMode: 'websocket',
      })

      // Create a task and claim it with the other worker first
      const task = await engine.createTask({ type: 'test', cost: 1 })
      await manager.claimTask(task.id, otherWorker.id)

      ws.sent.length = 0

      // Try to accept from our handler
      await handler.handleMessage(JSON.stringify({ type: 'accept', taskId: task.id }))

      expect(ws.sent).toHaveLength(1)
      const msg = ws.sent[0] as Record<string, unknown>
      expect(msg.type).toBe('error')
      expect(msg.taskId).toBe(task.id)
      expect(msg.message).toBeDefined()
    })
  })

  describe('decline', () => {
    it('declines a task and sends declined', async () => {
      const { ws, handler, engine, manager } = makeEnv()
      await handler.handleMessage(JSON.stringify({ type: 'register', capacity: 5 }))
      const workerId = handler.registeredWorkerId!

      // Create and claim a task
      const task = await engine.createTask({ type: 'test', cost: 1 })
      await manager.claimTask(task.id, workerId)

      // Offer task to handler so it's in pendingOffers
      handler.offerTask(task)
      ws.sent.length = 0

      // Decline the task
      await handler.handleMessage(JSON.stringify({ type: 'decline', taskId: task.id }))

      expect(ws.sent).toHaveLength(1)
      const msg = ws.sent[0] as Record<string, unknown>
      expect(msg.type).toBe('declined')
      expect(msg.taskId).toBe(task.id)

      // Task should be back to pending
      const updatedTask = await engine.getTask(task.id)
      expect(updatedTask!.status).toBe('pending')
    })

    it('passes blacklist option through', async () => {
      const { ws, handler, engine, manager } = makeEnv()
      await handler.handleMessage(JSON.stringify({ type: 'register', capacity: 5 }))
      const workerId = handler.registeredWorkerId!

      const task = await engine.createTask({ type: 'test', cost: 1 })
      await manager.claimTask(task.id, workerId)

      ws.sent.length = 0

      await handler.handleMessage(JSON.stringify({
        type: 'decline',
        taskId: task.id,
        blacklist: true,
      }))

      const msg = ws.sent[0] as Record<string, unknown>
      expect(msg.type).toBe('declined')

      // Verify blacklist was applied
      const updatedTask = await engine.getTask(task.id)
      const blacklisted = (updatedTask!.metadata?._blacklistedWorkers as string[]) ?? []
      expect(blacklisted).toContain(workerId)
    })
  })

  describe('claim (ws-race mode)', () => {
    it('claims task atomically and sends claimed with success', async () => {
      const { ws, handler, engine } = makeEnv()
      await handler.handleMessage(JSON.stringify({ type: 'register', capacity: 5 }))

      const task = await engine.createTask({ type: 'test', cost: 1 })
      ws.sent.length = 0

      await handler.handleMessage(JSON.stringify({ type: 'claim', taskId: task.id }))

      expect(ws.sent).toHaveLength(1)
      const msg = ws.sent[0] as Record<string, unknown>
      expect(msg.type).toBe('claimed')
      expect(msg.taskId).toBe(task.id)
      expect(msg.success).toBe(true)
    })

    it('sends claimed with failure when task already taken', async () => {
      const { ws, handler, engine, manager } = makeEnv()
      await handler.handleMessage(JSON.stringify({ type: 'register', capacity: 5 }))

      const otherWorker = await manager.registerWorker({
        matchRule: {},
        capacity: 5,
        connectionMode: 'websocket',
      })

      const task = await engine.createTask({ type: 'test', cost: 1 })
      await manager.claimTask(task.id, otherWorker.id)

      ws.sent.length = 0

      await handler.handleMessage(JSON.stringify({ type: 'claim', taskId: task.id }))

      expect(ws.sent).toHaveLength(1)
      const msg = ws.sent[0] as Record<string, unknown>
      expect(msg.type).toBe('claimed')
      expect(msg.taskId).toBe(task.id)
      expect(msg.success).toBe(false)
    })
  })

  describe('update', () => {
    it('updates worker weight', async () => {
      const { ws, handler, manager } = makeEnv()
      await handler.handleMessage(JSON.stringify({ type: 'register', capacity: 5, weight: 50 }))
      const workerId = handler.registeredWorkerId!

      ws.sent.length = 0

      await handler.handleMessage(JSON.stringify({ type: 'update', weight: 90 }))

      // update does not send a reply
      expect(ws.sent).toHaveLength(0)

      const worker = await manager.getWorker(workerId)
      expect(worker!.weight).toBe(90)
    })

    it('updates worker capacity and matchRule', async () => {
      const { handler, manager } = makeEnv()
      await handler.handleMessage(JSON.stringify({ type: 'register', capacity: 5 }))
      const workerId = handler.registeredWorkerId!

      await handler.handleMessage(JSON.stringify({
        type: 'update',
        capacity: 20,
        matchRule: { taskTypes: ['gpu'] },
      }))

      const worker = await manager.getWorker(workerId)
      expect(worker!.capacity).toBe(20)
      expect(worker!.matchRule).toEqual({ taskTypes: ['gpu'] })
    })
  })

  describe('drain', () => {
    it('sets worker status to draining', async () => {
      const { handler, manager } = makeEnv()
      await handler.handleMessage(JSON.stringify({ type: 'register', capacity: 5 }))
      const workerId = handler.registeredWorkerId!

      await handler.handleMessage(JSON.stringify({ type: 'drain' }))

      const worker = await manager.getWorker(workerId)
      expect(worker!.status).toBe('draining')
    })
  })

  describe('pong (heartbeat)', () => {
    it('updates heartbeat timestamp', async () => {
      const { handler, manager } = makeEnv()
      await handler.handleMessage(JSON.stringify({ type: 'register', capacity: 5 }))
      const workerId = handler.registeredWorkerId!

      const workerBefore = await manager.getWorker(workerId)
      const heartbeatBefore = workerBefore!.lastHeartbeatAt

      // Small delay to ensure timestamp changes
      await new Promise((r) => setTimeout(r, 10))

      await handler.handleMessage(JSON.stringify({ type: 'pong' }))

      const workerAfter = await manager.getWorker(workerId)
      expect(workerAfter!.lastHeartbeatAt).toBeGreaterThanOrEqual(heartbeatBefore)
    })
  })

  describe('error handling', () => {
    it('sends error for unknown message type', async () => {
      const { ws, handler } = makeEnv()
      await handler.handleMessage(JSON.stringify({ type: 'register', capacity: 5 }))
      ws.sent.length = 0

      await handler.handleMessage(JSON.stringify({ type: 'foobar' }))

      expect(ws.sent).toHaveLength(1)
      const msg = ws.sent[0] as Record<string, unknown>
      expect(msg.type).toBe('error')
      expect(msg.message).toBe('Unknown message type: foobar')
    })

    it('sends error for invalid JSON', async () => {
      const { ws, handler } = makeEnv()
      await handler.handleMessage('not valid json {{{')

      expect(ws.sent).toHaveLength(1)
      const msg = ws.sent[0] as Record<string, unknown>
      expect(msg.type).toBe('error')
      expect(msg.message).toBe('Invalid JSON')
    })

    it('sends error for messages before register (accept)', async () => {
      const { ws, handler } = makeEnv()
      expect(handler.registeredWorkerId).toBeNull()

      await handler.handleMessage(JSON.stringify({ type: 'accept', taskId: 'task-1' }))

      expect(ws.sent).toHaveLength(1)
      const msg = ws.sent[0] as Record<string, unknown>
      expect(msg.type).toBe('error')
      expect(msg.message).toBe('Not registered')
    })

    it('sends error for messages before register (decline)', async () => {
      const { ws, handler } = makeEnv()
      await handler.handleMessage(JSON.stringify({ type: 'decline', taskId: 'task-1' }))

      expect(ws.sent).toHaveLength(1)
      const msg = ws.sent[0] as Record<string, unknown>
      expect(msg.type).toBe('error')
      expect(msg.message).toBe('Not registered')
    })

    it('sends error for messages before register (claim)', async () => {
      const { ws, handler } = makeEnv()
      await handler.handleMessage(JSON.stringify({ type: 'claim', taskId: 'task-1' }))

      expect(ws.sent).toHaveLength(1)
      const msg = ws.sent[0] as Record<string, unknown>
      expect(msg.type).toBe('error')
      expect(msg.message).toBe('Not registered')
    })

    it('sends error for messages before register (update)', async () => {
      const { ws, handler } = makeEnv()
      await handler.handleMessage(JSON.stringify({ type: 'update', weight: 80 }))

      expect(ws.sent).toHaveLength(1)
      const msg = ws.sent[0] as Record<string, unknown>
      expect(msg.type).toBe('error')
      expect(msg.message).toBe('Not registered')
    })

    it('sends error for messages before register (drain)', async () => {
      const { ws, handler } = makeEnv()
      await handler.handleMessage(JSON.stringify({ type: 'drain' }))

      expect(ws.sent).toHaveLength(1)
      const msg = ws.sent[0] as Record<string, unknown>
      expect(msg.type).toBe('error')
      expect(msg.message).toBe('Not registered')
    })

    it('sends error for messages before register (pong)', async () => {
      const { ws, handler } = makeEnv()
      await handler.handleMessage(JSON.stringify({ type: 'pong' }))

      expect(ws.sent).toHaveLength(1)
      const msg = ws.sent[0] as Record<string, unknown>
      expect(msg.type).toBe('error')
      expect(msg.message).toBe('Not registered')
    })

    it('sends error for undefined message type', async () => {
      const { ws, handler } = makeEnv()
      await handler.handleMessage(JSON.stringify({ data: 'no type field' }))

      expect(ws.sent).toHaveLength(1)
      const msg = ws.sent[0] as Record<string, unknown>
      expect(msg.type).toBe('error')
      expect(msg.message).toBe('Unknown message type: undefined')
    })
  })

  describe('offerTask', () => {
    it('sends offer with task summary', async () => {
      const { ws, handler, engine } = makeEnv()
      await handler.handleMessage(JSON.stringify({ type: 'register', capacity: 5 }))
      ws.sent.length = 0

      const task = await engine.createTask({
        type: 'llm-inference',
        cost: 3,
        tags: ['gpu', 'fast'],
        params: { model: 'gpt-4' },
      })

      handler.offerTask(task)

      expect(ws.sent).toHaveLength(1)
      const msg = ws.sent[0] as Record<string, unknown>
      expect(msg.type).toBe('offer')
      expect(msg.taskId).toBe(task.id)

      const summary = msg.task as Record<string, unknown>
      expect(summary.id).toBe(task.id)
      expect(summary.type).toBe('llm-inference')
      expect(summary.cost).toBe(3)
      expect(summary.tags).toEqual(['gpu', 'fast'])
      expect(summary.params).toEqual({ model: 'gpt-4' })
    })

    it('sends offer with minimal task summary (no optional fields)', async () => {
      const { ws, handler, engine } = makeEnv()
      await handler.handleMessage(JSON.stringify({ type: 'register', capacity: 5 }))
      ws.sent.length = 0

      const task = await engine.createTask({})

      handler.offerTask(task)

      expect(ws.sent).toHaveLength(1)
      const msg = ws.sent[0] as Record<string, unknown>
      expect(msg.type).toBe('offer')
      expect(msg.taskId).toBe(task.id)

      const summary = msg.task as Record<string, unknown>
      expect(summary.id).toBe(task.id)
      // Optional fields should not be present
      expect(summary.type).toBeUndefined()
      expect(summary.cost).toBeUndefined()
      expect(summary.tags).toBeUndefined()
      expect(summary.params).toBeUndefined()
    })
  })

  describe('broadcastAvailable', () => {
    it('sends available with task summary', async () => {
      const { ws, handler, engine } = makeEnv()
      await handler.handleMessage(JSON.stringify({ type: 'register', capacity: 5 }))
      ws.sent.length = 0

      const task = await engine.createTask({
        type: 'batch',
        cost: 2,
        tags: ['cpu'],
        params: { input: 'data' },
      })

      handler.broadcastAvailable(task)

      expect(ws.sent).toHaveLength(1)
      const msg = ws.sent[0] as Record<string, unknown>
      expect(msg.type).toBe('available')
      expect(msg.taskId).toBe(task.id)

      const summary = msg.task as Record<string, unknown>
      expect(summary.id).toBe(task.id)
      expect(summary.type).toBe('batch')
      expect(summary.cost).toBe(2)
      expect(summary.tags).toEqual(['cpu'])
      expect(summary.params).toEqual({ input: 'data' })
    })
  })

  describe('handleDisconnect', () => {
    it('unregisters worker on disconnect', async () => {
      const { handler, manager } = makeEnv()
      await handler.handleMessage(JSON.stringify({ type: 'register', capacity: 5 }))
      const workerId = handler.registeredWorkerId!

      // Verify worker exists
      const workerBefore = await manager.getWorker(workerId)
      expect(workerBefore).not.toBeNull()

      // Disconnect
      await handler.handleDisconnect()

      // Verify worker is gone
      const workerAfter = await manager.getWorker(workerId)
      expect(workerAfter).toBeNull()
    })

    it('does nothing when not registered', async () => {
      const { handler, manager } = makeEnv()
      // No register call — should not throw
      await expect(handler.handleDisconnect()).resolves.toBeUndefined()
    })
  })

  describe('registeredWorkerId getter', () => {
    it('returns null before registration', () => {
      const { handler } = makeEnv()
      expect(handler.registeredWorkerId).toBeNull()
    })

    it('returns workerId after registration', async () => {
      const { handler } = makeEnv()
      await handler.handleMessage(JSON.stringify({
        type: 'register',
        workerId: 'w-123',
      }))
      expect(handler.registeredWorkerId).toBe('w-123')
    })
  })
})
