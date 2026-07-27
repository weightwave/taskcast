import type {
  Task,
  TaskEvent,
  TaskStatus,
  BroadcastProvider,
  ShortTermStore,
  EventQueryOptions,
  TaskFilter,
  Worker,
  WorkerFilter,
  WorkerAssignment,
  TaskArchiveImportOptions,
  TaskArchiveRestoreData,
  StorageLease,
  TaskMutationSnapshot,
  TaskWriteFence,
  ClosedWriteFence,
  HotWriteToken,
  SeriesResult,
  ArchiveSourcePage,
  RehydrateSnapshot,
  TaskStoragePresence,
  StorageWriterRegistration,
  StorageReleaseRequest,
  LongTermStore,
  ArchiveBatch,
  ArchiveBatchReceipt,
  ArchiveGeneration,
  DurableSeriesState,
  TaskStorageMetadata,
  TaskStorageMetadataCas,
  WorkerAuditEvent,
  TerminalProjection,
  TerminalProjectionResult,
  TtlClaim,
} from './types.js'
import { StorageFenceConflictError, StorageIntegrityError } from './types.js'
import { TaskConflictError } from './engine.js'
import {
  canonicalJson,
  computeArchiveBatchDigest,
  computeArchiveSourceDigest,
  computeArchiveSourcePageDigest,
  computeSeriesStateDigest,
} from './storage-digest.js'

export class MemoryBroadcastProvider implements BroadcastProvider {
  private listeners = new Map<string, Set<(event: TaskEvent) => void>>()

  async publish(channel: string, event: TaskEvent): Promise<void> {
    const handlers = this.listeners.get(channel)
    if (!handlers) return
    for (const handler of handlers) {
      handler(event)
    }
  }

  subscribe(channel: string, handler: (event: TaskEvent) => void): () => void {
    if (!this.listeners.has(channel)) {
      this.listeners.set(channel, new Set())
    }
    this.listeners.get(channel)!.add(handler)
    return () => {
      this.listeners.get(channel)?.delete(handler)
    }
  }
}

export class MemoryShortTermStore implements ShortTermStore {
  readonly supportsHotColdRelease = true
  private tasks = new Map<string, Task>()
  private taskRevisions = new Map<string, number>()
  private events = new Map<string, TaskEvent[]>()
  private seriesLatest = new Map<string, TaskEvent>()
  private indexCounters = new Map<string, number>()
  private workers = new Map<string, Worker>()
  private assignments = new Map<string, WorkerAssignment>()
  private storageLocks = new Map<
    string,
    { lockToken: string; generation: string; storageEpoch: number; expiresAt: number }
  >()
  private writeFences = new Map<string, TaskWriteFence>()
  private storageWriters = new Map<string, StorageWriterRegistration>()

  async saveTask(task: Task): Promise<void> {
    this.tasks.set(task.id, { ...task })
    this.bumpTaskRevision(task.id)
    if (!this.writeFences.has(task.id)) {
      this.writeFences.set(task.id, {
        taskId: task.id,
        acceptingWrites: true,
        storageEpoch: 1,
        activeReleaseGeneration: null,
      })
    }
  }

  async getTask(taskId: string): Promise<Task | null> {
    return this.tasks.get(taskId) ?? null
  }

  async getTaskMutationSnapshot(taskId: string): Promise<TaskMutationSnapshot | null> {
    const task = this.tasks.get(taskId)
    if (!task) return null
    return {
      task: structuredClone(task),
      revision: String(this.taskRevisions.get(taskId) ?? 0),
    }
  }

  async nextIndex(taskId: string): Promise<number> {
    const current = this.indexCounters.get(taskId) ?? -1
    const next = current + 1
    this.indexCounters.set(taskId, next)
    return next
  }

  async appendEvent(taskId: string, event: TaskEvent): Promise<void> {
    this.appendEventSync(taskId, event)
  }

  async validateTaskArchiveRestore(
    data: TaskArchiveRestoreData,
    options?: TaskArchiveImportOptions,
  ): Promise<void> {
    const taskId = data.task.id
    const existing = this.tasks.get(taskId)
    if (existing && options?.overwrite !== true) {
      throw new TaskConflictError(taskId)
    }
  }

  async restoreTaskArchive(
    data: TaskArchiveRestoreData,
    options?: TaskArchiveImportOptions,
  ): Promise<{ overwritten: boolean }> {
    const taskId = data.task.id
    const existing = this.tasks.get(taskId)
    await this.validateTaskArchiveRestore(data, options)

    for (const key of Array.from(this.seriesLatest.keys())) {
      if (key.startsWith(`${taskId}:`)) {
        this.seriesLatest.delete(key)
      }
    }

    this.tasks.set(taskId, { ...data.task })
    this.bumpTaskRevision(taskId)
    this.events.set(taskId, data.events.map((event) => ({ ...event })))
    this.indexCounters.set(taskId, data.nextIndex - 1)

    for (const entry of data.seriesLatest) {
      this.seriesLatest.set(`${entry.taskId}:${entry.seriesId}`, { ...entry.event })
    }

    return { overwritten: existing !== undefined }
  }

  async acquireStorageLock(
    taskId: string,
    lockToken: string,
    generation: string,
    ttlMs: number,
  ): Promise<StorageLease | null> {
    const now = Date.now()
    const current = this.storageLocks.get(taskId)
    if (current && current.expiresAt > now) {
      if (current.lockToken !== lockToken || current.generation !== generation) return null
      current.expiresAt = now + ttlMs
      return { taskId, lockToken, generation, storageEpoch: current.storageEpoch }
    }

    const storageEpoch = this.writeFences.get(taskId)?.storageEpoch ?? 1
    this.storageLocks.set(taskId, {
      lockToken,
      generation,
      storageEpoch,
      expiresAt: now + ttlMs,
    })
    return { taskId, lockToken, generation, storageEpoch }
  }

  async renewStorageLock(lease: StorageLease, ttlMs: number): Promise<boolean> {
    const current = this.getOwnedStorageLock(lease)
    if (!current) return false
    current.expiresAt = Date.now() + ttlMs
    return true
  }

  async releaseStorageLock(lease: StorageLease): Promise<boolean> {
    if (!this.getOwnedStorageLock(lease)) return false
    this.storageLocks.delete(lease.taskId)
    return true
  }

  async getWriteFence(taskId: string): Promise<TaskWriteFence | null> {
    const fence = this.writeFences.get(taskId)
    return fence ? { ...fence } : null
  }

  async closeWriteFence(lease: StorageLease, expectedEpoch: number): Promise<ClosedWriteFence> {
    this.assertOwnedStorageLock(lease)
    const fence = this.writeFences.get(lease.taskId)
    if (!fence || fence.storageEpoch !== expectedEpoch) {
      throw new StorageFenceConflictError()
    }

    const highWatermark = Math.max(
      this.indexCounters.get(lease.taskId) ?? -1,
      ...(this.events.get(lease.taskId) ?? []).map((event) => event.index),
    )
    if (!fence.acceptingWrites) {
      const adopted: ClosedWriteFence = {
        ...fence,
        acceptingWrites: false,
        activeReleaseGeneration: lease.generation,
        highWatermark,
      }
      this.writeFences.set(lease.taskId, adopted)
      return { ...adopted }
    }
    const closed: ClosedWriteFence = {
      ...fence,
      acceptingWrites: false,
      activeReleaseGeneration: lease.generation,
      highWatermark,
    }
    this.writeFences.set(lease.taskId, closed)
    return { ...closed }
  }

  async reopenWriteFence(lease: StorageLease, expectedEpoch: number): Promise<HotWriteToken> {
    this.assertOwnedStorageLock(lease)
    const fence = this.writeFences.get(lease.taskId)
    if (
      !fence ||
      fence.acceptingWrites ||
      fence.storageEpoch !== expectedEpoch ||
      fence.activeReleaseGeneration !== lease.generation
    ) {
      throw new StorageFenceConflictError()
    }

    const storageEpoch = expectedEpoch + 1
    this.writeFences.set(lease.taskId, {
      taskId: lease.taskId,
      acceptingWrites: true,
      storageEpoch,
      activeReleaseGeneration: null,
    })
    return { taskId: lease.taskId, storageEpoch }
  }

  async commitEventFenced(
    taskId: string,
    event: Omit<TaskEvent, 'index'>,
    token: HotWriteToken,
  ): Promise<SeriesResult> {
    const fence = this.writeFences.get(taskId)
    if (
      token.taskId !== taskId ||
      !fence?.acceptingWrites ||
      fence.storageEpoch !== token.storageEpoch
    ) {
      throw new StorageFenceConflictError()
    }

    const index = (this.indexCounters.get(taskId) ?? -1) + 1
    const committed = { ...event, taskId, index } as TaskEvent
    this.indexCounters.set(taskId, index)

    if (committed.seriesId && committed.seriesMode === 'latest') {
      this.replaceLastSeriesEventSync(taskId, committed.seriesId, committed)
      return { event: committed, stored: true }
    }

    if (committed.seriesId && committed.seriesMode === 'accumulate') {
      this.appendEventSync(taskId, committed)
      const accumulatedEvent = this.accumulateSeriesSync(
        taskId,
        committed.seriesId,
        committed,
        committed.seriesAccField ?? 'delta',
      )
      return { event: committed, accumulatedEvent, stored: true }
    }

    this.appendEventSync(taskId, committed)
    return { event: committed, stored: true }
  }

  async saveTaskFenced(task: Task, token: HotWriteToken): Promise<void> {
    const fence = this.writeFences.get(task.id)
    if (
      token.taskId !== task.id ||
      !fence?.acceptingWrites ||
      fence.storageEpoch !== token.storageEpoch
    ) {
      throw new StorageFenceConflictError()
    }
    this.tasks.set(task.id, { ...task })
    this.bumpTaskRevision(task.id)
  }

  async commitTaskEventsFenced(
    task: Task,
    expectedRevision: string,
    events: Omit<TaskEvent, 'index'>[],
    token: HotWriteToken,
  ): Promise<TaskEvent[] | null> {
    const fence = this.writeFences.get(task.id)
    if (
      token.taskId !== task.id ||
      !fence?.acceptingWrites ||
      fence.storageEpoch !== token.storageEpoch ||
      events.some(
        (event) =>
          event.taskId !== task.id ||
          event.seriesId !== undefined ||
          event.seriesMode !== undefined,
      )
    ) {
      throw new StorageFenceConflictError()
    }
    if (String(this.taskRevisions.get(task.id) ?? 0) !== expectedRevision) return null
    const committed = events.map((event) => {
      const index = (this.indexCounters.get(task.id) ?? -1) + 1
      this.indexCounters.set(task.id, index)
      return { ...event, taskId: task.id, index } as TaskEvent
    })
    this.tasks.set(task.id, structuredClone(task))
    this.bumpTaskRevision(task.id)
    for (const event of committed) this.appendEventSync(task.id, event)
    return structuredClone(committed)
  }

  async readArchiveSourcePage(
    taskId: string,
    watermark: number,
    cursor: string | null,
    limit: number,
  ): Promise<ArchiveSourcePage> {
    const offset = cursor === null ? 0 : Number.parseInt(cursor, 10)
    const source = (this.events.get(taskId) ?? [])
      .filter((event) => event.index <= watermark)
      .sort((left, right) => left.index - right.index)
    const events = source.slice(offset, offset + Math.max(1, limit)).map((event) => ({ ...event }))
    const nextOffset = offset + events.length
    const done = nextOffset >= source.length
    return {
      taskId,
      watermark,
      cursor,
      nextCursor: done ? null : String(nextOffset),
      events,
      done,
    }
  }

  async deleteTaskStorageFenced(lease: StorageLease, expectedEpoch: number): Promise<void> {
    this.assertOwnedStorageLock(lease)
    const fence = this.writeFences.get(lease.taskId)
    if (
      !fence ||
      fence.acceptingWrites ||
      fence.storageEpoch !== expectedEpoch ||
      fence.activeReleaseGeneration !== lease.generation
    ) {
      throw new StorageFenceConflictError()
    }

    this.tasks.delete(lease.taskId)
    this.taskRevisions.delete(lease.taskId)
    this.events.delete(lease.taskId)
    this.indexCounters.delete(lease.taskId)
    this.writeFences.delete(lease.taskId)
    const prefix = `${lease.taskId}:`
    for (const key of this.seriesLatest.keys()) {
      if (key.startsWith(prefix)) this.seriesLatest.delete(key)
    }
  }

  async restoreHotTaskFenced(
    snapshot: RehydrateSnapshot,
    lease: StorageLease,
    nextEpoch: number,
  ): Promise<HotWriteToken> {
    this.assertOwnedStorageLock(lease)
    if (snapshot.task.id !== lease.taskId || nextEpoch <= snapshot.storageEpoch) {
      throw new StorageFenceConflictError()
    }

    const taskId = snapshot.task.id
    this.tasks.set(taskId, { ...snapshot.task })
    this.taskRevisions.set(taskId, 1)
    this.events.set(
      taskId,
      snapshot.replayEvents.map((event) => ({ ...event })),
    )
    this.indexCounters.set(taskId, snapshot.maxEventIndex)
    const prefix = `${taskId}:`
    for (const key of this.seriesLatest.keys()) {
      if (key.startsWith(prefix)) this.seriesLatest.delete(key)
    }
    for (const entry of snapshot.seriesLatest) {
      this.seriesLatest.set(`${taskId}:${entry.seriesId}`, { ...entry.event })
    }
    this.writeFences.set(taskId, {
      taskId,
      acceptingWrites: true,
      storageEpoch: nextEpoch,
      activeReleaseGeneration: null,
    })
    return { taskId, storageEpoch: nextEpoch }
  }

  async projectTerminalFenced(
    projection: TerminalProjection,
    lease: StorageLease,
    expectedEpoch: number,
    nextEpoch: number,
  ): Promise<TerminalProjectionResult> {
    this.assertOwnedStorageLock(lease)
    const taskId = projection.task.id
    const fence = this.writeFences.get(taskId)
    if (
      taskId !== lease.taskId ||
      projection.event.taskId !== taskId ||
      (projection.assignment !== null &&
        projection.assignment.taskId !== taskId) ||
      !fence ||
      fence.acceptingWrites ||
      fence.storageEpoch !== expectedEpoch ||
      fence.activeReleaseGeneration !== lease.generation ||
      nextEpoch !== expectedEpoch + 1
    ) {
      throw new StorageFenceConflictError()
    }

    const taskEvents = this.events.get(taskId) ?? []
    const existing = taskEvents.find(
      (event) => event.index === projection.event.index,
    )
    let projected = false
    if (existing) {
      if (canonicalJson(existing) !== canonicalJson(projection.event)) {
        throw new StorageIntegrityError(
          'Terminal projection conflicts with the hot event at its index',
        )
      }
    } else {
      const nextIndex = (this.indexCounters.get(taskId) ?? -1) + 1
      if (nextIndex !== projection.event.index) {
        throw new StorageIntegrityError(
          'Terminal projection event index is not contiguous in hot storage',
        )
      }
      this.indexCounters.set(taskId, projection.event.index)
      this.appendEventSync(taskId, projection.event)
      projected = true
    }

    this.tasks.set(taskId, structuredClone(projection.task))
    this.bumpTaskRevision(taskId)
    const assignment = projection.assignment
    if (assignment) {
      const current = this.assignments.get(taskId)
      if (current && canonicalJson(current) !== canonicalJson(assignment)) {
        throw new StorageIntegrityError(
          'Terminal projection conflicts with the hot assignment',
        )
      }
      if (current) {
        this.assignments.delete(taskId)
        const worker = this.workers.get(assignment.workerId)
        if (worker) {
          worker.usedSlots = Math.max(0, worker.usedSlots - assignment.cost)
          if (worker.status !== 'offline' && worker.status !== 'draining') {
            worker.status = worker.usedSlots >= worker.capacity ? 'busy' : 'idle'
          }
        }
      }
    }
    this.writeFences.set(taskId, {
      taskId,
      acceptingWrites: true,
      storageEpoch: nextEpoch,
      activeReleaseGeneration: null,
    })
    return {
      token: { taskId, storageEpoch: nextEpoch },
      projected,
    }
  }

  async getTaskStoragePresence(taskId: string): Promise<TaskStoragePresence> {
    const prefix = `${taskId}:`
    let seriesStateCount = 0
    for (const key of this.seriesLatest.keys()) {
      if (key.startsWith(prefix)) seriesStateCount += 1
    }
    return {
      task: this.tasks.has(taskId),
      eventCount: this.events.get(taskId)?.length ?? 0,
      nextIndex: this.indexCounters.has(taskId),
      seriesStateCount,
      writeFence: this.writeFences.has(taskId),
    }
  }

  async registerStorageWriter(
    registration: StorageWriterRegistration,
    ttlMs: number,
  ): Promise<void> {
    this.storageWriters.set(registration.instanceId, {
      ...registration,
      expiresAt: Date.now() + ttlMs,
    })
  }

  async listStorageWriters(): Promise<StorageWriterRegistration[]> {
    const now = Date.now()
    for (const [instanceId, registration] of this.storageWriters) {
      if (registration.expiresAt <= now) this.storageWriters.delete(instanceId)
    }
    return Array.from(this.storageWriters.values()).map((registration) => ({ ...registration }))
  }

  async getEvents(taskId: string, opts?: EventQueryOptions): Promise<TaskEvent[]> {
    const all = this.events.get(taskId) ?? []
    let result = all

    if (opts?.since?.id) {
      const idx = result.findIndex((e) => e.id === opts.since!.id)
      result = idx >= 0 ? result.slice(idx + 1) : result
    } else if (opts?.since?.index !== undefined) {
      result = result.filter((e) => e.index > opts.since!.index!)
    } else if (opts?.since?.timestamp !== undefined) {
      result = result.filter((e) => e.timestamp > opts.since!.timestamp!)
    }

    if (opts?.limit) result = result.slice(0, opts.limit)
    return result
  }

  async setTTL(_taskId: string, _ttlSeconds: number): Promise<void> {
    // no-op in memory adapter
  }

  async getSeriesLatest(taskId: string, seriesId: string): Promise<TaskEvent | null> {
    return this.seriesLatest.get(`${taskId}:${seriesId}`) ?? null
  }

  async setSeriesLatest(taskId: string, seriesId: string, event: TaskEvent): Promise<void> {
    this.seriesLatest.set(`${taskId}:${seriesId}`, { ...event })
  }

  async accumulateSeries(taskId: string, seriesId: string, event: TaskEvent, field: string): Promise<TaskEvent> {
    return this.accumulateSeriesSync(taskId, seriesId, event, field)
  }

  private accumulateSeriesSync(
    taskId: string,
    seriesId: string,
    event: TaskEvent,
    field: string,
  ): TaskEvent {
    const key = `${taskId}:${seriesId}`
    const prev = this.seriesLatest.get(key)

    let accumulated = event
    if (prev !== null && prev !== undefined) {
      const prevData = (typeof prev.data === 'object' && prev.data !== null)
        ? prev.data as Record<string, unknown> : {}
      const newData = (typeof event.data === 'object' && event.data !== null)
        ? event.data as Record<string, unknown> : {}
      if (typeof prevData[field] === 'string' && typeof newData[field] === 'string') {
        accumulated = {
          ...event,
          data: { ...newData, [field]: prevData[field] + newData[field] },
        }
      }
    }

    this.seriesLatest.set(key, { ...accumulated })
    return accumulated
  }

  async replaceLastSeriesEvent(taskId: string, seriesId: string, event: TaskEvent): Promise<void> {
    this.replaceLastSeriesEventSync(taskId, seriesId, event)
  }

  private replaceLastSeriesEventSync(taskId: string, seriesId: string, event: TaskEvent): void {
    const key = `${taskId}:${seriesId}`
    const prev = this.seriesLatest.get(key)
    if (prev) {
      const taskEvents = this.events.get(taskId)
      if (taskEvents) {
        // Find the last index manually (findLastIndex requires ES2023+)
        let idx = -1
        for (let i = taskEvents.length - 1; i >= 0; i--) {
          if (taskEvents[i]?.id === prev.id) {
            idx = i
            break
          }
        }
        if (idx >= 0) taskEvents[idx] = { ...event }
      }
    } else {
      this.appendEventSync(taskId, event)
    }
    this.seriesLatest.set(key, { ...event })
  }

  private appendEventSync(taskId: string, event: TaskEvent): void {
    if (!this.events.has(taskId)) this.events.set(taskId, [])
    this.events.get(taskId)!.push({ ...event })
  }

  private getOwnedStorageLock(
    lease: StorageLease,
  ): { lockToken: string; generation: string; storageEpoch: number; expiresAt: number } | null {
    const current = this.storageLocks.get(lease.taskId)
    if (!current || current.expiresAt <= Date.now()) {
      if (current) this.storageLocks.delete(lease.taskId)
      return null
    }
    if (
      current.lockToken !== lease.lockToken ||
      current.generation !== lease.generation ||
      current.storageEpoch !== lease.storageEpoch
    ) {
      return null
    }
    return current
  }

  private assertOwnedStorageLock(lease: StorageLease): void {
    if (!this.getOwnedStorageLock(lease)) {
      throw new StorageFenceConflictError('Storage lease is stale')
    }
  }

  // Task query
  async listTasks(filter: TaskFilter): Promise<Task[]> {
    let tasks = Array.from(this.tasks.values())

    if (filter.status?.length) {
      tasks = tasks.filter((t) => filter.status!.includes(t.status))
    }
    if (filter.types?.length) {
      tasks = tasks.filter((t) => t.type !== undefined && filter.types!.includes(t.type))
    }
    if (filter.tags) {
      const { all, any, none } = filter.tags
      tasks = tasks.filter((t) => {
        const taskTags = t.tags ?? []
        if (all && !all.every((tag) => taskTags.includes(tag))) return false
        if (any && !any.some((tag) => taskTags.includes(tag))) return false
        if (none && none.some((tag) => taskTags.includes(tag))) return false
        return true
      })
    }
    if (filter.assignMode?.length) {
      tasks = tasks.filter((t) => t.assignMode !== undefined && filter.assignMode!.includes(t.assignMode))
    }
    if (filter.excludeTaskIds?.length) {
      const excluded = new Set(filter.excludeTaskIds)
      tasks = tasks.filter((t) => !excluded.has(t.id))
    }
    if (filter.limit !== undefined) {
      tasks = tasks.slice(0, filter.limit)
    }

    return tasks
  }

  // Worker state
  async saveWorker(worker: Worker): Promise<void> {
    this.workers.set(worker.id, { ...worker })
  }

  async getWorker(workerId: string): Promise<Worker | null> {
    return this.workers.get(workerId) ?? null
  }

  async listWorkers(filter?: WorkerFilter): Promise<Worker[]> {
    let workers = Array.from(this.workers.values())

    if (filter?.status?.length) {
      workers = workers.filter((w) => filter.status!.includes(w.status))
    }
    if (filter?.connectionMode?.length) {
      workers = workers.filter((w) => filter.connectionMode!.includes(w.connectionMode))
    }

    return workers
  }

  async deleteWorker(workerId: string): Promise<void> {
    this.workers.delete(workerId)
  }

  // Atomic claim — single-threaded JS makes this safe without locking.
  // The Redis adapter uses a Lua script for the same guarantee across processes.
  async claimTask(taskId: string, workerId: string, cost: number): Promise<boolean> {
    const worker = this.workers.get(workerId)
    if (!worker || worker.usedSlots + cost > worker.capacity) return false

    const task = this.tasks.get(taskId)
    if (!task || (task.status !== 'pending' && task.status !== 'assigned')) return false

    task.status = 'assigned'
    task.assignedWorker = workerId
    task.cost = cost
    task.updatedAt = Date.now()
    this.bumpTaskRevision(taskId)

    worker.usedSlots += cost
    return true
  }

  private bumpTaskRevision(taskId: string): void {
    this.taskRevisions.set(taskId, (this.taskRevisions.get(taskId) ?? 0) + 1)
  }

  // Worker assignments
  async addAssignment(assignment: WorkerAssignment): Promise<void> {
    this.assignments.set(assignment.taskId, { ...assignment })
  }

  async removeAssignment(taskId: string): Promise<void> {
    this.assignments.delete(taskId)
  }

  async getWorkerAssignments(workerId: string): Promise<WorkerAssignment[]> {
    return Array.from(this.assignments.values()).filter((a) => a.workerId === workerId)
  }

  async getTaskAssignment(taskId: string): Promise<WorkerAssignment | null> {
    return this.assignments.get(taskId) ?? null
  }

  // TTL management — no-op in memory adapter (setTTL is also a no-op)
  async clearTTL(_taskId: string): Promise<void> {
    // no-op in memory adapter
  }

  // Task query by status
  async listByStatus(statuses: TaskStatus[]): Promise<Task[]> {
    return Array.from(this.tasks.values()).filter((t) => statuses.includes(t.status))
  }
}

export class MemoryLongTermStore implements LongTermStore {
  readonly supportsHotColdRelease = true
  readonly supportsDurableTtl = true
  private tasks = new Map<string, Task>()
  private events = new Map<string, TaskEvent[]>()
  private metadata = new Map<string, TaskStorageMetadata>()
  private generations = new Map<string, ArchiveGeneration>()
  private batches = new Map<string, Map<number, ArchiveBatch>>()
  private series = new Map<string, DurableSeriesState[]>()
  private workerEvents = new Map<string, WorkerAuditEvent[]>()
  private releaseRequests = new Map<string, StorageReleaseRequest>()
  private ttlClaims = new Map<string, TtlClaim>()
  private durableAssignments = new Map<string, WorkerAssignment>()
  private terminalProjections = new Map<
    string,
    { projection: TerminalProjection; projectedAt: number | null }
  >()
  private creationClaims = new Map<
    string,
    { token: string; claimedAt: number; expiresAt: number | null; completedAt: number | null }
  >()

  async saveTask(task: Task): Promise<void> {
    const existing = this.tasks.get(task.id)
    const metadata = this.metadata.get(task.id)
    if (
      existing &&
      isMemoryTerminal(existing.status) &&
      existing.status !== task.status
    ) {
      throw new StorageFenceConflictError(
        `Durable terminal task cannot be overwritten: ${task.id}`,
      )
    }
    this.tasks.set(task.id, structuredClone(task))
    if (!metadata) {
      this.metadata.set(task.id, {
        taskId: task.id,
        storageState: 'hot',
        storageEpoch: 1,
        activeReleaseGeneration: null,
        archiveWatermark: -1,
        lastEventAt: null,
        coldAt: null,
        executionDeadlineAt:
          hasMemoryExecutionDeadline(task)
            ? Date.now() + task.ttl! * 1_000
            : null,
        taskVersion: 0,
      })
      return
    }
    metadata.executionDeadlineAt = hasMemoryExecutionDeadline(task)
      ? (
          metadata.executionDeadlineAt === null ||
          existing?.status === 'paused' ||
          existing?.ttl !== task.ttl
            ? Date.now() + task.ttl! * 1_000
            : metadata.executionDeadlineAt
        )
      : null
    metadata.taskVersion += 1
    this.ttlClaims.delete(task.id)
  }

  async createTaskIfAbsent(task: Task): Promise<boolean> {
    if (this.tasks.has(task.id)) return false
    await this.saveTask(task)
    return true
  }

  async claimTaskCreation(
    task: Task,
    creationToken: string,
    claimTtlMs: number,
  ): Promise<boolean> {
    if (claimTtlMs <= 0) throw new StorageIntegrityError('Creation claim TTL must be positive')
    const now = Date.now()
    const existingClaim = this.creationClaims.get(task.id)
    if (this.tasks.has(task.id)) {
      if (
        !existingClaim ||
        existingClaim.completedAt !== null ||
        (existingClaim.expiresAt !== null && existingClaim.expiresAt > now) ||
        !this.isPristineCreationClaim(task.id)
      ) {
        return false
      }
    }
    this.tasks.set(task.id, structuredClone(task))
    this.metadata.set(task.id, {
      taskId: task.id,
      storageState: 'hot',
      storageEpoch: 1,
      activeReleaseGeneration: null,
      archiveWatermark: -1,
      lastEventAt: null,
      coldAt: null,
      executionDeadlineAt:
        hasMemoryExecutionDeadline(task)
          ? now + task.ttl! * 1_000
          : null,
      taskVersion: 0,
    })
    this.creationClaims.set(task.id, {
      token: creationToken,
      claimedAt: now,
      expiresAt: now + claimTtlMs,
      completedAt: null,
    })
    return true
  }

  async completeTaskCreation(taskId: string, creationToken: string): Promise<boolean> {
    const claim = this.creationClaims.get(taskId)
    if (claim?.token !== creationToken) return false
    claim.completedAt ??= Date.now()
    claim.expiresAt = null
    return true
  }

  async abortTaskCreation(taskId: string, creationToken: string): Promise<boolean> {
    const claim = this.creationClaims.get(taskId)
    if (
      claim?.token !== creationToken ||
      claim.completedAt !== null ||
      !this.isPristineCreationClaim(taskId)
    ) return false
    this.creationClaims.delete(taskId)
    this.tasks.delete(taskId)
    this.metadata.delete(taskId)
    return true
  }

  private isPristineCreationClaim(taskId: string): boolean {
    const metadata = this.metadata.get(taskId)
    const task = this.tasks.get(taskId)
    return Boolean(
      task &&
      task.status === 'pending' &&
      task.updatedAt === task.createdAt &&
      task.result === undefined &&
      task.error === undefined &&
      task.completedAt === undefined &&
      metadata &&
      metadata.storageState === 'hot' &&
      metadata.storageEpoch === 1 &&
      metadata.activeReleaseGeneration === null &&
      metadata.archiveWatermark === -1 &&
      metadata.lastEventAt === null &&
      metadata.coldAt === null &&
      metadata.taskVersion === 0 &&
      (this.events.get(taskId)?.length ?? 0) === 0
    )
  }

  async getTask(taskId: string): Promise<Task | null> {
    return structuredClone(this.tasks.get(taskId) ?? null)
  }

  async saveEvent(event: TaskEvent): Promise<void> {
    this.upsertEvent(event)
    const metadata = this.metadata.get(event.taskId)
    if (metadata) metadata.lastEventAt = event.timestamp
  }

  async replaceLastSeriesEvent(
    taskId: string,
    seriesId: string,
    event: TaskEvent,
  ): Promise<void> {
    const states = this.series.get(taskId) ?? []
    const existing = states.find((state) => state.seriesId === seriesId)
    const metadata = this.metadata.get(taskId)
    if (!metadata) {
      throw new StorageIntegrityError(`Series task does not exist: ${taskId}`)
    }
    if (
      metadata.archiveWatermark >= event.index ||
      (existing?.throughIndex ?? -1) >= event.index
    ) {
      if (metadata.archiveWatermark >= event.index && !existing) {
        throw new StorageIntegrityError(
          `Archived latest series state is missing for ${taskId}:${seriesId}`,
        )
      }
      return
    }
    if (
      existing &&
      (existing.mode !== 'latest' ||
        existing.event.seriesAccField !== event.seriesAccField)
    ) {
      throw new StorageIntegrityError(
        `Durable series semantics conflict for ${taskId}:${seriesId}`,
      )
    }
    if (existing) {
      const events = this.events.get(taskId) ?? []
      this.events.set(
        taskId,
        events.filter((candidate) => candidate.id !== existing.event.id),
      )
    }
    this.upsertEvent(event)
    this.upsertSeriesState({
      taskId,
      seriesId,
      mode: 'latest',
      event: structuredClone(event),
      throughIndex: event.index,
    })
  }

  async accumulateSeries(
    taskId: string,
    seriesId: string,
    event: TaskEvent,
    field: string,
  ): Promise<TaskEvent> {
    const previous = (this.series.get(taskId) ?? []).find(
      (state) => state.seriesId === seriesId,
    )
    const metadata = this.metadata.get(taskId)
    if (!metadata) {
      throw new StorageIntegrityError(`Series task does not exist: ${taskId}`)
    }
    if (
      metadata.archiveWatermark >= event.index ||
      (previous?.throughIndex ?? -1) >= event.index
    ) {
      if (!previous) {
        throw new StorageIntegrityError(
          `Archived accumulate series state is missing for ${taskId}:${seriesId}`,
        )
      }
      return structuredClone(previous.event)
    }
    if (
      previous &&
      (previous.mode !== 'accumulate' ||
        (previous.event.seriesAccField ?? 'delta') !== field ||
        (event.seriesAccField ?? 'delta') !== field)
    ) {
      throw new StorageIntegrityError(
        `Durable series semantics conflict for ${taskId}:${seriesId}`,
      )
    }
    let accumulated = structuredClone(event)
    const previousData = previous?.event.data
    const previousField =
      previousData && typeof previousData === 'object' && !Array.isArray(previousData)
        ? (previousData as Record<string, unknown>)[field]
        : undefined
    const nextField =
      event.data && typeof event.data === 'object' && !Array.isArray(event.data)
        ? (event.data as Record<string, unknown>)[field]
        : undefined
    if (
      previousData &&
      typeof previousData === 'object' &&
      !Array.isArray(previousData) &&
      event.data &&
      typeof event.data === 'object' &&
      !Array.isArray(event.data) &&
      typeof previousField === 'string' &&
      typeof nextField === 'string'
    ) {
      accumulated = {
        ...structuredClone(event),
        data: {
          ...(event.data as Record<string, unknown>),
          [field]: previousField + nextField,
        },
      }
    }
    if (previous) {
      const events = this.events.get(taskId) ?? []
      this.events.set(
        taskId,
        events.filter((candidate) => candidate.id !== previous.event.id),
      )
    }
    this.upsertEvent(accumulated)
    this.upsertSeriesState({
      taskId,
      seriesId,
      mode: 'accumulate',
      event: structuredClone(accumulated),
      throughIndex: event.index,
    })
    return accumulated
  }

  async getEvents(
    taskId: string,
    opts?: EventQueryOptions,
  ): Promise<TaskEvent[]> {
    let result = structuredClone(this.events.get(taskId) ?? [])
    if (opts?.since?.id) {
      const index = result.findIndex((event) => event.id === opts.since!.id)
      if (index >= 0) result = result.slice(index + 1)
    } else if (opts?.since?.index !== undefined) {
      result = result.filter((event) => event.index > opts.since!.index!)
    } else if (opts?.since?.timestamp !== undefined) {
      result = result.filter(
        (event) => event.timestamp > opts.since!.timestamp!,
      )
    }
    if (opts?.limit !== undefined) result = result.slice(0, opts.limit)
    return result
  }

  async getTaskStorageMetadata(
    taskId: string,
  ): Promise<TaskStorageMetadata | null> {
    return structuredClone(this.metadata.get(taskId) ?? null)
  }

  async persistStorageReleaseRequest(request: StorageReleaseRequest): Promise<boolean> {
    if (!this.tasks.has(request.taskId)) return false
    this.releaseRequests.set(request.taskId, structuredClone(request))
    return true
  }

  async clearStorageReleaseRequest(request: StorageReleaseRequest): Promise<boolean> {
    const current = this.releaseRequests.get(request.taskId)
    if (
      !current ||
      current.requestedAt !== request.requestedAt ||
      current.expectedLastEventIndex !== request.expectedLastEventIndex ||
      current.inactiveSince !== request.inactiveSince
    ) {
      return false
    }
    this.releaseRequests.delete(request.taskId)
    return true
  }

  async listStorageReleaseRequests(limit: number): Promise<StorageReleaseRequest[]> {
    return Array.from(this.releaseRequests.values())
      .sort((left, right) => left.requestedAt - right.requestedAt)
      .slice(0, limit)
      .map((request) => structuredClone(request))
  }

  async compareAndSetTaskStorageMetadata(
    update: TaskStorageMetadataCas,
  ): Promise<boolean> {
    const current = this.metadata.get(update.taskId)
    if (
      !current ||
      current.storageState !== update.expectedStorageState ||
      current.storageEpoch !== update.expectedStorageEpoch ||
      current.activeReleaseGeneration !== update.expectedReleaseGeneration
    ) {
      return false
    }
    this.metadata.set(update.taskId, structuredClone(update.next))
    return true
  }

  async beginArchive(generation: ArchiveGeneration): Promise<ArchiveGeneration> {
    const metadata = this.metadata.get(generation.taskId)
    if (
      !metadata ||
      metadata.storageState !== 'releasing' ||
      metadata.storageEpoch !== generation.storageEpoch ||
      metadata.activeReleaseGeneration !== generation.generation ||
      metadata.archiveWatermark !== generation.manifest.priorWatermark
    ) {
      throw new StorageIntegrityError(
        'Archive generation lost its durable release fence',
      )
    }
    const key = this.archiveKey(generation.taskId, generation.generation)
    const existing = this.generations.get(key)
    if (existing) {
      if (
        existing.storageEpoch !== generation.storageEpoch ||
        existing.targetWatermark !== generation.targetWatermark ||
        canonicalJson(existing.manifest) !== canonicalJson(generation.manifest)
      ) {
        throw new StorageIntegrityError('Archive generation replay conflicts')
      }
      return structuredClone(existing)
    }
    this.generations.set(key, structuredClone(generation))
    return structuredClone(generation)
  }

  async archiveBatch(
    taskId: string,
    generation: string,
    batch: ArchiveBatch,
  ): Promise<ArchiveBatchReceipt> {
    const expectedDigest = await computeArchiveBatchDigest(
      batch.receipt.previousBatchDigest,
      batch.events,
      batch.seriesLatest,
    )
    if (expectedDigest !== batch.receipt.batchDigest) {
      throw new StorageIntegrityError('Archive batch digest mismatch')
    }
    const key = this.archiveKey(taskId, generation)
    const archive = this.generations.get(key)
    if (!archive || archive.status !== 'open') {
      throw new StorageIntegrityError('Archive generation is not open')
    }
    const metadata = this.metadata.get(taskId)
    if (
      !metadata ||
      metadata.storageState !== 'releasing' ||
      metadata.storageEpoch !== archive.storageEpoch ||
      metadata.activeReleaseGeneration !== generation
    ) {
      throw new StorageIntegrityError(
        'Archive batch lost its durable release fence',
      )
    }
    const batches = this.batches.get(key) ?? new Map<number, ArchiveBatch>()
    this.batches.set(key, batches)
    const existing = batches.get(batch.receipt.ordinal)
    if (existing) {
      if (canonicalJson(existing) !== canonicalJson(batch)) {
        throw new StorageIntegrityError('Archive batch replay conflicts')
      }
      return structuredClone(existing.receipt)
    }
    if (
      archive.manifest.expectedBatchOrdinals[batches.size] !==
      batch.receipt.ordinal
    ) {
      throw new StorageIntegrityError('Archive batch ordinal is out of order')
    }
    batches.set(batch.receipt.ordinal, structuredClone(batch))
    for (const event of batch.events) this.upsertEvent(event)
    return structuredClone(batch.receipt)
  }

  async finalizeArchive(
    taskId: string,
    generation: string,
    task: Task,
    seriesLatest: DurableSeriesState[],
  ): Promise<number> {
    const key = this.archiveKey(taskId, generation)
    const archive = this.generations.get(key)
    if (!archive) throw new StorageIntegrityError('Archive generation is missing')
    const batches = Array.from(this.batches.get(key)?.values() ?? []).sort(
      (left, right) => left.receipt.ordinal - right.receipt.ordinal,
    )
    if (
      canonicalJson(batches.map((batch) => batch.receipt.ordinal)) !==
      canonicalJson(archive.manifest.expectedBatchOrdinals)
    ) {
      throw new StorageIntegrityError('Archive generation is missing batches')
    }
    const pageDigests = await Promise.all(
      batches.map((batch) => computeArchiveSourcePageDigest(batch.events)),
    )
    const sourceEntryCount = batches.reduce(
      (total, batch) => total + batch.events.length,
      0,
    )
    if (
      sourceEntryCount !== archive.manifest.sourceEntryCount ||
      (await computeArchiveSourceDigest(pageDigests)) !==
        archive.manifest.sourceDigest ||
      (await computeSeriesStateDigest(seriesLatest)) !==
        archive.manifest.seriesStateDigest
    ) {
      throw new StorageIntegrityError(
        'Archive generation manifest verification failed',
      )
    }
    const metadata = this.metadata.get(taskId)
    if (
      !metadata ||
      metadata.storageState !== 'releasing' ||
      metadata.storageEpoch !== archive.storageEpoch ||
      metadata.activeReleaseGeneration !== generation
    ) {
      throw new StorageIntegrityError(
        'Archive finalization lost its durable release fence',
      )
    }
    metadata.archiveWatermark = archive.targetWatermark
    this.tasks.set(taskId, structuredClone(task))
    this.series.set(taskId, structuredClone(seriesLatest))
    const compactSeriesIds = new Set(
      seriesLatest.map((state) => state.seriesId),
    )
    const events = (this.events.get(taskId) ?? []).filter(
      (event) =>
        !event.seriesId ||
        !compactSeriesIds.has(event.seriesId) ||
        !(
          event.seriesMode === 'latest' ||
          event.seriesMode === 'accumulate'
        ),
    )
    events.push(...seriesLatest.map((state) => structuredClone(state.event)))
    events.sort((left, right) => left.index - right.index)
    this.events.set(taskId, events)
    archive.status = 'finalized'
    archive.updatedAt = Date.now()
    return archive.targetWatermark
  }

  async getArchiveWatermark(taskId: string): Promise<number> {
    const metadata = this.metadata.get(taskId)
    if (!metadata) throw new StorageIntegrityError('Task does not exist')
    return metadata.archiveWatermark
  }

  async getLastEventIndex(taskId: string): Promise<number> {
    const events = this.events.get(taskId) ?? []
    const series = this.series.get(taskId) ?? []
    return Math.max(
      this.metadata.get(taskId)?.archiveWatermark ?? -1,
      ...events.map((event) => event.index),
      ...series.map((state) => state.throughIndex),
    )
  }

  async getRecentEvents(taskId: string, limit: number): Promise<TaskEvent[]> {
    return structuredClone((this.events.get(taskId) ?? []).slice(-limit))
  }

  async getDurableSeriesState(taskId: string): Promise<DurableSeriesState[]> {
    return structuredClone(this.series.get(taskId) ?? [])
  }

  async claimOverdueTasks(limit: number, claimTtlMs: number): Promise<TtlClaim[]> {
    if (
      !Number.isSafeInteger(limit) ||
      limit <= 0 ||
      !Number.isSafeInteger(claimTtlMs) ||
      claimTtlMs <= 0
    ) {
      throw new StorageIntegrityError('TTL claim bounds are invalid')
    }
    const now = Date.now()
    const due = Array.from(this.metadata.values())
      .filter((metadata) => {
        const task = this.tasks.get(metadata.taskId)
        const active = this.ttlClaims.get(metadata.taskId)
        return (
          task !== undefined &&
          !isMemoryTerminal(task.status) &&
          metadata.executionDeadlineAt !== null &&
          metadata.executionDeadlineAt <= now &&
          (active === undefined || active.claimUntil <= now)
        )
      })
      .sort((left, right) =>
        (left.executionDeadlineAt! - right.executionDeadlineAt!) ||
        left.taskId.localeCompare(right.taskId),
      )
      .slice(0, limit)

    return due.map((metadata) => {
      const claim: TtlClaim = {
        taskId: metadata.taskId,
        claimToken: `${metadata.taskId}:${now}:${Math.random()}`,
        claimUntil: now + claimTtlMs,
        taskVersion: metadata.taskVersion,
        executionDeadlineAt: metadata.executionDeadlineAt!,
      }
      this.ttlClaims.set(metadata.taskId, claim)
      return structuredClone(claim)
    })
  }

  async terminalizeTtlClaim(
    claim: TtlClaim,
    task: Task,
    event: TaskEvent,
    assignment: WorkerAssignment | null,
  ): Promise<TerminalProjection | null> {
    const now = Date.now()
    const currentClaim = this.ttlClaims.get(claim.taskId)
    const currentTask = this.tasks.get(claim.taskId)
    const metadata = this.metadata.get(claim.taskId)
    if (
      !currentClaim ||
      canonicalJson(currentClaim) !== canonicalJson(claim) ||
      claim.claimUntil <= now ||
      !currentTask ||
      isMemoryTerminal(currentTask.status) ||
      !metadata ||
      metadata.taskVersion !== claim.taskVersion ||
      metadata.executionDeadlineAt !== claim.executionDeadlineAt
    ) {
      return null
    }
    if (
      task.id !== claim.taskId ||
      task.status !== 'timeout' ||
      event.taskId !== claim.taskId ||
      event.type !== 'taskcast:status'
    ) {
      throw new StorageIntegrityError('TTL terminalization input is invalid')
    }
    const durableAssignment = this.durableAssignments.get(claim.taskId) ?? null
    if (canonicalJson(durableAssignment) !== canonicalJson(assignment)) {
      throw new StorageIntegrityError(
        `Durable assignment changed before TTL terminalization: ${claim.taskId}`,
      )
    }
    const lastIndex = Math.max(
      metadata.archiveWatermark,
      ...(this.events.get(claim.taskId) ?? []).map((candidate) => candidate.index),
      ...(this.series.get(claim.taskId) ?? []).map((state) => state.throughIndex),
    )
    if (event.index !== lastIndex + 1) {
      throw new StorageIntegrityError(
        `TTL timeout event index is not contiguous for ${claim.taskId}`,
      )
    }

    this.tasks.set(claim.taskId, structuredClone(task))
    this.upsertEvent(event)
    metadata.executionDeadlineAt = null
    metadata.taskVersion += 1
    metadata.lastEventAt = event.timestamp
    this.ttlClaims.delete(claim.taskId)
    this.durableAssignments.delete(claim.taskId)
    const projection: TerminalProjection = {
      projectionId: `ttl:${event.id}`,
      task: structuredClone(task),
      event: structuredClone(event),
      assignment: structuredClone(durableAssignment),
      claimToken: claim.claimToken,
      claimUntil: claim.claimUntil,
    }
    this.terminalProjections.set(projection.projectionId, {
      projection: structuredClone(projection),
      projectedAt: null,
    })
    return projection
  }

  async claimTerminalProjections(
    limit: number,
    claimToken: string,
    claimTtlMs: number,
  ): Promise<TerminalProjection[]> {
    if (
      !Number.isSafeInteger(limit) ||
      limit <= 0 ||
      claimToken.length === 0 ||
      !Number.isSafeInteger(claimTtlMs) ||
      claimTtlMs <= 0
    ) {
      throw new StorageIntegrityError('Terminal projection claim bounds are invalid')
    }
    const now = Date.now()
    const claimed: TerminalProjection[] = []
    for (const row of this.terminalProjections.values()) {
      if (
        row.projectedAt !== null ||
        (row.projection.claimUntil !== null && row.projection.claimUntil > now)
      ) {
        continue
      }
      row.projection.claimToken = claimToken
      row.projection.claimUntil = now + claimTtlMs
      claimed.push(structuredClone(row.projection))
      if (claimed.length === limit) break
    }
    return claimed
  }

  async completeTerminalProjection(projection: TerminalProjection): Promise<void> {
    const row = this.terminalProjections.get(projection.projectionId)
    if (row?.projectedAt !== null && row?.projectedAt !== undefined) return
    if (
      !row ||
      projection.claimToken === null ||
      projection.claimUntil === null ||
      projection.claimUntil <= Date.now() ||
      row.projection.claimToken !== projection.claimToken ||
      row.projection.claimUntil !== projection.claimUntil
    ) {
      throw new StorageFenceConflictError('Terminal projection claim was lost')
    }
    row.projectedAt = Date.now()
    row.projection.claimToken = null
    row.projection.claimUntil = null
  }

  async saveDurableAssignment(assignment: WorkerAssignment): Promise<void> {
    this.durableAssignments.set(assignment.taskId, structuredClone(assignment))
  }

  async deleteDurableAssignment(
    taskId: string,
    assignmentId?: string,
  ): Promise<void> {
    const assignment = this.durableAssignments.get(taskId)
    if (
      assignment &&
      (
        assignmentId === undefined ||
        assignmentId === memoryAssignmentId(assignment)
      )
    ) {
      this.durableAssignments.delete(taskId)
    }
  }

  async saveWorkerEvent(event: WorkerAuditEvent): Promise<void> {
    const events = this.workerEvents.get(event.workerId) ?? []
    events.push(structuredClone(event))
    this.workerEvents.set(event.workerId, events)
  }

  async getWorkerEvents(
    workerId: string,
    opts?: EventQueryOptions,
  ): Promise<WorkerAuditEvent[]> {
    let events = structuredClone(this.workerEvents.get(workerId) ?? [])
    if (opts?.since?.id) {
      const index = events.findIndex((event) => event.id === opts.since!.id)
      if (index >= 0) events = events.slice(index + 1)
    } else if (opts?.since?.timestamp !== undefined) {
      events = events.filter(
        (event) => event.timestamp > opts.since!.timestamp!,
      )
    }
    if (opts?.limit !== undefined) events = events.slice(0, opts.limit)
    return events
  }

  private upsertEvent(event: TaskEvent): void {
    const events = this.events.get(event.taskId) ?? []
    const existing = events.findIndex(
      (candidate) => candidate.index === event.index,
    )
    if (existing >= 0) events[existing] = structuredClone(event)
    else events.push(structuredClone(event))
    events.sort((left, right) => left.index - right.index)
    this.events.set(event.taskId, events)
  }

  private upsertSeriesState(state: DurableSeriesState): void {
    const states = this.series.get(state.taskId) ?? []
    const existing = states.findIndex(
      (candidate) => candidate.seriesId === state.seriesId,
    )
    if (existing >= 0) states[existing] = structuredClone(state)
    else states.push(structuredClone(state))
    states.sort((left, right) => left.seriesId.localeCompare(right.seriesId))
    this.series.set(state.taskId, states)
  }

  private archiveKey(taskId: string, generation: string): string {
    return `${taskId}\u0000${generation}`
  }
}

function isMemoryTerminal(status: TaskStatus): boolean {
  return (
    status === 'completed' ||
    status === 'failed' ||
    status === 'timeout' ||
    status === 'cancelled'
  )
}

function hasMemoryExecutionDeadline(task: Task): boolean {
  return (
    task.ttl !== undefined &&
    task.status !== 'paused' &&
    !isMemoryTerminal(task.status)
  )
}

function memoryAssignmentId(assignment: WorkerAssignment): string {
  return `${assignment.taskId}:${assignment.workerId}:${assignment.assignedAt}`
}
