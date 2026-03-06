import { describe, it, expect, vi } from 'vitest'
import {
  TaskEngine,
  MemoryBroadcastProvider,
  MemoryShortTermStore,
  WorkerManager,
} from '@taskcast/core'
import { createTaskcastApp, WorkerWSHandler, WorkerWSRegistry } from '../src/index.js'
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
  const engine = new TaskEngine({ shortTermStore: store, broadcast })
  const wm = new WorkerManager({ engine, shortTermStore: store, broadcast })
  const { app, wsRegistry, stop } = createTaskcastApp({
    engine,
    workerManager: wm,
    shortTermStore: store,
    scheduler: { enabled: false },
    heartbeat: { enabled: false },
  })
  return { store, broadcast, engine, wm, app, wsRegistry: wsRegistry!, stop }
}

async function registerWSWorker(
  manager: WorkerManager,
  ws: WSLike & { sent: unknown[] },
  registry: WorkerWSRegistry,
  opts?: { workerId?: string; capacity?: number; matchRule?: Record<string, unknown>; weight?: number },
) {
  const handler = new WorkerWSHandler(manager, ws, undefined, registry)
  const regMsg: Record<string, unknown> = {
    type: 'register',
    capacity: opts?.capacity ?? 10,
  }
  if (opts?.workerId) regMsg.workerId = opts.workerId
  if (opts?.matchRule) regMsg.matchRule = opts.matchRule
  if (opts?.weight !== undefined) regMsg.weight = opts.weight
  await handler.handleMessage(JSON.stringify(regMsg))
  return handler
}

describe('WS Dispatch', () => {
  describe('ws-offer', () => {
    it('sends offer to best matching worker on task creation', async () => {
      const { engine, wm, wsRegistry, stop } = makeEnv()
      const ws = mockWS()
      const handler = await registerWSWorker(wm, ws, wsRegistry, { workerId: 'w1' })
      ws.sent.length = 0 // clear registration messages

      // Create task with ws-offer
      const task = await engine.createTask({
        type: 'test',
        assignMode: 'ws-offer',
        cost: 1,
      })

      // Give fire-and-forget a tick to resolve
      await new Promise((r) => setTimeout(r, 50))

      // Worker should have received an offer
      const offers = ws.sent.filter((m: any) => m.type === 'offer')
      expect(offers).toHaveLength(1)
      const offer = offers[0] as Record<string, unknown>
      expect(offer.taskId).toBe(task.id)
      expect((offer.task as Record<string, unknown>).id).toBe(task.id)

      stop()
    })

    it('offers to highest-weight worker when multiple are available', async () => {
      const { engine, wm, wsRegistry, stop } = makeEnv()
      const ws1 = mockWS()
      const ws2 = mockWS()
      await registerWSWorker(wm, ws1, wsRegistry, { workerId: 'w1', weight: 50 })
      await registerWSWorker(wm, ws2, wsRegistry, { workerId: 'w2', weight: 90 })
      ws1.sent.length = 0
      ws2.sent.length = 0

      await engine.createTask({
        type: 'test',
        assignMode: 'ws-offer',
        cost: 1,
      })

      await new Promise((r) => setTimeout(r, 50))

      // w2 has higher weight, should get the offer
      const w2Offers = ws2.sent.filter((m: any) => m.type === 'offer')
      const w1Offers = ws1.sent.filter((m: any) => m.type === 'offer')
      expect(w2Offers).toHaveLength(1)
      expect(w1Offers).toHaveLength(0)

      stop()
    })

    it('does not crash when no matching worker exists', async () => {
      const { engine, stop } = makeEnv()

      // Create task with ws-offer but no workers registered
      const task = await engine.createTask({
        type: 'test',
        assignMode: 'ws-offer',
        cost: 1,
      })

      await new Promise((r) => setTimeout(r, 50))

      // Task should still be pending (no crash)
      const current = await engine.getTask(task.id)
      expect(current!.status).toBe('pending')

      stop()
    })

    it('does not offer to worker not in registry (pull mode worker)', async () => {
      const { engine, wm, wsRegistry, stop } = makeEnv()

      // Register a pull-mode worker directly (not through WS, so not in registry)
      await wm.registerWorker({
        id: 'pull-worker',
        matchRule: {},
        capacity: 10,
        connectionMode: 'pull',
      })

      const task = await engine.createTask({
        type: 'test',
        assignMode: 'ws-offer',
        cost: 1,
      })

      await new Promise((r) => setTimeout(r, 50))

      // dispatchTask will match the pull worker but since it's not in registry,
      // no offer is sent. The task stays pending.
      const current = await engine.getTask(task.id)
      expect(current!.status).toBe('pending')

      // Registry should have no handlers
      expect(wsRegistry.get('pull-worker')).toBeUndefined()

      stop()
    })

    it('re-dispatches when task transitions back to pending after decline', async () => {
      const { engine, wm, wsRegistry, stop } = makeEnv()
      const ws1 = mockWS()
      await registerWSWorker(wm, ws1, wsRegistry, { workerId: 'w1', weight: 90 })
      ws1.sent.length = 0

      // Create ws-offer task
      const task = await engine.createTask({
        type: 'test',
        assignMode: 'ws-offer',
        cost: 1,
      })

      await new Promise((r) => setTimeout(r, 50))

      // w1 should get the initial offer
      expect(ws1.sent.filter((m: any) => m.type === 'offer')).toHaveLength(1)

      // w1 accepts
      ws1.sent.length = 0
      await wm.claimTask(task.id, 'w1')

      // Then w1 declines (without blacklist, so it can be re-offered)
      await wm.declineTask(task.id, 'w1')

      await new Promise((r) => setTimeout(r, 50))

      // After decline, task goes back to pending, transition listener fires,
      // re-dispatch should send another offer to w1 (the only worker)
      const reOffers = ws1.sent.filter((m: any) => m.type === 'offer')
      expect(reOffers).toHaveLength(1)
      expect((reOffers[0] as Record<string, unknown>).taskId).toBe(task.id)

      stop()
    })
  })

  describe('ws-race', () => {
    it('broadcasts to all matching websocket workers on task creation', async () => {
      const { engine, wm, wsRegistry, stop } = makeEnv()
      const ws1 = mockWS()
      const ws2 = mockWS()
      const ws3 = mockWS()
      await registerWSWorker(wm, ws1, wsRegistry, { workerId: 'w1' })
      await registerWSWorker(wm, ws2, wsRegistry, { workerId: 'w2' })
      await registerWSWorker(wm, ws3, wsRegistry, { workerId: 'w3' })
      ws1.sent.length = 0
      ws2.sent.length = 0
      ws3.sent.length = 0

      const task = await engine.createTask({
        type: 'test',
        assignMode: 'ws-race',
        cost: 1,
      })

      await new Promise((r) => setTimeout(r, 50))

      // All three workers should receive 'available'
      for (const ws of [ws1, ws2, ws3]) {
        const avail = ws.sent.filter((m: any) => m.type === 'available')
        expect(avail).toHaveLength(1)
        expect((avail[0] as Record<string, unknown>).taskId).toBe(task.id)
      }

      stop()
    })

    it('first worker to claim wins the race', async () => {
      const { engine, wm, wsRegistry, stop } = makeEnv()
      const ws1 = mockWS()
      const ws2 = mockWS()
      const handler1 = await registerWSWorker(wm, ws1, wsRegistry, { workerId: 'w1' })
      const handler2 = await registerWSWorker(wm, ws2, wsRegistry, { workerId: 'w2' })
      ws1.sent.length = 0
      ws2.sent.length = 0

      const task = await engine.createTask({
        type: 'test',
        assignMode: 'ws-race',
        cost: 1,
      })

      await new Promise((r) => setTimeout(r, 50))

      // Both got 'available'
      expect(ws1.sent.filter((m: any) => m.type === 'available')).toHaveLength(1)
      expect(ws2.sent.filter((m: any) => m.type === 'available')).toHaveLength(1)

      ws1.sent.length = 0
      ws2.sent.length = 0

      // w1 claims first
      await handler1.handleMessage(JSON.stringify({ type: 'claim', taskId: task.id }))
      const w1Claim = ws1.sent.find((m: any) => m.type === 'claimed') as Record<string, unknown>
      expect(w1Claim.success).toBe(true)

      // w2 tries to claim — should fail
      await handler2.handleMessage(JSON.stringify({ type: 'claim', taskId: task.id }))
      const w2Claim = ws2.sent.find((m: any) => m.type === 'claimed') as Record<string, unknown>
      expect(w2Claim.success).toBe(false)

      // Task should be assigned to w1
      const current = await engine.getTask(task.id)
      expect(current!.status).toBe('assigned')
      expect(current!.assignedWorker).toBe('w1')

      stop()
    })

    it('only broadcasts to workers with matching rules', async () => {
      const { engine, wm, wsRegistry, stop } = makeEnv()
      const wsGpu = mockWS()
      const wsCpu = mockWS()
      await registerWSWorker(wm, wsGpu, wsRegistry, {
        workerId: 'gpu-worker',
        matchRule: { taskTypes: ['gpu.*'] },
      })
      await registerWSWorker(wm, wsCpu, wsRegistry, {
        workerId: 'cpu-worker',
        matchRule: { taskTypes: ['cpu.*'] },
      })
      wsGpu.sent.length = 0
      wsCpu.sent.length = 0

      // Create a GPU task
      await engine.createTask({
        type: 'gpu.inference',
        assignMode: 'ws-race',
        cost: 1,
      })

      await new Promise((r) => setTimeout(r, 50))

      // Only GPU worker should receive 'available'
      expect(wsGpu.sent.filter((m: any) => m.type === 'available')).toHaveLength(1)
      expect(wsCpu.sent.filter((m: any) => m.type === 'available')).toHaveLength(0)

      stop()
    })

    it('does not broadcast to workers at capacity', async () => {
      const { engine, wm, wsRegistry, stop } = makeEnv()
      const ws1 = mockWS()
      const ws2 = mockWS()
      await registerWSWorker(wm, ws1, wsRegistry, { workerId: 'w1', capacity: 1 })
      await registerWSWorker(wm, ws2, wsRegistry, { workerId: 'w2', capacity: 5 })

      // Claim a task on w1 to fill its capacity
      const filler = await engine.createTask({ type: 'test', cost: 1 })
      await wm.claimTask(filler.id, 'w1')

      ws1.sent.length = 0
      ws2.sent.length = 0

      // Create ws-race task
      await engine.createTask({
        type: 'test',
        assignMode: 'ws-race',
        cost: 1,
      })

      await new Promise((r) => setTimeout(r, 50))

      // w1 is at capacity, should not receive broadcast
      expect(ws1.sent.filter((m: any) => m.type === 'available')).toHaveLength(0)
      // w2 has capacity, should receive broadcast
      expect(ws2.sent.filter((m: any) => m.type === 'available')).toHaveLength(1)

      stop()
    })

    it('does not broadcast to pull-mode workers', async () => {
      const { engine, wm, wsRegistry, stop } = makeEnv()
      const ws1 = mockWS()
      await registerWSWorker(wm, ws1, wsRegistry, { workerId: 'ws-worker' })

      // Register a pull-mode worker directly
      await wm.registerWorker({
        id: 'pull-worker',
        matchRule: {},
        capacity: 10,
        connectionMode: 'pull',
      })

      ws1.sent.length = 0

      await engine.createTask({
        type: 'test',
        assignMode: 'ws-race',
        cost: 1,
      })

      await new Promise((r) => setTimeout(r, 50))

      // Only the WS worker should get broadcast
      expect(ws1.sent.filter((m: any) => m.type === 'available')).toHaveLength(1)
      // Pull worker is not in registry, so no offer
      expect(wsRegistry.get('pull-worker')).toBeUndefined()

      stop()
    })
  })

  describe('no dispatch for other assign modes', () => {
    it('does not dispatch for pull mode tasks', async () => {
      const { engine, wm, wsRegistry, stop } = makeEnv()
      const ws = mockWS()
      await registerWSWorker(wm, ws, wsRegistry, { workerId: 'w1' })
      ws.sent.length = 0

      await engine.createTask({
        type: 'test',
        assignMode: 'pull',
      })

      await new Promise((r) => setTimeout(r, 50))

      // No offer or available messages
      expect(ws.sent.filter((m: any) => m.type === 'offer' || m.type === 'available')).toHaveLength(0)

      stop()
    })

    it('does not dispatch for external mode tasks', async () => {
      const { engine, wm, wsRegistry, stop } = makeEnv()
      const ws = mockWS()
      await registerWSWorker(wm, ws, wsRegistry, { workerId: 'w1' })
      ws.sent.length = 0

      await engine.createTask({
        type: 'test',
        assignMode: 'external',
      })

      await new Promise((r) => setTimeout(r, 50))

      expect(ws.sent.filter((m: any) => m.type === 'offer' || m.type === 'available')).toHaveLength(0)

      stop()
    })

    it('does not dispatch for tasks with no assignMode', async () => {
      const { engine, wm, wsRegistry, stop } = makeEnv()
      const ws = mockWS()
      await registerWSWorker(wm, ws, wsRegistry, { workerId: 'w1' })
      ws.sent.length = 0

      await engine.createTask({ type: 'test' })

      await new Promise((r) => setTimeout(r, 50))

      expect(ws.sent.filter((m: any) => m.type === 'offer' || m.type === 'available')).toHaveLength(0)

      stop()
    })
  })

  describe('WorkerWSRegistry', () => {
    it('registers and retrieves handlers', () => {
      const registry = new WorkerWSRegistry()
      const store = new MemoryShortTermStore()
      const broadcast = new MemoryBroadcastProvider()
      const engine = new TaskEngine({ shortTermStore: store, broadcast })
      const manager = new WorkerManager({ engine, shortTermStore: store, broadcast })
      const ws = mockWS()
      const handler = new WorkerWSHandler(manager, ws)

      registry.register('w1', handler)
      expect(registry.get('w1')).toBe(handler)
      expect(registry.getAll().size).toBe(1)
    })

    it('unregisters handlers', () => {
      const registry = new WorkerWSRegistry()
      const store = new MemoryShortTermStore()
      const broadcast = new MemoryBroadcastProvider()
      const engine = new TaskEngine({ shortTermStore: store, broadcast })
      const manager = new WorkerManager({ engine, shortTermStore: store, broadcast })
      const ws = mockWS()
      const handler = new WorkerWSHandler(manager, ws)

      registry.register('w1', handler)
      registry.unregister('w1')
      expect(registry.get('w1')).toBeUndefined()
      expect(registry.getAll().size).toBe(0)
    })

    it('returns undefined for unregistered worker', () => {
      const registry = new WorkerWSRegistry()
      expect(registry.get('nonexistent')).toBeUndefined()
    })

    it('auto-registers on WS handler register message', async () => {
      const store = new MemoryShortTermStore()
      const broadcast = new MemoryBroadcastProvider()
      const engine = new TaskEngine({ shortTermStore: store, broadcast })
      const manager = new WorkerManager({ engine, shortTermStore: store, broadcast })
      const registry = new WorkerWSRegistry()
      const ws = mockWS()
      const handler = new WorkerWSHandler(manager, ws, undefined, registry)

      await handler.handleMessage(JSON.stringify({ type: 'register', workerId: 'w1', capacity: 5 }))

      expect(registry.get('w1')).toBe(handler)
    })

    it('auto-unregisters on WS handler disconnect', async () => {
      const store = new MemoryShortTermStore()
      const broadcast = new MemoryBroadcastProvider()
      const engine = new TaskEngine({ shortTermStore: store, broadcast })
      const manager = new WorkerManager({ engine, shortTermStore: store, broadcast })
      const registry = new WorkerWSRegistry()
      const ws = mockWS()
      const handler = new WorkerWSHandler(manager, ws, undefined, registry)

      await handler.handleMessage(JSON.stringify({ type: 'register', workerId: 'w1', capacity: 5 }))
      expect(registry.get('w1')).toBe(handler)

      await handler.handleDisconnect()
      expect(registry.get('w1')).toBeUndefined()
    })
  })

  describe('createTaskcastApp wsRegistry', () => {
    it('exposes wsRegistry when workerManager is provided', () => {
      const store = new MemoryShortTermStore()
      const broadcast = new MemoryBroadcastProvider()
      const engine = new TaskEngine({ shortTermStore: store, broadcast })
      const wm = new WorkerManager({ engine, shortTermStore: store, broadcast })
      const { wsRegistry, stop } = createTaskcastApp({
        engine,
        workerManager: wm,
        shortTermStore: store,
        scheduler: { enabled: false },
        heartbeat: { enabled: false },
      })
      expect(wsRegistry).toBeInstanceOf(WorkerWSRegistry)
      stop()
    })

    it('does not expose wsRegistry when workerManager is not provided', () => {
      const store = new MemoryShortTermStore()
      const broadcast = new MemoryBroadcastProvider()
      const engine = new TaskEngine({ shortTermStore: store, broadcast })
      const { wsRegistry, stop } = createTaskcastApp({
        engine,
        shortTermStore: store,
        scheduler: { enabled: false },
        heartbeat: { enabled: false },
      })
      expect(wsRegistry).toBeUndefined()
      stop()
    })
  })
})
