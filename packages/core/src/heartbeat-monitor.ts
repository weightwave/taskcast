import type { TaskEngine } from './engine.js'
import type { ShortTermStore, DisconnectPolicy, Worker, TaskStatus } from './types.js'
import type { WorkerManager } from './worker-manager.js'
import { canTransition } from './state-machine.js'

export interface HeartbeatMonitorOptions {
  workerManager: WorkerManager
  engine: TaskEngine
  shortTermStore: ShortTermStore
  checkIntervalMs?: number          // default 30_000
  heartbeatTimeoutMs?: number       // default 90_000
  defaultDisconnectPolicy?: DisconnectPolicy  // default 'reassign'
  disconnectGraceMs?: number        // default 30_000
}

export class HeartbeatMonitor {
  private workerManager: WorkerManager
  private engine: TaskEngine
  private shortTermStore: ShortTermStore
  private checkIntervalMs: number
  private heartbeatTimeoutMs: number
  private defaultDisconnectPolicy: DisconnectPolicy
  private disconnectGraceMs: number
  private timer?: ReturnType<typeof setInterval>
  private graceTimers = new Map<string, ReturnType<typeof setTimeout>>()

  constructor(opts: HeartbeatMonitorOptions) {
    this.workerManager = opts.workerManager
    this.engine = opts.engine
    this.shortTermStore = opts.shortTermStore
    this.checkIntervalMs = opts.checkIntervalMs ?? 30_000
    this.heartbeatTimeoutMs = opts.heartbeatTimeoutMs ?? 90_000
    this.defaultDisconnectPolicy = opts.defaultDisconnectPolicy ?? 'reassign'
    this.disconnectGraceMs = opts.disconnectGraceMs ?? 30_000
  }

  start(): void {
    this.timer = setInterval(() => this.tick().catch(() => {}), this.checkIntervalMs)
  }

  stop(): void {
    if (this.timer) clearInterval(this.timer)
    for (const t of this.graceTimers.values()) clearTimeout(t)
    this.graceTimers.clear()
  }

  async tick(): Promise<void> {
    // List all non-offline workers
    const workers = await this.shortTermStore.listWorkers({ status: ['idle', 'busy', 'draining'] })
    const now = Date.now()

    for (const worker of workers) {
      if (now - worker.lastHeartbeatAt > this.heartbeatTimeoutMs) {
        // Already processing this worker
        if (this.graceTimers.has(worker.id)) continue

        await this._handleTimeout(worker)
      }
    }
  }

  /** Cancel grace timer if worker comes back (call from heartbeat handler) */
  cancelGrace(workerId: string): void {
    const timer = this.graceTimers.get(workerId)
    if (timer) {
      clearTimeout(timer)
      this.graceTimers.delete(workerId)
    }
  }

  private async _handleTimeout(worker: Worker): Promise<void> {
    // Mark worker offline
    worker.status = 'offline'
    await this.shortTermStore.saveWorker(worker)

    // Get all assignments for this worker
    const assignments = await this.workerManager.getWorkerTasks(worker.id)

    // Determine policy per task (task-level override or default)
    for (const assignment of assignments) {
      const task = await this.engine.getTask(assignment.taskId)
      if (!task) continue

      const policy = task.disconnectPolicy ?? this.defaultDisconnectPolicy

      switch (policy) {
        case 'fail':
          try {
            await this.engine.transitionTask(assignment.taskId, 'failed', {
              error: { code: 'WORKER_DISCONNECT', message: `Worker ${worker.id} disconnected (heartbeat timeout)` },
            })
          } catch { /* task may already be terminal */ }
          await this.workerManager.releaseTask(assignment.taskId)
          break

        case 'mark':
          // Just mark offline, leave tasks as-is for external intervention
          break

        case 'reassign':
          this._startGraceTimer(worker.id, assignment.taskId)
          break
      }
    }
  }

  /**
   * Transition a task back to pending through valid state machine transitions.
   * running -> paused -> assigned -> pending
   * assigned -> pending
   * Other states -> try direct pending transition
   */
  private async _transitionToPending(taskId: string): Promise<void> {
    const task = await this.engine.getTask(taskId)
    if (!task) return

    const status = task.status

    // Build a path from current status to pending
    const path = this._findPathToPending(status)
    for (const step of path) {
      try {
        await this.engine.transitionTask(taskId, step)
      } catch {
        // Transition may fail if task was changed externally
        return
      }
    }
  }

  private _findPathToPending(from: TaskStatus): TaskStatus[] {
    if (from === 'pending') return []
    if (canTransition(from, 'pending')) return ['pending']

    // running -> paused -> assigned -> pending
    if (from === 'running') return ['paused', 'assigned', 'pending']
    // paused -> assigned -> pending
    if (from === 'paused') return ['assigned', 'pending']
    // blocked -> assigned -> pending
    if (from === 'blocked') return ['assigned', 'pending']

    return []
  }

  private _startGraceTimer(workerId: string, taskId: string): void {
    // Don't duplicate grace timers for the same worker
    if (this.graceTimers.has(workerId)) return

    const timer = setTimeout(async () => {
      this.graceTimers.delete(workerId)

      // Check if worker came back
      const worker = await this.shortTermStore.getWorker(workerId)
      if (worker && worker.status !== 'offline') return

      // Reassign: revert task to pending through valid transitions
      await this._transitionToPending(taskId)
      await this.workerManager.releaseTask(taskId)
    }, this.disconnectGraceMs)

    this.graceTimers.set(workerId, timer)
  }
}
