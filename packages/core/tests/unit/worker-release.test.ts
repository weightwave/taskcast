import { describe, it, expect, vi } from 'vitest'
import { TaskEngine } from '../../src/engine.js'
import { MemoryBroadcastProvider, MemoryShortTermStore } from '../../src/memory-adapters.js'
import { WorkerManager } from '../../src/worker-manager.js'
import type { TaskcastHooks, LongTermStore, TaskStatus } from '../../src/types.js'
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

// ─── releaseTask() ──────────────────────────────────────────────────────────

describe('WorkerManager — releaseTask', () => {
  it('releases capacity when task has assignment (usedSlots goes down, assignment removed)', async () => {
    const { manager, engine, store } = makeSetup()
    const worker = await manager.registerWorker(defaultRegistration)
    const task = await engine.createTask({ type: 'test', cost: 2 })
    await manager.claimTask(task.id, worker.id)

    // Verify worker usedSlots increased after claim
    const workerBefore = await store.getWorker(worker.id)
    expect(workerBefore!.usedSlots).toBe(2)

    // Complete the task (releaseTask does not transition — we do it manually)
    await engine.transitionTask(task.id, 'running')
    await engine.transitionTask(task.id, 'completed')

    // Release the task
    await manager.releaseTask(task.id)

    // Verify usedSlots decreased and assignment removed
    const workerAfter = await store.getWorker(worker.id)
    expect(workerAfter!.usedSlots).toBe(0)

    const assignment = await store.getTaskAssignment(task.id)
    expect(assignment).toBeNull()
  })

  it('no-op when no assignment exists', async () => {
    const { manager, engine } = makeSetup()
    const task = await engine.createTask({ type: 'test' })

    // Should not throw — just silently return
    await expect(manager.releaseTask(task.id)).resolves.toBeUndefined()
  })

  it('sets worker idle when usedSlots reaches 0', async () => {
    const { manager, engine, store } = makeSetup()
    const worker = await manager.registerWorker({ ...defaultRegistration, capacity: 3 })
    const task = await engine.createTask({ type: 'test', cost: 3 })
    await manager.claimTask(task.id, worker.id)

    // Worker should be busy (usedSlots === capacity)
    const workerBusy = await store.getWorker(worker.id)
    expect(workerBusy!.status).toBe('busy')

    await engine.transitionTask(task.id, 'running')
    await engine.transitionTask(task.id, 'completed')
    await manager.releaseTask(task.id)

    const workerAfter = await store.getWorker(worker.id)
    expect(workerAfter!.usedSlots).toBe(0)
    expect(workerAfter!.status).toBe('idle')
  })

  it('preserves draining status (worker stays draining after release)', async () => {
    const { manager, engine, store } = makeSetup()
    const worker = await manager.registerWorker({ ...defaultRegistration, capacity: 3 })
    const task = await engine.createTask({ type: 'test', cost: 1 })
    await manager.claimTask(task.id, worker.id)

    // Set worker to draining
    await manager.updateWorker(worker.id, { status: 'draining' })
    const workerDraining = await store.getWorker(worker.id)
    expect(workerDraining!.status).toBe('draining')

    await engine.transitionTask(task.id, 'running')
    await engine.transitionTask(task.id, 'completed')
    await manager.releaseTask(task.id)

    // Worker should still be draining, not idle
    const workerAfter = await store.getWorker(worker.id)
    expect(workerAfter!.usedSlots).toBe(0)
    expect(workerAfter!.status).toBe('draining')
  })

  it('handles deleted worker gracefully (no error)', async () => {
    const { manager, engine, store } = makeSetup()
    const worker = await manager.registerWorker(defaultRegistration)
    const task = await engine.createTask({ type: 'test', cost: 1 })
    await manager.claimTask(task.id, worker.id)

    // Delete the worker from the store
    await store.deleteWorker(worker.id)

    await engine.transitionTask(task.id, 'running')
    await engine.transitionTask(task.id, 'completed')

    // Should not throw even though worker is deleted
    await expect(manager.releaseTask(task.id)).resolves.toBeUndefined()

    // Assignment should still be removed
    const assignment = await store.getTaskAssignment(task.id)
    expect(assignment).toBeNull()
  })

  it('emits task_released audit event (needs longTermStore)', async () => {
    const { manager, engine, longTermStore } = makeSetupWithLongTerm()
    const worker = await manager.registerWorker(defaultRegistration)
    const task = await engine.createTask({ type: 'test', cost: 1 })
    await manager.claimTask(task.id, worker.id)

    vi.mocked(longTermStore.saveWorkerEvent).mockClear()

    await engine.transitionTask(task.id, 'running')
    await engine.transitionTask(task.id, 'completed')
    await manager.releaseTask(task.id)

    // Flush async saveWorkerEvent calls
    await Promise.resolve()
    await Promise.resolve()

    expect(longTermStore.saveWorkerEvent).toHaveBeenCalledWith(
      expect.objectContaining({
        action: 'task_released',
        workerId: worker.id,
        data: { taskId: task.id },
      }),
    )
  })

  it('concurrent double-release is idempotent (second call is no-op)', async () => {
    const { manager, engine, store } = makeSetup()
    const worker = await manager.registerWorker({ ...defaultRegistration, capacity: 5 })
    const task = await engine.createTask({ type: 'test', cost: 2 })
    await manager.claimTask(task.id, worker.id)

    await engine.transitionTask(task.id, 'running')
    await engine.transitionTask(task.id, 'completed')

    // Release twice concurrently
    await Promise.all([
      manager.releaseTask(task.id),
      manager.releaseTask(task.id),
    ])

    // usedSlots should be 0 (not negative)
    const workerAfter = await store.getWorker(worker.id)
    expect(workerAfter!.usedSlots).toBeGreaterThanOrEqual(0)
    expect(workerAfter!.status).toBe('idle')
  })

  it('does not transition task state (task stays completed)', async () => {
    const { manager, engine } = makeSetup()
    const worker = await manager.registerWorker(defaultRegistration)
    const task = await engine.createTask({ type: 'test', cost: 1 })
    await manager.claimTask(task.id, worker.id)

    await engine.transitionTask(task.id, 'running')
    await engine.transitionTask(task.id, 'completed')
    await manager.releaseTask(task.id)

    // Task should still be completed — releaseTask does NOT change task status
    const taskAfter = await engine.getTask(task.id)
    expect(taskAfter!.status).toBe('completed')
  })

  it('multiple tasks with different costs released correctly (verify usedSlots math)', async () => {
    const { manager, engine, store } = makeSetup()
    const worker = await manager.registerWorker({ ...defaultRegistration, capacity: 10 })

    // Create three tasks with different costs
    const task1 = await engine.createTask({ type: 'test', cost: 2 })
    const task2 = await engine.createTask({ type: 'test', cost: 3 })
    const task3 = await engine.createTask({ type: 'test', cost: 4 })

    await manager.claimTask(task1.id, worker.id)
    await manager.claimTask(task2.id, worker.id)
    await manager.claimTask(task3.id, worker.id)

    // Total usedSlots should be 2 + 3 + 4 = 9
    const workerFull = await store.getWorker(worker.id)
    expect(workerFull!.usedSlots).toBe(9)

    // Complete and release task2 (cost 3)
    await engine.transitionTask(task2.id, 'running')
    await engine.transitionTask(task2.id, 'completed')
    await manager.releaseTask(task2.id)

    const workerAfter1 = await store.getWorker(worker.id)
    expect(workerAfter1!.usedSlots).toBe(6) // 9 - 3 = 6

    // Complete and release task1 (cost 2)
    await engine.transitionTask(task1.id, 'running')
    await engine.transitionTask(task1.id, 'completed')
    await manager.releaseTask(task1.id)

    const workerAfter2 = await store.getWorker(worker.id)
    expect(workerAfter2!.usedSlots).toBe(4) // 6 - 2 = 4

    // Complete and release task3 (cost 4)
    await engine.transitionTask(task3.id, 'running')
    await engine.transitionTask(task3.id, 'completed')
    await manager.releaseTask(task3.id)

    const workerAfter3 = await store.getWorker(worker.id)
    expect(workerAfter3!.usedSlots).toBe(0) // 4 - 4 = 0
    expect(workerAfter3!.status).toBe('idle')
  })
})

// ─── addTransitionListener() ────────────────────────────────────────────────

describe('TaskEngine — addTransitionListener', () => {
  it('listener called on transition with correct args', async () => {
    const { engine } = makeSetup()
    const listener = vi.fn()
    engine.addTransitionListener(listener)

    const task = await engine.createTask({ type: 'test' })
    await engine.transitionTask(task.id, 'running')

    expect(listener).toHaveBeenCalledOnce()
    expect(listener).toHaveBeenCalledWith(
      expect.objectContaining({ id: task.id, status: 'running' }),
      'pending',
      'running',
    )
  })

  it('multiple listeners all called', async () => {
    const { engine } = makeSetup()
    const listener1 = vi.fn()
    const listener2 = vi.fn()
    const listener3 = vi.fn()
    engine.addTransitionListener(listener1)
    engine.addTransitionListener(listener2)
    engine.addTransitionListener(listener3)

    const task = await engine.createTask({ type: 'test' })
    await engine.transitionTask(task.id, 'running')

    expect(listener1).toHaveBeenCalledOnce()
    expect(listener2).toHaveBeenCalledOnce()
    expect(listener3).toHaveBeenCalledOnce()

    // All received the same args
    for (const listener of [listener1, listener2, listener3]) {
      expect(listener).toHaveBeenCalledWith(
        expect.objectContaining({ id: task.id, status: 'running' }),
        'pending',
        'running',
      )
    }
  })

  it('works alongside existing hooks (both fire)', async () => {
    const onTaskTransitioned = vi.fn()
    const { engine } = makeSetup({ onTaskTransitioned })
    const listener = vi.fn()
    engine.addTransitionListener(listener)

    const task = await engine.createTask({ type: 'test' })
    await engine.transitionTask(task.id, 'running')

    // Both the hook and the listener should fire
    expect(onTaskTransitioned).toHaveBeenCalledOnce()
    expect(onTaskTransitioned).toHaveBeenCalledWith(
      expect.objectContaining({ id: task.id, status: 'running' }),
      'pending',
      'running',
    )
    expect(listener).toHaveBeenCalledOnce()
    expect(listener).toHaveBeenCalledWith(
      expect.objectContaining({ id: task.id, status: 'running' }),
      'pending',
      'running',
    )
  })
})
