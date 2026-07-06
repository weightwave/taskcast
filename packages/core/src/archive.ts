import type { SeriesLatestEntry, TaskArchive, TaskArchiveEvent, TaskArchiveRestoreData } from './types.js'

export const TASK_ARCHIVE_SCHEMA = 'taskcast.taskArchive' as const
export const TASK_ARCHIVE_VERSION = 1 as const

export class InvalidTaskArchiveError extends Error {
  constructor(message: string) {
    super(message)
    this.name = 'InvalidTaskArchiveError'
  }
}

export function normalizeTaskArchive(archive: TaskArchive): TaskArchive {
  if (archive.schema !== TASK_ARCHIVE_SCHEMA) {
    throw new InvalidTaskArchiveError(`Unsupported archive schema: ${String(archive.schema)}`)
  }
  if (archive.version !== TASK_ARCHIVE_VERSION) {
    throw new InvalidTaskArchiveError(`Unsupported archive version: ${String(archive.version)}`)
  }
  if (!archive.task?.id) {
    throw new InvalidTaskArchiveError('Archive task.id is required')
  }

  const sorted = [...archive.events].sort((a, b) => a.index - b.index)
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
        `Archive events must be a complete un-compacted event stream with contiguous indexes from 0; expected ${expectedIndex}, got ${event.index}`,
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

function assertArchivePersistableEvent(event: TaskArchiveEvent): void {
  const candidate = event as TaskArchiveEvent & {
    seriesSnapshot?: unknown
    _accumulatedData?: unknown
  }

  if (candidate.seriesSnapshot !== undefined) {
    throw new InvalidTaskArchiveError(
      `Archive events must be complete raw delta events; seriesSnapshot is a collapsed presentation field on event ${event.id}`,
    )
  }
  if (candidate._accumulatedData !== undefined) {
    throw new InvalidTaskArchiveError(
      `Archive events must be raw persisted deltas; _accumulatedData is a transient broadcast field on event ${event.id}`,
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
