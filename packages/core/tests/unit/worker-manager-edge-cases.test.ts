import { describe, it, expect, vi } from 'vitest'
import { TaskEngine } from '../../src/engine.js'
import { MemoryBroadcastProvider, MemoryShortTermStore } from '../../src/memory-adapters.js'
import { WorkerManager } from '../../src/worker-manager.js'
import type { TaskcastHooks, LongTermStore } from '../../src/types.js'
import type { WorkerRegistration } from '../../src/worker-manager.js'

function makeSetup(hooks?: TaskcastHooks) {
  const store = new MemoryShortTermStore()
  const broadcast = new MemoryBroadcastProvider()
  const engine = new TaskEngine({ shortTermStore: store, broadcast, hooks })
  const manager = new WorkerManager({ engine, shortTermStore: store, broadcast, hooks })
  return { store, broadcast, engine, manager }
}

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

const defaultRegistration: WorkerRegistration = {
  matchRule: {},
  capacity: 5,
  connectionMode: 'pull',
}

// ─── Boundary Values ────────────────────────────────────────────────────────

describe('WorkerManager — Boundary Values', () => {
  describe('cost = 0', () => {
    it('dispatches a task with cost 0 successfully', async () => {
      const { manager, engine } = makeSetup()
      await manager.registerWorker({ ...defaultRegistration, id: 'w1', capacity: 1 })

      const task = await engine.createTask({ type: 'test', cost: 0 })
      const result = await manager.dispatchTask(task.id)
      expect(result.matched).toBe(true)
      expect(result.workerId).toBe('w1')
    })

    it('claims a task with cost 0 without consuming capacity', async () => {
      const { manager, engine, store } = makeSetup()
      const worker = await manager.registerWorker({ ...defaultRegistration, id: 'w1', capacity: 1 })

      const task = await engine.createTask({ type: 'test', cost: 0 })
      const result = await manager.claimTask(task.id, worker.id)
      expect(result.success).toBe(true)

      const updated = await store.getWorker(worker.id)
      expect(updated?.usedSlots).toBe(0)
      expect(updated?.status).toBe('idle') // 0 < 1, still idle
    })

    it('allows multiple cost-0 tasks to be claimed by same worker', async () => {
      const { manager, engine, store } = makeSetup()
      const worker = await manager.registerWorker({ ...defaultRegistration, id: 'w1', capacity: 1 })

      const task1 = await engine.createTask({ type: 'test', cost: 0 })
      const task2 = await engine.createTask({ type: 'test', cost: 0 })
      const task3 = await engine.createTask({ type: 'test', cost: 0 })

      const r1 = await manager.claimTask(task1.id, worker.id)
      const r2 = await manager.claimTask(task2.id, worker.id)
      const r3 = await manager.claimTask(task3.id, worker.id)

      expect(r1.success).toBe(true)
      expect(r2.success).toBe(true)
      expect(r3.success).toBe(true)

      const updated = await store.getWorker(worker.id)
      expect(updated?.usedSlots).toBe(0)
    })
  })

  describe('cost > capacity', () => {
    it('dispatch returns no match when task cost exceeds all worker capacities', async () => {
      const { manager, engine } = makeSetup()
      await manager.registerWorker({ ...defaultRegistration, id: 'w1', capacity: 3 })
      await manager.registerWorker({ ...defaultRegistration, id: 'w2', capacity: 5 })

      const task = await engine.createTask({ type: 'test', cost: 10 })
      const result = await manager.dispatchTask(task.id)
      expect(result.matched).toBe(false)
    })

    it('claimTask fails when cost exceeds worker capacity', async () => {
      const { manager, engine } = makeSetup()
      const worker = await manager.registerWorker({ ...defaultRegistration, id: 'w1', capacity: 2 })

      const task = await engine.createTask({ type: 'test', cost: 5 })
      const result = await manager.claimTask(task.id, worker.id)
      // claimTask delegates to store.claimTask which checks capacity
      expect(result.success).toBe(false)
    })
  })

  describe('weight = 0', () => {
    it('worker with weight 0 is still selectable when it is the only candidate', async () => {
      const { manager, engine } = makeSetup()
      await manager.registerWorker({ ...defaultRegistration, id: 'w1', weight: 0 })

      const task = await engine.createTask({ type: 'test' })
      const result = await manager.dispatchTask(task.id)
      expect(result.matched).toBe(true)
      expect(result.workerId).toBe('w1')
    })

    it('worker with weight 0 loses to worker with weight > 0', async () => {
      const { manager, engine } = makeSetup()
      await manager.registerWorker({ ...defaultRegistration, id: 'w-zero', weight: 0 })
      await manager.registerWorker({ ...defaultRegistration, id: 'w-one', weight: 1 })

      const task = await engine.createTask({ type: 'test' })
      const result = await manager.dispatchTask(task.id)
      expect(result.matched).toBe(true)
      expect(result.workerId).toBe('w-one')
    })
  })

  describe('identical weight, slots, connectedAt — tiebreaker determinism', () => {
    it('produces deterministic results for workers with identical sort keys', async () => {
      const { manager, engine, store } = makeSetup()
      // Register workers with identical weight and capacity
      await manager.registerWorker({ ...defaultRegistration, id: 'w-a', weight: 50, capacity: 5 })
      await manager.registerWorker({ ...defaultRegistration, id: 'w-b', weight: 50, capacity: 5 })
      await manager.registerWorker({ ...defaultRegistration, id: 'w-c', weight: 50, capacity: 5 })

      // Force identical connectedAt
      const fixedTime = Date.now()
      for (const id of ['w-a', 'w-b', 'w-c']) {
        const w = await store.getWorker(id)
        if (w) {
          w.connectedAt = fixedTime
          await store.saveWorker(w)
        }
      }

      const task1 = await engine.createTask({ type: 'test' })
      const result1 = await manager.dispatchTask(task1.id)

      const task2 = await engine.createTask({ type: 'test' })
      const result2 = await manager.dispatchTask(task2.id)

      // Both dispatches should pick the same worker (deterministic)
      expect(result1.matched).toBe(true)
      expect(result2.matched).toBe(true)
      expect(result1.workerId).toBe(result2.workerId)
    })
  })

  describe('very large capacity values', () => {
    it('handles capacity of Number.MAX_SAFE_INTEGER', async () => {
      const { manager, engine } = makeSetup()
      await manager.registerWorker({
        ...defaultRegistration,
        id: 'w-huge',
        capacity: Number.MAX_SAFE_INTEGER,
      })

      const task = await engine.createTask({ type: 'test', cost: 1000000 })
      const result = await manager.dispatchTask(task.id)
      expect(result.matched).toBe(true)
      expect(result.workerId).toBe('w-huge')
    })

    it('claims a task with very large cost against very large capacity', async () => {
      const { manager, engine, store } = makeSetup()
      const worker = await manager.registerWorker({
        ...defaultRegistration,
        id: 'w-huge',
        capacity: Number.MAX_SAFE_INTEGER,
      })

      const task = await engine.createTask({ type: 'test', cost: 999999999 })
      const result = await manager.claimTask(task.id, worker.id)
      expect(result.success).toBe(true)

      const updated = await store.getWorker(worker.id)
      expect(updated?.usedSlots).toBe(999999999)
    })
  })
})

// ─── Blacklist Edge Cases ───────────────────────────────────────────────────

describe('WorkerManager — Blacklist Edge Cases', () => {
  it('accumulates blacklist entries across multiple declines by different workers', async () => {
    const { manager, engine, store } = makeSetup()
    const wA = await manager.registerWorker({ ...defaultRegistration, id: 'w-a', capacity: 5 })
    const wB = await manager.registerWorker({ ...defaultRegistration, id: 'w-b', capacity: 5 })
    const wC = await manager.registerWorker({ ...defaultRegistration, id: 'w-c', capacity: 5 })

    const task = await engine.createTask({ type: 'test', cost: 1 })

    // Worker A claims and declines with blacklist
    await manager.claimTask(task.id, wA.id)
    await manager.declineTask(task.id, wA.id, { blacklist: true })

    // Worker B claims and declines with blacklist
    await manager.claimTask(task.id, wB.id)
    await manager.declineTask(task.id, wB.id, { blacklist: true })

    // Worker C claims and declines with blacklist
    await manager.claimTask(task.id, wC.id)
    await manager.declineTask(task.id, wC.id, { blacklist: true })

    const updated = await store.getTask(task.id)
    const blacklist = updated?.metadata?._blacklistedWorkers as string[]
    expect(blacklist).toContain('w-a')
    expect(blacklist).toContain('w-b')
    expect(blacklist).toContain('w-c')
    expect(blacklist).toHaveLength(3)
  })

  it('blacklist persists when task is re-dispatched multiple times', async () => {
    const { manager, engine, store } = makeSetup()
    await manager.registerWorker({ ...defaultRegistration, id: 'w-bad', weight: 100, capacity: 5 })
    await manager.registerWorker({ ...defaultRegistration, id: 'w-good', weight: 10, capacity: 5 })

    const task = await engine.createTask({ type: 'test', cost: 1 })

    // First dispatch picks w-bad (higher weight)
    const first = await manager.dispatchTask(task.id)
    expect(first.workerId).toBe('w-bad')

    // Claim, decline with blacklist
    await manager.claimTask(task.id, 'w-bad')
    await manager.declineTask(task.id, 'w-bad', { blacklist: true })

    // Re-dispatch: should skip w-bad now
    const second = await manager.dispatchTask(task.id)
    expect(second.matched).toBe(true)
    expect(second.workerId).toBe('w-good')

    // Claim w-good and decline with blacklist
    await manager.claimTask(task.id, 'w-good')
    await manager.declineTask(task.id, 'w-good', { blacklist: true })

    // Re-dispatch: both workers are blacklisted, no match
    const third = await manager.dispatchTask(task.id)
    expect(third.matched).toBe(false)

    // Verify blacklist still has both entries
    const updated = await store.getTask(task.id)
    const blacklist = updated?.metadata?._blacklistedWorkers as string[]
    expect(blacklist).toContain('w-bad')
    expect(blacklist).toContain('w-good')
  })

  it('dispatch skips ALL blacklisted workers even when they have higher weight', async () => {
    const { manager, engine } = makeSetup()
    await manager.registerWorker({ ...defaultRegistration, id: 'w-best', weight: 100, capacity: 5 })
    await manager.registerWorker({ ...defaultRegistration, id: 'w-good', weight: 80, capacity: 5 })
    await manager.registerWorker({ ...defaultRegistration, id: 'w-ok', weight: 1, capacity: 5 })

    const task = await engine.createTask({
      type: 'test',
      metadata: { _blacklistedWorkers: ['w-best', 'w-good'] },
    })

    const result = await manager.dispatchTask(task.id)
    expect(result.matched).toBe(true)
    expect(result.workerId).toBe('w-ok')
  })
})

// ─── Error Propagation — Hooks ──────────────────────────────────────────────

describe('WorkerManager — Hook Error Propagation', () => {
  describe('onWorkerConnected throws', () => {
    it('propagates the error from registerWorker', async () => {
      const hooks: TaskcastHooks = {
        onWorkerConnected: vi.fn(() => {
          throw new Error('hook explosion')
        }),
      }
      const { manager } = makeSetup(hooks)

      // The hook is called synchronously at the end of registerWorker;
      // if it throws, the error should propagate.
      await expect(manager.registerWorker(defaultRegistration)).rejects.toThrow('hook explosion')
    })
  })

  describe('onWorkerDisconnected throws', () => {
    it('propagates the error from unregisterWorker', async () => {
      const hooks: TaskcastHooks = {
        onWorkerDisconnected: vi.fn(() => {
          throw new Error('disconnect hook error')
        }),
      }
      const { manager } = makeSetup(hooks)
      const worker = await manager.registerWorker(defaultRegistration)

      await expect(manager.unregisterWorker(worker.id)).rejects.toThrow('disconnect hook error')
    })
  })

  describe('onTaskAssigned throws', () => {
    it('propagates the error from claimTask', async () => {
      const hooks: TaskcastHooks = {
        onTaskAssigned: vi.fn(() => {
          throw new Error('assign hook error')
        }),
      }
      const { manager, engine } = makeSetup(hooks)
      const worker = await manager.registerWorker(defaultRegistration)
      const task = await engine.createTask({ type: 'test' })

      await expect(manager.claimTask(task.id, worker.id)).rejects.toThrow('assign hook error')
    })
  })

  describe('onTaskDeclined throws', () => {
    it('propagates the error from declineTask', async () => {
      // Use a separate hooks object where onTaskAssigned works, but onTaskDeclined throws
      let assignCallCount = 0
      const hooks: TaskcastHooks = {
        onTaskDeclined: vi.fn(() => {
          throw new Error('decline hook error')
        }),
      }
      const { manager, engine } = makeSetup(hooks)
      const worker = await manager.registerWorker({ ...defaultRegistration, capacity: 5 })
      const task = await engine.createTask({ type: 'test', cost: 1 })

      // Claim first (no onTaskAssigned hook error since it's not set)
      await manager.claimTask(task.id, worker.id)

      await expect(manager.declineTask(task.id, worker.id)).rejects.toThrow('decline hook error')
    })
  })

  describe('emitTaskAudit error handling', () => {
    it('silently catches when publishEvent throws during claim', async () => {
      const { manager, engine } = makeSetup()
      const worker = await manager.registerWorker(defaultRegistration)
      const task = await engine.createTask({ type: 'test' })

      // Make publishEvent throw to trigger the catch block
      const spy = vi.spyOn(engine, 'publishEvent').mockRejectedValue(
        new Error('terminal state'),
      )

      const result = await manager.claimTask(task.id, worker.id)
      expect(result.success).toBe(true) // audit failure does not block claim
      spy.mockRestore()
    })

    it('silently catches when publishEvent throws during decline', async () => {
      const { manager, engine } = makeSetup()
      const worker = await manager.registerWorker({ ...defaultRegistration, capacity: 5 })
      const task = await engine.createTask({ type: 'test', cost: 1 })
      await manager.claimTask(task.id, worker.id)

      // Make publishEvent throw
      const spy = vi.spyOn(engine, 'publishEvent').mockRejectedValue(
        new Error('terminal state'),
      )

      // declineTask calls emitTaskAudit; it should not throw
      await expect(manager.declineTask(task.id, worker.id)).resolves.not.toThrow()
      spy.mockRestore()
    })
  })
})

// ─── Worker Lifecycle Edge Cases ────────────────────────────────────────────

describe('WorkerManager — Worker Lifecycle Edge Cases', () => {
  describe('reconnect with same worker_id', () => {
    it('overwrites existing worker data when registering with same id', async () => {
      const { manager, store } = makeSetup()

      const first = await manager.registerWorker({
        ...defaultRegistration,
        id: 'w-reconnect',
        weight: 10,
        capacity: 3,
      })
      expect(first.weight).toBe(10)

      const second = await manager.registerWorker({
        ...defaultRegistration,
        id: 'w-reconnect',
        weight: 90,
        capacity: 8,
      })
      expect(second.id).toBe('w-reconnect')
      expect(second.weight).toBe(90)
      expect(second.capacity).toBe(8)
      expect(second.usedSlots).toBe(0) // reset on re-register

      const stored = await store.getWorker('w-reconnect')
      expect(stored?.weight).toBe(90)
      expect(stored?.capacity).toBe(8)
    })

    it('fires onWorkerConnected hook on each registration', async () => {
      const onWorkerConnected = vi.fn()
      const { manager } = makeSetup({ onWorkerConnected })

      await manager.registerWorker({ ...defaultRegistration, id: 'w-reconnect' })
      await manager.registerWorker({ ...defaultRegistration, id: 'w-reconnect' })

      expect(onWorkerConnected).toHaveBeenCalledTimes(2)
    })
  })

  describe('operations on deleted/unregistered worker', () => {
    it('claimTask fails when worker is not registered', async () => {
      const { manager, engine } = makeSetup()
      const task = await engine.createTask({ type: 'test' })

      // No worker registered — store.claimTask checks worker existence
      const result = await manager.claimTask(task.id, 'ghost-worker')
      expect(result.success).toBe(false)
    })

    it('declineTask is a no-op when no assignment exists for unregistered worker', async () => {
      const { manager, engine } = makeSetup()
      const task = await engine.createTask({ type: 'test' })

      await expect(manager.declineTask(task.id, 'ghost-worker')).resolves.not.toThrow()
    })

    it('heartbeat is a no-op for unregistered worker', async () => {
      const { manager } = makeSetup()

      await expect(manager.heartbeat('ghost-worker')).resolves.not.toThrow()
    })

    it('updateWorker returns null for unregistered worker', async () => {
      const { manager } = makeSetup()

      const result = await manager.updateWorker('ghost-worker', { weight: 99 })
      expect(result).toBeNull()
    })
  })

  describe('draining worker', () => {
    it('draining worker is excluded from dispatch candidates', async () => {
      const { manager, engine } = makeSetup()
      await manager.registerWorker({ ...defaultRegistration, id: 'w-drain', weight: 100 })
      await manager.registerWorker({ ...defaultRegistration, id: 'w-idle', weight: 10 })

      await manager.updateWorker('w-drain', { status: 'draining' })

      const task = await engine.createTask({ type: 'test' })
      const result = await manager.dispatchTask(task.id)
      expect(result.matched).toBe(true)
      expect(result.workerId).toBe('w-idle')
    })

    it('draining worker that is the only candidate results in no match', async () => {
      const { manager, engine } = makeSetup()
      await manager.registerWorker({ ...defaultRegistration, id: 'w-drain' })
      await manager.updateWorker('w-drain', { status: 'draining' })

      const task = await engine.createTask({ type: 'test' })
      const result = await manager.dispatchTask(task.id)
      expect(result.matched).toBe(false)
    })
  })

  describe('multiple heartbeats in quick succession', () => {
    it('each heartbeat updates lastHeartbeatAt to a monotonically increasing value', async () => {
      const { manager, store } = makeSetup()
      const worker = await manager.registerWorker({ ...defaultRegistration, id: 'w-hb' })
      const timestamps: number[] = []

      for (let i = 0; i < 5; i++) {
        await manager.heartbeat('w-hb')
        const w = await store.getWorker('w-hb')
        timestamps.push(w!.lastHeartbeatAt)
      }

      // Each timestamp should be >= the previous one
      for (let i = 1; i < timestamps.length; i++) {
        expect(timestamps[i]).toBeGreaterThanOrEqual(timestamps[i - 1]!)
      }
    })
  })

  describe('decline on already-declined task (idempotency)', () => {
    it('second decline is a no-op because assignment was already removed', async () => {
      const { manager, engine, store } = makeSetup()
      const worker = await manager.registerWorker({ ...defaultRegistration, capacity: 5 })
      const task = await engine.createTask({ type: 'test', cost: 1 })

      await manager.claimTask(task.id, worker.id)
      await manager.declineTask(task.id, worker.id, { blacklist: true })

      // Second decline — assignment no longer exists
      await expect(manager.declineTask(task.id, worker.id, { blacklist: true })).resolves.not.toThrow()

      // Blacklist should not have duplicate entries
      const updated = await store.getTask(task.id)
      const blacklist = updated?.metadata?._blacklistedWorkers as string[]
      expect(blacklist).toEqual([worker.id])
    })
  })

  describe('claim on already-claimed task by different worker', () => {
    it('second claim by different worker fails due to non-pending status', async () => {
      const { manager, engine } = makeSetup()
      const w1 = await manager.registerWorker({ ...defaultRegistration, id: 'w1', capacity: 5 })
      const w2 = await manager.registerWorker({ ...defaultRegistration, id: 'w2', capacity: 5 })

      const task = await engine.createTask({ type: 'test' })

      // First claim succeeds
      const r1 = await manager.claimTask(task.id, w1.id)
      expect(r1.success).toBe(true)

      // Second claim by different worker fails
      const r2 = await manager.claimTask(task.id, w2.id)
      expect(r2.success).toBe(false)
      expect(r2.reason).toContain('not pending')
    })
  })
})

// ─── Capacity & Assignment ──────────────────────────────────────────────────

describe('WorkerManager — Capacity & Assignment', () => {
  describe('multiple tasks exhausting capacity exactly', () => {
    it('worker transitions to busy when usedSlots equals capacity', async () => {
      const { manager, engine, store } = makeSetup()
      const worker = await manager.registerWorker({
        ...defaultRegistration,
        id: 'w1',
        capacity: 5,
      })

      // Claim tasks with costs that sum to exactly capacity: 2 + 2 + 1 = 5
      const task1 = await engine.createTask({ type: 'test', cost: 2 })
      const task2 = await engine.createTask({ type: 'test', cost: 2 })
      const task3 = await engine.createTask({ type: 'test', cost: 1 })

      const r1 = await manager.claimTask(task1.id, worker.id)
      expect(r1.success).toBe(true)
      let w = await store.getWorker(worker.id)
      expect(w?.usedSlots).toBe(2)
      expect(w?.status).toBe('idle')

      const r2 = await manager.claimTask(task2.id, worker.id)
      expect(r2.success).toBe(true)
      w = await store.getWorker(worker.id)
      expect(w?.usedSlots).toBe(4)
      expect(w?.status).toBe('idle')

      const r3 = await manager.claimTask(task3.id, worker.id)
      expect(r3.success).toBe(true)
      w = await store.getWorker(worker.id)
      expect(w?.usedSlots).toBe(5)
      expect(w?.status).toBe('busy')
    })

    it('further claims are rejected when capacity is exactly exhausted', async () => {
      const { manager, engine } = makeSetup()
      const worker = await manager.registerWorker({
        ...defaultRegistration,
        id: 'w1',
        capacity: 3,
      })

      const task1 = await engine.createTask({ type: 'test', cost: 3 })
      const task2 = await engine.createTask({ type: 'test', cost: 1 })

      await manager.claimTask(task1.id, worker.id)

      // Worker is at capacity (3/3), next claim should fail
      const r2 = await manager.claimTask(task2.id, worker.id)
      expect(r2.success).toBe(false)
    })
  })

  describe('declining multiple tasks restores correct capacity', () => {
    it('restores exact cost of each declined task', async () => {
      const { manager, engine, store } = makeSetup()
      const worker = await manager.registerWorker({
        ...defaultRegistration,
        id: 'w1',
        capacity: 10,
      })

      const task1 = await engine.createTask({ type: 'test', cost: 3 })
      const task2 = await engine.createTask({ type: 'test', cost: 5 })

      await manager.claimTask(task1.id, worker.id)
      await manager.claimTask(task2.id, worker.id)

      let w = await store.getWorker(worker.id)
      expect(w?.usedSlots).toBe(8) // 3 + 5

      // Decline task1 (cost 3)
      await manager.declineTask(task1.id, worker.id)
      w = await store.getWorker(worker.id)
      expect(w?.usedSlots).toBe(5) // 8 - 3
      expect(w?.status).toBe('idle')

      // Decline task2 (cost 5)
      await manager.declineTask(task2.id, worker.id)
      w = await store.getWorker(worker.id)
      expect(w?.usedSlots).toBe(0) // 5 - 5
      expect(w?.status).toBe('idle')
    })

    it('usedSlots never goes below 0 on decline', async () => {
      // This tests the Math.max(0, ...) guard in declineTask
      const { manager, engine, store } = makeSetup()
      const worker = await manager.registerWorker({
        ...defaultRegistration,
        id: 'w1',
        capacity: 5,
      })

      const task = await engine.createTask({ type: 'test', cost: 2 })
      await manager.claimTask(task.id, worker.id)

      // Manually set usedSlots to 0 to simulate a bug/race condition
      const w = await store.getWorker(worker.id)
      if (w) {
        w.usedSlots = 0
        await store.saveWorker(w)
      }

      // Decline should not result in negative usedSlots
      await manager.declineTask(task.id, worker.id)
      const updated = await store.getWorker(worker.id)
      expect(updated?.usedSlots).toBe(0)
    })
  })

  describe('worker status transitions: idle → busy → idle', () => {
    it('transitions idle → busy when at capacity, then back to idle after decline', async () => {
      const { manager, engine, store } = makeSetup()
      const worker = await manager.registerWorker({
        ...defaultRegistration,
        id: 'w1',
        capacity: 2,
      })

      // Initially idle
      let w = await store.getWorker(worker.id)
      expect(w?.status).toBe('idle')

      // Claim task with cost 1 — still idle (1 < 2)
      const task1 = await engine.createTask({ type: 'test', cost: 1 })
      await manager.claimTask(task1.id, worker.id)
      w = await store.getWorker(worker.id)
      expect(w?.status).toBe('idle')
      expect(w?.usedSlots).toBe(1)

      // Claim task with cost 1 — now busy (2 >= 2)
      const task2 = await engine.createTask({ type: 'test', cost: 1 })
      await manager.claimTask(task2.id, worker.id)
      w = await store.getWorker(worker.id)
      expect(w?.status).toBe('busy')
      expect(w?.usedSlots).toBe(2)

      // Decline task1 — back to idle (1 < 2)
      await manager.declineTask(task1.id, worker.id)
      w = await store.getWorker(worker.id)
      expect(w?.status).toBe('idle')
      expect(w?.usedSlots).toBe(1)

      // Decline task2 — still idle (0 < 2)
      await manager.declineTask(task2.id, worker.id)
      w = await store.getWorker(worker.id)
      expect(w?.status).toBe('idle')
      expect(w?.usedSlots).toBe(0)
    })
  })
})
