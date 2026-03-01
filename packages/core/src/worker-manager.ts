import { ulid } from 'ulidx'
import type {
  Task,
  Worker,
  WorkerFilter,
  WorkerAssignment,
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
    this.hooks?.onWorkerConnected?.(worker)
    return worker
  }

  async unregisterWorker(workerId: string): Promise<void> {
    const worker = await this.shortTerm.getWorker(workerId)
    await this.shortTerm.deleteWorker(workerId)
    if (worker) {
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
        this.hooks?.onTaskDeclined?.(task, worker, opts?.blacklist ?? false)
      }
    }
  }

  // ─── Worker Tasks ──────────────────────────────────────────────────────

  async getWorkerTasks(workerId: string): Promise<WorkerAssignment[]> {
    return this.shortTerm.getWorkerAssignments(workerId)
  }
}
