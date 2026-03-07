import { describe, it, expect, vi, beforeEach } from 'vitest'
import { TaskEngine } from '../../src/engine.js'
import { MemoryBroadcastProvider, MemoryShortTermStore } from '../../src/memory-adapters.js'
import { WorkerManager } from '../../src/worker-manager.js'
import type { TaskcastHooks, Worker, LongTermStore } from '../../src/types.js'
import type { WorkerRegistration } from '../../src/worker-manager.js'

function makeSetup(hooks?: TaskcastHooks) {
  const store = new MemoryShortTermStore()
  const broadcast = new MemoryBroadcastProvider()
  const engine = new TaskEngine({ shortTermStore: store, broadcast, hooks })
  const manager = new WorkerManager({ engine, shortTermStore: store, broadcast, hooks })
  return { store, broadcast, engine, manager }
}

const defaultRegistration: WorkerRegistration = {
  matchRule: {},
  capacity: 5,
  connectionMode: 'pull',
}

// ─── heartbeatIntervalMs getter ─────────────────────────────────────────────

describe('WorkerManager — heartbeatIntervalMs getter', () => {
  it('returns default 30000 when no defaults are configured', () => {
    const { manager } = makeSetup()
    expect(manager.heartbeatIntervalMs).toBe(30_000)
  })

  it('returns custom value when defaults.heartbeatIntervalMs is set', () => {
    const store = new MemoryShortTermStore()
    const broadcast = new MemoryBroadcastProvider()
    const engine = new TaskEngine({ shortTermStore: store, broadcast })
    const manager = new WorkerManager({
      engine,
      shortTermStore: store,
      broadcast,
      defaults: { heartbeatIntervalMs: 15_000 },
    })
    expect(manager.heartbeatIntervalMs).toBe(15_000)
  })
})

// ─── Worker Registration & Lifecycle (Task 3.1) ────────────────────────────

describe('WorkerManager — Registration & Lifecycle', () => {
  describe('registerWorker', () => {
    it('creates a worker with idle status', async () => {
      const { manager } = makeSetup()
      const worker = await manager.registerWorker(defaultRegistration)
      expect(worker.status).toBe('idle')
      expect(worker.capacity).toBe(5)
      expect(worker.connectionMode).toBe('pull')
    })

    it('generates ULID id if not provided', async () => {
      const { manager } = makeSetup()
      const worker = await manager.registerWorker(defaultRegistration)
      expect(worker.id).toBeTruthy()
      expect(worker.id.length).toBeGreaterThan(0)
    })

    it('uses provided id when given', async () => {
      const { manager } = makeSetup()
      const worker = await manager.registerWorker({ ...defaultRegistration, id: 'my-worker' })
      expect(worker.id).toBe('my-worker')
    })

    it('defaults weight to 50', async () => {
      const { manager } = makeSetup()
      const worker = await manager.registerWorker(defaultRegistration)
      expect(worker.weight).toBe(50)
    })

    it('uses provided weight', async () => {
      const { manager } = makeSetup()
      const worker = await manager.registerWorker({ ...defaultRegistration, weight: 80 })
      expect(worker.weight).toBe(80)
    })

    it('initializes usedSlots to 0', async () => {
      const { manager } = makeSetup()
      const worker = await manager.registerWorker(defaultRegistration)
      expect(worker.usedSlots).toBe(0)
    })

    it('sets connectedAt and lastHeartbeatAt', async () => {
      const { manager } = makeSetup()
      const before = Date.now()
      const worker = await manager.registerWorker(defaultRegistration)
      const after = Date.now()
      expect(worker.connectedAt).toBeGreaterThanOrEqual(before)
      expect(worker.connectedAt).toBeLessThanOrEqual(after)
      expect(worker.lastHeartbeatAt).toBe(worker.connectedAt)
    })

    it('persists worker in store', async () => {
      const { manager, store } = makeSetup()
      const worker = await manager.registerWorker(defaultRegistration)
      const stored = await store.getWorker(worker.id)
      expect(stored).not.toBeNull()
      expect(stored?.id).toBe(worker.id)
    })

    it('calls onWorkerConnected hook', async () => {
      const onWorkerConnected = vi.fn()
      const { manager } = makeSetup({ onWorkerConnected })
      const worker = await manager.registerWorker(defaultRegistration)
      expect(onWorkerConnected).toHaveBeenCalledOnce()
      expect(onWorkerConnected).toHaveBeenCalledWith(expect.objectContaining({ id: worker.id }))
    })

    it('stores metadata when provided', async () => {
      const { manager } = makeSetup()
      const worker = await manager.registerWorker({
        ...defaultRegistration,
        metadata: { region: 'us-east' },
      })
      expect(worker.metadata).toEqual({ region: 'us-east' })
    })

    it('omits metadata when not provided', async () => {
      const { manager } = makeSetup()
      const worker = await manager.registerWorker(defaultRegistration)
      expect(worker).not.toHaveProperty('metadata')
    })
  })

  describe('unregisterWorker', () => {
    it('removes worker from store', async () => {
      const { manager, store } = makeSetup()
      const worker = await manager.registerWorker(defaultRegistration)
      await manager.unregisterWorker(worker.id)
      const stored = await store.getWorker(worker.id)
      expect(stored).toBeNull()
    })

    it('calls onWorkerDisconnected hook', async () => {
      const onWorkerDisconnected = vi.fn()
      const { manager } = makeSetup({ onWorkerDisconnected })
      const worker = await manager.registerWorker(defaultRegistration)
      await manager.unregisterWorker(worker.id)
      expect(onWorkerDisconnected).toHaveBeenCalledOnce()
      expect(onWorkerDisconnected).toHaveBeenCalledWith(
        expect.objectContaining({ id: worker.id }),
        'unregistered',
      )
    })

    it('does not throw when unregistering unknown worker', async () => {
      const { manager } = makeSetup()
      await expect(manager.unregisterWorker('unknown')).resolves.not.toThrow()
    })

    it('does not call hook when worker not found', async () => {
      const onWorkerDisconnected = vi.fn()
      const { manager } = makeSetup({ onWorkerDisconnected })
      await manager.unregisterWorker('unknown')
      expect(onWorkerDisconnected).not.toHaveBeenCalled()
    })
  })

  describe('updateWorker', () => {
    it('updates weight', async () => {
      const { manager } = makeSetup()
      const worker = await manager.registerWorker(defaultRegistration)
      const updated = await manager.updateWorker(worker.id, { weight: 90 })
      expect(updated?.weight).toBe(90)
    })

    it('updates capacity', async () => {
      const { manager } = makeSetup()
      const worker = await manager.registerWorker(defaultRegistration)
      const updated = await manager.updateWorker(worker.id, { capacity: 10 })
      expect(updated?.capacity).toBe(10)
    })

    it('updates matchRule', async () => {
      const { manager } = makeSetup()
      const worker = await manager.registerWorker(defaultRegistration)
      const newRule = { taskTypes: ['llm.*'] }
      const updated = await manager.updateWorker(worker.id, { matchRule: newRule })
      expect(updated?.matchRule).toEqual(newRule)
    })

    it('returns null for unknown worker', async () => {
      const { manager } = makeSetup()
      const result = await manager.updateWorker('unknown', { weight: 90 })
      expect(result).toBeNull()
    })

    it('persists changes to store', async () => {
      const { manager, store } = makeSetup()
      const worker = await manager.registerWorker(defaultRegistration)
      await manager.updateWorker(worker.id, { weight: 90, capacity: 10 })
      const stored = await store.getWorker(worker.id)
      expect(stored?.weight).toBe(90)
      expect(stored?.capacity).toBe(10)
    })
  })

  describe('heartbeat', () => {
    it('updates lastHeartbeatAt', async () => {
      const { manager, store } = makeSetup()
      const worker = await manager.registerWorker(defaultRegistration)
      const originalHeartbeat = worker.lastHeartbeatAt

      // Small delay to ensure different timestamp
      await new Promise((r) => setTimeout(r, 2))

      await manager.heartbeat(worker.id)
      const stored = await store.getWorker(worker.id)
      expect(stored?.lastHeartbeatAt).toBeGreaterThanOrEqual(originalHeartbeat)
    })

    it('does not throw for unknown worker', async () => {
      const { manager } = makeSetup()
      await expect(manager.heartbeat('unknown')).resolves.not.toThrow()
    })
  })

  describe('getWorker', () => {
    it('returns worker by id', async () => {
      const { manager } = makeSetup()
      const worker = await manager.registerWorker(defaultRegistration)
      const found = await manager.getWorker(worker.id)
      expect(found?.id).toBe(worker.id)
    })

    it('returns null for unknown worker', async () => {
      const { manager } = makeSetup()
      const result = await manager.getWorker('unknown')
      expect(result).toBeNull()
    })
  })

  describe('listWorkers', () => {
    it('returns all workers', async () => {
      const { manager } = makeSetup()
      await manager.registerWorker({ ...defaultRegistration, id: 'w1' })
      await manager.registerWorker({ ...defaultRegistration, id: 'w2' })
      const workers = await manager.listWorkers()
      expect(workers).toHaveLength(2)
    })

    it('delegates filter to store', async () => {
      const { manager } = makeSetup()
      await manager.registerWorker({ ...defaultRegistration, id: 'w1', connectionMode: 'pull' })
      await manager.registerWorker({ ...defaultRegistration, id: 'w2', connectionMode: 'websocket' })
      const pullWorkers = await manager.listWorkers({ connectionMode: ['pull'] })
      expect(pullWorkers).toHaveLength(1)
      expect(pullWorkers[0]?.connectionMode).toBe('pull')
    })

    it('returns empty array when no workers', async () => {
      const { manager } = makeSetup()
      const workers = await manager.listWorkers()
      expect(workers).toEqual([])
    })
  })
})

// ─── Task Dispatch, Claim & Decline (Task 3.2) ─────────────────────────────

describe('WorkerManager — Dispatch, Claim & Decline', () => {
  describe('dispatchTask', () => {
    it('finds best worker by weight (highest first)', async () => {
      const { manager, engine } = makeSetup()
      await manager.registerWorker({ ...defaultRegistration, id: 'w-low', weight: 10 })
      await manager.registerWorker({ ...defaultRegistration, id: 'w-high', weight: 90 })
      await manager.registerWorker({ ...defaultRegistration, id: 'w-mid', weight: 50 })

      const task = await engine.createTask({ type: 'llm.chat' })
      const result = await manager.dispatchTask(task.id)
      expect(result.matched).toBe(true)
      expect(result.workerId).toBe('w-high')
    })

    it('uses available slots as tiebreaker (more available first)', async () => {
      const { manager, engine, store } = makeSetup()
      const w1 = await manager.registerWorker({ ...defaultRegistration, id: 'w1', weight: 50, capacity: 5 })
      const w2 = await manager.registerWorker({ ...defaultRegistration, id: 'w2', weight: 50, capacity: 10 })

      // Occupy some slots on w2 so w1 has more available
      w2.usedSlots = 8  // 2 available
      await store.saveWorker(w2)
      // w1 has 5 available

      const task = await engine.createTask({ type: 'test' })
      const result = await manager.dispatchTask(task.id)
      expect(result.matched).toBe(true)
      expect(result.workerId).toBe('w1') // 5 available > 2 available
    })

    it('uses connectedAt as final tiebreaker (oldest first)', async () => {
      const { manager, engine, store } = makeSetup()
      // Create workers with same weight and capacity but different connectedAt
      await manager.registerWorker({ ...defaultRegistration, id: 'w-new', weight: 50, capacity: 5 })
      await manager.registerWorker({ ...defaultRegistration, id: 'w-old', weight: 50, capacity: 5 })

      // Make w-old have an earlier connectedAt
      const wOld = await store.getWorker('w-old')
      if (wOld) {
        wOld.connectedAt = Date.now() - 10000
        await store.saveWorker(wOld)
      }

      const task = await engine.createTask({ type: 'test' })
      const result = await manager.dispatchTask(task.id)
      expect(result.matched).toBe(true)
      expect(result.workerId).toBe('w-old')
    })

    it('skips workers with no capacity', async () => {
      const { manager, engine, store } = makeSetup()
      await manager.registerWorker({ ...defaultRegistration, id: 'w-full', capacity: 1 })
      await manager.registerWorker({ ...defaultRegistration, id: 'w-free', capacity: 5 })

      // Fill up w-full
      const wFull = await store.getWorker('w-full')
      if (wFull) {
        wFull.usedSlots = 1
        await store.saveWorker(wFull)
      }

      const task = await engine.createTask({ type: 'test' })
      const result = await manager.dispatchTask(task.id)
      expect(result.matched).toBe(true)
      expect(result.workerId).toBe('w-free')
    })

    it('returns no match when no workers exist', async () => {
      const { manager, engine } = makeSetup()
      const task = await engine.createTask({ type: 'test' })
      const result = await manager.dispatchTask(task.id)
      expect(result.matched).toBe(false)
    })

    it('returns no match when all workers are at capacity', async () => {
      const { manager, engine, store } = makeSetup()
      await manager.registerWorker({ ...defaultRegistration, id: 'w1', capacity: 1 })
      const w1 = await store.getWorker('w1')
      if (w1) { w1.usedSlots = 1; await store.saveWorker(w1) }

      const task = await engine.createTask({ type: 'test' })
      const result = await manager.dispatchTask(task.id)
      expect(result.matched).toBe(false)
    })

    it('returns no match for non-pending task', async () => {
      const { manager, engine } = makeSetup()
      await manager.registerWorker(defaultRegistration)
      const task = await engine.createTask({ type: 'test' })
      await engine.transitionTask(task.id, 'running')
      const result = await manager.dispatchTask(task.id)
      expect(result.matched).toBe(false)
    })

    it('returns no match for unknown task', async () => {
      const { manager } = makeSetup()
      await manager.registerWorker(defaultRegistration)
      const result = await manager.dispatchTask('nonexistent')
      expect(result.matched).toBe(false)
    })

    it('skips blacklisted workers', async () => {
      const { manager, engine } = makeSetup()
      await manager.registerWorker({ ...defaultRegistration, id: 'w-banned', weight: 100 })
      await manager.registerWorker({ ...defaultRegistration, id: 'w-ok', weight: 10 })

      const task = await engine.createTask({
        type: 'test',
        metadata: { _blacklistedWorkers: ['w-banned'] },
      })
      const result = await manager.dispatchTask(task.id)
      expect(result.matched).toBe(true)
      expect(result.workerId).toBe('w-ok')
    })

    it('returns no match when all matching workers are blacklisted', async () => {
      const { manager, engine } = makeSetup()
      await manager.registerWorker({ ...defaultRegistration, id: 'w1' })

      const task = await engine.createTask({
        type: 'test',
        metadata: { _blacklistedWorkers: ['w1'] },
      })
      const result = await manager.dispatchTask(task.id)
      expect(result.matched).toBe(false)
    })

    it('skips draining workers', async () => {
      const { manager, engine, store } = makeSetup()
      await manager.registerWorker({ ...defaultRegistration, id: 'w-draining', weight: 100 })
      await manager.registerWorker({ ...defaultRegistration, id: 'w-idle', weight: 10 })

      const wDraining = await store.getWorker('w-draining')
      if (wDraining) {
        wDraining.status = 'draining'
        await store.saveWorker(wDraining)
      }

      const task = await engine.createTask({ type: 'test' })
      const result = await manager.dispatchTask(task.id)
      expect(result.matched).toBe(true)
      expect(result.workerId).toBe('w-idle')
    })

    it('skips offline workers', async () => {
      const { manager, engine, store } = makeSetup()
      await manager.registerWorker({ ...defaultRegistration, id: 'w-offline', weight: 100 })
      await manager.registerWorker({ ...defaultRegistration, id: 'w-idle', weight: 10 })

      const wOffline = await store.getWorker('w-offline')
      if (wOffline) {
        wOffline.status = 'offline'
        await store.saveWorker(wOffline)
      }

      const task = await engine.createTask({ type: 'test' })
      const result = await manager.dispatchTask(task.id)
      expect(result.matched).toBe(true)
      expect(result.workerId).toBe('w-idle')
    })

    it('filters workers by matchRule taskTypes', async () => {
      const { manager, engine } = makeSetup()
      await manager.registerWorker({
        ...defaultRegistration,
        id: 'w-llm',
        matchRule: { taskTypes: ['llm.*'] },
        weight: 100,
      })
      await manager.registerWorker({
        ...defaultRegistration,
        id: 'w-any',
        matchRule: {},
        weight: 10,
      })

      const task = await engine.createTask({ type: 'image.generate' })
      const result = await manager.dispatchTask(task.id)
      expect(result.matched).toBe(true)
      expect(result.workerId).toBe('w-any') // w-llm doesn't match image.generate
    })

    it('respects task cost when checking capacity', async () => {
      const { manager, engine } = makeSetup()
      await manager.registerWorker({ ...defaultRegistration, id: 'w1', capacity: 3 })

      const task = await engine.createTask({ type: 'test', cost: 4 })
      const result = await manager.dispatchTask(task.id)
      expect(result.matched).toBe(false) // cost 4 > capacity 3
    })

    it('uses default cost of 1 when task has no cost', async () => {
      const { manager, engine, store } = makeSetup()
      await manager.registerWorker({ ...defaultRegistration, id: 'w1', capacity: 1 })

      // usedSlots = 0, capacity = 1, cost defaults to 1 => 0 + 1 <= 1 => matches
      const task = await engine.createTask({ type: 'test' })
      const result = await manager.dispatchTask(task.id)
      expect(result.matched).toBe(true)
    })
  })

  describe('claimTask', () => {
    it('transitions task to assigned', async () => {
      const { manager, engine, store } = makeSetup()
      const worker = await manager.registerWorker(defaultRegistration)
      const task = await engine.createTask({ type: 'test' })

      const result = await manager.claimTask(task.id, worker.id)
      expect(result.success).toBe(true)

      const updated = await store.getTask(task.id)
      expect(updated?.status).toBe('assigned')
      expect(updated?.assignedWorker).toBe(worker.id)
    })

    it('creates assignment record', async () => {
      const { manager, engine, store } = makeSetup()
      const worker = await manager.registerWorker(defaultRegistration)
      const task = await engine.createTask({ type: 'test' })

      await manager.claimTask(task.id, worker.id)

      const assignment = await store.getTaskAssignment(task.id)
      expect(assignment).not.toBeNull()
      expect(assignment?.taskId).toBe(task.id)
      expect(assignment?.workerId).toBe(worker.id)
      expect(assignment?.cost).toBe(1) // default cost
      expect(assignment?.status).toBe('assigned')
    })

    it('uses task.cost for assignment cost', async () => {
      const { manager, engine, store } = makeSetup()
      const worker = await manager.registerWorker({ ...defaultRegistration, capacity: 10 })
      const task = await engine.createTask({ type: 'test', cost: 3 })

      await manager.claimTask(task.id, worker.id)

      const assignment = await store.getTaskAssignment(task.id)
      expect(assignment?.cost).toBe(3)
    })

    it('updates worker usedSlots', async () => {
      const { manager, engine, store } = makeSetup()
      const worker = await manager.registerWorker({ ...defaultRegistration, capacity: 5 })
      const task = await engine.createTask({ type: 'test', cost: 2 })

      await manager.claimTask(task.id, worker.id)

      const updated = await store.getWorker(worker.id)
      expect(updated?.usedSlots).toBe(2)
      expect(updated?.status).toBe('idle') // 2 < 5, still idle
    })

    it('sets worker status to busy when at capacity', async () => {
      const { manager, engine, store } = makeSetup()
      const worker = await manager.registerWorker({ ...defaultRegistration, capacity: 1 })
      const task = await engine.createTask({ type: 'test' })

      await manager.claimTask(task.id, worker.id)

      const updated = await store.getWorker(worker.id)
      expect(updated?.usedSlots).toBe(1)
      expect(updated?.status).toBe('busy')
    })

    it('fails for non-pending task', async () => {
      const { manager, engine } = makeSetup()
      const worker = await manager.registerWorker(defaultRegistration)
      const task = await engine.createTask({ type: 'test' })
      await engine.transitionTask(task.id, 'running')

      const result = await manager.claimTask(task.id, worker.id)
      expect(result.success).toBe(false)
      expect(result.reason).toContain('not pending')
    })

    it('fails for unknown task', async () => {
      const { manager } = makeSetup()
      const worker = await manager.registerWorker(defaultRegistration)

      const result = await manager.claimTask('nonexistent', worker.id)
      expect(result.success).toBe(false)
      expect(result.reason).toContain('not found')
    })

    it('calls onTaskAssigned hook', async () => {
      const onTaskAssigned = vi.fn()
      const { manager, engine } = makeSetup({ onTaskAssigned })
      const worker = await manager.registerWorker(defaultRegistration)
      const task = await engine.createTask({ type: 'test' })

      await manager.claimTask(task.id, worker.id)

      expect(onTaskAssigned).toHaveBeenCalledOnce()
      expect(onTaskAssigned).toHaveBeenCalledWith(
        expect.objectContaining({ id: task.id, status: 'assigned' }),
        expect.objectContaining({ id: worker.id }),
      )
    })

    it('saves to longTermStore when configured', async () => {
      const store = new MemoryShortTermStore()
      const broadcast = new MemoryBroadcastProvider()
      const longTermStore = {
        saveTask: vi.fn().mockResolvedValue(undefined),
        getTask: vi.fn().mockResolvedValue(null),
        saveEvent: vi.fn().mockResolvedValue(undefined),
        getEvents: vi.fn().mockResolvedValue([]),
        saveWorkerEvent: vi.fn().mockResolvedValue(undefined),
        getWorkerEvents: vi.fn().mockResolvedValue([]),
      }
      const engine = new TaskEngine({ shortTermStore: store, broadcast, longTermStore })
      const manager = new WorkerManager({ engine, shortTermStore: store, broadcast, longTermStore })

      const worker = await manager.registerWorker(defaultRegistration)
      const task = await engine.createTask({ type: 'test' })
      vi.mocked(longTermStore.saveTask).mockClear()

      await manager.claimTask(task.id, worker.id)

      // Should have been called for the assigned task
      expect(longTermStore.saveTask).toHaveBeenCalled()
    })
  })

  describe('declineTask', () => {
    async function setupClaimedTask(hooks?: TaskcastHooks) {
      const setup = makeSetup(hooks)
      const worker = await setup.manager.registerWorker({ ...defaultRegistration, capacity: 5 })
      const task = await setup.engine.createTask({ type: 'test', cost: 2 })
      await setup.manager.claimTask(task.id, worker.id)
      return { ...setup, worker, task }
    }

    it('returns task to pending', async () => {
      const { manager, store, task, worker } = await setupClaimedTask()

      await manager.declineTask(task.id, worker.id)

      const updated = await store.getTask(task.id)
      expect(updated?.status).toBe('pending')
    })

    it('clears assignedWorker', async () => {
      const { manager, store, task, worker } = await setupClaimedTask()

      await manager.declineTask(task.id, worker.id)

      const updated = await store.getTask(task.id)
      expect(updated?.assignedWorker).toBeUndefined()
    })

    it('restores worker capacity', async () => {
      const { manager, store, task, worker } = await setupClaimedTask()

      // Before decline, worker should have usedSlots = 2
      const beforeDecline = await store.getWorker(worker.id)
      expect(beforeDecline?.usedSlots).toBe(2)

      await manager.declineTask(task.id, worker.id)

      const updated = await store.getWorker(worker.id)
      expect(updated?.usedSlots).toBe(0)
      expect(updated?.status).toBe('idle')
    })

    it('removes assignment record', async () => {
      const { manager, store, task, worker } = await setupClaimedTask()

      await manager.declineTask(task.id, worker.id)

      const assignment = await store.getTaskAssignment(task.id)
      expect(assignment).toBeNull()
    })

    it('with blacklist adds workerId to exclusion list', async () => {
      const { manager, store, task, worker } = await setupClaimedTask()

      await manager.declineTask(task.id, worker.id, { blacklist: true })

      const updated = await store.getTask(task.id)
      expect(updated?.metadata?._blacklistedWorkers).toContain(worker.id)
    })

    it('with blacklist appends to existing blacklist', async () => {
      const setup = makeSetup()
      const worker1 = await setup.manager.registerWorker({ ...defaultRegistration, id: 'w1', capacity: 5 })
      const worker2 = await setup.manager.registerWorker({ ...defaultRegistration, id: 'w2', capacity: 5 })
      const task = await setup.engine.createTask({
        type: 'test',
        cost: 1,
        metadata: { _blacklistedWorkers: ['w0'] },
      })

      // Claim and decline with w1
      await setup.manager.claimTask(task.id, worker1.id)
      await setup.manager.declineTask(task.id, worker1.id, { blacklist: true })

      const updated = await setup.store.getTask(task.id)
      const blacklist = updated?.metadata?._blacklistedWorkers as string[]
      expect(blacklist).toContain('w0')
      expect(blacklist).toContain('w1')
      expect(blacklist).toHaveLength(2)
    })

    it('without blacklist does not modify metadata', async () => {
      const { manager, store, task, worker } = await setupClaimedTask()

      await manager.declineTask(task.id, worker.id)

      const updated = await store.getTask(task.id)
      expect(updated?.metadata?._blacklistedWorkers).toBeUndefined()
    })

    it('does nothing when assignment not found', async () => {
      const { manager, engine } = makeSetup()
      const worker = await manager.registerWorker(defaultRegistration)
      const task = await engine.createTask({ type: 'test' })

      // No claim was made, so decline should be a no-op
      await expect(manager.declineTask(task.id, worker.id)).resolves.not.toThrow()
    })

    it('does nothing when workerId does not match assignment', async () => {
      const { manager, store, task, worker } = await setupClaimedTask()

      // Try to decline with a different worker
      const otherWorker = await manager.registerWorker({ ...defaultRegistration, id: 'other' })
      await manager.declineTask(task.id, otherWorker.id)

      // Task should still be assigned
      const updated = await store.getTask(task.id)
      expect(updated?.status).toBe('assigned')
    })

    it('calls onTaskDeclined hook', async () => {
      const onTaskDeclined = vi.fn()
      const { manager, task, worker } = await setupClaimedTask({ onTaskDeclined })

      await manager.declineTask(task.id, worker.id, { blacklist: true })

      expect(onTaskDeclined).toHaveBeenCalledOnce()
      expect(onTaskDeclined).toHaveBeenCalledWith(
        expect.objectContaining({ id: task.id }),
        expect.objectContaining({ id: worker.id }),
        true,
      )
    })

    it('calls onTaskDeclined with blacklisted=false when not blacklisting', async () => {
      const onTaskDeclined = vi.fn()
      const { manager, task, worker } = await setupClaimedTask({ onTaskDeclined })

      await manager.declineTask(task.id, worker.id)

      expect(onTaskDeclined).toHaveBeenCalledWith(
        expect.anything(),
        expect.anything(),
        false,
      )
    })
  })

  describe('getWorkerTasks', () => {
    it('returns assignments for a worker', async () => {
      const { manager, engine } = makeSetup()
      const worker = await manager.registerWorker({ ...defaultRegistration, capacity: 10 })
      const task1 = await engine.createTask({ type: 'test' })
      const task2 = await engine.createTask({ type: 'test' })

      await manager.claimTask(task1.id, worker.id)
      await manager.claimTask(task2.id, worker.id)

      const assignments = await manager.getWorkerTasks(worker.id)
      expect(assignments).toHaveLength(2)
      expect(assignments.map((a) => a.taskId).sort()).toEqual([task1.id, task2.id].sort())
    })

    it('returns empty array for worker with no assignments', async () => {
      const { manager } = makeSetup()
      const worker = await manager.registerWorker(defaultRegistration)
      const assignments = await manager.getWorkerTasks(worker.id)
      expect(assignments).toEqual([])
    })

    it('returns empty array for unknown worker', async () => {
      const { manager } = makeSetup()
      const assignments = await manager.getWorkerTasks('unknown')
      expect(assignments).toEqual([])
    })
  })
})

// ─── Pull Mode — waitForTask & notifyNewTask (Task 5.1) ──────────────────

describe('WorkerManager — Pull Mode', () => {
  describe('waitForTask', () => {
    it('resolves immediately if pending pull task exists', async () => {
      const { manager, engine } = makeSetup()
      const worker = await manager.registerWorker({
        ...defaultRegistration,
        id: 'w1',
        connectionMode: 'pull',
      })

      // Create a pending pull task
      const task = await engine.createTask({ type: 'test', assignMode: 'pull' })

      const result = await manager.waitForTask(worker.id)
      expect(result.id).toBe(task.id)
      expect(result.status).toBe('assigned')
    })

    it('resolves when matching task is notified', async () => {
      const { manager, engine } = makeSetup()
      const worker = await manager.registerWorker({
        ...defaultRegistration,
        id: 'w1',
        connectionMode: 'pull',
      })

      // No existing tasks — start waiting, then create and notify
      const waitPromise = manager.waitForTask(worker.id)

      // Small delay to let the subscription establish
      await new Promise((r) => setTimeout(r, 10))

      const task = await engine.createTask({ type: 'test', assignMode: 'pull' })
      await manager.notifyNewTask(task.id)

      const result = await waitPromise
      expect(result.id).toBe(task.id)
      expect(result.status).toBe('assigned')
    })

    it('can be aborted', async () => {
      const { manager } = makeSetup()
      await manager.registerWorker({
        ...defaultRegistration,
        id: 'w1',
        connectionMode: 'pull',
      })

      const controller = new AbortController()

      const waitPromise = manager.waitForTask('w1', controller.signal)

      // Small delay, then abort
      await new Promise((r) => setTimeout(r, 10))
      controller.abort()

      await expect(waitPromise).rejects.toThrow('aborted')
    })

    it('throws for unknown worker', async () => {
      const { manager } = makeSetup()
      await expect(manager.waitForTask('unknown')).rejects.toThrow('Worker not found')
    })

    it('throws immediately if signal already aborted', async () => {
      const { manager } = makeSetup()
      await manager.registerWorker({
        ...defaultRegistration,
        id: 'w1',
        connectionMode: 'pull',
      })

      const controller = new AbortController()
      controller.abort()

      await expect(manager.waitForTask('w1', controller.signal)).rejects.toThrow('aborted')
    })

    it('skips tasks that do not match worker rule', async () => {
      const { manager, engine } = makeSetup()
      await manager.registerWorker({
        ...defaultRegistration,
        id: 'w1',
        connectionMode: 'pull',
        matchRule: { taskTypes: ['gpu.*'] },
      })

      // Create a task that doesn't match the worker's rule
      await engine.createTask({ type: 'cpu.inference', assignMode: 'pull' })

      const controller = new AbortController()
      const timeout = setTimeout(() => controller.abort(), 50)

      await expect(manager.waitForTask('w1', controller.signal)).rejects.toThrow('aborted')
      clearTimeout(timeout)
    })

    it('skips blacklisted tasks', async () => {
      const { manager, engine } = makeSetup()
      await manager.registerWorker({
        ...defaultRegistration,
        id: 'w1',
        connectionMode: 'pull',
      })

      // Create a task where w1 is blacklisted
      await engine.createTask({
        type: 'test',
        assignMode: 'pull',
        metadata: { _blacklistedWorkers: ['w1'] },
      })

      const controller = new AbortController()
      const timeout = setTimeout(() => controller.abort(), 50)

      await expect(manager.waitForTask('w1', controller.signal)).rejects.toThrow('aborted')
      clearTimeout(timeout)
    })

    it('skips non-pull tasks when notified', async () => {
      const { manager, engine } = makeSetup()
      await manager.registerWorker({
        ...defaultRegistration,
        id: 'w1',
        connectionMode: 'pull',
      })

      const controller = new AbortController()
      const waitPromise = manager.waitForTask('w1', controller.signal)

      await new Promise((r) => setTimeout(r, 10))

      // Create an external task and notify — should not match
      const task = await engine.createTask({ type: 'test', assignMode: 'external' })
      await manager.notifyNewTask(task.id)

      // Wait a bit, then abort since it shouldn't have resolved
      await new Promise((r) => setTimeout(r, 20))
      controller.abort()

      await expect(waitPromise).rejects.toThrow('aborted')
    })
  })

  describe('notifyNewTask', () => {
    it('publishes to broadcast channel', async () => {
      const { manager, broadcast } = makeSetup()
      const publishSpy = vi.spyOn(broadcast, 'publish')

      await manager.notifyNewTask('task-123')

      expect(publishSpy).toHaveBeenCalledOnce()
      expect(publishSpy).toHaveBeenCalledWith(
        'taskcast:worker:new-task',
        expect.objectContaining({
          taskId: 'system',
          type: 'taskcast:worker:new-task',
          data: 'task-123',
        }),
      )
    })
  })
})

// ─── Audit Events (Task 7.1) ─────────────────────────────────────────────

describe('WorkerManager — Audit Events', () => {
  function makeLongTermMock(): LongTermStore {
    return {
      saveTask: vi.fn().mockResolvedValue(undefined),
      getTask: vi.fn().mockResolvedValue(null),
      saveEvent: vi.fn().mockResolvedValue(undefined),
      getEvents: vi.fn().mockResolvedValue([]),
      saveWorkerEvent: vi.fn().mockResolvedValue(undefined),
      getWorkerEvents: vi.fn().mockResolvedValue([]),
    }
  }

  function makeSetupWithLongTerm(hooks?: TaskcastHooks) {
    const store = new MemoryShortTermStore()
    const broadcast = new MemoryBroadcastProvider()
    const longTermStore = makeLongTermMock()
    const engine = new TaskEngine({ shortTermStore: store, broadcast, longTermStore, hooks })
    const manager = new WorkerManager({ engine, shortTermStore: store, broadcast, longTermStore, hooks })
    return { store, broadcast, engine, manager, longTermStore }
  }

  describe('task audit events', () => {
    it('emits taskcast:audit event on claim', async () => {
      const { manager, engine, store } = makeSetupWithLongTerm()
      const worker = await manager.registerWorker(defaultRegistration)
      const task = await engine.createTask({ type: 'test' })

      await manager.claimTask(task.id, worker.id)

      const events = await store.getEvents(task.id)
      const auditEvents = events.filter((e) => e.type === 'taskcast:audit')
      expect(auditEvents.length).toBeGreaterThanOrEqual(1)
      const assignedAudit = auditEvents.find(
        (e) => (e.data as Record<string, unknown>).action === 'assigned',
      )
      expect(assignedAudit).toBeDefined()
      expect((assignedAudit!.data as Record<string, unknown>).workerId).toBe(worker.id)
    })

    it('emits taskcast:audit event on decline', async () => {
      const { manager, engine, store } = makeSetupWithLongTerm()
      const worker = await manager.registerWorker({ ...defaultRegistration, capacity: 5 })
      const task = await engine.createTask({ type: 'test', cost: 1 })

      await manager.claimTask(task.id, worker.id)
      await manager.declineTask(task.id, worker.id, { blacklist: true })

      const events = await store.getEvents(task.id)
      const auditEvents = events.filter((e) => e.type === 'taskcast:audit')
      const declinedAudit = auditEvents.find(
        (e) => (e.data as Record<string, unknown>).action === 'declined',
      )
      expect(declinedAudit).toBeDefined()
      expect((declinedAudit!.data as Record<string, unknown>).workerId).toBe(worker.id)
      expect((declinedAudit!.data as Record<string, unknown>).blacklisted).toBe(true)
    })
  })

  describe('worker audit events', () => {
    it('emits worker audit event to longTermStore on register', async () => {
      const { manager, longTermStore } = makeSetupWithLongTerm()
      const worker = await manager.registerWorker(defaultRegistration)

      // Allow async saveWorkerEvent to settle
      await new Promise((r) => setTimeout(r, 10))

      expect(longTermStore.saveWorkerEvent).toHaveBeenCalledWith(
        expect.objectContaining({
          workerId: worker.id,
          action: 'connected',
        }),
      )
    })

    it('emits worker audit event on unregister', async () => {
      const { manager, longTermStore } = makeSetupWithLongTerm()
      const worker = await manager.registerWorker(defaultRegistration)
      vi.mocked(longTermStore.saveWorkerEvent).mockClear()

      await manager.unregisterWorker(worker.id)

      // Allow async saveWorkerEvent to settle
      await new Promise((r) => setTimeout(r, 10))

      expect(longTermStore.saveWorkerEvent).toHaveBeenCalledWith(
        expect.objectContaining({
          workerId: worker.id,
          action: 'disconnected',
          data: { reason: 'unregistered' },
        }),
      )
    })

    it('emits task_assigned worker audit event on claim', async () => {
      const { manager, engine, longTermStore } = makeSetupWithLongTerm()
      const worker = await manager.registerWorker(defaultRegistration)
      const task = await engine.createTask({ type: 'test' })
      vi.mocked(longTermStore.saveWorkerEvent).mockClear()

      await manager.claimTask(task.id, worker.id)

      // Allow async saveWorkerEvent to settle
      await new Promise((r) => setTimeout(r, 10))

      expect(longTermStore.saveWorkerEvent).toHaveBeenCalledWith(
        expect.objectContaining({
          workerId: worker.id,
          action: 'task_assigned',
          data: { taskId: task.id },
        }),
      )
    })

    it('emits task_declined worker audit event on decline', async () => {
      const { manager, engine, longTermStore } = makeSetupWithLongTerm()
      const worker = await manager.registerWorker({ ...defaultRegistration, capacity: 5 })
      const task = await engine.createTask({ type: 'test', cost: 1 })
      await manager.claimTask(task.id, worker.id)
      vi.mocked(longTermStore.saveWorkerEvent).mockClear()

      await manager.declineTask(task.id, worker.id)

      // Allow async saveWorkerEvent to settle
      await new Promise((r) => setTimeout(r, 10))

      expect(longTermStore.saveWorkerEvent).toHaveBeenCalledWith(
        expect.objectContaining({
          workerId: worker.id,
          action: 'task_declined',
          data: { taskId: task.id },
        }),
      )
    })

    it('emits pull_request audit event', async () => {
      const { manager, engine, longTermStore } = makeSetupWithLongTerm()
      const worker = await manager.registerWorker({
        ...defaultRegistration,
        id: 'w1',
        connectionMode: 'pull',
      })
      const task = await engine.createTask({ type: 'test', assignMode: 'pull' })
      vi.mocked(longTermStore.saveWorkerEvent).mockClear()

      await manager.waitForTask('w1')

      // Allow async saveWorkerEvent to settle
      await new Promise((r) => setTimeout(r, 10))

      expect(longTermStore.saveWorkerEvent).toHaveBeenCalledWith(
        expect.objectContaining({
          workerId: 'w1',
          action: 'pull_request',
          data: expect.objectContaining({ matched: true, taskId: task.id }),
        }),
      )
    })

    it('does not emit worker audit events without longTermStore', async () => {
      const { manager } = makeSetup()
      // registerWorker should not throw without longTermStore
      await expect(manager.registerWorker(defaultRegistration)).resolves.not.toThrow()
    })
  })
})

// ─── Uncovered Edge Cases ────────────────────────────────────────────────────

describe('WorkerManager — emitTaskAudit catch block', () => {
  it('silently catches when publishEvent throws (best-effort audit)', async () => {
    const { manager, engine } = makeSetup()
    const worker = await manager.registerWorker(defaultRegistration)
    const task = await engine.createTask({ type: 'test' })

    // Make publishEvent throw to trigger the catch block in emitTaskAudit
    const publishSpy = vi.spyOn(engine, 'publishEvent').mockRejectedValueOnce(
      new Error('Cannot publish to task in terminal status: completed'),
    )

    // claimTask calls emitTaskAudit which will hit the catch block
    const result = await manager.claimTask(task.id, worker.id)
    // claimTask should still succeed — audit is best-effort
    expect(result.success).toBe(true)
    expect(publishSpy).toHaveBeenCalled()

    publishSpy.mockRestore()
  })
})

describe('WorkerManager — claimTask concurrent modification failure', () => {
  it('returns failure when shortTermStore.claimTask returns false', async () => {
    const store = new MemoryShortTermStore()
    const broadcast = new MemoryBroadcastProvider()
    const engine = new TaskEngine({ shortTermStore: store, broadcast })
    const manager = new WorkerManager({ engine, shortTermStore: store, broadcast })

    const worker = await manager.registerWorker(defaultRegistration)
    const task = await engine.createTask({ type: 'test' })

    // Mock claimTask to return false, simulating concurrent modification
    const claimSpy = vi.spyOn(store, 'claimTask').mockResolvedValueOnce(false)

    const result = await manager.claimTask(task.id, worker.id)
    expect(result.success).toBe(false)
    expect(result.reason).toContain('concurrent modification')

    claimSpy.mockRestore()
  })
})

describe('WorkerManager — waitForTask broadcast edge cases', () => {
  it('rejects when worker is deleted during broadcast wait', async () => {
    const { manager, engine, store } = makeSetup()
    await manager.registerWorker({
      ...defaultRegistration,
      id: 'w1',
      connectionMode: 'pull',
    })

    // No existing pending pull tasks, so it will enter the broadcast wait
    const waitPromise = manager.waitForTask('w1')

    // Wait for subscription to establish
    await new Promise((r) => setTimeout(r, 10))

    // Delete the worker while it's waiting
    await store.deleteWorker('w1')

    // Notify a new task — the handler will re-fetch the worker and find it gone
    const task = await engine.createTask({ type: 'test', assignMode: 'pull' })
    await manager.notifyNewTask(task.id)

    await expect(waitPromise).rejects.toThrow('Worker not found: w1')
  })

  it('catches errors thrown in the broadcast handler', async () => {
    const store = new MemoryShortTermStore()
    const broadcast = new MemoryBroadcastProvider()
    const engine = new TaskEngine({ shortTermStore: store, broadcast })
    const manager = new WorkerManager({ engine, shortTermStore: store, broadcast })

    await manager.registerWorker({
      ...defaultRegistration,
      id: 'w1',
      connectionMode: 'pull',
    })

    // Start waiting
    const controller = new AbortController()
    const waitPromise = manager.waitForTask('w1', controller.signal)

    // Wait for subscription to establish
    await new Promise((r) => setTimeout(r, 10))

    // Make getWorker throw an error to trigger the catch block at line 390
    const getWorkerSpy = vi.spyOn(store, 'getWorker').mockRejectedValueOnce(
      new Error('store connection lost'),
    )

    // Notify a new task — the handler will try getWorker, which throws
    const task = await engine.createTask({ type: 'test', assignMode: 'pull' })
    await manager.notifyNewTask(task.id)

    // Wait for the error to be caught (silently)
    await new Promise((r) => setTimeout(r, 20))

    // The promise should still be pending since the error was caught
    // Abort to clean up
    controller.abort()

    await expect(waitPromise).rejects.toThrow('aborted')

    getWorkerSpy.mockRestore()
  })
})

describe('WorkerManager — declineTask with longTermStore', () => {
  it('saves task to longTermStore when configured', async () => {
    const store = new MemoryShortTermStore()
    const broadcast = new MemoryBroadcastProvider()
    const longTermStore: LongTermStore = {
      saveTask: vi.fn().mockResolvedValue(undefined),
      getTask: vi.fn().mockResolvedValue(null),
      saveEvent: vi.fn().mockResolvedValue(undefined),
      getEvents: vi.fn().mockResolvedValue([]),
      saveWorkerEvent: vi.fn().mockResolvedValue(undefined),
      getWorkerEvents: vi.fn().mockResolvedValue([]),
    }
    const engine = new TaskEngine({ shortTermStore: store, broadcast, longTermStore })
    const manager = new WorkerManager({ engine, shortTermStore: store, broadcast, longTermStore })

    const worker = await manager.registerWorker({ ...defaultRegistration, capacity: 5 })
    const task = await engine.createTask({ type: 'test', cost: 1 })
    await manager.claimTask(task.id, worker.id)

    vi.mocked(longTermStore.saveTask).mockClear()

    await manager.declineTask(task.id, worker.id)

    // declineTask should call longTermStore.saveTask to persist the declined task
    expect(longTermStore.saveTask).toHaveBeenCalled()
    const savedTask = vi.mocked(longTermStore.saveTask).mock.calls.find(
      (call) => (call[0] as { id: string }).id === task.id && (call[0] as { status: string }).status === 'pending'
    )
    expect(savedTask).toBeDefined()
  })
})
