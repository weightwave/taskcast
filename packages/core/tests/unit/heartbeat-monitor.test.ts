import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest'
import { TaskEngine } from '../../src/engine.js'
import { MemoryBroadcastProvider, MemoryShortTermStore } from '../../src/memory-adapters.js'
import { WorkerManager } from '../../src/worker-manager.js'
import { HeartbeatMonitor } from '../../src/heartbeat-monitor.js'
import type { DisconnectPolicy } from '../../src/types.js'

function makeSetup(opts?: {
  heartbeatTimeoutMs?: number
  defaultDisconnectPolicy?: DisconnectPolicy
  disconnectGraceMs?: number
}) {
  const store = new MemoryShortTermStore()
  const broadcast = new MemoryBroadcastProvider()
  const engine = new TaskEngine({ shortTermStore: store, broadcast })
  const manager = new WorkerManager({ engine, shortTermStore: store, broadcast })
  const monitor = new HeartbeatMonitor({
    workerManager: manager,
    engine,
    shortTermStore: store,
    checkIntervalMs: 1000,
    heartbeatTimeoutMs: opts?.heartbeatTimeoutMs ?? 5000,
    defaultDisconnectPolicy: opts?.defaultDisconnectPolicy ?? 'reassign',
    disconnectGraceMs: opts?.disconnectGraceMs ?? 3000,
  })
  return { store, broadcast, engine, manager, monitor }
}

/** Helper: register a worker, create a task, claim it, and transition to running */
async function setupWorkerWithTask(
  manager: WorkerManager,
  engine: TaskEngine,
  overrides?: { disconnectPolicy?: DisconnectPolicy },
) {
  const worker = await manager.registerWorker({
    matchRule: {},
    capacity: 5,
    connectionMode: 'pull',
  })
  const task = await engine.createTask({
    assignMode: 'pull',
    ...overrides,
  })
  await manager.claimTask(task.id, worker.id)
  // Transition from assigned to running
  await engine.transitionTask(task.id, 'running')
  return { worker, task }
}

describe('HeartbeatMonitor', () => {
  // ─── Timeout Detection ─────────────────────────────────────────────

  describe('tick() — timeout detection', () => {
    it('marks worker offline when heartbeat times out', async () => {
      const { store, manager, monitor } = makeSetup({ heartbeatTimeoutMs: 5000 })

      const worker = await manager.registerWorker({
        matchRule: {},
        capacity: 5,
        connectionMode: 'pull',
      })

      // Simulate stale heartbeat
      const staleWorker = await store.getWorker(worker.id)
      staleWorker!.lastHeartbeatAt = Date.now() - 10_000
      await store.saveWorker(staleWorker!)

      await monitor.tick()

      const updated = await store.getWorker(worker.id)
      expect(updated!.status).toBe('offline')
    })

    it('does not affect workers with recent heartbeat', async () => {
      const { manager, monitor } = makeSetup({ heartbeatTimeoutMs: 5000 })

      const worker = await manager.registerWorker({
        matchRule: {},
        capacity: 5,
        connectionMode: 'pull',
      })

      // Heartbeat is fresh (just registered)
      await monitor.tick()

      const updated = await manager.getWorker(worker.id)
      expect(updated!.status).toBe('idle')
    })
  })

  // ─── Fail Policy ──────────────────────────────────────────────────

  describe('tick() with fail policy', () => {
    it('transitions task to failed and releases assignment', async () => {
      const { store, engine, manager, monitor } = makeSetup({
        heartbeatTimeoutMs: 5000,
        defaultDisconnectPolicy: 'fail',
      })

      const { worker, task } = await setupWorkerWithTask(manager, engine)

      // Expire the heartbeat
      const w = await store.getWorker(worker.id)
      w!.lastHeartbeatAt = Date.now() - 10_000
      await store.saveWorker(w!)

      await monitor.tick()

      // Worker should be offline
      const updatedWorker = await store.getWorker(worker.id)
      expect(updatedWorker!.status).toBe('offline')

      // Task should be failed
      const updatedTask = await engine.getTask(task.id)
      expect(updatedTask!.status).toBe('failed')
      expect(updatedTask!.error?.code).toBe('WORKER_DISCONNECT')

      // Assignment should be removed
      const assignment = await store.getTaskAssignment(task.id)
      expect(assignment).toBeNull()
    })
  })

  // ─── Mark Policy ──────────────────────────────────────────────────

  describe('tick() with mark policy', () => {
    it('only marks offline, leaves tasks as-is', async () => {
      const { store, engine, manager, monitor } = makeSetup({
        heartbeatTimeoutMs: 5000,
        defaultDisconnectPolicy: 'mark',
      })

      const { worker, task } = await setupWorkerWithTask(manager, engine)

      // Expire the heartbeat
      const w = await store.getWorker(worker.id)
      w!.lastHeartbeatAt = Date.now() - 10_000
      await store.saveWorker(w!)

      await monitor.tick()

      // Worker should be offline
      const updatedWorker = await store.getWorker(worker.id)
      expect(updatedWorker!.status).toBe('offline')

      // Task should still be running (not changed)
      const updatedTask = await engine.getTask(task.id)
      expect(updatedTask!.status).toBe('running')

      // Assignment should still exist
      const assignment = await store.getTaskAssignment(task.id)
      expect(assignment).not.toBeNull()
    })
  })

  // ─── Reassign Policy ──────────────────────────────────────────────

  describe('tick() with reassign policy', () => {
    beforeEach(() => {
      vi.useFakeTimers()
    })

    afterEach(() => {
      vi.useRealTimers()
    })

    it('starts grace timer then reassigns after grace period', async () => {
      const { store, engine, manager, monitor } = makeSetup({
        heartbeatTimeoutMs: 5000,
        defaultDisconnectPolicy: 'reassign',
        disconnectGraceMs: 3000,
      })

      const { worker, task } = await setupWorkerWithTask(manager, engine)

      // Expire the heartbeat
      const w = await store.getWorker(worker.id)
      w!.lastHeartbeatAt = Date.now() - 10_000
      await store.saveWorker(w!)

      await monitor.tick()

      // Worker should be offline immediately
      const updatedWorker = await store.getWorker(worker.id)
      expect(updatedWorker!.status).toBe('offline')

      // Task should still be running during grace period
      const taskDuringGrace = await engine.getTask(task.id)
      expect(taskDuringGrace!.status).toBe('running')

      // Advance past grace period
      await vi.advanceTimersByTimeAsync(3500)

      // Task should now be reverted to pending
      const reassigned = await engine.getTask(task.id)
      expect(reassigned!.status).toBe('pending')

      // Assignment should be removed
      const assignment = await store.getTaskAssignment(task.id)
      expect(assignment).toBeNull()
    })

    it('cancelGrace() prevents reassignment', async () => {
      const { store, engine, manager, monitor } = makeSetup({
        heartbeatTimeoutMs: 5000,
        defaultDisconnectPolicy: 'reassign',
        disconnectGraceMs: 3000,
      })

      const { worker, task } = await setupWorkerWithTask(manager, engine)

      // Expire the heartbeat
      const w = await store.getWorker(worker.id)
      w!.lastHeartbeatAt = Date.now() - 10_000
      await store.saveWorker(w!)

      await monitor.tick()

      // Cancel grace before it fires
      monitor.cancelGrace(worker.id)

      // Advance past grace period
      await vi.advanceTimersByTimeAsync(5000)

      // Task should still be running (grace was cancelled)
      const taskAfter = await engine.getTask(task.id)
      expect(taskAfter!.status).toBe('running')
    })

    it('worker comes back before grace expires — no reassignment', async () => {
      const { store, engine, manager, monitor } = makeSetup({
        heartbeatTimeoutMs: 5000,
        defaultDisconnectPolicy: 'reassign',
        disconnectGraceMs: 3000,
      })

      const { worker, task } = await setupWorkerWithTask(manager, engine)

      // Expire the heartbeat
      const w = await store.getWorker(worker.id)
      w!.lastHeartbeatAt = Date.now() - 10_000
      await store.saveWorker(w!)

      await monitor.tick()

      // Worker comes back online before grace expires
      const comeback = await store.getWorker(worker.id)
      comeback!.status = 'idle'
      comeback!.lastHeartbeatAt = Date.now()
      await store.saveWorker(comeback!)

      // Advance past grace period
      await vi.advanceTimersByTimeAsync(3500)

      // Task should still be running because worker came back
      const taskAfter = await engine.getTask(task.id)
      expect(taskAfter!.status).toBe('running')
    })
  })

  // ─── Task-Level Policy Override ────────────────────────────────────

  describe('task-level disconnectPolicy overrides default', () => {
    it('uses task disconnectPolicy=fail even when default is reassign', async () => {
      const { store, engine, manager, monitor } = makeSetup({
        heartbeatTimeoutMs: 5000,
        defaultDisconnectPolicy: 'reassign',
      })

      const { worker, task } = await setupWorkerWithTask(manager, engine, {
        disconnectPolicy: 'fail',
      })

      // Expire the heartbeat
      const w = await store.getWorker(worker.id)
      w!.lastHeartbeatAt = Date.now() - 10_000
      await store.saveWorker(w!)

      await monitor.tick()

      // Task should be failed (task-level override to 'fail')
      const updatedTask = await engine.getTask(task.id)
      expect(updatedTask!.status).toBe('failed')
      expect(updatedTask!.error?.code).toBe('WORKER_DISCONNECT')
    })

    it('uses task disconnectPolicy=mark even when default is fail', async () => {
      const { store, engine, manager, monitor } = makeSetup({
        heartbeatTimeoutMs: 5000,
        defaultDisconnectPolicy: 'fail',
      })

      const { worker, task } = await setupWorkerWithTask(manager, engine, {
        disconnectPolicy: 'mark',
      })

      // Expire the heartbeat
      const w = await store.getWorker(worker.id)
      w!.lastHeartbeatAt = Date.now() - 10_000
      await store.saveWorker(w!)

      await monitor.tick()

      // Task should still be running (mark policy)
      const updatedTask = await engine.getTask(task.id)
      expect(updatedTask!.status).toBe('running')
    })
  })

  // ─── stop() ────────────────────────────────────────────────────────

  describe('stop()', () => {
    beforeEach(() => {
      vi.useFakeTimers()
    })

    afterEach(() => {
      vi.useRealTimers()
    })

    it('clears interval and all grace timers', async () => {
      const { store, engine, manager, monitor } = makeSetup({
        heartbeatTimeoutMs: 5000,
        defaultDisconnectPolicy: 'reassign',
        disconnectGraceMs: 3000,
      })

      const { worker, task } = await setupWorkerWithTask(manager, engine)

      // Expire the heartbeat
      const w = await store.getWorker(worker.id)
      w!.lastHeartbeatAt = Date.now() - 10_000
      await store.saveWorker(w!)

      monitor.start()
      await monitor.tick()

      // Grace timer should be active now
      monitor.stop()

      // Advance past grace period — no reassignment should happen
      await vi.advanceTimersByTimeAsync(5000)

      // Task should still be running (grace timer was cleared by stop)
      const taskAfter = await engine.getTask(task.id)
      expect(taskAfter!.status).toBe('running')
    })

    it('is safe to call without start()', () => {
      const { monitor } = makeSetup()
      expect(() => monitor.stop()).not.toThrow()
    })
  })

  // ─── Multiple Assignments ──────────────────────────────────────────

  describe('multiple assignments per worker', () => {
    it('handles all tasks for a disconnected worker', async () => {
      const { store, engine, manager, monitor } = makeSetup({
        heartbeatTimeoutMs: 5000,
        defaultDisconnectPolicy: 'fail',
      })

      // Register worker with enough capacity for two tasks
      const worker = await manager.registerWorker({
        matchRule: {},
        capacity: 5,
        connectionMode: 'pull',
      })

      // Create and assign two tasks
      const task1 = await engine.createTask({ assignMode: 'pull' })
      const task2 = await engine.createTask({ assignMode: 'pull' })

      await manager.claimTask(task1.id, worker.id)
      await manager.claimTask(task2.id, worker.id)
      await engine.transitionTask(task1.id, 'running')
      await engine.transitionTask(task2.id, 'running')

      // Expire the heartbeat
      const w = await store.getWorker(worker.id)
      w!.lastHeartbeatAt = Date.now() - 10_000
      await store.saveWorker(w!)

      await monitor.tick()

      // Both tasks should be failed
      const updated1 = await engine.getTask(task1.id)
      const updated2 = await engine.getTask(task2.id)
      expect(updated1!.status).toBe('failed')
      expect(updated2!.status).toBe('failed')
      expect(updated1!.error?.code).toBe('WORKER_DISCONNECT')
      expect(updated2!.error?.code).toBe('WORKER_DISCONNECT')
    })
  })

  // ─── Start/Stop Lifecycle ──────────────────────────────────────────

  describe('start/stop lifecycle', () => {
    beforeEach(() => {
      vi.useFakeTimers()
    })

    afterEach(() => {
      vi.useRealTimers()
    })

    it('start() creates periodic interval that calls tick', async () => {
      const { monitor } = makeSetup()
      const tickSpy = vi.spyOn(monitor, 'tick').mockResolvedValue(undefined)

      monitor.start()

      expect(tickSpy).not.toHaveBeenCalled()

      await vi.advanceTimersByTimeAsync(1000)
      expect(tickSpy).toHaveBeenCalledTimes(1)

      await vi.advanceTimersByTimeAsync(1000)
      expect(tickSpy).toHaveBeenCalledTimes(2)

      monitor.stop()
    })

    it('stop() clears the interval so tick is no longer called', async () => {
      const { monitor } = makeSetup()
      const tickSpy = vi.spyOn(monitor, 'tick').mockResolvedValue(undefined)

      monitor.start()

      await vi.advanceTimersByTimeAsync(1000)
      expect(tickSpy).toHaveBeenCalledTimes(1)

      monitor.stop()

      await vi.advanceTimersByTimeAsync(5000)
      expect(tickSpy).toHaveBeenCalledTimes(1)
    })
  })
})
