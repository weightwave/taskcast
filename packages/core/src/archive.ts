import type { SeriesLatestEntry, TaskArchive, TaskArchiveEvent, TaskArchiveRestoreData } from './types.js'

export const TASK_ARCHIVE_SCHEMA = 'taskcast.taskArchive' as const
export const TASK_ARCHIVE_VERSION = 1 as const
const TASK_STATUSES = new Set(['pending', 'assigned', 'running', 'paused', 'blocked', 'completed', 'failed', 'timeout', 'cancelled'])
const EVENT_LEVELS = new Set(['debug', 'info', 'warn', 'error'])
const SERIES_MODES = new Set(['keep-all', 'accumulate', 'latest'])

export class InvalidTaskArchiveError extends Error {
  constructor(message: string) {
    super(message)
    this.name = 'InvalidTaskArchiveError'
  }
}

export function normalizeTaskArchive(archive: TaskArchive): TaskArchive {
  if (!isRecord(archive)) {
    throw new InvalidTaskArchiveError('Archive must be an object')
  }
  if (archive.schema !== TASK_ARCHIVE_SCHEMA) {
    throw new InvalidTaskArchiveError(`Unsupported archive schema: ${String(archive.schema)}`)
  }
  if (archive.version !== TASK_ARCHIVE_VERSION) {
    throw new InvalidTaskArchiveError(`Unsupported archive version: ${String(archive.version)}`)
  }
  if (!Number.isFinite(archive.exportedAt)) {
    throw new InvalidTaskArchiveError('Archive exportedAt must be a finite number')
  }
  assertArchiveTask(archive.task)
  if (!Array.isArray(archive.events)) {
    throw new InvalidTaskArchiveError('Archive events must be an array')
  }

  const events = archive.events.map(assertArchiveEvent)
  const sorted = [...events].sort((a, b) => a.index - b.index)
  const seenIds = new Set<string>()
  const seenIndexes = new Set<number>()

  for (let expectedIndex = 0; expectedIndex < sorted.length; expectedIndex++) {
    const event = sorted[expectedIndex]!
    if (event.taskId !== archive.task.id) {
      throw new InvalidTaskArchiveError(`Archive event taskId mismatch for event ${event.id}`)
    }
    assertArchivePersistableEvent(event)
    if (seenIds.has(event.id)) {
      throw new InvalidTaskArchiveError(`Archive contains duplicate event id: ${event.id}`)
    }
    seenIds.add(event.id)
    if (seenIndexes.has(event.index)) {
      throw new InvalidTaskArchiveError(`Archive contains duplicate event index: ${event.index}`)
    }
    seenIndexes.add(event.index)
    if (event.index !== expectedIndex) {
      throw new InvalidTaskArchiveError(
        `Archive events must have contiguous indexes from 0; expected ${expectedIndex}, got ${event.index}`,
      )
    }
  }

  return { ...archive, events: sorted.map(sanitizeTaskArchiveEvent) }
}

export function buildTaskArchiveRestoreData(archive: TaskArchive): TaskArchiveRestoreData {
  const normalized = normalizeTaskArchive(archive)
  return {
    task: { ...normalized.task },
    events: normalized.events.map(sanitizeTaskArchiveEvent),
    nextIndex: normalized.events.length,
    seriesLatest: buildSeriesLatest(normalized.events),
  }
}

function buildSeriesLatest(events: TaskArchiveEvent[]): SeriesLatestEntry[] {
  const latest = new Map<string, TaskArchiveEvent>()

  for (const event of events) {
    if (!event.seriesId || !event.seriesMode) continue
    if (event.seriesMode === 'keep-all') continue

    const key = `${event.taskId}:${event.seriesId}`
    if (event.seriesMode === 'latest') {
      latest.set(key, sanitizeTaskArchiveEvent(event))
      continue
    }

    const field = event.seriesAccField ?? 'delta'
    const previous = latest.get(key)
    if (!previous) {
      latest.set(key, sanitizeTaskArchiveEvent(event))
      continue
    }

    const prevData = isRecord(previous.data) ? previous.data : {}
    const newData = isRecord(event.data) ? event.data : {}
    if (typeof prevData[field] === 'string' && typeof newData[field] === 'string') {
      latest.set(key, {
        ...sanitizeTaskArchiveEvent(event),
        data: { ...newData, [field]: prevData[field] + newData[field] },
      })
    } else {
      latest.set(key, sanitizeTaskArchiveEvent(event))
    }
  }

  return Array.from(latest.values()).map((event) => ({
    taskId: event.taskId,
    seriesId: event.seriesId!,
    event,
  }))
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value)
}

function assertArchiveTask(task: unknown): asserts task is TaskArchive['task'] {
  if (!isRecord(task)) {
    throw new InvalidTaskArchiveError('Archive task must be an object')
  }
  if (!isNonEmptyString(task.id)) {
    throw new InvalidTaskArchiveError('Archive task.id must be a string')
  }
  if (typeof task.status !== 'string' || !TASK_STATUSES.has(task.status)) {
    throw new InvalidTaskArchiveError('Archive task.status is invalid')
  }
  if (!Number.isFinite(task.createdAt)) {
    throw new InvalidTaskArchiveError('Archive task.createdAt must be a finite number')
  }
  if (!Number.isFinite(task.updatedAt)) {
    throw new InvalidTaskArchiveError('Archive task.updatedAt must be a finite number')
  }
}

function assertArchiveEvent(event: unknown): TaskArchiveEvent {
  if (!isRecord(event)) {
    throw new InvalidTaskArchiveError('Archive event must be an object')
  }
  const index = event.index
  if (!isNonEmptyString(event.id)) {
    throw new InvalidTaskArchiveError('Archive event.id must be a string')
  }
  if (!isNonEmptyString(event.taskId)) {
    throw new InvalidTaskArchiveError(`Archive event.taskId must be a string for event ${String(event.id)}`)
  }
  if (typeof index !== 'number' || !Number.isInteger(index) || index < 0) {
    throw new InvalidTaskArchiveError(`Archive event.index must be a non-negative integer for event ${String(event.id)}`)
  }
  if (!Number.isFinite(event.timestamp)) {
    throw new InvalidTaskArchiveError(`Archive event.timestamp must be a finite number for event ${String(event.id)}`)
  }
  if (typeof event.type !== 'string') {
    throw new InvalidTaskArchiveError(`Archive event.type must be a string for event ${String(event.id)}`)
  }
  if (typeof event.level !== 'string' || !EVENT_LEVELS.has(event.level)) {
    throw new InvalidTaskArchiveError(`Archive event.level is invalid for event ${String(event.id)}`)
  }
  if (!Object.prototype.hasOwnProperty.call(event, 'data')) {
    throw new InvalidTaskArchiveError(`Archive event.data is required for event ${String(event.id)}`)
  }
  if (event.seriesMode !== undefined && (typeof event.seriesMode !== 'string' || !SERIES_MODES.has(event.seriesMode))) {
    throw new InvalidTaskArchiveError(`Archive event.seriesMode is invalid for event ${String(event.id)}`)
  }
  if (event.seriesId !== undefined && typeof event.seriesId !== 'string') {
    throw new InvalidTaskArchiveError(`Archive event.seriesId must be a string for event ${String(event.id)}`)
  }
  if (event.seriesAccField !== undefined && typeof event.seriesAccField !== 'string') {
    throw new InvalidTaskArchiveError(`Archive event.seriesAccField must be a string for event ${String(event.id)}`)
  }

  return event as unknown as TaskArchiveEvent
}

function isNonEmptyString(value: unknown): value is string {
  return typeof value === 'string' && value.length > 0
}

function assertArchivePersistableEvent(event: TaskArchiveEvent): void {
  const candidate = event as TaskArchiveEvent & {
    seriesSnapshot?: unknown
    _accumulatedData?: unknown
  }

  if (candidate.seriesSnapshot !== undefined) {
    throw new InvalidTaskArchiveError(
      `Archive events cannot include presentation fields; seriesSnapshot is a collapsed presentation field on event ${event.id}`,
    )
  }
  if (candidate._accumulatedData !== undefined) {
    throw new InvalidTaskArchiveError(
      `Archive events cannot include transient broadcast fields; _accumulatedData is transient on event ${event.id}`,
    )
  }
}

function sanitizeTaskArchiveEvent(event: TaskArchiveEvent): TaskArchiveEvent {
  const { id, taskId, index, timestamp, type, level, data, seriesId, seriesMode, seriesAccField } = event
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
