import { ulid } from 'ulidx'
import { canTransition, isTerminal } from './state-machine.js'
import { processSeries } from './series.js'
import type {
  Task,
  TaskStatus,
  TaskEvent,
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

export class TaskEngine {
  private shortTermStore: ShortTermStore
  private longTermStore: LongTermStore | undefined
  private broadcast: BroadcastProvider
  private hooks: TaskcastHooks | undefined

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
    const now = Date.now()
    const task: Task = {
      id: input.id ?? ulid(),
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
    return task
  }

  async getTask(taskId: string): Promise<Task | null> {
    const fromShort = await this.shortTermStore.getTask(taskId)
    if (fromShort) return fromShort
    return this.longTermStore?.getTask(taskId) ?? null
  }

  async transitionTask(
    taskId: string,
    to: TaskStatus,
    payload?: { result?: Task['result']; error?: Task['error'] },
  ): Promise<Task> {
    const task = await this.getTask(taskId)
    if (!task) throw new Error(`Task not found: ${taskId}`)
    if (!canTransition(task.status, to)) {
      throw new Error(`Invalid transition: ${task.status} → ${to}`)
    }

    const now = Date.now()
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

    await this.shortTermStore.saveTask(updated)
    if (this.longTermStore) await this.longTermStore.saveTask(updated)

    await this._emit(taskId, {
      type: 'taskcast:status',
      level: 'info',
      data: { status: to, result: updated.result, error: updated.error },
    })

    if (to === 'failed' && updated.error) {
      this.hooks?.onTaskFailed?.(updated, updated.error)
    }
    if (to === 'timeout') {
      this.hooks?.onTaskTimeout?.(updated)
    }

    this.hooks?.onTaskTransitioned?.(updated, task.status, to)

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
