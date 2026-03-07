import { ulid } from 'ulidx'
import { canTransition, isTerminal, isSuspended } from './state-machine.js'
import { processSeries } from './series.js'
import type {
  Task,
  TaskStatus,
  TaskEvent,
  BlockedRequest,
  TaskFilter,
  BroadcastProvider,
  ShortTermStore,
  LongTermStore,
  TaskcastHooks,
  EventQueryOptions,
} from './types.js'

interface TaskEngineOptionsBase {
  broadcast: BroadcastProvider
  hooks?: TaskcastHooks
}

interface TaskEngineOptionsCanonical extends TaskEngineOptionsBase {
  shortTermStore: ShortTermStore
  longTermStore?: LongTermStore
}

/** @deprecated Use shortTermStore/longTermStore instead */
interface TaskEngineOptionsLegacy extends TaskEngineOptionsBase {
  shortTerm: ShortTermStore
  longTerm?: LongTermStore
}

export type TaskEngineOptions = TaskEngineOptionsCanonical | TaskEngineOptionsLegacy

export interface PublishEventInput {
  type: string
  level: TaskEvent['level']
  data: unknown
  seriesId?: string
  seriesMode?: TaskEvent['seriesMode']
  seriesAccField?: string
}

export interface CreateTaskInput {
  id?: string
  type?: string
  params?: Record<string, unknown>
  metadata?: Record<string, unknown>
  ttl?: number
  webhooks?: Task['webhooks']
  cleanup?: Task['cleanup']
  authConfig?: Task['authConfig']
  tags?: string[]
  assignMode?: Task['assignMode']
  cost?: number
  disconnectPolicy?: Task['disconnectPolicy']
}

export type TransitionListener = (task: Task, from: TaskStatus, to: TaskStatus) => void
export type CreationListener = (task: Task) => void

export class TaskEngine {
  private shortTermStore: ShortTermStore
  private longTermStore: LongTermStore | undefined
  private broadcast: BroadcastProvider
  private hooks: TaskcastHooks | undefined
  private transitionListeners: TransitionListener[] = []
  private creationListeners: CreationListener[] = []

  constructor(opts: TaskEngineOptions) {
    if ('shortTerm' in opts && 'shortTermStore' in opts) {
      throw new Error('Cannot specify both shortTerm and shortTermStore')
    }
    if ('longTerm' in opts && 'longTermStore' in opts) {
      throw new Error('Cannot specify both longTerm and longTermStore')
    }
    this.shortTermStore = 'shortTermStore' in opts ? opts.shortTermStore : opts.shortTerm
    this.longTermStore = 'longTermStore' in opts
      ? opts.longTermStore
      : 'longTerm' in opts
        ? opts.longTerm
        : undefined
    this.broadcast = opts.broadcast
    if (opts.hooks !== undefined) this.hooks = opts.hooks
  }

  async createTask(input: CreateTaskInput): Promise<Task> {
    if (input.ttl !== undefined && input.ttl <= 0) {
      throw new Error(`Invalid TTL: ${input.ttl}. TTL must be a positive number.`)
    }
    if (input.cost !== undefined && input.cost < 0) {
      throw new Error(`Invalid cost: ${input.cost}. Cost must be non-negative.`)
    }

    const now = Date.now()
    const id = input.id ?? ulid()

    // Check for duplicate user-supplied IDs
    if (input.id !== undefined) {
      const existing = await this.shortTermStore.getTask(id)
      if (existing) {
        throw new Error(`Task already exists: ${id}`)
      }
    }

    const task: Task = {
      id,
      status: 'pending',
      createdAt: now,
      updatedAt: now,
      ...(input.type !== undefined && { type: input.type }),
      ...(input.params !== undefined && { params: input.params }),
      ...(input.metadata !== undefined && { metadata: input.metadata }),
      ...(input.ttl !== undefined && { ttl: input.ttl }),
      ...(input.webhooks !== undefined && { webhooks: input.webhooks }),
      ...(input.cleanup !== undefined && { cleanup: input.cleanup }),
      ...(input.authConfig !== undefined && { authConfig: input.authConfig }),
      ...(input.tags !== undefined && { tags: input.tags }),
      ...(input.assignMode !== undefined && { assignMode: input.assignMode }),
      ...(input.cost !== undefined && { cost: input.cost }),
      ...(input.disconnectPolicy !== undefined && { disconnectPolicy: input.disconnectPolicy }),
    }
    await this.shortTermStore.saveTask(task)
    if (this.longTermStore) await this.longTermStore.saveTask(task)
    if (task.ttl) await this.shortTermStore.setTTL(task.id, task.ttl)
    this.hooks?.onTaskCreated?.(task)
    for (const listener of this.creationListeners) {
      try { listener(task) } catch { /* best-effort */ }
    }
    return task
  }

  addTransitionListener(listener: TransitionListener): void {
    this.transitionListeners.push(listener)
  }

  addCreationListener(listener: CreationListener): void {
    this.creationListeners.push(listener)
  }

  removeCreationListener(listener: CreationListener): void {
    const idx = this.creationListeners.indexOf(listener)
    if (idx !== -1) this.creationListeners.splice(idx, 1)
  }

  async getTask(taskId: string): Promise<Task | null> {
    const fromShort = await this.shortTermStore.getTask(taskId)
    if (fromShort) return fromShort
    return this.longTermStore?.getTask(taskId) ?? null
  }

  async transitionTask(
    taskId: string,
    to: TaskStatus,
    payload?: {
      result?: Task['result']
      error?: Task['error']
      reason?: string
      resumeAfterMs?: number
      blockedRequest?: BlockedRequest
      ttl?: number
    },
  ): Promise<Task> {
    const task = await this.getTask(taskId)
    if (!task) throw new Error(`Task not found: ${taskId}`)
    if (!canTransition(task.status, to)) {
      throw new Error(`Invalid transition: ${task.status} → ${to}`)
    }

    const now = Date.now()
    const from = task.status
    const newResult = payload?.result ?? task.result
    const newError = payload?.error ?? task.error
    const newCompletedAt = isTerminal(to) ? now : task.completedAt
    const updated: Task = {
      ...task,
      status: to,
      updatedAt: now,
      ...(newCompletedAt !== undefined && { completedAt: newCompletedAt }),
      ...(newResult !== undefined && { result: newResult }),
      ...(newError !== undefined && { error: newError }),
    }

    // ─── Suspended-state field management ────────────────────────────────
    // Set reason when entering suspended state
    if (isSuspended(to)) {
      if (payload?.reason !== undefined) updated.reason = payload.reason
    } else {
      // Clear suspended fields when leaving suspended state
      delete updated.reason
      delete updated.blockedRequest
      delete updated.resumeAt
    }

    // Blocked-specific: set blockedRequest and resumeAt
    if (to === 'blocked') {
      if (payload?.blockedRequest !== undefined) updated.blockedRequest = payload.blockedRequest
      if (payload?.resumeAfterMs !== undefined) {
        updated.resumeAt = now + payload.resumeAfterMs
      }
    }

    // ─── TTL manipulation for suspended states ───────────────────────────
    // → paused: stop TTL clock
    if (to === 'paused') {
      await this.shortTermStore.clearTTL(taskId)
    }
    // → blocked from paused: restart TTL (clock resumes)
    if (from === 'paused' && to === 'blocked' && updated.ttl) {
      await this.shortTermStore.setTTL(taskId, updated.ttl)
    }
    // paused → running: reset full TTL
    if (from === 'paused' && to === 'running' && updated.ttl) {
      await this.shortTermStore.setTTL(taskId, updated.ttl)
    }
    // blocked → paused: stop TTL clock
    if (from === 'blocked' && to === 'paused') {
      await this.shortTermStore.clearTTL(taskId)
    }

    // TTL override from payload
    if (payload?.ttl !== undefined) {
      updated.ttl = payload.ttl
      if (to !== 'paused') {
        await this.shortTermStore.setTTL(taskId, payload.ttl)
      }
    }

    await this.shortTermStore.saveTask(updated)
    if (this.longTermStore) await this.longTermStore.saveTask(updated)

    await this._emit(taskId, {
      type: 'taskcast:status',
      level: 'info',
      data: { status: to, result: updated.result, error: updated.error },
    })

    // Emit taskcast:blocked when entering blocked with a blockedRequest
    if (to === 'blocked' && updated.blockedRequest) {
      await this._emit(taskId, {
        type: 'taskcast:blocked',
        level: 'info',
        data: { reason: updated.reason, request: updated.blockedRequest },
      })
    }

    // Emit taskcast:resolved when leaving blocked to running (if had a blockedRequest)
    if (from === 'blocked' && to === 'running' && task.blockedRequest) {
      await this._emit(taskId, {
        type: 'taskcast:resolved',
        level: 'info',
        data: { resolution: payload?.result },
      })
    }

    if (to === 'failed' && updated.error) {
      this.hooks?.onTaskFailed?.(updated, updated.error)
    }
    if (to === 'timeout') {
      this.hooks?.onTaskTimeout?.(updated)
    }

    this.hooks?.onTaskTransitioned?.(updated, from, to)

    for (const listener of this.transitionListeners) {
      try { listener(updated, from, to) } catch { /* best-effort */ }
    }

    return updated
  }

  async publishEvent(taskId: string, input: PublishEventInput): Promise<TaskEvent> {
    const task = await this.getTask(taskId)
    if (!task) throw new Error(`Task not found: ${taskId}`)
    if (isTerminal(task.status)) {
      throw new Error(`Cannot publish to task in terminal status: ${task.status}`)
    }

    return this._emit(taskId, input)
  }

  async listTasks(filter: TaskFilter): Promise<Task[]> {
    return this.shortTermStore.listTasks(filter)
  }

  async getEvents(taskId: string, opts?: EventQueryOptions): Promise<TaskEvent[]> {
    return this.shortTermStore.getEvents(taskId, opts)
  }

  subscribe(taskId: string, handler: (event: TaskEvent) => void): () => void {
    return this.broadcast.subscribe(taskId, handler)
  }

  private async _emit(taskId: string, input: PublishEventInput): Promise<TaskEvent> {
    const index = await this.shortTermStore.nextIndex(taskId)
    const raw: TaskEvent = {
      id: ulid(),
      taskId,
      index,
      timestamp: Date.now(),
      type: input.type,
      level: input.level,
      data: input.data,
      ...(input.seriesId !== undefined && { seriesId: input.seriesId }),
      ...(input.seriesMode !== undefined && { seriesMode: input.seriesMode }),
      ...(input.seriesAccField !== undefined && { seriesAccField: input.seriesAccField }),
    }

    const event = await processSeries(raw, this.shortTermStore)
    await this.shortTermStore.appendEvent(taskId, event)
    await this.broadcast.publish(taskId, event)

    if (this.longTermStore) {
      this.longTermStore.saveEvent(event).catch((err) => {
        this.hooks?.onEventDropped?.(event, String(err))
      })
    }

    return event
  }

}
