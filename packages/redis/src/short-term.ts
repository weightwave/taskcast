import type { Redis } from 'ioredis'
import type {
  ArchiveSourcePage,
  ClosedWriteFence,
  HotWriteToken,
  RehydrateSnapshot,
  SeriesResult,
  StorageLease,
  StorageWriterRegistration,
  Task,
  TaskEvent,
  TaskMutationSnapshot,
  TaskStatus,
  ShortTermStore,
  TaskStoragePresence,
  TaskWriteFence,
  EventQueryOptions,
  TaskArchiveImportOptions,
  TaskArchiveRestoreData,
  TaskFilter,
  Worker,
  WorkerFilter,
  WorkerAssignment,
  TerminalProjection,
  TerminalProjectionResult,
} from '@taskcast/core'
import { StorageFenceConflictError, StorageIntegrityError } from '@taskcast/core'
import {
  classifyRedisError,
  observeRedisCommand,
  type RedisOperationOptions,
} from './connectivity.js'

function makeKeys(prefix: string) {
  return {
    task: (id: string) => `${prefix}:task:${id}`,
    taskStatus: (id: string) => `${prefix}:taskStatus:${id}`,
    taskSet: `${prefix}:tasks`,
    events: (id: string) => `${prefix}:events:${id}`,
    idx: (id: string) => `${prefix}:idx:${id}`,
    seriesState: (taskId: string) => `${prefix}:seriesState:${taskId}`,
    seriesListEntries: (taskId: string) => `${prefix}:seriesListEntries:${taskId}`,
    legacySeriesLatest: (taskId: string, seriesId: string) =>
      `${prefix}:series:${taskId}:${seriesId}`,
    legacySeriesIds: (taskId: string) => `${prefix}:seriesIds:${taskId}`,
    fence: (id: string) => `${prefix}:writeFence:${id}`,
    storageLock: (id: string) => `${prefix}:storageLock:${id}`,
    hotWindow: (id: string) => `${prefix}:hotWindow:${id}`,
    writers: `${prefix}:storageWriters`,
    writer: (instanceId: string) => `${prefix}:storageWriter:${instanceId}`,
    seriesPrefix: (taskId: string) => `${prefix}:series:${taskId}:`,
    worker: (id: string) => `${prefix}:worker:${id}`,
    workerSet: `${prefix}:workers`,
    assignment: (taskId: string) => `${prefix}:assignment:${taskId}`,
    workerAssignments: (workerId: string) => `${prefix}:workerAssignments:${workerId}`,
    terminalProjection: (projectionId: string) =>
      `${prefix}:terminalProjection:${projectionId}`,
  }
}

export class RedisShortTermStore implements ShortTermStore {
  readonly supportsHotColdRelease = true
  private KEY: ReturnType<typeof makeKeys>
  private legacySeriesWrites: boolean

  constructor(
    private redis: Redis,
    private options: {
      prefix?: string
      legacySeriesWrites?: boolean
    } & RedisOperationOptions = {},
  ) {
    const { prefix, legacySeriesWrites } = options
    const resolvedPrefix = prefix ?? process.env['TASKCAST_REDIS_PREFIX'] ?? 'taskcast'
    this.KEY = makeKeys(resolvedPrefix)
    this.legacySeriesWrites =
      legacySeriesWrites ??
      process.env['TASKCAST_REDIS_LEGACY_SERIES_WRITES']?.toLowerCase() ===
        'true'
    if (options.managed) {
      return new Proxy(this, {
        get: (target, property, receiver) => {
          const value = Reflect.get(target, property, receiver)
          if (typeof value !== 'function') return value
          return (...args: unknown[]) =>
            observeRedisCommand(target.options, () =>
              (value as (...methodArgs: unknown[]) => Promise<unknown>).apply(target, args),
            )
        },
      })
    }
  }

  async saveTask(task: Task): Promise<void> {
    await this.redis.eval(
      RedisShortTermStore.SAVE_TASK_LUA,
      4,
      this.KEY.task(task.id),
      this.KEY.taskSet,
      this.KEY.fence(task.id),
      this.KEY.taskStatus(task.id),
      JSON.stringify(task),
      task.id,
      JSON.stringify({
        taskId: task.id,
        acceptingWrites: true,
        storageEpoch: 1,
        activeReleaseGeneration: null,
      } satisfies TaskWriteFence),
      task.status,
    )
  }

  async getTask(taskId: string): Promise<Task | null> {
    const raw = await this.redis.get(this.KEY.task(taskId))
    return raw ? (JSON.parse(raw) as Task) : null
  }

  async getTaskMutationSnapshot(taskId: string): Promise<TaskMutationSnapshot | null> {
    const raw = await this.redis.get(this.KEY.task(taskId))
    if (!raw) return null
    return { task: JSON.parse(raw) as Task, revision: raw }
  }

  async acquireStorageLock(
    taskId: string,
    lockToken: string,
    generation: string,
    ttlMs: number,
  ): Promise<StorageLease | null> {
    if (ttlMs <= 0) throw new StorageIntegrityError('Storage lock TTL must be positive')
    const raw = await this.redis.eval(
      RedisShortTermStore.ACQUIRE_STORAGE_LOCK_LUA,
      2,
      this.KEY.storageLock(taskId),
      this.KEY.fence(taskId),
      taskId,
      lockToken,
      generation,
      String(Math.floor(ttlMs)),
    )
    return typeof raw === 'string' ? (JSON.parse(raw) as StorageLease) : null
  }

  async renewStorageLock(lease: StorageLease, ttlMs: number): Promise<boolean> {
    if (ttlMs <= 0) return false
    const result = await this.redis.eval(
      RedisShortTermStore.RENEW_STORAGE_LOCK_LUA,
      1,
      this.KEY.storageLock(lease.taskId),
      lease.taskId,
      lease.lockToken,
      lease.generation,
      String(lease.storageEpoch),
      String(Math.floor(ttlMs)),
    )
    return result === 1
  }

  async releaseStorageLock(lease: StorageLease): Promise<boolean> {
    const result = await this.redis.eval(
      RedisShortTermStore.RELEASE_STORAGE_LOCK_LUA,
      1,
      this.KEY.storageLock(lease.taskId),
      lease.taskId,
      lease.lockToken,
      lease.generation,
      String(lease.storageEpoch),
    )
    return result === 1
  }

  async getWriteFence(taskId: string): Promise<TaskWriteFence | null> {
    const raw = await this.redis.get(this.KEY.fence(taskId))
    return raw ? (JSON.parse(raw) as TaskWriteFence) : null
  }

  async closeWriteFence(
    lease: StorageLease,
    expectedEpoch: number,
  ): Promise<ClosedWriteFence> {
    const raw = await this.evalFenced<[string, string]>(
      RedisShortTermStore.CLOSE_WRITE_FENCE_LUA,
      3,
      this.KEY.storageLock(lease.taskId),
      this.KEY.fence(lease.taskId),
      this.KEY.idx(lease.taskId),
      lease.taskId,
      lease.lockToken,
      lease.generation,
      String(lease.storageEpoch),
      String(expectedEpoch),
    )
    const highWatermark = Number(raw[1])
    if (!Number.isSafeInteger(highWatermark) || highWatermark < -1) {
      throw new StorageIntegrityError('Redis returned an invalid high watermark')
    }
    const fence = JSON.parse(raw[0]) as TaskWriteFence
    if (fence.acceptingWrites !== false) {
      throw new StorageIntegrityError('Redis returned an invalid closed write fence')
    }
    return {
      ...fence,
      acceptingWrites: false,
      highWatermark,
    }
  }

  async reopenWriteFence(
    lease: StorageLease,
    expectedEpoch: number,
  ): Promise<HotWriteToken> {
    const raw = await this.evalFenced<string>(
      RedisShortTermStore.REOPEN_WRITE_FENCE_LUA,
      2,
      this.KEY.storageLock(lease.taskId),
      this.KEY.fence(lease.taskId),
      lease.taskId,
      lease.lockToken,
      lease.generation,
      String(lease.storageEpoch),
      String(expectedEpoch),
    )
    return JSON.parse(raw) as HotWriteToken
  }

  async commitEventFenced(
    taskId: string,
    event: Omit<TaskEvent, 'index'>,
    token: HotWriteToken,
  ): Promise<SeriesResult> {
    if (token.taskId !== taskId || event.taskId !== taskId) {
      throw new StorageFenceConflictError()
    }
    const seriesId = event.seriesId ?? ''
    const seriesMode = event.seriesMode ?? ''
    const field = event.seriesAccField ?? 'delta'
    const eventTemplate = this.makeIndexedEventTemplate(event)

    for (;;) {
      const snapshot = await this.redis
        .pipeline()
        .hget(this.KEY.seriesState(taskId), seriesId)
        .get(this.KEY.legacySeriesLatest(taskId, seriesId))
        .lindex(this.KEY.events(taskId), 0)
        .lindex(this.KEY.events(taskId), 1)
        .exec()
      if (!snapshot || snapshot.some(([error]) => error)) {
        throw new Error(`Redis pipeline failed while preparing event commit ${taskId}`)
      }

      const hashStateRaw =
        typeof snapshot[0]![1] === 'string' ? snapshot[0]![1] : ''
      const legacyCandidateRaw =
        typeof snapshot[1]![1] === 'string' ? snapshot[1]![1] : ''
      const state = this.selectSeriesState(
        hashStateRaw,
        legacyCandidateRaw,
        taskId,
        seriesId,
      )
      if (
        this.legacySeriesWrites &&
        legacyCandidateRaw &&
        !state.legacyStateRaw
      ) {
        throw new StorageIntegrityError(
          'Legacy series key collides with another task or series',
        )
      }
      const stateRaw = state.selectedRaw
      const firstRaw = typeof snapshot[2]![1] === 'string' ? snapshot[2]![1] : ''
      const secondRaw = typeof snapshot[3]![1] === 'string' ? snapshot[3]![1] : ''
      const previous = state.selectedEvent
      const first = firstRaw ? this.parseEventListHead(firstRaw, taskId) : null
      const second = secondRaw ? this.parseEventListHead(secondRaw, taskId) : null
      const accumulatedBase =
        seriesMode === 'accumulate'
          ? this.accumulateEvent(previous, event, field)
          : event
      const accumulatedTemplate = this.makeIndexedEventTemplate(accumulatedBase)

      const raw = await this.evalFenced<[string, string]>(
        RedisShortTermStore.COMMIT_EVENT_FENCED_LUA,
        8,
        this.KEY.fence(taskId),
        this.KEY.idx(taskId),
        this.KEY.events(taskId),
        this.KEY.seriesState(taskId),
        this.KEY.seriesListEntries(taskId),
        this.KEY.legacySeriesLatest(taskId, seriesId),
        this.KEY.legacySeriesIds(taskId),
        this.KEY.hotWindow(taskId),
        String(token.storageEpoch),
        eventTemplate.prefix,
        eventTemplate.suffix,
        seriesId,
        seriesMode,
        stateRaw,
        accumulatedTemplate.prefix,
        accumulatedTemplate.suffix,
        firstRaw,
        secondRaw,
        first ? String(first.index) : '',
        second ? String(second.index) : '',
        previous?.seriesMode === 'latest' ? '1' : '0',
        state.legacyStateRaw ? '1' : '0',
        hashStateRaw,
        legacyCandidateRaw,
        this.legacySeriesWrites ? '1' : '0',
      )
      if (raw[0] === 'RETRY') continue
      if (raw[0] !== 'COMMITTED') {
        throw new StorageIntegrityError('Redis returned an invalid fenced commit result')
      }
      const index = Number(raw[1])
      if (!Number.isSafeInteger(index) || index < 0) {
        throw new StorageIntegrityError('Redis returned an invalid event index')
      }

      const result: SeriesResult = {
        event: { ...event, index },
        stored: true,
      }
      if (seriesMode === 'accumulate') {
        result.accumulatedEvent = { ...accumulatedBase, index }
      }
      return result
    }
  }

  async saveTaskFenced(task: Task, token: HotWriteToken): Promise<void> {
    if (token.taskId !== task.id) throw new StorageFenceConflictError()
    await this.evalFenced<number>(
      RedisShortTermStore.SAVE_TASK_FENCED_LUA,
      3,
      this.KEY.fence(task.id),
      this.KEY.task(task.id),
      this.KEY.taskStatus(task.id),
      String(token.storageEpoch),
      JSON.stringify(task),
      task.status,
    )
  }

  async commitTaskEventsFenced(
    task: Task,
    expectedRevision: string,
    events: Omit<TaskEvent, 'index'>[],
    token: HotWriteToken,
  ): Promise<TaskEvent[] | null> {
    if (
      token.taskId !== task.id ||
      events.length === 0 ||
      events.some(
        (event) =>
          event.taskId !== task.id ||
          event.seriesId !== undefined ||
          event.seriesMode !== undefined,
      )
    ) {
      throw new StorageFenceConflictError()
    }
    const templates = events.flatMap((event) => {
      const template = this.makeIndexedEventTemplate(event)
      return [template.prefix, template.suffix]
    })
    const raw = await this.evalFenced<string[]>(
      RedisShortTermStore.COMMIT_TASK_EVENTS_FENCED_LUA,
      6,
      this.KEY.fence(task.id),
      this.KEY.task(task.id),
      this.KEY.idx(task.id),
      this.KEY.events(task.id),
      this.KEY.hotWindow(task.id),
      this.KEY.taskStatus(task.id),
      String(token.storageEpoch),
      JSON.stringify(task),
      expectedRevision,
      task.status,
      String(events.length),
      ...templates,
    )
    if (raw[0] === 'TASK_CONFLICT') return null
    if (
      raw[0] !== 'COMMITTED' ||
      raw.length !== events.length + 2
    ) {
      throw new StorageIntegrityError(
        'Redis returned an invalid fenced task-event commit result',
      )
    }
    return raw.slice(2).map((eventJson) => JSON.parse(eventJson) as TaskEvent)
  }

  async readArchiveSourcePage(
    taskId: string,
    watermark: number,
    cursor: string | null,
    limit: number,
  ): Promise<ArchiveSourcePage> {
    if (
      !Number.isSafeInteger(watermark) ||
      !Number.isSafeInteger(limit) ||
      watermark < -1 ||
      limit <= 0
    ) {
      throw new StorageIntegrityError('Invalid archive source bounds')
    }
    const boundedLimit = limit
    const position = this.decodeArchiveCursor(cursor, watermark)
    if (position.offset > Number.MAX_SAFE_INTEGER - boundedLimit) {
      throw new StorageIntegrityError('Archive source cursor exceeds safe bounds')
    }
    const raw = await this.redis.lrange(
      this.KEY.events(taskId),
      position.offset,
      position.offset + boundedLimit - 1,
    )
    const length = await this.redis.llen(this.KEY.events(taskId))
    const events: TaskEvent[] = []
    let lastIndex = position.lastIndex
    let beyondWatermark = false

    for (const encoded of raw) {
      let event: TaskEvent
      try {
        event = JSON.parse(encoded) as TaskEvent
      } catch {
        throw new StorageIntegrityError('Archive source contains invalid event JSON')
      }
      if (event.taskId !== taskId || !Number.isSafeInteger(event.index)) {
        throw new StorageIntegrityError('Archive source contains an invalid task or index')
      }
      if (event.index <= lastIndex) {
        throw new StorageIntegrityError('Archive source indexes are not strictly increasing')
      }
      if (event.index > watermark) {
        beyondWatermark = true
        break
      }
      events.push(event)
      lastIndex = event.index
    }

    const nextOffset = position.offset + raw.length
    const done = beyondWatermark || nextOffset >= length
    return {
      taskId,
      watermark,
      cursor,
      nextCursor: done
        ? null
        : this.encodeArchiveCursor(watermark, nextOffset, lastIndex),
      events,
      done,
    }
  }

  async deleteTaskStorageFenced(
    lease: StorageLease,
    expectedEpoch: number,
  ): Promise<void> {
    const seriesKeys = await this.scanSeriesKeys(lease.taskId)
    const keys = [
      this.KEY.storageLock(lease.taskId),
      this.KEY.fence(lease.taskId),
      this.KEY.task(lease.taskId),
      this.KEY.taskStatus(lease.taskId),
      this.KEY.events(lease.taskId),
      this.KEY.idx(lease.taskId),
      this.KEY.seriesState(lease.taskId),
      this.KEY.seriesListEntries(lease.taskId),
      this.KEY.legacySeriesIds(lease.taskId),
      this.KEY.taskSet,
      this.KEY.hotWindow(lease.taskId),
      ...seriesKeys,
    ]
    await this.evalFenced<number>(
      RedisShortTermStore.DELETE_TASK_STORAGE_LUA,
      keys.length,
      ...keys,
      lease.taskId,
      lease.lockToken,
      lease.generation,
      String(lease.storageEpoch),
      String(expectedEpoch),
    )
  }

  async restoreHotTaskFenced(
    snapshot: RehydrateSnapshot,
    lease: StorageLease,
    nextEpoch: number,
  ): Promise<HotWriteToken> {
    this.validateRehydrateSnapshot(snapshot, lease, nextEpoch)
    const taskId = snapshot.task.id
    const firstIndex = snapshot.replayEvents[0]?.index ?? null
    const lastIndex = snapshot.replayEvents.at(-1)?.index ?? null
    const replayById = new Map(
      snapshot.replayEvents.map((event) => [
        `${event.index}\u0000${event.id}`,
        JSON.stringify(event),
      ]),
    )
    await this.evalFenced<number>(
      RedisShortTermStore.RESTORE_HOT_TASK_LUA,
      10,
      this.KEY.storageLock(taskId),
      this.KEY.fence(taskId),
      this.KEY.task(taskId),
      this.KEY.events(taskId),
      this.KEY.idx(taskId),
      this.KEY.seriesState(taskId),
      this.KEY.seriesListEntries(taskId),
      this.KEY.taskSet,
      this.KEY.hotWindow(taskId),
      this.KEY.taskStatus(taskId),
      taskId,
      lease.lockToken,
      lease.generation,
      String(lease.storageEpoch),
      String(snapshot.storageEpoch),
      String(nextEpoch),
      JSON.stringify(snapshot.task),
      JSON.stringify(snapshot.replayEvents.map((event) => JSON.stringify(event))),
      JSON.stringify(
        snapshot.seriesLatest.map((entry) => ({
          seriesId: entry.seriesId,
          eventJson: JSON.stringify(entry.event),
          listEventJson:
            entry.mode === 'latest'
              ? replayById.get(`${entry.event.index}\u0000${entry.event.id}`) ?? ''
              : '',
        })),
      ),
      String(snapshot.maxEventIndex + 1),
      JSON.stringify({ firstIndex, lastIndex }),
      JSON.stringify({
        taskId,
        acceptingWrites: true,
        storageEpoch: nextEpoch,
        activeReleaseGeneration: null,
      } satisfies TaskWriteFence),
      snapshot.task.status,
    )
    return { taskId, storageEpoch: nextEpoch }
  }

  async projectTerminalFenced(
    projection: TerminalProjection,
    lease: StorageLease,
    expectedEpoch: number,
    nextEpoch: number,
  ): Promise<TerminalProjectionResult> {
    const taskId = projection.task.id
    if (
      taskId !== lease.taskId ||
      projection.task.status !== 'timeout' ||
      projection.event.taskId !== taskId ||
      (projection.assignment !== null &&
        projection.assignment.taskId !== taskId) ||
      nextEpoch !== expectedEpoch + 1
    ) {
      throw new StorageFenceConflictError()
    }
    const assignment = projection.assignment
    const raw = await this.evalFenced<[string, string]>(
      RedisShortTermStore.PROJECT_TERMINAL_FENCED_LUA,
      11,
      this.KEY.storageLock(taskId),
      this.KEY.fence(taskId),
      this.KEY.task(taskId),
      this.KEY.taskStatus(taskId),
      this.KEY.events(taskId),
      this.KEY.idx(taskId),
      this.KEY.hotWindow(taskId),
      this.KEY.assignment(taskId),
      this.KEY.workerAssignments(assignment?.workerId ?? ''),
      this.KEY.worker(assignment?.workerId ?? ''),
      this.KEY.terminalProjection(projection.projectionId),
      taskId,
      lease.lockToken,
      lease.generation,
      String(lease.storageEpoch),
      String(expectedEpoch),
      String(nextEpoch),
      JSON.stringify(projection.task),
      JSON.stringify(projection.event),
      String(projection.event.index),
      assignment ? JSON.stringify(assignment) : '',
      assignment?.workerId ?? '',
      String(assignment?.cost ?? 0),
      JSON.stringify({
        taskId,
        acceptingWrites: true,
        storageEpoch: nextEpoch,
        activeReleaseGeneration: null,
      } satisfies TaskWriteFence),
      String(7 * 24 * 60 * 60 * 1_000),
    )
    if ((raw[0] !== '0' && raw[0] !== '1') || Number(raw[1]) !== nextEpoch) {
      throw new StorageIntegrityError('Redis returned an invalid terminal projection result')
    }
    return {
      token: { taskId, storageEpoch: nextEpoch },
      projected: raw[0] === '1',
    }
  }

  async getTaskStoragePresence(taskId: string): Promise<TaskStoragePresence> {
    const legacySeriesKeys = await this.scanSeriesKeys(taskId)
    const results = await this.redis
      .pipeline()
      .exists(this.KEY.task(taskId), this.KEY.taskStatus(taskId))
      .llen(this.KEY.events(taskId))
      .exists(this.KEY.idx(taskId))
      .hlen(this.KEY.seriesState(taskId))
      .exists(this.KEY.fence(taskId))
      .exec()
    if (!results || results.some(([error]) => error)) {
      throw new Error(`Redis pipeline failed while inspecting task storage ${taskId}`)
    }
    return {
      task: Number(results[0]![1]) > 0,
      eventCount: Number(results[1]![1]),
      nextIndex: results[2]![1] === 1,
      seriesStateCount: Number(results[3]![1]) + legacySeriesKeys.length,
      writeFence: results[4]![1] === 1,
    }
  }

  async registerStorageWriter(
    registration: StorageWriterRegistration,
    ttlMs: number,
  ): Promise<void> {
    if (ttlMs <= 0) throw new StorageIntegrityError('Writer readiness TTL must be positive')
    const stored: StorageWriterRegistration = {
      ...registration,
      expiresAt: Date.now() + ttlMs,
    }
    await this.redis.eval(
      RedisShortTermStore.REGISTER_STORAGE_WRITER_LUA,
      2,
      this.KEY.writer(registration.instanceId),
      this.KEY.writers,
      registration.instanceId,
      JSON.stringify(stored),
      String(Math.floor(ttlMs)),
    )
  }

  async listStorageWriters(): Promise<StorageWriterRegistration[]> {
    const instanceIds = await this.redis.smembers(this.KEY.writers)
    if (instanceIds.length === 0) return []
    const values = await this.redis.mget(
      ...instanceIds.map((instanceId) => this.KEY.writer(instanceId)),
    )
    const stale: string[] = []
    const registrations: StorageWriterRegistration[] = []
    for (let index = 0; index < instanceIds.length; index++) {
      const raw = values[index]
      if (!raw) {
        stale.push(instanceIds[index]!)
        continue
      }
      registrations.push(JSON.parse(raw) as StorageWriterRegistration)
    }
    if (stale.length > 0) await this.redis.srem(this.KEY.writers, ...stale)
    return registrations
  }

  async nextIndex(taskId: string): Promise<number> {
    // INCR is atomic — safe across multiple instances sharing the same Redis
    return (await this.redis.incr(this.KEY.idx(taskId))) - 1
  }

  async appendEvent(taskId: string, event: TaskEvent): Promise<void> {
    await this.redis.rpush(this.KEY.events(taskId), JSON.stringify(event))
  }

  async validateTaskArchiveRestore(
    data: TaskArchiveRestoreData,
    options?: TaskArchiveImportOptions,
  ): Promise<void> {
    const taskId = data.task.id
    const taskKey = this.KEY.task(taskId)
    const taskType = await this.redis.type(taskKey)
    await this.assertRedisType(taskKey, taskType, ['none', 'string'])

    const exists = taskType !== 'none'
    if (exists && options?.overwrite !== true) {
      throw new Error(`Task already exists: ${taskId}`)
    }

    await this.assertRedisType(this.KEY.taskSet, await this.redis.type(this.KEY.taskSet), ['none', 'set'])
    await this.assertRedisType(this.KEY.events(taskId), await this.redis.type(this.KEY.events(taskId)), ['none', 'list'])
    await this.assertRedisType(this.KEY.idx(taskId), await this.redis.type(this.KEY.idx(taskId)), ['none', 'string'])
    await this.assertRedisType(this.KEY.fence(taskId), await this.redis.type(this.KEY.fence(taskId)), ['none', 'string'])
    await this.assertRedisType(this.KEY.taskStatus(taskId), await this.redis.type(this.KEY.taskStatus(taskId)), ['none', 'string'])
    await this.assertRedisType(this.KEY.hotWindow(taskId), await this.redis.type(this.KEY.hotWindow(taskId)), ['none', 'string'])

    await this.assertRedisType(
      this.KEY.seriesState(taskId),
      await this.redis.type(this.KEY.seriesState(taskId)),
      ['none', 'hash'],
    )
    await this.assertRedisType(
      this.KEY.seriesListEntries(taskId),
      await this.redis.type(this.KEY.seriesListEntries(taskId)),
      ['none', 'hash'],
    )
    await this.assertRedisType(
      this.KEY.legacySeriesIds(taskId),
      await this.redis.type(this.KEY.legacySeriesIds(taskId)),
      ['none', 'set'],
    )
    await this.scanSeriesKeys(taskId)
  }

  private async assertRedisType(key: string, actual: string, allowed: string[]): Promise<void> {
    if (!allowed.includes(actual)) {
      throw new Error(`Redis key type mismatch for ${key}: expected ${allowed.join(' or ')}, got ${actual}`)
    }
  }

  async restoreTaskArchive(
    data: TaskArchiveRestoreData,
    options?: TaskArchiveImportOptions,
  ): Promise<{ overwritten: boolean }> {
    const taskId = data.task.id
    const taskKey = this.KEY.task(taskId)
    const exists = await this.redis.exists(taskKey)
    await this.validateTaskArchiveRestore(data, options)

    const legacySeriesKeys = await this.scanSeriesKeys(taskId)
    const pipeline = this.redis.pipeline()

    pipeline.set(taskKey, JSON.stringify(data.task))
    pipeline.set(this.KEY.taskStatus(taskId), data.task.status)
    pipeline.sadd(this.KEY.taskSet, taskId)
    pipeline.del(this.KEY.events(taskId))
    for (const event of data.events) {
      pipeline.rpush(this.KEY.events(taskId), JSON.stringify(event))
    }
    pipeline.set(this.KEY.idx(taskId), String(data.nextIndex))
    pipeline.set(
      this.KEY.fence(taskId),
      JSON.stringify({
        taskId,
        acceptingWrites: true,
        storageEpoch: 1,
        activeReleaseGeneration: null,
      } satisfies TaskWriteFence),
    )
    pipeline.set(
      this.KEY.hotWindow(taskId),
      JSON.stringify({
        firstIndex: data.events[0]?.index ?? null,
        lastIndex: data.events.at(-1)?.index ?? null,
      }),
    )

    pipeline.del(this.KEY.seriesState(taskId))
    pipeline.del(this.KEY.seriesListEntries(taskId))
    pipeline.del(this.KEY.legacySeriesIds(taskId))
    if (legacySeriesKeys.length > 0) pipeline.del(...legacySeriesKeys)
    for (const entry of data.seriesLatest) {
      const eventJson = JSON.stringify(entry.event)
      pipeline.hset(this.KEY.seriesState(entry.taskId), entry.seriesId, eventJson)
      if (
        entry.event.seriesMode === 'latest' &&
        data.events.some(
          (event) =>
            event.index === entry.event.index &&
            event.id === entry.event.id &&
            JSON.stringify(event) === eventJson,
        )
      ) {
        pipeline.hset(
          this.KEY.seriesListEntries(entry.taskId),
          entry.seriesId,
          eventJson,
        )
      }
    }

    await this.execPipelineOrThrow(pipeline, `restore task archive ${taskId}`)
    return { overwritten: Boolean(exists) }
  }

  private async execPipelineOrThrow(
    pipeline: ReturnType<Redis['pipeline']>,
    context: string,
  ): Promise<void> {
    const results = await pipeline.exec()
    if (!results) {
      throw new Error(`Redis pipeline failed during ${context}: no results returned`)
    }

    for (const [index, [err]] of results.entries()) {
      if (err) {
        if (classifyRedisError(err)) throw err
        throw new Error(`Redis pipeline failed during ${context} at command ${index}: ${err.message}`)
      }
    }
  }

  async getEvents(taskId: string, opts?: EventQueryOptions): Promise<TaskEvent[]> {
    const raw = await this.redis.lrange(this.KEY.events(taskId), 0, -1)
    let events = raw.map((r) => JSON.parse(r) as TaskEvent)

    const since = opts?.since
    if (since?.id) {
      const idx = events.findIndex((e) => e.id === since.id)
      events = idx >= 0 ? events.slice(idx + 1) : events
    } else if (since?.index !== undefined) {
      events = events.filter((e) => e.index > since.index!)
    } else if (since?.timestamp !== undefined) {
      events = events.filter((e) => e.timestamp > since.timestamp!)
    }

    if (opts?.limit) events = events.slice(0, opts.limit)
    return events
  }

  async setTTL(taskId: string, ttlSeconds: number): Promise<void> {
    await this.redis.expire(this.KEY.task(taskId), ttlSeconds)
    await this.redis.expire(this.KEY.taskStatus(taskId), ttlSeconds)
    await this.redis.expire(this.KEY.events(taskId), ttlSeconds)
    await this.redis.expire(this.KEY.idx(taskId), ttlSeconds)
    await this.redis.expire(this.KEY.fence(taskId), ttlSeconds)
    await this.redis.expire(this.KEY.hotWindow(taskId), ttlSeconds)
    await this.redis.expire(this.KEY.seriesState(taskId), ttlSeconds)
    await this.redis.expire(this.KEY.seriesListEntries(taskId), ttlSeconds)

    const legacySeriesKeys = await this.scanSeriesKeys(taskId)
    const pipeline = this.redis.pipeline()
    for (const key of legacySeriesKeys) {
      pipeline.expire(key, ttlSeconds)
    }
    pipeline.expire(this.KEY.legacySeriesIds(taskId), ttlSeconds)
    await this.inspectPipelineForConnectionErrors(await pipeline.exec())
  }

  async getSeriesLatest(taskId: string, seriesId: string): Promise<TaskEvent | null> {
    const [hashStateRaw, legacyCandidateRaw] = await Promise.all([
      this.redis.hget(this.KEY.seriesState(taskId), seriesId),
      this.redis.get(this.KEY.legacySeriesLatest(taskId, seriesId)),
    ])
    return this.selectSeriesState(
      hashStateRaw ?? '',
      legacyCandidateRaw ?? '',
      taskId,
      seriesId,
    ).selectedEvent
  }

  async setSeriesLatest(taskId: string, seriesId: string, event: TaskEvent): Promise<void> {
    const legacyCandidateRaw =
      (await this.redis.get(this.KEY.legacySeriesLatest(taskId, seriesId))) ?? ''
    const state = this.selectSeriesState('', legacyCandidateRaw, taskId, seriesId)
    if (
      this.legacySeriesWrites &&
      legacyCandidateRaw &&
      !state.legacyStateRaw
    ) {
      throw new StorageIntegrityError(
        'Legacy series key collides with another task or series',
      )
    }
    await this.redis.eval(
      RedisShortTermStore.SET_SERIES_LATEST_LUA,
      4,
      this.KEY.seriesState(taskId),
      this.KEY.seriesListEntries(taskId),
      this.KEY.legacySeriesLatest(taskId, seriesId),
      this.KEY.legacySeriesIds(taskId),
      JSON.stringify(event),
      seriesId,
      state.legacyStateRaw ? '1' : '0',
      this.legacySeriesWrites ? '1' : '0',
    )
  }

  async accumulateSeries(taskId: string, seriesId: string, event: TaskEvent, field: string): Promise<TaskEvent> {
    for (;;) {
      const [hashRaw, legacyCandidate] = await Promise.all([
        this.redis.hget(this.KEY.seriesState(taskId), seriesId),
        this.redis.get(this.KEY.legacySeriesLatest(taskId, seriesId)),
      ])
      const state = this.selectSeriesState(
        hashRaw ?? '',
        legacyCandidate ?? '',
        taskId,
        seriesId,
      )
      if (
        this.legacySeriesWrites &&
        legacyCandidate &&
        !state.legacyStateRaw
      ) {
        throw new StorageIntegrityError(
          'Legacy series key collides with another task or series',
        )
      }
      const previous = state.selectedEvent
      const accumulated = this.accumulateEvent(previous, event, field) as TaskEvent
      const result = await this.redis.eval(
        RedisShortTermStore.ACCUMULATE_LUA,
        4,
        this.KEY.seriesState(taskId),
        this.KEY.seriesListEntries(taskId),
        this.KEY.legacySeriesLatest(taskId, seriesId),
        this.KEY.legacySeriesIds(taskId),
        seriesId,
        JSON.stringify(accumulated),
        state.legacyStateRaw ? '1' : '0',
        hashRaw ?? '',
        legacyCandidate ?? '',
        this.legacySeriesWrites ? '1' : '0',
      )
      if (result === 'RETRY') continue
      return accumulated
    }
  }

  async replaceLastSeriesEvent(taskId: string, seriesId: string, event: TaskEvent): Promise<void> {
    const prev = await this.getSeriesLatest(taskId, seriesId)
    if (prev) {
      // Find and replace the previous event in the list
      const raw = await this.redis.lrange(this.KEY.events(taskId), 0, -1)
      let idx = -1
      for (let i = raw.length - 1; i >= 0; i--) {
        try {
          if ((JSON.parse(raw[i]!) as TaskEvent).id === prev.id) {
            idx = i
            break
          }
        } catch {
          // ignore parse errors
        }
      }
      if (idx >= 0) {
        await this.redis.lset(this.KEY.events(taskId), idx, JSON.stringify(event))
      }
    } else {
      await this.appendEvent(taskId, event)
    }
    await this.setSeriesLatest(taskId, seriesId, event)
    await this.redis.hset(
      this.KEY.seriesListEntries(taskId),
      seriesId,
      JSON.stringify(event),
    )
  }

  // Task query
  async listTasks(filter: TaskFilter): Promise<Task[]> {
    const taskIds = await this.redis.smembers(this.KEY.taskSet)
    if (taskIds.length === 0) return []

    const pipeline = this.redis.pipeline()
    for (const id of taskIds) {
      pipeline.get(this.KEY.task(id))
    }
    const results = await pipeline.exec()

    let tasks: Task[] = []
    const staleIds: string[] = []
    if (results) {
      for (let i = 0; i < results.length; i++) {
        const entry = results[i]!
        const [err, raw] = entry
        if (err && classifyRedisError(err)) throw err
        if (!err && typeof raw === 'string') {
          tasks.push(JSON.parse(raw) as Task)
        } else if (!err) {
          staleIds.push(taskIds[i]!)
        }
      }
    }
    if (staleIds.length > 0) {
      await this.redis.srem(this.KEY.taskSet, ...staleIds)
    }

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
    await this.redis.set(this.KEY.worker(worker.id), JSON.stringify(worker))
    await this.redis.sadd(this.KEY.workerSet, worker.id)
  }

  async getWorker(workerId: string): Promise<Worker | null> {
    const raw = await this.redis.get(this.KEY.worker(workerId))
    return raw ? (JSON.parse(raw) as Worker) : null
  }

  async listWorkers(filter?: WorkerFilter): Promise<Worker[]> {
    const workerIds = await this.redis.smembers(this.KEY.workerSet)
    if (workerIds.length === 0) return []

    const pipeline = this.redis.pipeline()
    for (const id of workerIds) {
      pipeline.get(this.KEY.worker(id))
    }
    const results = await pipeline.exec()

    let workers: Worker[] = []
    if (results) {
      for (const [err, raw] of results) {
        if (err && classifyRedisError(err)) throw err
        if (!err && typeof raw === 'string') {
          workers.push(JSON.parse(raw) as Worker)
        }
      }
    }

    if (filter?.status?.length) {
      workers = workers.filter((w) => filter.status!.includes(w.status))
    }
    if (filter?.connectionMode?.length) {
      workers = workers.filter((w) => filter.connectionMode!.includes(w.connectionMode))
    }

    return workers
  }

  async deleteWorker(workerId: string): Promise<void> {
    await this.redis.del(this.KEY.worker(workerId))
    await this.redis.srem(this.KEY.workerSet, workerId)
  }

  private makeIndexedEventTemplate(
    event: Omit<TaskEvent, 'index'>,
  ): { prefix: string; suffix: string } {
    const withoutIndex = { ...(event as TaskEvent) } as Partial<TaskEvent>
    delete withoutIndex.index
    const encoded = JSON.stringify({ index: 0, ...withoutIndex })
    const marker = '{"index":0'
    if (!encoded.startsWith(marker)) {
      throw new StorageIntegrityError('Unable to build an opaque indexed event')
    }
    return {
      prefix: encoded.slice(0, marker.length - 1),
      suffix: encoded.slice(marker.length),
    }
  }

  private parseSeriesState(
    raw: string,
    taskId: string,
    seriesId: string,
  ): TaskEvent {
    let event: TaskEvent
    try {
      event = JSON.parse(raw) as TaskEvent
    } catch {
      throw new StorageIntegrityError('Series state contains invalid event JSON')
    }
    if (
      event.taskId !== taskId ||
      (event.seriesId !== undefined && event.seriesId !== seriesId) ||
      !Number.isSafeInteger(event.index) ||
      event.index < 0
    ) {
      throw new StorageIntegrityError('Series state does not match its task and series')
    }
    return event
  }

  private selectSeriesState(
    hashStateRaw: string,
    legacyCandidateRaw: string,
    taskId: string,
    seriesId: string,
  ): {
    selectedRaw: string
    selectedEvent: TaskEvent | null
    legacyStateRaw: string
  } {
    const hashEvent = hashStateRaw
      ? this.parseSeriesState(hashStateRaw, taskId, seriesId)
      : null
    let legacyStateRaw = ''
    let legacyEvent: TaskEvent | null = null
    if (legacyCandidateRaw) {
      let candidate: TaskEvent
      try {
        candidate = JSON.parse(legacyCandidateRaw) as TaskEvent
      } catch {
        throw new StorageIntegrityError('Legacy series state contains invalid event JSON')
      }
      if (
        candidate.taskId === taskId &&
        (candidate.seriesId === undefined || candidate.seriesId === seriesId) &&
        Number.isSafeInteger(candidate.index) &&
        candidate.index >= 0
      ) {
        legacyStateRaw = legacyCandidateRaw
        legacyEvent = candidate
      }
    }

    if (!hashEvent) {
      return {
        selectedRaw: legacyStateRaw,
        selectedEvent: legacyEvent,
        legacyStateRaw,
      }
    }
    if (!legacyEvent) {
      return {
        selectedRaw: hashStateRaw,
        selectedEvent: hashEvent,
        legacyStateRaw,
      }
    }
    if (hashEvent.index === legacyEvent.index) {
      if (hashStateRaw !== legacyStateRaw) {
        throw new StorageIntegrityError(
          'Hash and legacy series state conflict at the same index',
        )
      }
      return {
        selectedRaw: hashStateRaw,
        selectedEvent: hashEvent,
        legacyStateRaw,
      }
    }
    // New writers update both representations atomically in compatibility
    // mode (or delete legacy in fixed mode). If they differ, only an old
    // writer can have updated the legacy key afterward, so legacy is newer
    // even when that writer reserved its event index earlier.
    return {
      selectedRaw: legacyStateRaw,
      selectedEvent: legacyEvent,
      legacyStateRaw,
    }
  }

  private parseEventListHead(raw: string, taskId: string): TaskEvent {
    let event: TaskEvent
    try {
      event = JSON.parse(raw) as TaskEvent
    } catch {
      throw new StorageIntegrityError('Event list contains invalid event JSON')
    }
    if (
      event.taskId !== taskId ||
      !Number.isSafeInteger(event.index) ||
      event.index < 0
    ) {
      throw new StorageIntegrityError('Event list head does not match its task')
    }
    return event
  }

  private accumulateEvent(
    previous: TaskEvent | null,
    event: Omit<TaskEvent, 'index'>,
    field: string,
  ): Omit<TaskEvent, 'index'> {
    if (
      previous === null ||
      typeof previous.data !== 'object' ||
      previous.data === null ||
      Array.isArray(previous.data) ||
      typeof event.data !== 'object' ||
      event.data === null ||
      Array.isArray(event.data) ||
      typeof (previous.data as Record<string, unknown>)[field] !== 'string' ||
      typeof (event.data as Record<string, unknown>)[field] !== 'string'
    ) {
      return event
    }

    return {
      ...event,
      data: {
        ...(event.data as Record<string, unknown>),
        [field]:
          (previous.data as Record<string, unknown>)[field] as string +
          ((event.data as Record<string, unknown>)[field] as string),
      },
    }
  }

  private async scanSeriesKeys(taskId: string): Promise<string[]> {
    const keys = new Set<string>()
    let escapedPrefix = ''
    for (const character of this.KEY.seriesPrefix(taskId)) {
      escapedPrefix +=
        character === '\\' ||
        character === '*' ||
        character === '?' ||
        character === '[' ||
        character === ']'
          ? `\\${character}`
          : character
    }
    let cursor = '0'
    do {
      const [nextCursor, page] = await this.redis.scan(
        cursor,
        'MATCH',
        `${escapedPrefix}*`,
        'COUNT',
        1000,
      )
      if (page.length > 0) {
        const values = await this.redis.mget(...page)
        for (let index = 0; index < page.length; index++) {
          const raw = values[index]
          if (!raw) continue
          let event: TaskEvent
          try {
            event = JSON.parse(raw) as TaskEvent
          } catch {
            throw new StorageIntegrityError(
              `Series state contains invalid event JSON: ${page[index]}`,
            )
          }
          if (event.taskId === taskId) keys.add(page[index]!)
          if (keys.size > 1000) {
            throw new StorageIntegrityError(
              'Legacy series state exceeds the bounded migration limit',
            )
          }
        }
      }
      cursor = nextCursor
    } while (cursor !== '0')
    return Array.from(keys)
  }

  private async evalFenced<T>(
    script: string,
    keyCount: number,
    ...args: string[]
  ): Promise<T> {
    try {
      return (await this.redis.eval(script, keyCount, ...args)) as T
    } catch (error) {
      if (error instanceof Error && error.message.includes('STORAGE_FENCE_CONFLICT')) {
        throw new StorageFenceConflictError()
      }
      if (error instanceof Error && error.message.includes('STORAGE_INTEGRITY_ERROR')) {
        throw new StorageIntegrityError('Redis storage state failed integrity validation')
      }
      throw error
    }
  }

  private encodeArchiveCursor(
    watermark: number,
    offset: number,
    lastIndex: number,
  ): string {
    return `tc1|${watermark}|${offset}|${lastIndex}`
  }

  private decodeArchiveCursor(
    cursor: string | null,
    watermark: number,
  ): { offset: number; lastIndex: number } {
    if (cursor === null) return { offset: 0, lastIndex: -1 }
    const match = /^tc1\|(-?\d+)\|(\d+)\|(-?\d+)$/.exec(cursor)
    if (!match) throw new StorageIntegrityError('Invalid archive source cursor')
    const cursorWatermark = Number(match[1])
    const offset = Number(match[2])
    const lastIndex = Number(match[3])
    if (
      cursorWatermark !== watermark ||
      !Number.isSafeInteger(offset) ||
      !Number.isSafeInteger(lastIndex)
    ) {
      throw new StorageIntegrityError('Archive source cursor does not match the request')
    }
    return { offset, lastIndex }
  }

  private validateRehydrateSnapshot(
    snapshot: RehydrateSnapshot,
    lease: StorageLease,
    nextEpoch: number,
  ): void {
    if (
      snapshot.task.id !== lease.taskId ||
      !Number.isSafeInteger(nextEpoch) ||
      nextEpoch <= snapshot.storageEpoch
    ) {
      throw new StorageFenceConflictError()
    }
    if (
      !Number.isSafeInteger(snapshot.maxEventIndex) ||
      !Number.isSafeInteger(snapshot.archiveWatermark) ||
      snapshot.maxEventIndex < -1 ||
      snapshot.maxEventIndex >= Number.MAX_SAFE_INTEGER ||
      snapshot.archiveWatermark > snapshot.maxEventIndex
    ) {
      throw new StorageIntegrityError('Invalid durable event bounds for rehydration')
    }
    let previousIndex = -1
    for (const event of snapshot.replayEvents) {
      if (
        event.taskId !== lease.taskId ||
        !Number.isSafeInteger(event.index) ||
        event.index <= previousIndex ||
        event.index > snapshot.maxEventIndex
      ) {
        throw new StorageIntegrityError('Rehydrate replay events are not strictly ordered')
      }
      previousIndex = event.index
    }
    const seriesIds = new Set<string>()
    for (const entry of snapshot.seriesLatest) {
      if (
        entry.taskId !== lease.taskId ||
        entry.event.taskId !== lease.taskId ||
        entry.event.seriesId !== entry.seriesId ||
        entry.event.seriesMode !== entry.mode ||
        !Number.isSafeInteger(entry.event.index) ||
        !Number.isSafeInteger(entry.throughIndex) ||
        entry.event.index > entry.throughIndex ||
        entry.throughIndex > snapshot.maxEventIndex ||
        seriesIds.has(entry.seriesId)
      ) {
        throw new StorageIntegrityError('Invalid durable series state for rehydration')
      }
      seriesIds.add(entry.seriesId)
    }
  }

  private static SAVE_TASK_LUA = `
    redis.call('SET', KEYS[1], ARGV[1])
    redis.call('SADD', KEYS[2], ARGV[2])
    redis.call('SETNX', KEYS[3], ARGV[3])
    redis.call('SET', KEYS[4], ARGV[4])
    return 1
  `

  private static ACQUIRE_STORAGE_LOCK_LUA = `
    local currentJson = redis.call('GET', KEYS[1])
    if currentJson then
      local current = cjson.decode(currentJson)
      if current.taskId == ARGV[1]
         and current.lockToken == ARGV[2]
         and current.generation == ARGV[3] then
        redis.call('PEXPIRE', KEYS[1], tonumber(ARGV[4]))
        return currentJson
      end
      return false
    end

    local epoch = 1
    local fenceJson = redis.call('GET', KEYS[2])
    if fenceJson then
      local fence = cjson.decode(fenceJson)
      epoch = fence.storageEpoch
    end
    local lease = {
      taskId = ARGV[1],
      lockToken = ARGV[2],
      generation = ARGV[3],
      storageEpoch = epoch
    }
    local encoded = cjson.encode(lease)
    redis.call('SET', KEYS[1], encoded, 'PX', tonumber(ARGV[4]))
    return encoded
  `

  private static RENEW_STORAGE_LOCK_LUA = `
    local currentJson = redis.call('GET', KEYS[1])
    if not currentJson then return 0 end
    local current = cjson.decode(currentJson)
    if current.taskId ~= ARGV[1]
       or current.lockToken ~= ARGV[2]
       or current.generation ~= ARGV[3]
       or current.storageEpoch ~= tonumber(ARGV[4]) then
      return 0
    end
    redis.call('PEXPIRE', KEYS[1], tonumber(ARGV[5]))
    return 1
  `

  private static RELEASE_STORAGE_LOCK_LUA = `
    local currentJson = redis.call('GET', KEYS[1])
    if not currentJson then return 0 end
    local current = cjson.decode(currentJson)
    if current.taskId ~= ARGV[1]
       or current.lockToken ~= ARGV[2]
       or current.generation ~= ARGV[3]
       or current.storageEpoch ~= tonumber(ARGV[4]) then
      return 0
    end
    redis.call('DEL', KEYS[1])
    return 1
  `

  private static CLOSE_WRITE_FENCE_LUA = `
    local leaseJson = redis.call('GET', KEYS[1])
    local fenceJson = redis.call('GET', KEYS[2])
    if not leaseJson or not fenceJson then
      return redis.error_reply('STORAGE_FENCE_CONFLICT')
    end
    local lease = cjson.decode(leaseJson)
    local fence = cjson.decode(fenceJson)
    if lease.taskId ~= ARGV[1]
       or lease.lockToken ~= ARGV[2]
       or lease.generation ~= ARGV[3]
       or lease.storageEpoch ~= tonumber(ARGV[4])
       or fence.taskId ~= ARGV[1]
       or (fence.acceptingWrites ~= true and fence.acceptingWrites ~= false)
       or fence.storageEpoch ~= tonumber(ARGV[5]) then
      return redis.error_reply('STORAGE_FENCE_CONFLICT')
    end
    local nextIndexJson = redis.call('GET', KEYS[3])
    local highWatermarkJson = '-1'
    if nextIndexJson then
      if not string.match(nextIndexJson, '^[0-9]+$')
         or (#nextIndexJson > 1 and string.sub(nextIndexJson, 1, 1) == '0')
         or #nextIndexJson > 16
         or (#nextIndexJson == 16 and nextIndexJson > '9007199254740991') then
        return redis.error_reply('STORAGE_INTEGRITY_ERROR')
      end
      redis.call('DECR', KEYS[3])
      highWatermarkJson = redis.call('GET', KEYS[3])
      redis.call('INCR', KEYS[3])
    end
    local closed = {
      taskId = ARGV[1],
      acceptingWrites = false,
      storageEpoch = tonumber(ARGV[5]),
      activeReleaseGeneration = ARGV[3]
    }
    local encoded = cjson.encode(closed)
    redis.call('SET', KEYS[2], encoded)
    return { encoded, highWatermarkJson }
  `

  private static REOPEN_WRITE_FENCE_LUA = `
    local leaseJson = redis.call('GET', KEYS[1])
    local fenceJson = redis.call('GET', KEYS[2])
    if not leaseJson or not fenceJson then
      return redis.error_reply('STORAGE_FENCE_CONFLICT')
    end
    local lease = cjson.decode(leaseJson)
    local fence = cjson.decode(fenceJson)
    if lease.taskId ~= ARGV[1]
       or lease.lockToken ~= ARGV[2]
       or lease.generation ~= ARGV[3]
       or lease.storageEpoch ~= tonumber(ARGV[4])
       or fence.acceptingWrites ~= false
       or fence.storageEpoch ~= tonumber(ARGV[5])
       or fence.activeReleaseGeneration ~= ARGV[3] then
      return redis.error_reply('STORAGE_FENCE_CONFLICT')
    end
    local nextEpoch = tonumber(ARGV[5]) + 1
    local reopened = {
      taskId = ARGV[1],
      acceptingWrites = true,
      storageEpoch = nextEpoch,
      activeReleaseGeneration = cjson.null
    }
    redis.call('SET', KEYS[2], cjson.encode(reopened))
    return cjson.encode({ taskId = ARGV[1], storageEpoch = nextEpoch })
  `

  private static COMMIT_EVENT_FENCED_LUA = `
    local function validType(key, expected)
      local actual = redis.call('TYPE', key).ok
      return actual == 'none' or actual == expected
    end
    local function validIndex(value, maximum)
      return string.match(value, '^[0-9]+$')
        and (#value == 1 or string.sub(value, 1, 1) ~= '0')
        and (#value < #maximum or (#value == #maximum and value <= maximum))
    end

    if not validType(KEYS[1], 'string')
       or not validType(KEYS[2], 'string')
       or not validType(KEYS[3], 'list')
       or not validType(KEYS[4], 'hash')
       or not validType(KEYS[5], 'hash')
       or not validType(KEYS[6], 'string')
       or not validType(KEYS[7], 'set')
       or not validType(KEYS[8], 'string') then
      return redis.error_reply('STORAGE_INTEGRITY_ERROR')
    end

    local fenceJson = redis.call('GET', KEYS[1])
    if not fenceJson then
      return redis.error_reply('STORAGE_FENCE_CONFLICT')
    end
    local fence = cjson.decode(fenceJson)
    if fence.acceptingWrites ~= true or fence.storageEpoch ~= tonumber(ARGV[1]) then
      return redis.error_reply('STORAGE_FENCE_CONFLICT')
    end

    local indexJson = redis.call('GET', KEYS[2]) or '0'
    if not validIndex(indexJson, '9007199254740990') then
      return redis.error_reply('STORAGE_INTEGRITY_ERROR')
    end

    local currentHashState = redis.call('HGET', KEYS[4], ARGV[4])
    local currentLegacyState = redis.call('GET', KEYS[6])
    if (ARGV[5] == 'latest' or ARGV[5] == 'accumulate')
       and ARGV[4] ~= ''
       and (
         (currentHashState or '') ~= ARGV[15]
         or (currentLegacyState or '') ~= ARGV[16]
       ) then
      return { 'RETRY', '' }
    end
    local currentState = ARGV[6] ~= '' and ARGV[6] or nil

    local currentFirst = redis.call('LINDEX', KEYS[3], 0)
    local currentSecond = redis.call('LINDEX', KEYS[3], 1)
    if (currentFirst or '') ~= ARGV[9]
       or (currentSecond or '') ~= ARGV[10] then
      return { 'RETRY', '' }
    end
    if currentFirst and not validIndex(ARGV[11], '9007199254740991') then
      return redis.error_reply('STORAGE_INTEGRITY_ERROR')
    end
    if currentSecond and not validIndex(ARGV[12], '9007199254740991') then
      return redis.error_reply('STORAGE_INTEGRITY_ERROR')
    end

    local eventJson = ARGV[2] .. indexJson .. ARGV[3]
    local removeHead = false
    local seriesWriteJson = ''

    if ARGV[5] == 'latest' and ARGV[4] ~= '' then
      local previousListJson = redis.call('HGET', KEYS[5], ARGV[4])
      if ARGV[13] == '1' and currentState and ARGV[6] ~= ARGV[15] then
        previousListJson = currentState
      elseif not previousListJson and ARGV[13] == '1' then
        previousListJson = currentState
      end
      if previousListJson then
        removeHead = currentFirst == previousListJson
        redis.call('LREM', KEYS[3], -1, previousListJson)
      end
      redis.call('RPUSH', KEYS[3], eventJson)
      redis.call('HSET', KEYS[4], ARGV[4], eventJson)
      redis.call('HSET', KEYS[5], ARGV[4], eventJson)
      seriesWriteJson = eventJson
    elseif ARGV[5] == 'accumulate' and ARGV[4] ~= '' then
      redis.call('RPUSH', KEYS[3], eventJson)
      local accumulatedJson = ARGV[7] .. indexJson .. ARGV[8]
      redis.call('HSET', KEYS[4], ARGV[4], accumulatedJson)
      redis.call('HDEL', KEYS[5], ARGV[4])
      seriesWriteJson = accumulatedJson
    else
      redis.call('RPUSH', KEYS[3], eventJson)
    end

    if seriesWriteJson ~= '' then
      if ARGV[17] == '1' then
        redis.call('SET', KEYS[6], seriesWriteJson)
        redis.call('SADD', KEYS[7], ARGV[4])
      elseif ARGV[14] == '1' then
        redis.call('DEL', KEYS[6])
        redis.call('SREM', KEYS[7], ARGV[4])
      end
    end
    redis.call('INCR', KEYS[2])
    local firstIndexJson = indexJson
    if currentFirst and not removeHead then
      firstIndexJson = ARGV[11]
    elseif currentSecond then
      firstIndexJson = ARGV[12]
    end
    redis.call(
      'SET',
      KEYS[8],
      '{"firstIndex":' .. firstIndexJson
        .. ',"lastIndex":' .. indexJson .. '}'
    )

    return { 'COMMITTED', indexJson }
  `

  private static SAVE_TASK_FENCED_LUA = `
    local fenceJson = redis.call('GET', KEYS[1])
    if not fenceJson then
      return redis.error_reply('STORAGE_FENCE_CONFLICT')
    end
    local fence = cjson.decode(fenceJson)
    if fence.acceptingWrites ~= true or fence.storageEpoch ~= tonumber(ARGV[1]) then
      return redis.error_reply('STORAGE_FENCE_CONFLICT')
    end
    redis.call('SET', KEYS[2], ARGV[2])
    redis.call('SET', KEYS[3], ARGV[3])
    return 1
  `

  private static COMMIT_TASK_EVENTS_FENCED_LUA = `
    local function validType(key, expected)
      local actual = redis.call('TYPE', key).ok
      return actual == 'none' or actual == expected
    end
    local function validIndex(value, maximum)
      return string.match(value, '^[0-9]+$')
        and (#value == 1 or string.sub(value, 1, 1) ~= '0')
        and (#value < #maximum or (#value == #maximum and value <= maximum))
    end
    local function increment(value)
      local carry = 1
      local result = ''
      for position = #value, 1, -1 do
        local digit = tonumber(string.sub(value, position, position)) + carry
        if digit >= 10 then
          digit = digit - 10
          carry = 1
        else
          carry = 0
        end
        result = tostring(digit) .. result
      end
      if carry == 1 then result = '1' .. result end
      return result
    end

    if not validType(KEYS[1], 'string')
       or not validType(KEYS[2], 'string')
       or not validType(KEYS[3], 'string')
       or not validType(KEYS[4], 'list')
       or not validType(KEYS[5], 'string')
       or not validType(KEYS[6], 'string') then
      return redis.error_reply('STORAGE_INTEGRITY_ERROR')
    end
    local fenceJson = redis.call('GET', KEYS[1])
    if not fenceJson then
      return redis.error_reply('STORAGE_FENCE_CONFLICT')
    end
    local fence = cjson.decode(fenceJson)
    if fence.acceptingWrites ~= true or fence.storageEpoch ~= tonumber(ARGV[1]) then
      return redis.error_reply('STORAGE_FENCE_CONFLICT')
    end
    local currentTaskJson = redis.call('GET', KEYS[2])
    if not currentTaskJson then
      return redis.error_reply('STORAGE_INTEGRITY_ERROR')
    end
    if currentTaskJson ~= ARGV[3] then
      return { 'TASK_CONFLICT' }
    end
    local eventCount = tonumber(ARGV[5])
    if not eventCount or eventCount < 1 or eventCount > 16
       or eventCount ~= math.floor(eventCount)
       or #ARGV ~= 5 + eventCount * 2 then
      return redis.error_reply('STORAGE_INTEGRITY_ERROR')
    end
    local indexJson = redis.call('GET', KEYS[3]) or '0'
    if not validIndex(indexJson, '9007199254740990') then
      return redis.error_reply('STORAGE_INTEGRITY_ERROR')
    end
    local finalIndex = indexJson
    for _ = 1, eventCount do
      finalIndex = increment(finalIndex)
    end
    if not validIndex(finalIndex, '9007199254740991') then
      return redis.error_reply('STORAGE_INTEGRITY_ERROR')
    end
    local originalIndex = indexJson
    local committed = { 'COMMITTED', '' }
    redis.call('SET', KEYS[2], ARGV[2])
    redis.call('SET', KEYS[6], ARGV[4])
    for ordinal = 0, eventCount - 1 do
      if not validIndex(indexJson, '9007199254740991') then
        return redis.error_reply('STORAGE_INTEGRITY_ERROR')
      end
      local eventJson = ARGV[6 + ordinal * 2] .. indexJson
        .. ARGV[7 + ordinal * 2]
      redis.call('RPUSH', KEYS[4], eventJson)
      table.insert(committed, eventJson)
      indexJson = increment(indexJson)
    end
    redis.call('SET', KEYS[3], indexJson)
    committed[2] = indexJson

    local firstIndex = originalIndex
    local existingWindow = redis.call('GET', KEYS[5])
    if existingWindow then
      local decoded = cjson.decode(existingWindow)
      if decoded.firstIndex ~= cjson.null then
        firstIndex = tostring(decoded.firstIndex)
      end
    end
    local lastIndex = tostring(tonumber(indexJson) - 1)
    redis.call(
      'SET',
      KEYS[5],
      '{"firstIndex":' .. firstIndex .. ',"lastIndex":' .. lastIndex .. '}'
    )
    return committed
  `

  private static DELETE_TASK_STORAGE_LUA = `
    local function validType(key, expected)
      local actual = redis.call('TYPE', key).ok
      return actual == 'none' or actual == expected
    end
    if not validType(KEYS[1], 'string')
       or not validType(KEYS[2], 'string')
       or not validType(KEYS[3], 'string')
       or not validType(KEYS[4], 'string')
       or not validType(KEYS[5], 'list')
       or not validType(KEYS[6], 'string')
       or not validType(KEYS[7], 'hash')
       or not validType(KEYS[8], 'hash')
       or not validType(KEYS[9], 'set')
       or not validType(KEYS[10], 'set')
       or not validType(KEYS[11], 'string') then
      return redis.error_reply('STORAGE_INTEGRITY_ERROR')
    end
    for index = 12, #KEYS do
      if not validType(KEYS[index], 'string') then
        return redis.error_reply('STORAGE_INTEGRITY_ERROR')
      end
    end

    local leaseJson = redis.call('GET', KEYS[1])
    local fenceJson = redis.call('GET', KEYS[2])
    if not leaseJson or not fenceJson then
      return redis.error_reply('STORAGE_FENCE_CONFLICT')
    end
    local lease = cjson.decode(leaseJson)
    local fence = cjson.decode(fenceJson)
    if lease.taskId ~= ARGV[1]
       or lease.lockToken ~= ARGV[2]
       or lease.generation ~= ARGV[3]
       or lease.storageEpoch ~= tonumber(ARGV[4])
       or fence.acceptingWrites ~= false
       or fence.storageEpoch ~= tonumber(ARGV[5])
       or fence.activeReleaseGeneration ~= ARGV[3] then
      return redis.error_reply('STORAGE_FENCE_CONFLICT')
    end

    for index = 12, #KEYS do
      redis.call('UNLINK', KEYS[index])
    end
    redis.call(
      'UNLINK',
      KEYS[2],
      KEYS[3],
      KEYS[4],
      KEYS[5],
      KEYS[6],
      KEYS[7],
      KEYS[8],
      KEYS[9],
      KEYS[11]
    )
    redis.call('SREM', KEYS[10], ARGV[1])
    return 1
  `

  private static RESTORE_HOT_TASK_LUA = `
    local function validType(key, expected)
      local actual = redis.call('TYPE', key).ok
      return actual == 'none' or actual == expected
    end
    if not validType(KEYS[1], 'string')
       or not validType(KEYS[2], 'string')
       or not validType(KEYS[3], 'string')
       or not validType(KEYS[4], 'list')
       or not validType(KEYS[5], 'string')
       or not validType(KEYS[6], 'hash')
       or not validType(KEYS[7], 'hash')
       or not validType(KEYS[8], 'set')
       or not validType(KEYS[9], 'string')
       or not validType(KEYS[10], 'string') then
      return redis.error_reply('STORAGE_INTEGRITY_ERROR')
    end

    local leaseJson = redis.call('GET', KEYS[1])
    if not leaseJson then
      return redis.error_reply('STORAGE_FENCE_CONFLICT')
    end
    local lease = cjson.decode(leaseJson)
    if lease.taskId ~= ARGV[1]
       or lease.lockToken ~= ARGV[2]
       or lease.generation ~= ARGV[3]
       or lease.storageEpoch ~= tonumber(ARGV[4])
       or tonumber(ARGV[6]) <= tonumber(ARGV[5]) then
      return redis.error_reply('STORAGE_FENCE_CONFLICT')
    end

    local existingFenceJson = redis.call('GET', KEYS[2])
    if existingFenceJson then
      local existingFence = cjson.decode(existingFenceJson)
      if existingFence.acceptingWrites == true
         and existingFence.storageEpoch == tonumber(ARGV[6])
         and redis.call('EXISTS', KEYS[3]) == 1 then
        redis.call('SET', KEYS[10], ARGV[13])
        return 2
      end
      return redis.error_reply('STORAGE_FENCE_CONFLICT')
    end

    local replay = cjson.decode(ARGV[8])
    local series = cjson.decode(ARGV[9])
    if type(replay) ~= 'table' or type(series) ~= 'table' then
      return redis.error_reply('STORAGE_INTEGRITY_ERROR')
    end
    for _, eventJson in ipairs(replay) do
      if type(eventJson) ~= 'string' then
        return redis.error_reply('STORAGE_INTEGRITY_ERROR')
      end
    end
    for _, entry in ipairs(series) do
      if type(entry) ~= 'table'
         or type(entry.seriesId) ~= 'string'
         or type(entry.eventJson) ~= 'string'
         or type(entry.listEventJson) ~= 'string' then
        return redis.error_reply('STORAGE_INTEGRITY_ERROR')
      end
    end
    redis.call('DEL', KEYS[4], KEYS[6], KEYS[7])
    for _, eventJson in ipairs(replay) do
      redis.call('RPUSH', KEYS[4], eventJson)
    end
    for _, entry in ipairs(series) do
      redis.call('HSET', KEYS[6], entry.seriesId, entry.eventJson)
      if entry.listEventJson ~= '' then
        redis.call('HSET', KEYS[7], entry.seriesId, entry.listEventJson)
      end
    end

    redis.call('SET', KEYS[3], ARGV[7])
    redis.call('SADD', KEYS[8], ARGV[1])
    redis.call('SET', KEYS[5], ARGV[10])
    redis.call('SET', KEYS[9], ARGV[11])
    redis.call('SET', KEYS[2], ARGV[12])
    redis.call('SET', KEYS[10], ARGV[13])
    return 1
  `

  private static PROJECT_TERMINAL_FENCED_LUA = `
    local function validType(key, expected)
      local actual = redis.call('TYPE', key).ok
      return actual == 'none' or actual == expected
    end
    if not validType(KEYS[1], 'string')
       or not validType(KEYS[2], 'string')
       or not validType(KEYS[3], 'string')
       or not validType(KEYS[4], 'string')
       or not validType(KEYS[5], 'list')
       or not validType(KEYS[6], 'string')
       or not validType(KEYS[7], 'string')
       or not validType(KEYS[8], 'string')
       or not validType(KEYS[9], 'set')
       or not validType(KEYS[10], 'string')
       or not validType(KEYS[11], 'string') then
      return redis.error_reply('STORAGE_INTEGRITY_ERROR')
    end

    local leaseJson = redis.call('GET', KEYS[1])
    local fenceJson = redis.call('GET', KEYS[2])
    if not leaseJson or not fenceJson then
      return redis.error_reply('STORAGE_FENCE_CONFLICT')
    end
    local lease = cjson.decode(leaseJson)
    local fence = cjson.decode(fenceJson)
    if lease.taskId ~= ARGV[1]
       or lease.lockToken ~= ARGV[2]
       or lease.generation ~= ARGV[3]
       or lease.storageEpoch ~= tonumber(ARGV[4])
       or fence.taskId ~= ARGV[1]
       or fence.acceptingWrites ~= false
       or fence.storageEpoch ~= tonumber(ARGV[5])
       or fence.activeReleaseGeneration ~= ARGV[3]
       or tonumber(ARGV[6]) ~= tonumber(ARGV[5]) + 1 then
      return redis.error_reply('STORAGE_FENCE_CONFLICT')
    end

    local eventIndex = tonumber(ARGV[9])
    local nextIndex = tonumber(redis.call('GET', KEYS[6]) or '0')
    if not eventIndex or eventIndex < 0 or not nextIndex then
      return redis.error_reply('STORAGE_INTEGRITY_ERROR')
    end
    local projected = 0
    if nextIndex == eventIndex then
      redis.call('RPUSH', KEYS[5], ARGV[8])
      redis.call('SET', KEYS[6], tostring(eventIndex + 1))
      projected = 1
    elseif nextIndex > eventIndex then
      local found = false
      for _, candidateJson in ipairs(redis.call('LRANGE', KEYS[5], 0, -1)) do
        local candidate = cjson.decode(candidateJson)
        if candidate.index == eventIndex then
          if candidateJson ~= ARGV[8] then
            return redis.error_reply('STORAGE_INTEGRITY_ERROR')
          end
          found = true
          break
        end
      end
      if not found then
        return redis.error_reply('STORAGE_INTEGRITY_ERROR')
      end
    else
      return redis.error_reply('STORAGE_INTEGRITY_ERROR')
    end

    redis.call('SET', KEYS[3], ARGV[7])
    redis.call('SET', KEYS[4], 'timeout')
    local windowJson = redis.call('GET', KEYS[7])
    local firstIndex = eventIndex
    if windowJson then
      local window = cjson.decode(windowJson)
      if window.firstIndex ~= cjson.null then firstIndex = window.firstIndex end
    end
    redis.call(
      'SET',
      KEYS[7],
      cjson.encode({ firstIndex = firstIndex, lastIndex = eventIndex })
    )

    if ARGV[10] ~= '' and redis.call('EXISTS', KEYS[11]) == 0 then
      local assignmentJson = redis.call('GET', KEYS[8])
      if assignmentJson and assignmentJson ~= ARGV[10] then
        return redis.error_reply('STORAGE_INTEGRITY_ERROR')
      end
      if assignmentJson then
        redis.call('DEL', KEYS[8])
        redis.call('SREM', KEYS[9], ARGV[1])
        local workerJson = redis.call('GET', KEYS[10])
        if workerJson then
          local worker = cjson.decode(workerJson)
          worker.usedSlots = math.max(0, worker.usedSlots - tonumber(ARGV[12]))
          if worker.status ~= 'offline' and worker.status ~= 'draining' then
            if worker.usedSlots >= worker.capacity then
              worker.status = 'busy'
            else
              worker.status = 'idle'
            end
          end
          redis.call('SET', KEYS[10], cjson.encode(worker))
        end
      end
      redis.call('SET', KEYS[11], '1', 'PX', tonumber(ARGV[14]))
    end

    redis.call('SET', KEYS[2], ARGV[13])
    return { tostring(projected), ARGV[6] }
  `

  private static REGISTER_STORAGE_WRITER_LUA = `
    redis.call('SET', KEYS[1], ARGV[2], 'PX', tonumber(ARGV[3]))
    redis.call('SADD', KEYS[2], ARGV[1])
    return 1
  `

  private static SET_SERIES_LATEST_LUA = `
    redis.call('HSET', KEYS[1], ARGV[2], ARGV[1])
    redis.call('HDEL', KEYS[2], ARGV[2])
    if ARGV[4] == '1' then
      redis.call('SET', KEYS[3], ARGV[1])
      redis.call('SADD', KEYS[4], ARGV[2])
    elseif ARGV[3] == '1' then
      redis.call('DEL', KEYS[3])
      redis.call('SREM', KEYS[4], ARGV[2])
    end
    return 1
  `

  // Atomic accumulate — uses a Lua script so the read-merge-write is a single Redis command.
  // This prevents two concurrent publishes from losing accumulated data.
  private static ACCUMULATE_LUA = `
    local currentHash = redis.call('HGET', KEYS[1], ARGV[1])
    local currentLegacy = redis.call('GET', KEYS[3])
    if (currentHash or '') ~= ARGV[4]
       or (currentLegacy or '') ~= ARGV[5] then
      return 'RETRY'
    end
    redis.call('HSET', KEYS[1], ARGV[1], ARGV[2])
    redis.call('HDEL', KEYS[2], ARGV[1])
    if ARGV[6] == '1' then
      redis.call('SET', KEYS[3], ARGV[2])
      redis.call('SADD', KEYS[4], ARGV[1])
    elseif ARGV[3] == '1' then
      redis.call('DEL', KEYS[3])
      redis.call('SREM', KEYS[4], ARGV[1])
    end
    return 'COMMITTED'
  `

  // Atomic claim — uses a Lua script so the read-check-write is a single Redis command.
  // This prevents two workers racing to claim the same task.
  private static CLAIM_LUA = `
    local taskJson = redis.call('GET', KEYS[1])
    if not taskJson then return 0 end

    local task = cjson.decode(taskJson)
    if task.status ~= 'pending' and task.status ~= 'assigned' then return 0 end

    local workerJson = redis.call('GET', KEYS[2])
    if not workerJson then return 0 end

    local worker = cjson.decode(workerJson)
    local cost = tonumber(ARGV[1])
    if worker.usedSlots + cost > worker.capacity then return 0 end

    worker.usedSlots = worker.usedSlots + cost
    redis.call('SET', KEYS[2], cjson.encode(worker))

    task.status = 'assigned'
    task.assignedWorker = ARGV[2]
    task.cost = cost
    task.updatedAt = tonumber(ARGV[3])
    redis.call('SET', KEYS[1], cjson.encode(task))
    redis.call('SET', KEYS[3], 'assigned')

    return 1
  `

  async claimTask(taskId: string, workerId: string, cost: number): Promise<boolean> {
    const result = await this.redis.eval(
      RedisShortTermStore.CLAIM_LUA,
      3,
      this.KEY.task(taskId),
      this.KEY.worker(workerId),
      this.KEY.taskStatus(taskId),
      String(cost),
      workerId,
      String(Date.now()),
    )
    return result === 1
  }

  // Worker assignments
  async addAssignment(assignment: WorkerAssignment): Promise<void> {
    await this.redis.set(this.KEY.assignment(assignment.taskId), JSON.stringify(assignment))
    await this.redis.sadd(this.KEY.workerAssignments(assignment.workerId), assignment.taskId)
  }

  async removeAssignment(taskId: string): Promise<void> {
    const raw = await this.redis.get(this.KEY.assignment(taskId))
    if (raw) {
      const assignment = JSON.parse(raw) as WorkerAssignment
      await this.redis.srem(this.KEY.workerAssignments(assignment.workerId), taskId)
    }
    await this.redis.del(this.KEY.assignment(taskId))
  }

  async getWorkerAssignments(workerId: string): Promise<WorkerAssignment[]> {
    const taskIds = await this.redis.smembers(this.KEY.workerAssignments(workerId))
    if (taskIds.length === 0) return []

    const pipeline = this.redis.pipeline()
    for (const id of taskIds) {
      pipeline.get(this.KEY.assignment(id))
    }
    const results = await pipeline.exec()

    const assignments: WorkerAssignment[] = []
    if (results) {
      for (const [err, raw] of results) {
        if (err && classifyRedisError(err)) throw err
        if (!err && typeof raw === 'string') {
          assignments.push(JSON.parse(raw) as WorkerAssignment)
        }
      }
    }
    return assignments
  }

  async getTaskAssignment(taskId: string): Promise<WorkerAssignment | null> {
    const raw = await this.redis.get(this.KEY.assignment(taskId))
    return raw ? (JSON.parse(raw) as WorkerAssignment) : null
  }

  // TTL management — remove expiry from task-related keys
  async clearTTL(taskId: string): Promise<void> {
    await this.redis.persist(this.KEY.task(taskId))
    await this.redis.persist(this.KEY.taskStatus(taskId))
    await this.redis.persist(this.KEY.events(taskId))
    await this.redis.persist(this.KEY.idx(taskId))
    await this.redis.persist(this.KEY.fence(taskId))
    await this.redis.persist(this.KEY.hotWindow(taskId))
    await this.redis.persist(this.KEY.seriesState(taskId))
    await this.redis.persist(this.KEY.seriesListEntries(taskId))

    const legacySeriesKeys = await this.scanSeriesKeys(taskId)
    const pipeline = this.redis.pipeline()
    for (const key of legacySeriesKeys) {
      pipeline.persist(key)
    }
    pipeline.persist(this.KEY.legacySeriesIds(taskId))
    await this.inspectPipelineForConnectionErrors(await pipeline.exec())
  }

  // Task query by status
  async listByStatus(statuses: TaskStatus[]): Promise<Task[]> {
    return this.listTasks({ status: statuses })
  }

  private async inspectPipelineForConnectionErrors(
    results: Awaited<ReturnType<ReturnType<Redis['pipeline']>['exec']>>,
  ): Promise<void> {
    if (!results) return
    for (const [error] of results) {
      if (error && classifyRedisError(error)) throw error
    }
  }
}
