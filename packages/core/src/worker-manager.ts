import { ulid } from 'ulidx'
import type {
  Task,
  TaskEvent,
  Worker,
  WorkerFilter,
  WorkerAssignment,
  WorkerAuditEvent,
  BroadcastProvider,
  ShortTermStore,
  LongTermStore,
  TaskcastHooks,
  AssignMode,
  DisconnectPolicy,
  WorkerMatchRule,
} from './types.js'
import type { TaskEngine } from './engine.js'
import { matchesWorkerRule } from './worker-matching.js'

// ─── Options & Defaults ─────────────────────────────────────────────────────

export interface WorkerManagerOptions {
  engine: TaskEngine
  shortTerm: ShortTermStore
  broadcast: BroadcastProvider
  longTerm?: LongTermStore
  hooks?: TaskcastHooks
  defaults?: WorkerManagerDefaults
}

export interface WorkerManagerDefaults {
  assignMode?: AssignMode
  heartbeatIntervalMs?: number       // default 30000
  heartbeatTimeoutMs?: number        // default 90000
  offerTimeoutMs?: number            // default 10000
  disconnectPolicy?: DisconnectPolicy // default 'reassign'
  disconnectGraceMs?: number         // default 30000
}

// ─── Registration & Update ──────────────────────────────────────────────────

export interface WorkerRegistration {
  id?: string
  matchRule: WorkerMatchRule
  capacity: number
  weight?: number
  connectionMode: Worker['connectionMode']
  metadata?: Record<string, unknown>
}

export interface WorkerUpdate {
  weight?: number
  capacity?: number
  matchRule?: WorkerMatchRule
}

// ─── Dispatch / Claim / Decline ─────────────────────────────────────────────

export interface DispatchResult {
  matched: boolean
  workerId?: string
}

export interface ClaimResult {
  success: boolean
  reason?: string
}

export interface DeclineOptions {
  blacklist?: boolean
}

// ─── WorkerManager ──────────────────────────────────────────────────────────

export class WorkerManager {
  private engine: TaskEngine
  private shortTerm: ShortTermStore
  private longTerm?: LongTermStore
  private hooks?: TaskcastHooks

  constructor(private opts: WorkerManagerOptions) {
    this.engine = opts.engine
    this.shortTerm = opts.shortTerm
    if (opts.longTerm) this.longTerm = opts.longTerm
    if (opts.hooks) this.hooks = opts.hooks
  }

  // ─── Audit Helpers ──────────────────────────────────────────────────────

  private async emitTaskAudit(taskId: string, action: string, extra?: Record<string, unknown>): Promise<void> {
    try {
      await this.opts.engine.publishEvent(taskId, {
        type: 'taskcast:audit',
        level: 'info',
        data: { action, ...extra },
      })
    } catch {
      // Task may be in terminal state or not found; audit is best-effort
    }
  }

  private emitWorkerAudit(action: WorkerAuditEvent['action'], workerId: string, data?: Record<string, unknown>): void {
    if (!this.opts.longTerm) return
    const event: WorkerAuditEvent = {
      id: ulid(),
      workerId,
      timestamp: Date.now(),
      action,
      ...(data !== undefined && { data }),
    }
    this.opts.longTerm.saveWorkerEvent(event).catch(() => {})
  }

  // ─── Worker Registration & Lifecycle ────────────────────────────────────

  async registerWorker(config: WorkerRegistration): Promise<Worker> {
    const now = Date.now()
    const worker: Worker = {
      id: config.id ?? ulid(),
      status: 'idle',
      matchRule: config.matchRule,
      capacity: config.capacity,
      usedSlots: 0,
      weight: config.weight ?? 50,
      connectionMode: config.connectionMode,
      connectedAt: now,
      lastHeartbeatAt: now,
      ...(config.metadata !== undefined && { metadata: config.metadata }),
    }
    await this.shortTerm.saveWorker(worker)
    this.emitWorkerAudit('connected', worker.id)
    this.hooks?.onWorkerConnected?.(worker)
    return worker
  }

  async unregisterWorker(workerId: string): Promise<void> {
    const worker = await this.shortTerm.getWorker(workerId)
    await this.shortTerm.deleteWorker(workerId)
    if (worker) {
      this.emitWorkerAudit('disconnected', workerId, { reason: 'unregistered' })
      this.hooks?.onWorkerDisconnected?.(worker, 'unregistered')
    }
  }

  async updateWorker(workerId: string, update: WorkerUpdate): Promise<Worker | null> {
    const worker = await this.shortTerm.getWorker(workerId)
    if (!worker) return null

    if (update.weight !== undefined) worker.weight = update.weight
    if (update.capacity !== undefined) worker.capacity = update.capacity
    if (update.matchRule !== undefined) worker.matchRule = update.matchRule

    await this.shortTerm.saveWorker(worker)
    return worker
  }

  async heartbeat(workerId: string): Promise<void> {
    const worker = await this.shortTerm.getWorker(workerId)
    if (!worker) return
    worker.lastHeartbeatAt = Date.now()
    await this.shortTerm.saveWorker(worker)
  }

  async getWorker(workerId: string): Promise<Worker | null> {
    return this.shortTerm.getWorker(workerId)
  }

  async listWorkers(filter?: WorkerFilter): Promise<Worker[]> {
    return this.shortTerm.listWorkers(filter)
  }

  // ─── Task Dispatch ─────────────────────────────────────────────────────

  async dispatchTask(taskId: string): Promise<DispatchResult> {
    const task = await this.engine.getTask(taskId)
    if (!task || task.status !== 'pending') {
      return { matched: false }
    }

    const blacklist = (task.metadata?._blacklistedWorkers as string[] | undefined) ?? []

    const workers = await this.shortTerm.listWorkers({ status: ['idle', 'busy'] })

    const taskCost = task.cost ?? 1
    const candidates = workers.filter((w) => {
      if (blacklist.includes(w.id)) return false
      if (w.usedSlots + taskCost > w.capacity) return false
      if (!matchesWorkerRule(task, w.matchRule)) return false
      return true
    })

    if (candidates.length === 0) {
      return { matched: false }
    }

    // Sort: weight desc → available slots desc → connectedAt asc
    candidates.sort((a, b) => {
      if (b.weight !== a.weight) return b.weight - a.weight
      const aAvailable = a.capacity - a.usedSlots
      const bAvailable = b.capacity - b.usedSlots
      if (bAvailable !== aAvailable) return bAvailable - aAvailable
      return a.connectedAt - b.connectedAt
    })

    return { matched: true, workerId: candidates[0]!.id }
  }

  // ─── Task Claim ────────────────────────────────────────────────────────

  async claimTask(taskId: string, workerId: string): Promise<ClaimResult> {
    const task = await this.engine.getTask(taskId)
    if (!task) {
      return { success: false, reason: 'Task not found' }
    }
    if (task.status !== 'pending') {
      return { success: false, reason: `Task is not pending (status: ${task.status})` }
    }

    const cost = task.cost ?? 1
    const claimed = await this.shortTerm.claimTask(taskId, workerId, cost)
    if (!claimed) {
      return { success: false, reason: 'Claim failed (concurrent modification)' }
    }

    // claimTask atomically sets status to 'assigned' and assignedWorker on the
    // store.  Re-read to get the authoritative state, then persist to longTerm.
    const updatedTask = (await this.shortTerm.getTask(taskId))!
    if (this.longTerm) await this.longTerm.saveTask(updatedTask)

    // Emit audit events for the claim
    this.emitWorkerAudit('task_assigned', workerId, { taskId })
    await this.emitTaskAudit(taskId, 'assigned', { workerId })

    // Create assignment record
    const assignment: WorkerAssignment = {
      taskId,
      workerId,
      cost,
      assignedAt: Date.now(),
      status: 'assigned',
    }
    await this.shortTerm.addAssignment(assignment)

    // Update worker status
    const worker = await this.shortTerm.getWorker(workerId)
    if (worker) {
      worker.usedSlots += cost
      worker.status = worker.usedSlots >= worker.capacity ? 'busy' : 'idle'
      await this.shortTerm.saveWorker(worker)

      this.hooks?.onTaskAssigned?.(updatedTask, worker)
    }

    return { success: true }
  }

  // ─── Task Decline ──────────────────────────────────────────────────────

  async declineTask(taskId: string, workerId: string, opts?: DeclineOptions): Promise<void> {
    const assignment = await this.shortTerm.getTaskAssignment(taskId)
    if (!assignment || assignment.workerId !== workerId) return

    // Remove assignment
    await this.shortTerm.removeAssignment(taskId)

    // Restore worker capacity
    const worker = await this.shortTerm.getWorker(workerId)
    if (worker) {
      worker.usedSlots = Math.max(0, worker.usedSlots - assignment.cost)
      worker.status = 'idle'
      await this.shortTerm.saveWorker(worker)
    }

    // Transition task back to pending
    await this.engine.transitionTask(taskId, 'pending')

    // Emit audit events for the decline
    const blacklisted = opts?.blacklist ?? false
    this.emitWorkerAudit('task_declined', workerId, { taskId })
    await this.emitTaskAudit(taskId, 'declined', { workerId, blacklisted })

    // Clear assignedWorker
    const task = await this.engine.getTask(taskId)
    if (task) {
      delete task.assignedWorker

      // Add to blacklist if requested
      if (opts?.blacklist) {
        const metadata = task.metadata ?? {}
        const existing = (metadata._blacklistedWorkers as string[] | undefined) ?? []
        metadata._blacklistedWorkers = [...existing, workerId]
        task.metadata = metadata
      }

      await this.shortTerm.saveTask(task)
      if (this.longTerm) await this.longTerm.saveTask(task)

      if (worker) {
        this.hooks?.onTaskDeclined?.(task, worker, blacklisted)
      }
    }
  }

  // ─── Worker Tasks ──────────────────────────────────────────────────────

  async getWorkerTasks(workerId: string): Promise<WorkerAssignment[]> {
    return this.shortTerm.getWorkerAssignments(workerId)
  }

  // ─── Pull Mode (Long-Poll) ─────────────────────────────────────────────

  async waitForTask(workerId: string, signal?: AbortSignal): Promise<Task> {
    const worker = await this.shortTerm.getWorker(workerId)
    if (!worker) throw new Error(`Worker not found: ${workerId}`)

    // Check if already aborted
    if (signal?.aborted) {
      throw new Error('aborted')
    }

    // Check existing pending pull tasks
    const pendingTasks = await this.shortTerm.listTasks({ status: ['pending'], assignMode: ['pull'] })
    const blacklist = new Set<string>()
    for (const task of pendingTasks) {
      const taskBlacklist = (task.metadata?._blacklistedWorkers as string[] | undefined) ?? []
      if (taskBlacklist.includes(workerId)) continue
      if (!matchesWorkerRule(task, worker.matchRule)) continue
      const taskCost = task.cost ?? 1
      if (worker.usedSlots + taskCost > worker.capacity) continue

      const result = await this.claimTask(task.id, workerId)
      if (result.success) {
        this.emitWorkerAudit('pull_request', workerId, { matched: true, taskId: task.id })
        const claimed = await this.engine.getTask(task.id)
        return claimed!
      }
    }

    // Wait for a new task notification via broadcast
    return new Promise<Task>((resolve, reject) => {
      let unsubscribe: (() => void) | undefined

      const cleanup = () => {
        unsubscribe?.()
        signal?.removeEventListener('abort', onAbort)
      }

      const onAbort = () => {
        cleanup()
        this.emitWorkerAudit('pull_request', workerId, { matched: false })
        reject(new Error('aborted'))
      }

      if (signal) {
        signal.addEventListener('abort', onAbort)
      }

      unsubscribe = this.opts.broadcast.subscribe('taskcast:worker:new-task', async (event: TaskEvent) => {
        const taskId = event.data as string
        try {
          // Re-fetch the worker to get current state
          const currentWorker = await this.shortTerm.getWorker(workerId)
          if (!currentWorker) {
            cleanup()
            reject(new Error(`Worker not found: ${workerId}`))
            return
          }

          const task = await this.engine.getTask(taskId)
          if (!task || task.status !== 'pending') return
          if (task.assignMode !== 'pull') return

          const taskBlacklist = (task.metadata?._blacklistedWorkers as string[] | undefined) ?? []
          if (taskBlacklist.includes(workerId)) return
          if (!matchesWorkerRule(task, currentWorker.matchRule)) return
          const taskCost = task.cost ?? 1
          if (currentWorker.usedSlots + taskCost > currentWorker.capacity) return

          const result = await this.claimTask(taskId, workerId)
          if (result.success) {
            cleanup()
            this.emitWorkerAudit('pull_request', workerId, { matched: true, taskId })
            const claimed = await this.engine.getTask(taskId)
            resolve(claimed!)
          }
        } catch {
          // Ignore errors from individual task checks
        }
      })
    })
  }

  async notifyNewTask(taskId: string): Promise<void> {
    const event: TaskEvent = {
      id: ulid(),
      taskId: 'system',
      index: 0,
      timestamp: Date.now(),
      type: 'taskcast:worker:new-task',
      level: 'info',
      data: taskId,
    }
    await this.opts.broadcast.publish('taskcast:worker:new-task', event)
  }
}
