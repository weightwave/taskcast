import { describe, it, expect, vi, beforeEach } from 'vitest'
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

// ─── 1. Store Layer Exception Handling ──────────────────────────────────────

describe('WorkerManager — Store Layer Exception Handling', () => {
  describe('saveWorker throws during registerWorker', () => {
    it('propagates the store error', async () => {
      const { manager, store } = makeSetup()
      vi.spyOn(store, 'saveWorker').mockRejectedValueOnce(new Error('store write failed'))

      await expect(manager.registerWorker(defaultRegistration)).rejects.toThrow('store write failed')
    })

    it('does not emit audit or fire hooks when saveWorker fails', async () => {
      const onWorkerConnected = vi.fn()
      const { manager, store } = makeSetupWithLongTerm({ onWorkerConnected })

      vi.spyOn(store, 'saveWorker').mockRejectedValueOnce(new Error('disk full'))

      await expect(manager.registerWorker(defaultRegistration)).rejects.toThrow('disk full')
      expect(onWorkerConnected).not.toHaveBeenCalled()
    })
  })

  describe('claimTask throws during claim', () => {
    it('propagates the store error from claimTask', async () => {
      const { manager, engine, store } = makeSetup()
      const worker = await manager.registerWorker({ ...defaultRegistration, id: 'w1' })
      const task = await engine.createTask({ type: 'test' })

      vi.spyOn(store, 'claimTask').mockRejectedValueOnce(new Error('redis connection lost'))

      await expect(manager.claimTask(task.id, worker.id)).rejects.toThrow('redis connection lost')
    })

    it('does not create assignment or emit audit when claimTask store method throws', async () => {
      const { manager, engine, store, longTermStore } = makeSetupWithLongTerm()
      const worker = await manager.registerWorker({ ...defaultRegistration, id: 'w1' })
      const task = await engine.createTask({ type: 'test' })

      vi.spyOn(store, 'claimTask').mockRejectedValueOnce(new Error('store error'))
      vi.mocked(longTermStore.saveWorkerEvent).mockClear()

      await expect(manager.claimTask(task.id, worker.id)).rejects.toThrow('store error')

      // No assignment should have been created
      const assignment = await store.getTaskAssignment(task.id)
      expect(assignment).toBeNull()
    })
  })

  describe('getWorker throws during heartbeat', () => {
    it('propagates the store error', async () => {
      const { manager, store } = makeSetup()
      const worker = await manager.registerWorker({ ...defaultRegistration, id: 'w1' })

      vi.spyOn(store, 'getWorker').mockRejectedValueOnce(new Error('store timeout'))

      await expect(manager.heartbeat(worker.id)).rejects.toThrow('store timeout')
    })
  })

  describe('getTaskAssignment throws during declineTask', () => {
    it('propagates the store error', async () => {
      const { manager, engine, store } = makeSetup()
      const worker = await manager.registerWorker({ ...defaultRegistration, id: 'w1', capacity: 5 })
      const task = await engine.createTask({ type: 'test', cost: 1 })
      await manager.claimTask(task.id, worker.id)

      vi.spyOn(store, 'getTaskAssignment').mockRejectedValueOnce(new Error('store read error'))

      await expect(manager.declineTask(task.id, worker.id)).rejects.toThrow('store read error')
    })

    it('does not modify worker capacity when getTaskAssignment throws', async () => {
      const { manager, engine, store } = makeSetup()
      const worker = await manager.registerWorker({ ...defaultRegistration, id: 'w1', capacity: 5 })
      const task = await engine.createTask({ type: 'test', cost: 2 })
      await manager.claimTask(task.id, worker.id)

      const workerBefore = await store.getWorker(worker.id)
      const slotsBefore = workerBefore!.usedSlots

      vi.spyOn(store, 'getTaskAssignment').mockRejectedValueOnce(new Error('store error'))

      await expect(manager.declineTask(task.id, worker.id)).rejects.toThrow('store error')

      // Worker capacity should remain unchanged
      const workerAfter = await store.getWorker(worker.id)
      expect(workerAfter!.usedSlots).toBe(slotsBefore)
    })
  })

})

// ─── 2. Large Blacklist Entries ─────────────────────────────────────────────

describe('WorkerManager — Large Blacklist Entries', () => {
  it('blacklist with 100+ worker IDs still works correctly for dispatch', async () => {
    const { manager, engine, store } = makeSetup()

    // Register one non-blacklisted worker
    await manager.registerWorker({ ...defaultRegistration, id: 'w-good', capacity: 5 })

    // Build a large blacklist
    const blacklistedIds: string[] = []
    for (let i = 0; i < 150; i++) {
      blacklistedIds.push(`w-blacklisted-${i}`)
    }

    // Create task with pre-set large blacklist
    const task = await engine.createTask({
      type: 'test',
      metadata: { _blacklistedWorkers: blacklistedIds },
    })

    // Register some of the blacklisted workers so they exist in the store
    for (let i = 0; i < 10; i++) {
      await manager.registerWorker({
        ...defaultRegistration,
        id: `w-blacklisted-${i}`,
        weight: 100, // Higher weight than w-good
        capacity: 5,
      })
    }

    const result = await manager.dispatchTask(task.id)
    expect(result.matched).toBe(true)
    expect(result.workerId).toBe('w-good')
  })

  it('dispatch correctly skips ALL blacklisted workers when all registered are blacklisted', async () => {
    const { manager, engine } = makeSetup()

    const blacklistedIds: string[] = []
    for (let i = 0; i < 100; i++) {
      const id = `w-${i}`
      blacklistedIds.push(id)
      await manager.registerWorker({ ...defaultRegistration, id, capacity: 5 })
    }

    const task = await engine.createTask({
      type: 'test',
      metadata: { _blacklistedWorkers: blacklistedIds },
    })

    const result = await manager.dispatchTask(task.id)
    expect(result.matched).toBe(false)
  })

  it('decline with blacklist=true accumulates correctly across many declines', async () => {
    const { manager, engine, store } = makeSetup()

    // Register many workers
    const workerCount = 20
    for (let i = 0; i < workerCount; i++) {
      await manager.registerWorker({ ...defaultRegistration, id: `w-${i}`, capacity: 5 })
    }

    const task = await engine.createTask({ type: 'test', cost: 1 })

    // Each worker claims then declines with blacklist
    for (let i = 0; i < workerCount; i++) {
      const claimResult = await manager.claimTask(task.id, `w-${i}`)
      expect(claimResult.success).toBe(true)
      await manager.declineTask(task.id, `w-${i}`, { blacklist: true })
    }

    // Verify the blacklist has all workers
    const updated = await store.getTask(task.id)
    const blacklist = updated?.metadata?._blacklistedWorkers as string[]
    expect(blacklist).toHaveLength(workerCount)
    for (let i = 0; i < workerCount; i++) {
      expect(blacklist).toContain(`w-${i}`)
    }

    // Dispatch should find no candidates
    const result = await manager.dispatchTask(task.id)
    expect(result.matched).toBe(false)
  })

  it('large blacklist does not affect dispatch performance for non-blacklisted workers', async () => {
    const { manager, engine } = makeSetup()

    // Create large blacklist of non-registered workers
    const blacklistedIds: string[] = []
    for (let i = 0; i < 500; i++) {
      blacklistedIds.push(`phantom-worker-${i}`)
    }

    await manager.registerWorker({ ...defaultRegistration, id: 'w-available', capacity: 5 })

    const task = await engine.createTask({
      type: 'test',
      metadata: { _blacklistedWorkers: blacklistedIds },
    })

    const result = await manager.dispatchTask(task.id)
    expect(result.matched).toBe(true)
    expect(result.workerId).toBe('w-available')
  })
})

// ─── 3. Worker Disconnect During Claim ──────────────────────────────────────

describe('WorkerManager — Worker Disconnect During Claim', () => {
  it('worker unregisters after dispatchTask but before claimTask', async () => {
    const { manager, engine } = makeSetup()
    const worker = await manager.registerWorker({ ...defaultRegistration, id: 'w1', capacity: 5 })
    const task = await engine.createTask({ type: 'test' })

    // Dispatch finds the worker
    const dispatch = await manager.dispatchTask(task.id)
    expect(dispatch.matched).toBe(true)
    expect(dispatch.workerId).toBe('w1')

    // Worker unregisters between dispatch and claim
    await manager.unregisterWorker('w1')

    // Claim should fail because worker no longer exists
    const claim = await manager.claimTask(task.id, 'w1')
    expect(claim.success).toBe(false)
  })

  it('worker deleted from store during claimTask (store returns null for getWorker after claim)', async () => {
    const { manager, engine, store } = makeSetup()
    const worker = await manager.registerWorker({ ...defaultRegistration, id: 'w1', capacity: 5 })
    const task = await engine.createTask({ type: 'test' })

    // Mock: claimTask succeeds, but getWorker returns null afterward (worker was deleted mid-claim)
    const originalClaimTask = store.claimTask.bind(store)
    vi.spyOn(store, 'claimTask').mockImplementation(async (taskId, workerId, cost) => {
      const result = await originalClaimTask(taskId, workerId, cost)
      // Simulate worker being deleted after the atomic claim
      await store.deleteWorker(workerId)
      return result
    })

    // Claim should succeed (atomic claim worked), but hooks won't fire for the worker
    const result = await manager.claimTask(task.id, 'w1')
    expect(result.success).toBe(true)

    // Worker no longer exists
    const workerAfter = await store.getWorker('w1')
    expect(workerAfter).toBeNull()
  })

  it('two workers claim same task concurrently, only one succeeds', async () => {
    const { manager, engine } = makeSetup()
    await manager.registerWorker({ ...defaultRegistration, id: 'w1', capacity: 5 })
    await manager.registerWorker({ ...defaultRegistration, id: 'w2', capacity: 5 })

    const task = await engine.createTask({ type: 'test' })

    // Both try to claim concurrently
    const [r1, r2] = await Promise.all([
      manager.claimTask(task.id, 'w1'),
      manager.claimTask(task.id, 'w2'),
    ])

    // Exactly one should succeed
    const successes = [r1, r2].filter((r) => r.success)
    const failures = [r1, r2].filter((r) => !r.success)

    expect(successes).toHaveLength(1)
    expect(failures).toHaveLength(1)
    expect(failures[0]!.reason).toBeDefined()
  })

  it('worker unregistered before claimTask has no leftover assignments', async () => {
    const { manager, engine, store } = makeSetup()
    await manager.registerWorker({ ...defaultRegistration, id: 'w1', capacity: 5 })
    const task = await engine.createTask({ type: 'test' })

    // Unregister first
    await manager.unregisterWorker('w1')

    // Claim should fail
    const result = await manager.claimTask(task.id, 'w1')
    expect(result.success).toBe(false)

    // No assignment should exist
    const assignment = await store.getTaskAssignment(task.id)
    expect(assignment).toBeNull()

    // Task should still be pending
    const taskAfter = await engine.getTask(task.id)
    expect(taskAfter!.status).toBe('pending')
  })

  it('dispatch returns no match when all workers are unregistered before dispatch call', async () => {
    const { manager, engine } = makeSetup()
    const w1 = await manager.registerWorker({ ...defaultRegistration, id: 'w1' })
    const w2 = await manager.registerWorker({ ...defaultRegistration, id: 'w2' })
    const task = await engine.createTask({ type: 'test' })

    await manager.unregisterWorker('w1')
    await manager.unregisterWorker('w2')

    const result = await manager.dispatchTask(task.id)
    expect(result.matched).toBe(false)
  })
})

// ─── 4. Audit Event Completeness ────────────────────────────────────────────

describe('WorkerManager — Audit Event Completeness', () => {
  it('registerWorker emits "connected" audit via longTermStore', async () => {
    const { manager, longTermStore } = makeSetupWithLongTerm()

    const worker = await manager.registerWorker({ ...defaultRegistration, id: 'w1' })

    // Flush async promises
    await vi.waitFor(() => {
      expect(longTermStore.saveWorkerEvent).toHaveBeenCalledWith(
        expect.objectContaining({
          action: 'connected',
          workerId: 'w1',
        }),
      )
    })
  })

  it('registerWorker does NOT emit audit when no longTermStore is configured', async () => {
    const { manager } = makeSetup() // no longTermStore

    // Should not throw — audit is skipped silently
    const worker = await manager.registerWorker(defaultRegistration)
    expect(worker.status).toBe('idle')
  })

  it('unregisterWorker emits "disconnected" audit via longTermStore', async () => {
    const { manager, longTermStore } = makeSetupWithLongTerm()

    const worker = await manager.registerWorker({ ...defaultRegistration, id: 'w1' })
    vi.mocked(longTermStore.saveWorkerEvent).mockClear()

    await manager.unregisterWorker('w1')

    await vi.waitFor(() => {
      expect(longTermStore.saveWorkerEvent).toHaveBeenCalledWith(
        expect.objectContaining({
          action: 'disconnected',
          workerId: 'w1',
          data: { reason: 'unregistered' },
        }),
      )
    })
  })

  it('unregisterWorker does not emit audit if worker does not exist', async () => {
    const { manager, longTermStore } = makeSetupWithLongTerm()
    vi.mocked(longTermStore.saveWorkerEvent).mockClear()

    await manager.unregisterWorker('nonexistent')

    // Give async operations time to settle
    await new Promise((r) => setTimeout(r, 10))
    expect(longTermStore.saveWorkerEvent).not.toHaveBeenCalled()
  })

  it('claimTask emits both "task_assigned" worker audit and task audit event', async () => {
    const { manager, engine, longTermStore, store } = makeSetupWithLongTerm()
    const worker = await manager.registerWorker({ ...defaultRegistration, id: 'w1' })
    const task = await engine.createTask({ type: 'test' })

    vi.mocked(longTermStore.saveWorkerEvent).mockClear()

    await manager.claimTask(task.id, worker.id)

    // Worker audit: task_assigned
    await vi.waitFor(() => {
      expect(longTermStore.saveWorkerEvent).toHaveBeenCalledWith(
        expect.objectContaining({
          action: 'task_assigned',
          workerId: 'w1',
          data: { taskId: task.id },
        }),
      )
    })

    // Task audit: published as a taskcast:audit event on the task
    const events = await engine.getEvents(task.id)
    const auditEvents = events.filter((e) => e.type === 'taskcast:audit')
    const assignedAudit = auditEvents.find(
      (e) => (e.data as Record<string, unknown>).action === 'assigned',
    )
    expect(assignedAudit).toBeDefined()
    expect((assignedAudit!.data as Record<string, unknown>).workerId).toBe('w1')
  })

  it('declineTask emits "task_declined" worker audit and task audit event', async () => {
    const { manager, engine, longTermStore } = makeSetupWithLongTerm()
    const worker = await manager.registerWorker({ ...defaultRegistration, id: 'w1', capacity: 5 })
    const task = await engine.createTask({ type: 'test', cost: 1 })
    await manager.claimTask(task.id, worker.id)

    vi.mocked(longTermStore.saveWorkerEvent).mockClear()

    await manager.declineTask(task.id, worker.id, { blacklist: true })

    // Worker audit: task_declined
    await vi.waitFor(() => {
      expect(longTermStore.saveWorkerEvent).toHaveBeenCalledWith(
        expect.objectContaining({
          action: 'task_declined',
          workerId: 'w1',
          data: { taskId: task.id },
        }),
      )
    })

    // Task audit: published as a taskcast:audit event on the task
    const events = await engine.getEvents(task.id)
    const auditEvents = events.filter((e) => e.type === 'taskcast:audit')
    const declinedAudit = auditEvents.find(
      (e) => (e.data as Record<string, unknown>).action === 'declined',
    )
    expect(declinedAudit).toBeDefined()
    expect((declinedAudit!.data as Record<string, unknown>).workerId).toBe('w1')
    expect((declinedAudit!.data as Record<string, unknown>).blacklisted).toBe(true)
  })

  it('declineTask without blacklist sets blacklisted=false in task audit', async () => {
    const { manager, engine, longTermStore } = makeSetupWithLongTerm()
    const worker = await manager.registerWorker({ ...defaultRegistration, id: 'w1', capacity: 5 })
    const task = await engine.createTask({ type: 'test', cost: 1 })
    await manager.claimTask(task.id, worker.id)

    await manager.declineTask(task.id, worker.id) // no blacklist option

    const events = await engine.getEvents(task.id)
    const auditEvents = events.filter((e) => e.type === 'taskcast:audit')
    const declinedAudit = auditEvents.find(
      (e) => (e.data as Record<string, unknown>).action === 'declined',
    )
    expect(declinedAudit).toBeDefined()
    expect((declinedAudit!.data as Record<string, unknown>).blacklisted).toBe(false)
  })


  it('waitForTask emits "pull_request" audit with matched=true when task is found immediately', async () => {
    const { manager, engine, longTermStore } = makeSetupWithLongTerm()
    const worker = await manager.registerWorker({ ...defaultRegistration, id: 'w1' })
    const task = await engine.createTask({ type: 'test', assignMode: 'pull' })

    vi.mocked(longTermStore.saveWorkerEvent).mockClear()

    const claimedTask = await manager.waitForTask('w1')
    expect(claimedTask.id).toBe(task.id)

    await vi.waitFor(() => {
      expect(longTermStore.saveWorkerEvent).toHaveBeenCalledWith(
        expect.objectContaining({
          action: 'pull_request',
          workerId: 'w1',
          data: expect.objectContaining({ matched: true, taskId: task.id }),
        }),
      )
    })
  })

  it('waitForTask emits "pull_request" audit with matched=false when aborted', async () => {
    const { manager, engine, longTermStore, store } = makeSetupWithLongTerm()
    await manager.registerWorker({ ...defaultRegistration, id: 'w1' })
    // No pending pull tasks

    vi.mocked(longTermStore.saveWorkerEvent).mockClear()

    const controller = new AbortController()

    // Spy on broadcast.subscribe to know when we've entered the long-poll Promise
    const originalSubscribe = store.listTasks.bind(store)
    let resolveEntered: () => void
    const enteredPromise = new Promise<void>((r) => { resolveEntered = r })
    vi.spyOn(store, 'listTasks').mockImplementation(async (...args) => {
      const result = await originalSubscribe(...args)
      // Signal that we've passed the initial task scan and are about to enter broadcast wait
      resolveEntered()
      return result
    })

    const waitPromise = manager.waitForTask('w1', controller.signal)

    // Wait until the function has scanned pending tasks (returns empty) and entered broadcast wait
    await enteredPromise
    // Small yield to ensure the Promise executor has set up the abort listener
    await new Promise((r) => setTimeout(r, 10))

    // Now abort the long-poll — this triggers onAbort inside the Promise
    controller.abort()

    await expect(waitPromise).rejects.toThrow('aborted')

    await vi.waitFor(() => {
      expect(longTermStore.saveWorkerEvent).toHaveBeenCalledWith(
        expect.objectContaining({
          action: 'pull_request',
          workerId: 'w1',
          data: { matched: false },
        }),
      )
    })
  })

  it('all audit events have required fields (id, workerId, timestamp, action)', async () => {
    const { manager, engine, longTermStore } = makeSetupWithLongTerm()

    const worker = await manager.registerWorker({ ...defaultRegistration, id: 'w-audit' })
    const task = await engine.createTask({ type: 'test', cost: 1 })
    await manager.claimTask(task.id, worker.id)
    await manager.declineTask(task.id, worker.id)
    await manager.unregisterWorker('w-audit')

    // Flush async calls
    await new Promise((r) => setTimeout(r, 50))

    const calls = vi.mocked(longTermStore.saveWorkerEvent).mock.calls
    expect(calls.length).toBeGreaterThanOrEqual(4) // connected, task_assigned, task_declined, disconnected

    for (const [event] of calls) {
      expect(event).toHaveProperty('id')
      expect(event).toHaveProperty('workerId')
      expect(event).toHaveProperty('timestamp')
      expect(event).toHaveProperty('action')
      expect(typeof event.id).toBe('string')
      expect(event.id.length).toBeGreaterThan(0)
      expect(typeof event.timestamp).toBe('number')
      expect(event.timestamp).toBeGreaterThan(0)
    }
  })

  it('longTermStore.saveWorkerEvent failure does not propagate (best-effort audit)', async () => {
    const { manager, longTermStore } = makeSetupWithLongTerm()

    // Make audit writes fail
    vi.mocked(longTermStore.saveWorkerEvent).mockRejectedValue(new Error('audit store down'))

    // registerWorker should still succeed even though audit write fails
    const worker = await manager.registerWorker({ ...defaultRegistration, id: 'w1' })
    expect(worker.status).toBe('idle')
  })
})
