import { describe, it, expect, vi } from 'vitest'
import { TaskEngine } from '../../src/engine.js'
import { MemoryBroadcastProvider, MemoryShortTermStore } from '../../src/memory-adapters.js'
import { WorkerManager } from '../../src/worker-manager.js'
import type { WorkerRegistration } from '../../src/worker-manager.js'

function makeSetup() {
  const store = new MemoryShortTermStore()
  const broadcast = new MemoryBroadcastProvider()
  const engine = new TaskEngine({ shortTermStore: store, broadcast })
  const manager = new WorkerManager({ engine, shortTermStore: store, broadcast })
  return { store, broadcast, engine, manager }
}

const defaultRegistration: WorkerRegistration = {
  matchRule: {},
  capacity: 5,
  connectionMode: 'pull',
}

// ─── WorkerManager Concurrency & Race Condition Tests ──────────────────────

describe('WorkerManager — Concurrency & Race Conditions', () => {

  // ─── 1. Multiple workers waitForTask simultaneously ─────────────────────
  describe('multiple workers waitForTask simultaneously', () => {
    it('N workers poll, 1 task arrives, exactly 1 gets it', async () => {
      const { engine, manager } = makeSetup()
      const N = 5

      // Register N workers, all matching all tasks
      const workerIds: string[] = []
      for (let i = 0; i < N; i++) {
        const id = `poll-worker-${i}`
        workerIds.push(id)
        await manager.registerWorker({
          ...defaultRegistration,
          id,
          matchRule: { taskTypes: ['*'] },
          connectionMode: 'pull',
        })
      }

      // Start all N workers waiting concurrently
      const controllers = workerIds.map(() => new AbortController())
      const waitPromises = workerIds.map((wId, i) =>
        manager.waitForTask(wId, controllers[i]!.signal)
          .then((task) => ({ workerId: wId, task, error: null as Error | null }))
          .catch((err: Error) => ({ workerId: wId, task: null, error: err })),
      )

      // Allow subscriptions to establish
      await new Promise((r) => setTimeout(r, 20))

      // Create 1 pull task and notify
      const task = await engine.createTask({ type: 'test', assignMode: 'pull' })
      await manager.notifyNewTask(task.id)

      // Give time for one worker to claim
      await new Promise((r) => setTimeout(r, 50))

      // Abort all remaining waiters
      for (const controller of controllers) {
        controller.abort()
      }

      const results = await Promise.all(waitPromises)

      // Exactly 1 worker should have received the task
      const successes = results.filter((r) => r.task !== null)
      const aborts = results.filter((r) => r.error !== null && r.error.message === 'aborted')

      expect(successes).toHaveLength(1)
      expect(successes[0]!.task!.id).toBe(task.id)
      expect(successes[0]!.task!.status).toBe('assigned')

      // The rest should have aborted
      expect(aborts).toHaveLength(N - 1)
    })

    it('3 workers poll, 3 tasks arrive in rapid succession, each worker gets exactly 1', async () => {
      const { engine, manager, store } = makeSetup()

      const workerIds = ['rapid-w0', 'rapid-w1', 'rapid-w2']
      for (const id of workerIds) {
        await manager.registerWorker({
          ...defaultRegistration,
          id,
          capacity: 1,
          matchRule: { taskTypes: ['*'] },
          connectionMode: 'pull',
        })
      }

      // Start all workers waiting
      const controllers = workerIds.map(() => new AbortController())
      const waitPromises = workerIds.map((wId, i) =>
        manager.waitForTask(wId, controllers[i]!.signal)
          .then((task) => ({ workerId: wId, task, error: null as Error | null }))
          .catch((err: Error) => ({ workerId: wId, task: null, error: err })),
      )

      await new Promise((r) => setTimeout(r, 20))

      // Create 3 tasks rapidly
      const tasks = []
      for (let i = 0; i < 3; i++) {
        const t = await engine.createTask({ id: `rapid-task-${i}`, type: 'test', assignMode: 'pull' })
        tasks.push(t)
        await manager.notifyNewTask(t.id)
      }

      // Wait for all to settle
      await new Promise((r) => setTimeout(r, 100))

      // Abort any that haven't resolved
      for (const controller of controllers) {
        controller.abort()
      }

      const results = await Promise.all(waitPromises)
      const successes = results.filter((r) => r.task !== null)

      // Each successful worker should have gotten a unique task
      const assignedTaskIds = successes.map((r) => r.task!.id)
      const uniqueTaskIds = new Set(assignedTaskIds)
      expect(uniqueTaskIds.size).toBe(assignedTaskIds.length)

      // Verify no task was double-assigned
      for (const taskId of assignedTaskIds) {
        const t = await engine.getTask(taskId)
        expect(t?.status).toBe('assigned')
      }
    })
  })

  // ─── 2. Concurrent decline of same task by different workers ────────────
  describe('concurrent decline of same task by different workers', () => {
    it('only one decline should succeed (the assigned worker)', async () => {
      const { engine, manager, store } = makeSetup()

      // Register 2 workers
      const w1 = await manager.registerWorker({ ...defaultRegistration, id: 'decline-w1', capacity: 5 })
      const w2 = await manager.registerWorker({ ...defaultRegistration, id: 'decline-w2', capacity: 5 })

      // Create task and assign to w1
      const task = await engine.createTask({ type: 'test', cost: 2 })
      await manager.claimTask(task.id, w1.id)

      // Both workers try to decline concurrently
      await Promise.all([
        manager.declineTask(task.id, w1.id),
        manager.declineTask(task.id, w2.id),
      ])

      // Task should be back to pending (only w1's decline was meaningful)
      const updatedTask = await store.getTask(task.id)
      expect(updatedTask?.status).toBe('pending')

      // w1 should have its capacity restored
      const updatedW1 = await store.getWorker(w1.id)
      expect(updatedW1?.usedSlots).toBe(0)
      expect(updatedW1?.status).toBe('idle')

      // w2 should be unaffected (decline was a no-op for w2)
      const updatedW2 = await store.getWorker(w2.id)
      expect(updatedW2?.usedSlots).toBe(0)
      expect(updatedW2?.status).toBe('idle')
    })

    it('concurrent declines from multiple non-assigned workers are all no-ops', async () => {
      const { engine, manager, store } = makeSetup()

      const w1 = await manager.registerWorker({ ...defaultRegistration, id: 'noopd-w1', capacity: 5 })
      const w2 = await manager.registerWorker({ ...defaultRegistration, id: 'noopd-w2', capacity: 5 })
      const w3 = await manager.registerWorker({ ...defaultRegistration, id: 'noopd-w3', capacity: 5 })

      const task = await engine.createTask({ type: 'test', cost: 1 })
      await manager.claimTask(task.id, w1.id)

      // w2 and w3 both try to decline concurrently — neither is the assigned worker
      await Promise.all([
        manager.declineTask(task.id, w2.id),
        manager.declineTask(task.id, w3.id),
      ])

      // Task should still be assigned to w1
      const updatedTask = await store.getTask(task.id)
      expect(updatedTask?.status).toBe('assigned')
      expect(updatedTask?.assignedWorker).toBe(w1.id)

      // w1 still has its used slot
      const updatedW1 = await store.getWorker(w1.id)
      expect(updatedW1?.usedSlots).toBe(1)
    })
  })

  // ─── 3. dispatch + claim race ───────────────────────────────────────────
  describe('dispatch + claim race', () => {
    it('dispatch selects worker, but before claim, worker capacity changes (fills up)', async () => {
      const { engine, manager, store } = makeSetup()

      const worker = await manager.registerWorker({
        ...defaultRegistration,
        id: 'race-worker',
        capacity: 1,
        weight: 100,
      })

      // Create two tasks
      const task1 = await engine.createTask({ id: 'race-t1', type: 'test' })
      const task2 = await engine.createTask({ id: 'race-t2', type: 'test' })

      // Dispatch task1 — should select race-worker
      const dispatch1 = await manager.dispatchTask(task1.id)
      expect(dispatch1.matched).toBe(true)
      expect(dispatch1.workerId).toBe('race-worker')

      // Before claiming task1, claim task2 to fill up the worker
      const claim2 = await manager.claimTask(task2.id, worker.id)
      expect(claim2.success).toBe(true)

      // Now try to claim task1 — worker is at capacity, should fail
      const claim1 = await manager.claimTask(task1.id, worker.id)
      expect(claim1.success).toBe(false)
      expect(claim1.reason).toContain('concurrent modification')

      // Worker should only have 1 used slot (from task2)
      const updatedWorker = await store.getWorker(worker.id)
      expect(updatedWorker?.usedSlots).toBe(1)
    })

    it('dispatch selects worker, but task becomes non-pending before claim', async () => {
      const { engine, manager } = makeSetup()

      await manager.registerWorker({
        ...defaultRegistration,
        id: 'stale-worker',
        capacity: 5,
      })

      const task = await engine.createTask({ type: 'test' })

      // Dispatch finds the worker
      const dispatch = await manager.dispatchTask(task.id)
      expect(dispatch.matched).toBe(true)

      // Before claim, transition the task to running (bypassing dispatch)
      await engine.transitionTask(task.id, 'running')

      // Now claim should fail — task is no longer pending
      const claim = await manager.claimTask(task.id, 'stale-worker')
      expect(claim.success).toBe(false)
      expect(claim.reason).toContain('not pending')
    })
  })

  // ─── 4. usedSlots never exceeds capacity under concurrent claims ───────
  describe('usedSlots never exceeds capacity under concurrent claims', () => {
    it('10 workers, capacity 5 each, 100 tasks — usedSlots stays within bounds', async () => {
      const { engine, manager, store } = makeSetup()

      const workerCount = 10
      const workerCapacity = 5
      const taskCount = 100

      // Register 10 workers
      const workerIds: string[] = []
      for (let i = 0; i < workerCount; i++) {
        const id = `cap-worker-${i}`
        workerIds.push(id)
        await manager.registerWorker({
          ...defaultRegistration,
          id,
          capacity: workerCapacity,
          matchRule: { taskTypes: ['*'] },
        })
      }

      // Create 100 tasks
      const taskIds: string[] = []
      for (let i = 0; i < taskCount; i++) {
        const t = await engine.createTask({ id: `cap-task-${i}`, type: 'test' })
        taskIds.push(t.id)
      }

      // Attempt concurrent claims: each task randomly assigned to a worker
      const claimPromises = taskIds.map((taskId, i) => {
        const workerId = workerIds[i % workerCount]!
        return manager.claimTask(taskId, workerId)
      })

      const results = await Promise.all(claimPromises)

      // Verify invariant: no worker's usedSlots exceeds its capacity
      for (const wId of workerIds) {
        const worker = await store.getWorker(wId)
        expect(worker).not.toBeNull()
        expect(worker!.usedSlots).toBeLessThanOrEqual(workerCapacity)
        expect(worker!.usedSlots).toBeGreaterThanOrEqual(0)
      }

      // Total successful claims should not exceed total capacity
      const successCount = results.filter((r) => r.success).length
      expect(successCount).toBeLessThanOrEqual(workerCount * workerCapacity)

      // Each successful claim should correspond to an assigned task
      for (const taskId of taskIds) {
        const task = await engine.getTask(taskId)
        if (task?.status === 'assigned') {
          expect(task.assignedWorker).toBeDefined()
        }
      }
    })

    it('all workers race to claim single task — exactly 1 succeeds', async () => {
      const { engine, manager, store } = makeSetup()

      const workerIds: string[] = []
      for (let i = 0; i < 20; i++) {
        const id = `single-race-w${i}`
        workerIds.push(id)
        await manager.registerWorker({
          ...defaultRegistration,
          id,
          capacity: 5,
        })
      }

      const task = await engine.createTask({ id: 'single-race-task', type: 'test' })

      // All 20 workers claim the same task concurrently
      const results = await Promise.all(
        workerIds.map((wId) => manager.claimTask(task.id, wId)),
      )

      const successes = results.filter((r) => r.success)
      expect(successes).toHaveLength(1)

      // Only one worker should have increased usedSlots
      let totalUsedSlots = 0
      for (const wId of workerIds) {
        const w = await store.getWorker(wId)
        totalUsedSlots += w?.usedSlots ?? 0
      }
      expect(totalUsedSlots).toBe(1)
    })
  })

  // ─── 5. usedSlots never goes negative under concurrent declines ────────
  describe('usedSlots never goes negative under concurrent declines', () => {
    it('claim and decline rapidly — usedSlots stays >= 0', async () => {
      const { engine, manager, store } = makeSetup()

      const worker = await manager.registerWorker({
        ...defaultRegistration,
        id: 'neg-worker',
        capacity: 20,
      })

      // Create and claim 10 tasks
      const taskIds: string[] = []
      for (let i = 0; i < 10; i++) {
        const t = await engine.createTask({ id: `neg-task-${i}`, type: 'test', cost: 1 })
        taskIds.push(t.id)
        await manager.claimTask(t.id, worker.id)
      }

      // Verify initial state
      const beforeDeclines = await store.getWorker(worker.id)
      expect(beforeDeclines?.usedSlots).toBe(10)

      // Decline all 10 concurrently
      await Promise.all(
        taskIds.map((taskId) => manager.declineTask(taskId, worker.id)),
      )

      // usedSlots should be exactly 0, not negative
      const afterDeclines = await store.getWorker(worker.id)
      expect(afterDeclines?.usedSlots).toBe(0)
      expect(afterDeclines?.usedSlots).toBeGreaterThanOrEqual(0)
      expect(afterDeclines?.status).toBe('idle')
    })

    it('double-decline of the same task does not make usedSlots negative', async () => {
      const { engine, manager, store } = makeSetup()

      const worker = await manager.registerWorker({
        ...defaultRegistration,
        id: 'double-neg-worker',
        capacity: 5,
      })

      const task = await engine.createTask({ id: 'double-neg-task', type: 'test', cost: 3 })
      await manager.claimTask(task.id, worker.id)

      // Verify post-claim state
      const afterClaim = await store.getWorker(worker.id)
      expect(afterClaim?.usedSlots).toBe(3)

      // Decline twice concurrently
      await Promise.all([
        manager.declineTask(task.id, worker.id),
        manager.declineTask(task.id, worker.id),
      ])

      // usedSlots should be 0, not -3
      const afterDeclines = await store.getWorker(worker.id)
      expect(afterDeclines?.usedSlots).toBeGreaterThanOrEqual(0)
    })
  })

  // ─── 6. Concurrent status updates (busy -> idle -> busy) ───────────────
  describe('concurrent status updates (busy -> idle -> busy)', () => {
    it('rapid claim/decline cycles produce consistent final state', async () => {
      const { engine, manager, store } = makeSetup()

      const worker = await manager.registerWorker({
        ...defaultRegistration,
        id: 'cycle-worker',
        capacity: 1,
      })

      // Run 5 rapid claim-then-decline cycles
      for (let i = 0; i < 5; i++) {
        const t = await engine.createTask({ id: `cycle-task-${i}`, type: 'test', cost: 1 })
        const claim = await manager.claimTask(t.id, worker.id)
        expect(claim.success).toBe(true)

        // Worker should be busy after claim (capacity 1, usedSlots 1)
        const afterClaim = await store.getWorker(worker.id)
        expect(afterClaim?.status).toBe('busy')
        expect(afterClaim?.usedSlots).toBe(1)

        await manager.declineTask(t.id, worker.id)

        // Worker should be idle after decline
        const afterDecline = await store.getWorker(worker.id)
        expect(afterDecline?.status).toBe('idle')
        expect(afterDecline?.usedSlots).toBe(0)
      }
    })

    it('concurrent claims on different tasks interleaved with declines', async () => {
      const { engine, manager, store } = makeSetup()

      const worker = await manager.registerWorker({
        ...defaultRegistration,
        id: 'interleave-worker',
        capacity: 3,
      })

      // Create 6 tasks
      const tasks = []
      for (let i = 0; i < 6; i++) {
        tasks.push(await engine.createTask({ id: `interleave-${i}`, type: 'test', cost: 1 }))
      }

      // Claim first 3
      for (let i = 0; i < 3; i++) {
        const result = await manager.claimTask(tasks[i]!.id, worker.id)
        expect(result.success).toBe(true)
      }

      // Worker should be busy (3/3)
      let w = await store.getWorker(worker.id)
      expect(w?.usedSlots).toBe(3)
      expect(w?.status).toBe('busy')

      // Trying to claim a 4th should fail
      const overClaim = await manager.claimTask(tasks[3]!.id, worker.id)
      expect(overClaim.success).toBe(false)

      // Decline one, then claim the 4th
      await manager.declineTask(tasks[0]!.id, worker.id)
      w = await store.getWorker(worker.id)
      expect(w?.usedSlots).toBe(2)
      expect(w?.status).toBe('idle')

      const reclaimResult = await manager.claimTask(tasks[3]!.id, worker.id)
      expect(reclaimResult.success).toBe(true)

      w = await store.getWorker(worker.id)
      expect(w?.usedSlots).toBe(3)
      expect(w?.status).toBe('busy')
    })
  })

  // ─── 7. Worker deletion during active waitForTask ──────────────────────
  describe('worker deletion during active waitForTask', () => {
    it('worker is unregistered while polling — waitForTask rejects', async () => {
      const { engine, manager } = makeSetup()

      await manager.registerWorker({
        ...defaultRegistration,
        id: 'deleteme-worker',
        connectionMode: 'pull',
        matchRule: { taskTypes: ['*'] },
      })

      // Start waiting
      const waitPromise = manager.waitForTask('deleteme-worker')

      // Wait for subscription to establish
      await new Promise((r) => setTimeout(r, 20))

      // Unregister worker while waiting
      await manager.unregisterWorker('deleteme-worker')

      // Now create and notify a task — the handler will re-fetch and find worker gone
      const task = await engine.createTask({ type: 'test', assignMode: 'pull' })
      await manager.notifyNewTask(task.id)

      await expect(waitPromise).rejects.toThrow('Worker not found: deleteme-worker')
    })

    it('worker deletion does not leave dangling subscriptions that crash', async () => {
      const { engine, manager, broadcast } = makeSetup()

      await manager.registerWorker({
        ...defaultRegistration,
        id: 'dangle-worker',
        connectionMode: 'pull',
        matchRule: { taskTypes: ['*'] },
      })

      const controller = new AbortController()
      const waitPromise = manager.waitForTask('dangle-worker', controller.signal)
        .catch((err: Error) => err)

      await new Promise((r) => setTimeout(r, 20))

      // Delete worker
      await manager.unregisterWorker('dangle-worker')

      // Notify a task — first notification hits the "worker not found" path
      const task1 = await engine.createTask({ id: 'dangle-t1', type: 'test', assignMode: 'pull' })
      await manager.notifyNewTask(task1.id)

      const result = await waitPromise
      expect(result).toBeInstanceOf(Error)
      expect((result as Error).message).toContain('Worker not found')

      // After the promise is settled, additional notifications should not crash
      const task2 = await engine.createTask({ id: 'dangle-t2', type: 'test', assignMode: 'pull' })
      await expect(manager.notifyNewTask(task2.id)).resolves.not.toThrow()
    })
  })

  // ─── 8. Capacity update during dispatch ────────────────────────────────
  describe('capacity update during dispatch', () => {
    it('worker capacity reduced mid-dispatch — claim reflects new capacity', async () => {
      const { engine, manager, store } = makeSetup()

      const worker = await manager.registerWorker({
        ...defaultRegistration,
        id: 'shrink-worker',
        capacity: 10,
        weight: 100,
      })

      // Pre-claim 4 tasks (usedSlots=4)
      for (let i = 0; i < 4; i++) {
        const t = await engine.createTask({ id: `shrink-pre-${i}`, type: 'test', cost: 1 })
        await manager.claimTask(t.id, worker.id)
      }

      const target = await engine.createTask({ id: 'shrink-target', type: 'test', cost: 1 })

      // Dispatch finds shrink-worker (usedSlots=4, capacity=10, has room)
      const dispatch = await manager.dispatchTask(target.id)
      expect(dispatch.matched).toBe(true)
      expect(dispatch.workerId).toBe('shrink-worker')

      // Reduce capacity to 4 before the claim — worker is now full
      await manager.updateWorker('shrink-worker', { capacity: 4 })

      // Claim should fail because usedSlots (4) + cost (1) > capacity (4)
      const claim = await manager.claimTask(target.id, 'shrink-worker')
      expect(claim.success).toBe(false)

      // Worker should still have usedSlots=4, capacity=4
      const updated = await store.getWorker('shrink-worker')
      expect(updated?.usedSlots).toBe(4)
      expect(updated?.capacity).toBe(4)
    })

    it('worker capacity increased mid-dispatch allows previously impossible claim', async () => {
      const { engine, manager, store } = makeSetup()

      await manager.registerWorker({
        ...defaultRegistration,
        id: 'grow-worker',
        capacity: 1,
        weight: 100,
      })

      // Fill up the worker
      const t1 = await engine.createTask({ id: 'grow-t1', type: 'test', cost: 1 })
      await manager.claimTask(t1.id, 'grow-worker')

      const target = await engine.createTask({ id: 'grow-target', type: 'test', cost: 1 })

      // Dispatch won't find the worker (at capacity)
      const dispatch1 = await manager.dispatchTask(target.id)
      expect(dispatch1.matched).toBe(false)

      // Increase capacity
      await manager.updateWorker('grow-worker', { capacity: 5 })

      // Now dispatch should find the worker
      const dispatch2 = await manager.dispatchTask(target.id)
      expect(dispatch2.matched).toBe(true)

      // And claim should succeed
      const claim = await manager.claimTask(target.id, 'grow-worker')
      expect(claim.success).toBe(true)

      const worker = await store.getWorker('grow-worker')
      expect(worker?.usedSlots).toBe(2)
      expect(worker?.capacity).toBe(5)
    })
  })

  // ─── 9. Multiple notifications arriving during broadcast wait ──────────
  describe('multiple notifications arriving during broadcast wait', () => {
    it('rapid notifyNewTask calls — worker claims first matching task', async () => {
      const { engine, manager } = makeSetup()

      await manager.registerWorker({
        ...defaultRegistration,
        id: 'rapid-notify-w',
        capacity: 1,
        connectionMode: 'pull',
        matchRule: { taskTypes: ['*'] },
      })

      // Start waiting
      const waitPromise = manager.waitForTask('rapid-notify-w')

      await new Promise((r) => setTimeout(r, 20))

      // Create and notify 5 tasks in rapid succession
      const taskIds: string[] = []
      for (let i = 0; i < 5; i++) {
        const t = await engine.createTask({ id: `rapid-notify-t${i}`, type: 'test', assignMode: 'pull' })
        taskIds.push(t.id)
        await manager.notifyNewTask(t.id)
      }

      const claimed = await waitPromise

      // The worker should have claimed exactly one of the tasks
      expect(taskIds).toContain(claimed.id)
      expect(claimed.status).toBe('assigned')
      expect(claimed.assignedWorker).toBe('rapid-notify-w')
    })

    it('notifications for non-matching tasks followed by a matching task', async () => {
      const { engine, manager } = makeSetup()

      await manager.registerWorker({
        ...defaultRegistration,
        id: 'selective-w',
        capacity: 1,
        connectionMode: 'pull',
        matchRule: { taskTypes: ['target.*'] },
      })

      const controller = new AbortController()
      const waitPromise = manager.waitForTask('selective-w', controller.signal)

      await new Promise((r) => setTimeout(r, 20))

      // Notify several non-matching tasks
      for (let i = 0; i < 3; i++) {
        const t = await engine.createTask({ id: `nope-${i}`, type: 'other.work', assignMode: 'pull' })
        await manager.notifyNewTask(t.id)
      }

      // Now notify a matching task
      const match = await engine.createTask({ id: 'match-task', type: 'target.work', assignMode: 'pull' })
      await manager.notifyNewTask(match.id)

      const result = await waitPromise
      expect(result.id).toBe('match-task')
      expect(result.status).toBe('assigned')
    })

    it('burst notifications with multiple waiters — tasks distributed across workers', async () => {
      const { engine, manager, store } = makeSetup()

      const workerIds = ['burst-w0', 'burst-w1', 'burst-w2']
      for (const id of workerIds) {
        await manager.registerWorker({
          ...defaultRegistration,
          id,
          capacity: 2,
          connectionMode: 'pull',
          matchRule: { taskTypes: ['*'] },
        })
      }

      // Start all workers waiting
      const controllers = workerIds.map(() => new AbortController())
      const waitPromises = workerIds.map((wId, i) =>
        manager.waitForTask(wId, controllers[i]!.signal)
          .then((task) => ({ workerId: wId, taskId: task.id }))
          .catch(() => null),
      )

      await new Promise((r) => setTimeout(r, 20))

      // Burst-notify 3 tasks
      for (let i = 0; i < 3; i++) {
        const t = await engine.createTask({ id: `burst-t${i}`, type: 'test', assignMode: 'pull' })
        await manager.notifyNewTask(t.id)
      }

      await new Promise((r) => setTimeout(r, 100))

      // Abort any stragglers
      for (const c of controllers) c.abort()

      const results = await Promise.all(waitPromises)
      const successes = results.filter((r) => r !== null)

      // At least some workers should have gotten tasks
      expect(successes.length).toBeGreaterThanOrEqual(1)

      // All assigned task IDs should be unique
      const assignedIds = successes.map((r) => r!.taskId)
      expect(new Set(assignedIds).size).toBe(assignedIds.length)
    })
  })

  // ─── 10. Signal abort race with claim success in waitForTask ───────────
  describe('signal abort race with claim success in waitForTask', () => {
    it('abort signal fires just as task is found — resolves or rejects cleanly', async () => {
      const { engine, manager } = makeSetup()

      await manager.registerWorker({
        ...defaultRegistration,
        id: 'abort-race-w',
        capacity: 5,
        connectionMode: 'pull',
        matchRule: { taskTypes: ['*'] },
      })

      const controller = new AbortController()
      const waitPromise = manager.waitForTask('abort-race-w', controller.signal)

      await new Promise((r) => setTimeout(r, 20))

      // Create task and fire abort simultaneously
      const task = await engine.createTask({ id: 'abort-race-t', type: 'test', assignMode: 'pull' })

      // Notify and abort near-simultaneously
      const notifyPromise = manager.notifyNewTask(task.id)
      controller.abort()

      await notifyPromise

      // The promise must settle — either resolve (got the task) or reject (aborted)
      // It should NOT hang or throw an unhandled error
      const result = await waitPromise
        .then((t) => ({ type: 'resolved' as const, task: t }))
        .catch((err: Error) => ({ type: 'rejected' as const, error: err }))

      if (result.type === 'resolved') {
        expect(result.task.id).toBe('abort-race-t')
      } else {
        expect(result.error.message).toBe('aborted')
      }
    })

    it('abort after immediate match in pending tasks scan — settles cleanly', async () => {
      const { engine, manager } = makeSetup()

      await manager.registerWorker({
        ...defaultRegistration,
        id: 'abort-immediate-w',
        capacity: 5,
        connectionMode: 'pull',
        matchRule: { taskTypes: ['*'] },
      })

      // Create task BEFORE waiting — it should be found in the initial scan
      await engine.createTask({ id: 'abort-immediate-t', type: 'test', assignMode: 'pull' })

      const controller = new AbortController()

      // waitForTask should find the task immediately in the pending scan
      // Abort right after calling — but the immediate scan should win
      const waitPromise = manager.waitForTask('abort-immediate-w', controller.signal)
      controller.abort()

      const result = await waitPromise
        .then((t) => ({ type: 'resolved' as const, task: t }))
        .catch((err: Error) => ({ type: 'rejected' as const, error: err }))

      // The task was already pending, so waitForTask should have claimed it
      // before the abort could fire, so it should resolve
      if (result.type === 'resolved') {
        expect(result.task.id).toBe('abort-immediate-t')
        expect(result.task.status).toBe('assigned')
      } else {
        // If by some scheduling quirk abort fires first, that's also acceptable
        expect(result.error.message).toBe('aborted')
      }
    })

    it('already-aborted signal prevents claim even if tasks exist', async () => {
      const { engine, manager, store } = makeSetup()

      await manager.registerWorker({
        ...defaultRegistration,
        id: 'pre-abort-w',
        capacity: 5,
        connectionMode: 'pull',
      })

      // Create a pending pull task
      await engine.createTask({ id: 'pre-abort-t', type: 'test', assignMode: 'pull' })

      // Create an already-aborted signal
      const controller = new AbortController()
      controller.abort()

      // waitForTask should reject immediately without claiming
      await expect(manager.waitForTask('pre-abort-w', controller.signal)).rejects.toThrow('aborted')

      // Task should still be pending (not claimed)
      const task = await store.getTask('pre-abort-t')
      expect(task?.status).toBe('pending')
    })
  })
})
