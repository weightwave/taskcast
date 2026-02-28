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

export interface TaskEngineOptions {
  shortTerm: ShortTermStore
  broadcast: BroadcastProvider
  longTerm?: LongTermStore
  hooks?: TaskcastHooks
}

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
}

export class TaskEngine {
  private indexCounters = new Map<string, number>()

  constructor(private opts: TaskEngineOptions) {}

  async createTask(input: CreateTaskInput): Promise<Task> {
    const now = Date.now()
    const task: Task = {
      id: input.id ?? ulid(),
      type: input.type,
      status: 'pending',
      params: input.params,
      metadata: input.metadata,
      createdAt: now,
      updatedAt: now,
      ttl: input.ttl,
      webhooks: input.webhooks,
      cleanup: input.cleanup,
      authConfig: input.authConfig,
    }
    await this.opts.shortTerm.saveTask(task)
    if (this.opts.longTerm) await this.opts.longTerm.saveTask(task)
    if (task.ttl) await this.opts.shortTerm.setTTL(task.id, task.ttl)
    return task
  }

  async getTask(taskId: string): Promise<Task | null> {
    const fromShort = await this.opts.shortTerm.getTask(taskId)
    if (fromShort) return fromShort
    return this.opts.longTerm?.getTask(taskId) ?? null
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
    const updated: Task = {
      ...task,
      status: to,
      updatedAt: now,
      completedAt: isTerminal(to) ? now : task.completedAt,
      result: payload?.result ?? task.result,
      error: payload?.error ?? task.error,
    }

    await this.opts.shortTerm.saveTask(updated)
    if (this.opts.longTerm) await this.opts.longTerm.saveTask(updated)

    await this._emit(taskId, {
      type: 'taskcast:status',
      level: 'info',
      data: { status: to, result: updated.result, error: updated.error },
    })

    if (to === 'failed' && updated.error) {
      this.opts.hooks?.onTaskFailed?.(updated, updated.error)
    }
    if (to === 'timeout') {
      this.opts.hooks?.onTaskTimeout?.(updated)
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

  async getEvents(taskId: string, opts?: EventQueryOptions): Promise<TaskEvent[]> {
    return this.opts.shortTerm.getEvents(taskId, opts)
  }

  subscribe(taskId: string, handler: (event: TaskEvent) => void): () => void {
    return this.opts.broadcast.subscribe(taskId, handler)
  }

  private async _emit(taskId: string, input: PublishEventInput): Promise<TaskEvent> {
    const index = this._nextIndex(taskId)
    const raw: TaskEvent = {
      id: ulid(),
      taskId,
      index,
      timestamp: Date.now(),
      type: input.type,
      level: input.level,
      data: input.data,
      seriesId: input.seriesId,
      seriesMode: input.seriesMode,
    }

    const event = await processSeries(raw, this.opts.shortTerm)
    await this.opts.shortTerm.appendEvent(taskId, event)
    await this.opts.broadcast.publish(taskId, event)

    if (this.opts.longTerm) {
      this.opts.longTerm.saveEvent(event).catch((err) => {
        this.opts.hooks?.onEventDropped?.(event, String(err))
      })
    }

    return event
  }

  private _nextIndex(taskId: string): number {
    const current = this.indexCounters.get(taskId) ?? -1
    const next = current + 1
    this.indexCounters.set(taskId, next)
    return next
  }
}
