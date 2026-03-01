import { describe, it, expect } from 'vitest'
import { TaskEngine } from '../../src/engine.js'
import { MemoryBroadcastProvider, MemoryShortTermStore } from '../../src/memory-adapters.js'
import { WorkerManager } from '../../src/worker-manager.js'

function makeSetup() {
  const store = new MemoryShortTermStore()
  const broadcast = new MemoryBroadcastProvider()
  const engine = new TaskEngine({ shortTerm: store, broadcast })
  const manager = new WorkerManager({ engine, shortTerm: store, broadcast })
  return { store, broadcast, engine, manager }
}

// ─── Integration: Full Worker Assignment Flow ────────────────────────────────

describe('Worker Assignment — End-to-End Integration', () => {

  // ─── 1. Full ws-offer flow ───────────────────────────────────────────────

  describe('Full ws-offer flow', () => {
    it('register → dispatch → claim → run → complete', async () => {
      const { engine, manager, store } = makeSetup()

      // Register worker W1 matching llm.*
      const w1 = await manager.registerWorker({
        id: 'W1',
        matchRule: { taskTypes: ['llm.*'] },
        capacity: 10,
        connectionMode: 'websocket',
      })
      expect(w1.status).toBe('idle')

      // Create task T1 with type='llm.chat', assignMode='ws-offer'
      const t1 = await engine.createTask({
        id: 'T1',
        type: 'llm.chat',
        assignMode: 'ws-offer',
      })
      expect(t1.status).toBe('pending')

      // Simulate onTaskCreated hook notifying new task
      await manager.notifyNewTask(t1.id)

      // Dispatch T1 — should match W1
      const dispatch = await manager.dispatchTask(t1.id)
      expect(dispatch.matched).toBe(true)
      expect(dispatch.workerId).toBe('W1')

      // Claim T1 by W1
      const claim = await manager.claimTask(t1.id, w1.id)
      expect(claim.success).toBe(true)

      // Verify: T1 status='assigned', T1.assignedWorker='W1'
      const afterClaim = await store.getTask(t1.id)
      expect(afterClaim?.status).toBe('assigned')
      expect(afterClaim?.assignedWorker).toBe('W1')

      // Transition T1 to 'running'
      await engine.transitionTask(t1.id, 'running')
      const afterRunning = await engine.getTask(t1.id)
      expect(afterRunning?.status).toBe('running')

      // Transition T1 to 'completed'
      await engine.transitionTask(t1.id, 'completed', {
        result: { output: 'Hello from LLM' },
      })
      const afterCompleted = await engine.getTask(t1.id)
      expect(afterCompleted?.status).toBe('completed')
      expect(afterCompleted?.result).toEqual({ output: 'Hello from LLM' })
      expect(afterCompleted?.completedAt).toBeDefined()
    })
  })

  // ─── 2. Full pull flow ───────────────────────────────────────────────────

  describe('Full pull flow', () => {
    it('waitForTask resolves when notifyNewTask fires for matching pull task', async () => {
      const { engine, manager } = makeSetup()

      // Register worker W2 matching all task types, pull mode
      await manager.registerWorker({
        id: 'W2',
        matchRule: { taskTypes: ['*'] },
        capacity: 5,
        connectionMode: 'pull',
      })

      // Create task T2 with assignMode='pull'
      const t2 = await engine.createTask({
        id: 'T2',
        type: 'test',
        assignMode: 'pull',
      })

      // Call waitForTask in parallel with notifyNewTask
      // waitForTask should find T2 immediately since it already exists as pending
      const waitPromise = manager.waitForTask('W2')

      // Small delay to let subscription establish if it goes to the broadcast path
      await new Promise((r) => setTimeout(r, 10))

      // Notify (in case waitForTask didn't find T2 immediately)
      await manager.notifyNewTask(t2.id)

      const claimed = await waitPromise
      expect(claimed.id).toBe('T2')
      expect(claimed.status).toBe('assigned')
      expect(claimed.assignedWorker).toBe('W2')
    })

    it('waitForTask waits and resolves when task is created after waiting starts', async () => {
      const { engine, manager } = makeSetup()

      // Register worker W2b with pull mode
      await manager.registerWorker({
        id: 'W2b',
        matchRule: { taskTypes: ['*'] },
        capacity: 5,
        connectionMode: 'pull',
      })

      // Start waiting before any tasks exist
      const waitPromise = manager.waitForTask('W2b')

      // Allow subscription to establish
      await new Promise((r) => setTimeout(r, 10))

      // Now create and notify
      const t2b = await engine.createTask({
        id: 'T2b',
        type: 'test',
        assignMode: 'pull',
      })
      await manager.notifyNewTask(t2b.id)

      const claimed = await waitPromise
      expect(claimed.id).toBe('T2b')
      expect(claimed.status).toBe('assigned')
    })
  })

  // ─── 3. Decline and re-dispatch ──────────────────────────────────────────

  describe('Decline and re-dispatch', () => {
    it('blacklisted worker is skipped on re-dispatch', async () => {
      const { engine, manager, store } = makeSetup()

      // Register W3 (high weight) and W4 (lower weight)
      await manager.registerWorker({
        id: 'W3',
        matchRule: { taskTypes: ['*'] },
        capacity: 5,
        weight: 100,
        connectionMode: 'websocket',
      })
      await manager.registerWorker({
        id: 'W4',
        matchRule: { taskTypes: ['*'] },
        capacity: 5,
        weight: 50,
        connectionMode: 'websocket',
      })

      // Create task T3
      const t3 = await engine.createTask({
        id: 'T3',
        type: 'generic',
      })

      // First dispatch should match W3 (higher weight)
      const dispatch1 = await manager.dispatchTask(t3.id)
      expect(dispatch1.matched).toBe(true)
      expect(dispatch1.workerId).toBe('W3')

      // Claim T3 by W3
      const claim1 = await manager.claimTask(t3.id, 'W3')
      expect(claim1.success).toBe(true)

      // Decline T3 by W3 with blacklist
      await manager.declineTask(t3.id, 'W3', { blacklist: true })

      // Verify task is back to pending
      const afterDecline = await store.getTask(t3.id)
      expect(afterDecline?.status).toBe('pending')
      expect(afterDecline?.assignedWorker).toBeUndefined()

      // Re-dispatch T3 — should match W4 (W3 is blacklisted)
      const dispatch2 = await manager.dispatchTask(t3.id)
      expect(dispatch2.matched).toBe(true)
      expect(dispatch2.workerId).toBe('W4')

      // Claim by W4 should succeed
      const claim2 = await manager.claimTask(t3.id, 'W4')
      expect(claim2.success).toBe(true)

      const final = await store.getTask(t3.id)
      expect(final?.status).toBe('assigned')
      expect(final?.assignedWorker).toBe('W4')
    })
  })

  // ─── 4. Capacity exhaustion ──────────────────────────────────────────────

  describe('Capacity exhaustion', () => {
    it('worker at capacity is not matched for new tasks', async () => {
      const { engine, manager, store } = makeSetup()

      // Register W5 with capacity=2
      await manager.registerWorker({
        id: 'W5',
        matchRule: { taskTypes: ['*'] },
        capacity: 2,
        connectionMode: 'websocket',
      })

      // Create T4 (cost=1), claim by W5
      const t4 = await engine.createTask({ id: 'T4', type: 'work', cost: 1 })
      const claim4 = await manager.claimTask(t4.id, 'W5')
      expect(claim4.success).toBe(true)

      // Verify W5 usedSlots=1, still idle
      const afterT4 = await store.getWorker('W5')
      expect(afterT4?.usedSlots).toBe(1)
      expect(afterT4?.status).toBe('idle')

      // Create T5 (cost=1), claim by W5
      const t5 = await engine.createTask({ id: 'T5', type: 'work', cost: 1 })
      const claim5 = await manager.claimTask(t5.id, 'W5')
      expect(claim5.success).toBe(true)

      // Verify W5 usedSlots=2, now busy
      const afterT5 = await store.getWorker('W5')
      expect(afterT5?.usedSlots).toBe(2)
      expect(afterT5?.status).toBe('busy')

      // Create T6 (cost=1) — dispatch should NOT match W5 (at capacity)
      const t6 = await engine.createTask({ id: 'T6', type: 'work', cost: 1 })
      const dispatch6 = await manager.dispatchTask(t6.id)
      expect(dispatch6.matched).toBe(false)
    })
  })

  // ─── 5. Concurrent claim race ────────────────────────────────────────────

  describe('Concurrent claim race', () => {
    it('exactly 1 of 10 concurrent claims succeeds', async () => {
      const { engine, manager } = makeSetup()

      // Register 10 workers (W10-W19) all matching the same rule
      const workerIds: string[] = []
      for (let i = 10; i <= 19; i++) {
        const id = `W${i}`
        workerIds.push(id)
        await manager.registerWorker({
          id,
          matchRule: { taskTypes: ['*'] },
          capacity: 5,
          connectionMode: 'websocket',
        })
      }

      // Create 1 task T7
      const t7 = await engine.createTask({ id: 'T7', type: 'race' })

      // Call claimTask for all 10 workers concurrently
      const results = await Promise.all(
        workerIds.map((wId) => manager.claimTask(t7.id, wId)),
      )

      const successes = results.filter((r) => r.success)
      const failures = results.filter((r) => !r.success)

      // Exactly 1 succeeds, 9 fail
      expect(successes).toHaveLength(1)
      expect(failures).toHaveLength(9)

      // Verify the task is assigned to exactly one worker
      const task = await engine.getTask(t7.id)
      expect(task?.status).toBe('assigned')
      expect(task?.assignedWorker).toBeDefined()
      expect(workerIds).toContain(task?.assignedWorker)
    })
  })

  // ─── 6. Worker lifecycle ─────────────────────────────────────────────────

  describe('Worker lifecycle', () => {
    it('idle → busy → idle (after decline) → unregistered', async () => {
      const { engine, manager, store } = makeSetup()

      // Register worker with capacity=2
      const worker = await manager.registerWorker({
        id: 'W-lifecycle',
        matchRule: { taskTypes: ['*'] },
        capacity: 2,
        connectionMode: 'websocket',
      })
      expect(worker.status).toBe('idle')
      expect(worker.usedSlots).toBe(0)

      // Claim task 1 (cost=1) → still idle (1/2)
      const t1 = await engine.createTask({ id: 'T-lc-1', type: 'work', cost: 1 })
      await manager.claimTask(t1.id, worker.id)
      const afterFirst = await store.getWorker(worker.id)
      expect(afterFirst?.usedSlots).toBe(1)
      expect(afterFirst?.status).toBe('idle')

      // Claim task 2 (cost=1) → now busy (2/2)
      const t2 = await engine.createTask({ id: 'T-lc-2', type: 'work', cost: 1 })
      await manager.claimTask(t2.id, worker.id)
      const afterSecond = await store.getWorker(worker.id)
      expect(afterSecond?.usedSlots).toBe(2)
      expect(afterSecond?.status).toBe('busy')

      // Decline task 1 → usedSlots decreases, back to idle
      await manager.declineTask(t1.id, worker.id)
      const afterDecline = await store.getWorker(worker.id)
      expect(afterDecline?.usedSlots).toBe(1)
      expect(afterDecline?.status).toBe('idle')

      // Unregister → worker gone
      await manager.unregisterWorker(worker.id)
      const afterUnregister = await store.getWorker(worker.id)
      expect(afterUnregister).toBeNull()
    })
  })
})
