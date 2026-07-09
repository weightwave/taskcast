import { ulid } from 'ulidx'
import { canTransition, isTerminal, isSuspended } from './state-machine.js'
import { processSeries } from './series.js'
import { InvalidTaskArchiveError, buildTaskArchiveRestoreData, normalizeTaskArchive } from './archive.js'
import type {
  Task,
  TaskStatus,
  TaskEvent,
  TaskArchive,
  TaskArchiveImportOptions,
  TaskArchiveImportResult,
  BlockedRequest,
  TaskFilter,
  BroadcastProvider,
  ShortTermStore,
  LongTermStore,
  TaskcastHooks,
  EventQueryOptions,
  TaskArchiveEvent,
} from './types.js'

// ─── Error Classes ──────────────────────────────────────────────────────────

export class TaskConflictError extends Error {
  constructor(taskId: string) {
    super(`Task already exists: ${taskId}`)
    this.name = 'TaskConflictError'
  }
}

export class InvalidTransitionError extends Error {
  public readonly from: TaskStatus
  public readonly to: TaskStatus

  constructor(from: TaskStatus, to: TaskStatus) {
    super(`Invalid transition: ${from} → ${to}`)
    this.name = 'InvalidTransitionError'
    this.from = from
    this.to = to
  }
}

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
  /** Per-task promise chain to serialize `_emit` calls, preventing race
   *  conditions where concurrent publishes store events out of index order. */
  private _emitChains = new Map<string, Promise<void>>()

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
      if (existing) throw new TaskConflictError(id)
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
      throw new InvalidTransitionError(task.status, to)
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

    // Clean up per-task emit chain — no more events can be published
    // to a terminal task (publishEvent rejects), so the chain is unused.
    // A reopened task will lazily recreate the entry on next emit.
    if (isTerminal(to)) {
      this._emitChains.delete(taskId)
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

  async exportTaskArchive(taskId: string): Promise<TaskArchive> {
    const task = await this.getTask(taskId)
    if (!task) throw new Error(`Task not found: ${taskId}`)

    return this.buildExportArchive(task)
  }

  private async buildExportArchive(task: Task): Promise<TaskArchive> {
    const shortTermEvents = await this.shortTermStore.getEvents(task.id)
    if (this.longTermStore) {
      const longTermEvents = await this.longTermStore.getEvents(task.id)
      if (longTermEvents.length > 0) {
        return this.normalizeExportArchive(task, this.mergeExportHistories(longTermEvents, shortTermEvents))
      }
    }

    return this.normalizeExportArchive(task, shortTermEvents)
  }

  private mergeExportHistories(longTermEvents: TaskEvent[], shortTermEvents: TaskEvent[]): TaskEvent[] {
    const shortTermEventsByIndex = new Map<number, TaskEvent>()
    for (const event of shortTermEvents) {
      shortTermEventsByIndex.set(event.index, event)
    }

    const longTermIndexes = new Set(longTermEvents.map((event) => event.index))
    const maxLongTermIndex = Math.max(...longTermIndexes)
    for (let index = 0; index <= maxLongTermIndex; index++) {
      if (longTermIndexes.has(index)) continue

      const shortTermEvent = shortTermEventsByIndex.get(index)
      if (shortTermEvent && !this.isCompactableSeriesEvent(shortTermEvent)) {
        throw new InvalidTaskArchiveError(
          `Cannot export sparse long-term history; missing durable non-series event at index ${index}`,
        )
      }
    }

    const mergedByKey = new Map<string, TaskEvent>()
    for (const event of longTermEvents) {
      mergedByKey.set(`${event.id}:${event.index}`, event)
    }

    const longTermPrefixEnd = this.getContiguousPrefixEnd(longTermEvents)
    for (const event of shortTermEvents) {
      const key = `${event.id}:${event.index}`
      if (mergedByKey.has(key)) continue
      if (event.index > longTermPrefixEnd || this.isCompactableSeriesEvent(event)) {
        mergedByKey.set(key, event)
      }
    }

    return Array.from(mergedByKey.values())
  }

  private getContiguousPrefixEnd(events: TaskEvent[]): number {
    const indexes = new Set<number>()
    for (const event of events) {
      indexes.add(event.index)
    }

    let expected = 0
    while (indexes.has(expected)) {
      expected += 1
    }
    return expected - 1
  }

  private async normalizeExportArchive(task: Task, events: TaskEvent[]): Promise<TaskArchive> {
    const compactedEvents = await this.compactExportEvents(task.id, events)
    const archive: TaskArchive = {
      schema: 'taskcast.taskArchive',
      version: 1,
      exportedAt: Date.now(),
      task: { ...task },
      events: compactedEvents,
    }

    return normalizeTaskArchive(archive)
  }

  private async compactExportEvents(taskId: string, events: TaskEvent[]): Promise<TaskArchiveEvent[]> {
    type ExportEntry = {
      event: TaskEvent
      firstIndex: number
      lastIndex: number
      order: number
    }

    const entries: ExportEntry[] = []
    const seriesEntries = new Map<string, ExportEntry>()
    const sorted = [...events].sort((a, b) => a.index - b.index)

    for (const event of sorted) {
      if (!this.isCompactableSeriesEvent(event)) {
        entries.push({
          event,
          firstIndex: event.index,
          lastIndex: event.index,
          order: entries.length,
        })
        continue
      }

      const key = `${event.taskId}:${event.seriesId}`
      const existing = seriesEntries.get(key)
      if (!existing) {
        const entry = {
          event,
          firstIndex: event.index,
          lastIndex: event.index,
          order: entries.length,
        }
        seriesEntries.set(key, entry)
        entries.push(entry)
        continue
      }

      if (event.index >= existing.lastIndex) {
        existing.event = event
        existing.lastIndex = event.index
      }
    }

    for (const entry of seriesEntries.values()) {
      const { event } = entry
      if (!event.seriesId) continue

      const shortTermLatest = await this.shortTermStore.getSeriesLatest(taskId, event.seriesId)
      if (shortTermLatest && shortTermLatest.index >= entry.lastIndex) {
        entry.event = shortTermLatest
        entry.lastIndex = shortTermLatest.index
      }
    }

    return entries
      .sort((a, b) => (a.firstIndex - b.firstIndex) || (a.order - b.order))
      .map((entry, index) => this.toArchiveEvent(entry.event, index))
  }

  private isCompactableSeriesEvent(event: TaskEvent): boolean {
    return Boolean(event.seriesId && (event.seriesMode === 'latest' || event.seriesMode === 'accumulate'))
  }

  private toArchiveEvent(event: TaskEvent, index: number): TaskArchiveEvent {
    const { id, taskId, timestamp, type, level, data, seriesId, seriesMode, seriesAccField } = event
    return {
      id,
      taskId,
      index,
      timestamp,
      type,
      level,
      data,
      ...(seriesId !== undefined ? { seriesId } : {}),
      ...(seriesMode !== undefined ? { seriesMode } : {}),
      ...(seriesAccField !== undefined ? { seriesAccField } : {}),
    }
  }

  async importTaskArchive(
    archive: TaskArchive,
    options?: TaskArchiveImportOptions,
  ): Promise<TaskArchiveImportResult> {
    const normalized = normalizeTaskArchive(archive)
    const taskId = normalized.task.id
    const existing = await this.getTask(taskId)

    if (existing && options?.overwrite !== true) throw new TaskConflictError(taskId)

    if (typeof this.shortTermStore.restoreTaskArchive !== 'function') {
      throw new Error('shortTermStore does not support restoreTaskArchive')
    }
    const longTermSharesArchiveRestoreStorage =
      this.longTermStore?.sharesTaskArchiveRestoreStorage === true

    if (
      this.longTermStore &&
      !longTermSharesArchiveRestoreStorage &&
      typeof this.longTermStore.restoreTaskArchive !== 'function'
    ) {
      throw new Error('longTermStore does not support restoreTaskArchive')
    }

    const restoreData = buildTaskArchiveRestoreData(normalized)
    await this.shortTermStore.validateTaskArchiveRestore?.(restoreData, options)
    if (this.longTermStore) {
      await this.longTermStore.validateTaskArchiveRestore?.(restoreData, options)
    }

    // Durable history is restored before the live short-term cache so a final
    // long-term failure cannot expose an imported task that was never persisted.
    if (this.longTermStore && !longTermSharesArchiveRestoreStorage) {
      await this.longTermStore.restoreTaskArchive!(restoreData, options)
    }
    await this.shortTermStore.restoreTaskArchive(restoreData, options)
    this._emitChains.delete(taskId)

    return {
      taskId,
      eventCount: normalized.events.length,
      overwritten: existing !== null,
    }
  }

  async listTasks(filter: TaskFilter): Promise<Task[]> {
    return this.shortTermStore.listTasks(filter)
  }

  async getEvents(taskId: string, opts?: EventQueryOptions): Promise<TaskEvent[]> {
    const fromShort = await this.shortTermStore.getEvents(taskId, opts)
    if (fromShort.length > 0) return fromShort
    if (this.longTermStore) {
      return this.longTermStore.getEvents(taskId, opts)
    }
    return []
  }

  subscribe(taskId: string, handler: (event: TaskEvent) => void): () => void {
    return this.broadcast.subscribe(taskId, handler)
  }

  async getSeriesLatest(taskId: string, seriesId: string): Promise<TaskEvent | null> {
    return this.shortTermStore.getSeriesLatest(taskId, seriesId)
  }

  private async _emit(taskId: string, input: PublishEventInput): Promise<TaskEvent> {
    // Serialize emit calls per task to prevent race conditions where
    // concurrent publishes store events in a different order than
    // their atomically-assigned indices.
    const prev = this._emitChains.get(taskId) ?? Promise.resolve()
    let release!: () => void
    const gate = new Promise<void>((r) => { release = r })
    this._emitChains.set(taskId, gate)

    await prev
    try {
      return await this._emitInner(taskId, input)
    } finally {
      release()
    }
  }

  private async _emitInner(taskId: string, input: PublishEventInput): Promise<TaskEvent> {
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

    const { event, accumulatedEvent, stored } = await processSeries(raw, this.shortTermStore)
    if (!stored) {
      await this.shortTermStore.appendEvent(taskId, event)
    }

    // Attach accumulated data to broadcast for SSE accumulated subscribers
    const broadcastEvent = accumulatedEvent
      ? { ...event, _accumulatedData: accumulatedEvent.data }
      : event
    await this.broadcast.publish(taskId, broadcastEvent)

    if (this.longTermStore) {
      const storeEvent = accumulatedEvent ?? event
      this.persistLongTermEvent(event, accumulatedEvent).catch((err) => {
        this.hooks?.onEventDropped?.(storeEvent, String(err))
      })
    }

    return event
  }

  private async persistLongTermEvent(event: TaskEvent, accumulatedEvent?: TaskEvent): Promise<void> {
    if (!this.longTermStore) return

    if (
      event.seriesId &&
      event.seriesMode === 'latest' &&
      typeof this.longTermStore.replaceLastSeriesEvent === 'function'
    ) {
      await this.longTermStore.replaceLastSeriesEvent(event.taskId, event.seriesId, event)
      return
    }

    if (
      event.seriesId &&
      event.seriesMode === 'accumulate' &&
      typeof this.longTermStore.accumulateSeries === 'function'
    ) {
      await this.longTermStore.accumulateSeries(
        event.taskId,
        event.seriesId,
        event,
        event.seriesAccField ?? 'delta',
      )
      return
    }

    // Compatibility fallback for older LongTermStore implementations.
    await this.longTermStore.saveEvent(accumulatedEvent ?? event)
  }

}
