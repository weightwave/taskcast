import { ulid } from 'ulidx'
import { canTransition, isTerminal, isSuspended } from './state-machine.js'
import { processSeries } from './series.js'
import { StorageCoordinator } from './storage-coordinator.js'
import {
  applyCanonicalHistoryQuery,
  mergeCanonicalHistory,
  resolveCanonicalSeriesLatest,
} from './canonical-history.js'
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
  ReleasePreconditions,
  ReleaseResult,
  StorageReleaseRequest,
  StorageWriterRegistration,
  HotWriteToken,
  DurableSeriesState,
} from './types.js'
import {
  StorageFenceConflictError,
  StorageIntegrityError,
  StoragePreconditionError,
  StorageReleaseUnsupportedError,
} from './types.js'

// ─── Error Classes ──────────────────────────────────────────────────────────

export class TaskConflictError extends Error {
  constructor(taskId: string) {
    super(`Task already exists: ${taskId}`)
    this.name = 'TaskConflictError'
  }
}

function canonicalDurableQuery(
  opts: EventQueryOptions | undefined,
  hotEvents: readonly TaskEvent[],
  durableSeries: readonly DurableSeriesState[],
): EventQueryOptions | undefined {
  if (!opts) return undefined
  let since = opts.since
  if (since?.id) {
    const anchor = hotEvents.find((event) => event.id === since!.id)
      ?? durableSeries.find((state) => state.event.id === since!.id)?.event
    if (anchor) since = { index: anchor.index }
  }
  return {
    ...(since && { since }),
    ...(opts.limit !== undefined && { limit: opts.limit }),
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
  private static readonly CREATION_CLAIM_TTL_MS = 30_000
  private shortTermStore: ShortTermStore
  private longTermStore: LongTermStore | undefined
  private broadcast: BroadcastProvider
  private hooks: TaskcastHooks | undefined
  private storageCoordinator: StorageCoordinator | undefined
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
    if (
      this.longTermStore?.supportsHotColdRelease === true &&
      this.shortTermStore.supportsHotColdRelease === true
    ) {
      this.storageCoordinator = new StorageCoordinator({
        shortTermStore: this.shortTermStore,
        longTermStore: this.longTermStore,
      })
    }
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
    const durable = this.longTermStore
    const canFenceCreation =
      durable?.claimTaskCreation !== undefined &&
      durable.completeTaskCreation !== undefined &&
      durable.abortTaskCreation !== undefined

    // Explicit IDs are durable identities, including while their hot state is cold.
    // A leased creation store must inspect the claim itself so an expired
    // pristine row left by a crashed creator can be taken over.
    if (input.id !== undefined && !canFenceCreation) {
      const existing = await this.getTask(id)
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
    let durableIdentityClaimed = false
    let creationToken: string | null = null
    if (input.id !== undefined && durable) {
      if (canFenceCreation) {
        creationToken = ulid()
        durableIdentityClaimed = await durable.claimTaskCreation!(
          task,
          creationToken,
          TaskEngine.CREATION_CLAIM_TTL_MS,
        )
      } else if (durable.supportsHotColdRelease === true) {
        throw new StorageReleaseUnsupportedError(
          'Hot/cold long-term stores must support token-fenced task creation',
        )
      } else if (durable.createTaskIfAbsent) {
        durableIdentityClaimed = await durable.createTaskIfAbsent(task)
      }
      if (
        (canFenceCreation || durable.createTaskIfAbsent !== undefined) &&
        !durableIdentityClaimed
      ) {
        throw new TaskConflictError(id)
      }
    }
    try {
      await this.shortTermStore.saveTask(task)
    } catch (error) {
      if (creationToken !== null) {
        await durable!.abortTaskCreation!(id, creationToken)
      }
      throw error
    }
    if (creationToken !== null) {
      let completed = false
      let completionError: unknown
      for (let attempt = 0; attempt < 3 && !completed; attempt++) {
        try {
          completed = await durable!.completeTaskCreation!(id, creationToken)
          completionError = undefined
        } catch (error) {
          completionError = error
        }
      }
      if (completionError !== undefined) throw completionError
      if (!completed) {
        throw new StorageReleaseUnsupportedError(
          `Durable creation claim was lost for task ${id}`,
        )
      }
    } else if (durable && !durableIdentityClaimed) {
      await durable.saveTask(task)
    }
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
    let expectedRevision: string | null = null
    let initialWriteToken: HotWriteToken | null = null
    let task: Task | null
    if (this.storageCoordinator) {
      const getSnapshot = this.shortTermStore.getTaskMutationSnapshot
      if (!getSnapshot) throw new StorageReleaseUnsupportedError()
      initialWriteToken = await this.storageCoordinator.ensureTaskHotForWrite(taskId)
      const snapshot = await getSnapshot.call(this.shortTermStore, taskId)
      task = snapshot?.task ?? null
      expectedRevision = snapshot?.revision ?? null
    } else {
      task = await this.getTask(taskId)
    }
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

    const derivedEvents: PublishEventInput[] = [{
      type: 'taskcast:status',
      level: 'info',
      data: { status: to, result: updated.result, error: updated.error },
    }]

    // Emit taskcast:blocked when entering blocked with a blockedRequest
    if (to === 'blocked' && updated.blockedRequest) {
      derivedEvents.push({
        type: 'taskcast:blocked',
        level: 'info',
        data: { reason: updated.reason, request: updated.blockedRequest },
      })
    }

    // Emit taskcast:resolved when leaving blocked to running (if had a blockedRequest)
    if (from === 'blocked' && to === 'running' && task.blockedRequest) {
      derivedEvents.push({
        type: 'taskcast:resolved',
        level: 'info',
        data: { resolution: payload?.result },
      })
    }

    if (this.storageCoordinator) {
      const committed = await this.commitTaskEventsForMutation(
        updated,
        expectedRevision!,
        from,
        derivedEvents,
        initialWriteToken!,
      )
      if (this.longTermStore) await this.longTermStore.saveTask(updated)
      for (const event of committed) {
        await this.finishCommittedEvent(event)
      }
    } else {
      await this.shortTermStore.saveTask(updated)
      if (this.longTermStore) await this.longTermStore.saveTask(updated)
      for (const event of derivedEvents) {
        await this._emit(taskId, event, true)
      }
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

  async releaseTaskStorage(
    taskId: string,
    preconditions: ReleasePreconditions,
  ): Promise<ReleaseResult> {
    if (!this.storageCoordinator) throw new StorageReleaseUnsupportedError()
    if (
      !Number.isSafeInteger(preconditions.expectedLastEventIndex) ||
      preconditions.expectedLastEventIndex < -1 ||
      !Number.isSafeInteger(preconditions.inactiveSince) ||
      preconditions.inactiveSince < 0
    ) {
      throw new StoragePreconditionError('Storage release preconditions are invalid')
    }
    const durable = this.longTermStore
    if (!durable?.persistStorageReleaseRequest || !durable.clearStorageReleaseRequest) {
      throw new StorageReleaseUnsupportedError(
        'Long-term store cannot persist storage release requests',
      )
    }
    const request: StorageReleaseRequest = {
      taskId,
      requestedAt: Date.now(),
      expectedLastEventIndex: preconditions.expectedLastEventIndex,
      inactiveSince: preconditions.inactiveSince,
    }
    const persisted = await durable.persistStorageReleaseRequest(request)
    if (!persisted) throw new Error(`Task not found: ${taskId}`)
    try {
      const result = await this.storageCoordinator.releaseTaskStorage(taskId, preconditions)
      await durable.clearStorageReleaseRequest(request)
      return result
    } catch (error) {
      if (error instanceof StoragePreconditionError) {
        await durable.clearStorageReleaseRequest(request)
      }
      throw error
    }
  }

  async registerStorageWriter(
    registration: StorageWriterRegistration,
    ttlMs: number,
  ): Promise<void> {
    if (
      this.shortTermStore.supportsHotColdRelease !== true ||
      !this.shortTermStore.registerStorageWriter
    ) {
      throw new StorageReleaseUnsupportedError(
        'Short-term store cannot register storage writers',
      )
    }
    await this.shortTermStore.registerStorageWriter(registration, ttlMs)
  }

  async listStorageWriters(): Promise<StorageWriterRegistration[]> {
    if (
      this.shortTermStore.supportsHotColdRelease !== true ||
      !this.shortTermStore.listStorageWriters
    ) {
      throw new StorageReleaseUnsupportedError(
        'Short-term store cannot list storage writers',
      )
    }
    return this.shortTermStore.listStorageWriters()
  }

  supportsStorageRelease(): boolean {
    return this.storageCoordinator !== undefined
  }

  async recoverTaskStorage(taskId: string): Promise<ReleaseResult> {
    if (!this.storageCoordinator) throw new StorageReleaseUnsupportedError()
    return this.storageCoordinator.recoverTaskStorage(taskId)
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
    if (!this.longTermStore) {
      return this.shortTermStore.getEvents(taskId, opts)
    }
    if (this.longTermStore.supportsHotColdRelease !== true) {
      const fromShort = await this.shortTermStore.getEvents(taskId, opts)
      return fromShort.length > 0
        ? fromShort
        : this.longTermStore.getEvents(taskId, opts)
    }

    const overlayHot = await this.shouldOverlayHotHistory(taskId)
    const hotEvents = overlayHot
      ? await this.shortTermStore.getEvents(taskId)
      : []
    const getDurableSeriesState = this.longTermStore.getDurableSeriesState
    if (!getDurableSeriesState) throw new StorageReleaseUnsupportedError()
    const durableSeries = await getDurableSeriesState.call(
      this.longTermStore,
      taskId,
    )
    const durableEvents = await this.loadCanonicalDurableEvents(
      taskId,
      opts,
      hotEvents,
      durableSeries,
    )
    return applyCanonicalHistoryQuery(
      mergeCanonicalHistory(durableEvents, hotEvents, durableSeries),
      opts,
    )
  }

  subscribe(taskId: string, handler: (event: TaskEvent) => void): () => void {
    return this.broadcast.subscribe(taskId, handler)
  }

  async getSeriesLatest(taskId: string, seriesId: string): Promise<TaskEvent | null> {
    if (
      !this.longTermStore ||
      this.longTermStore.supportsHotColdRelease !== true
    ) {
      return this.shortTermStore.getSeriesLatest(taskId, seriesId)
    }
    const getDurableSeriesState = this.longTermStore.getDurableSeriesState
    if (!getDurableSeriesState) throw new StorageReleaseUnsupportedError()
    const durable = (await getDurableSeriesState.call(this.longTermStore, taskId))
      .find((state) => state.seriesId === seriesId)
    if (!durable) return this.shortTermStore.getSeriesLatest(taskId, seriesId)
    if (!(await this.shouldOverlayHotHistory(taskId))) return durable.event
    const hotEvents = await this.shortTermStore.getEvents(taskId)
    return resolveCanonicalSeriesLatest(durable, hotEvents)
  }

  private async shouldOverlayHotHistory(taskId: string): Promise<boolean> {
    if (this.longTermStore?.supportsHotColdRelease !== true) return true
    const getMetadata = this.longTermStore.getTaskStorageMetadata
    if (!getMetadata) throw new StorageReleaseUnsupportedError()
    const metadata = await getMetadata.call(this.longTermStore, taskId)
    if (metadata?.storageState === 'cold') return false
    return true
  }

  private async loadCanonicalDurableEvents(
    taskId: string,
    opts: EventQueryOptions | undefined,
    hotEvents: readonly TaskEvent[],
    durableSeries: readonly DurableSeriesState[],
  ): Promise<TaskEvent[]> {
    if (!this.longTermStore) return []
    const requestedLimit = opts?.limit
    if (requestedLimit === undefined) {
      return this.longTermStore.getEvents(
        taskId,
        canonicalDurableQuery(opts, hotEvents, durableSeries),
      )
    }
    if (requestedLimit <= 0) return []

    const pageSize = Math.min(requestedLimit, 1_000)
    const loaded: TaskEvent[] = []
    let query = canonicalDurableQuery(
      { ...opts, limit: pageSize },
      hotEvents,
      durableSeries,
    )
    let previousBoundary = -1
    for (;;) {
      const page = await this.longTermStore.getEvents(taskId, query)
      loaded.push(...page)
      const assembled = applyCanonicalHistoryQuery(
        mergeCanonicalHistory(loaded, hotEvents, durableSeries),
        opts,
      )
      if (page.length < pageSize) return loaded

      const boundary = page.at(-1)!.index
      if (boundary <= previousBoundary) {
        throw new StorageIntegrityError(
          'Durable history pagination did not advance',
        )
      }
      if (
        assembled.length >= requestedLimit &&
        assembled[requestedLimit - 1]!.index <= boundary
      ) {
        return loaded
      }
      previousBoundary = boundary
      query = { since: { index: boundary }, limit: pageSize }
    }
  }

  private async _emit(
    taskId: string,
    input: PublishEventInput,
    allowTerminal = false,
  ): Promise<TaskEvent> {
    // Serialize emit calls per task to prevent race conditions where
    // concurrent publishes store events in a different order than
    // their atomically-assigned indices.
    const prev = this._emitChains.get(taskId) ?? Promise.resolve()
    let release!: () => void
    const gate = new Promise<void>((r) => { release = r })
    this._emitChains.set(taskId, gate)

    await prev
    try {
      if (!allowTerminal) {
        const current = await this.getTask(taskId)
        if (!current) throw new Error(`Task not found: ${taskId}`)
        if (isTerminal(current.status)) {
          throw new Error(
            `Cannot publish to task in terminal status: ${current.status}`,
          )
        }
      }
      return await this._emitInner(taskId, input)
    } finally {
      release()
    }
  }

  private async _emitInner(taskId: string, input: PublishEventInput): Promise<TaskEvent> {
    if (this.storageCoordinator) {
      const raw: Omit<TaskEvent, 'index'> = {
        id: ulid(),
        taskId,
        timestamp: Date.now(),
        type: input.type,
        level: input.level,
        data: input.data,
        ...(input.seriesId !== undefined && { seriesId: input.seriesId }),
        ...(input.seriesMode !== undefined && { seriesMode: input.seriesMode }),
        ...(input.seriesAccField !== undefined && {
          seriesAccField: input.seriesAccField,
        }),
      }
      let initialStorageEpoch: number | null = null
      for (let attempt = 0; attempt < 3; attempt++) {
        const token = await this.storageCoordinator.ensureTaskHotForWrite(
          taskId,
          attempt === 0,
        )
        if (initialStorageEpoch === null) {
          initialStorageEpoch = token.storageEpoch
        } else if (token.storageEpoch !== initialStorageEpoch) {
          throw new StorageFenceConflictError(
            'Task storage epoch changed after the write mutation started',
          )
        }
        try {
          const result = await this.shortTermStore.commitEventFenced!(
            taskId,
            raw,
            token,
          )
          await this.finishCommittedEvent(result.event, result.accumulatedEvent)
          return result.event
        } catch (error) {
          if (!(error instanceof StorageFenceConflictError) || attempt === 2) {
            throw error
          }
        }
      }
      throw new StorageFenceConflictError()
    }

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

    await this.finishCommittedEvent(event, accumulatedEvent)

    return event
  }

  private async commitTaskEventsForMutation(
    task: Task,
    expectedRevision: string,
    expectedStatus: TaskStatus,
    inputs: PublishEventInput[],
    initialToken: HotWriteToken,
  ): Promise<TaskEvent[]> {
    const coordinator = this.storageCoordinator
    const commit = this.shortTermStore.commitTaskEventsFenced
    if (!coordinator || !commit) throw new StorageReleaseUnsupportedError()

    const previous = this._emitChains.get(task.id) ?? Promise.resolve()
    let release!: () => void
    const gate = new Promise<void>((resolve) => { release = resolve })
    this._emitChains.set(task.id, gate)
    await previous
    try {
      const events = inputs.map((input): Omit<TaskEvent, 'index'> => ({
        id: ulid(),
        taskId: task.id,
        timestamp: Date.now(),
        type: input.type,
        level: input.level,
        data: input.data,
      }))
      for (let attempt = 0; attempt < 3; attempt++) {
        const token = attempt === 0
          ? initialToken
          : await coordinator.ensureTaskHotForWrite(task.id, false)
        if (token.storageEpoch !== initialToken.storageEpoch) {
          throw new StorageFenceConflictError(
            'Task storage epoch changed after the write mutation started',
          )
        }
        try {
          const committed = await commit.call(
            this.shortTermStore,
            task,
            expectedRevision,
            events,
            token,
          )
          if (committed !== null) return committed
          const current = await this.getTask(task.id)
          throw new InvalidTransitionError(
            current?.status ?? expectedStatus,
            task.status,
          )
        } catch (error) {
          if (!(error instanceof StorageFenceConflictError) || attempt === 2) {
            throw error
          }
        }
      }
      throw new StorageFenceConflictError()
    } finally {
      release()
    }
  }

  private async finishCommittedEvent(
    event: TaskEvent,
    accumulatedEvent?: TaskEvent,
  ): Promise<void> {
    const broadcastEvent = accumulatedEvent
      ? { ...event, _accumulatedData: accumulatedEvent.data }
      : event
    await this.broadcast.publish(event.taskId, broadcastEvent)

    if (this.longTermStore) {
      const storeEvent = accumulatedEvent ?? event
      this.persistLongTermEvent(event, accumulatedEvent).catch((err) => {
        this.hooks?.onEventDropped?.(storeEvent, String(err))
      })
    }
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
