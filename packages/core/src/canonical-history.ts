import { archiveEventRecord } from './storage-digest.js'
import {
  StorageIntegrityError,
  type DurableSeriesState,
  type EventQueryOptions,
  type TaskEvent,
} from './types.js'

export function mergeCanonicalHistory(
  durableEvents: readonly TaskEvent[],
  hotEvents: readonly TaskEvent[],
  durableSeriesState: readonly DurableSeriesState[],
): TaskEvent[] {
  const seriesState = new Map<string, DurableSeriesState>()
  for (const state of durableSeriesState) {
    validateSeriesState(state)
    const key = seriesKey(state.taskId, state.seriesId)
    if (seriesState.has(key)) {
      throw new StorageIntegrityError(
        `Duplicate durable series state for ${state.taskId}:${state.seriesId}`,
      )
    }
    seriesState.set(key, state)
  }

  const byIndex = new Map<number, TaskEvent>()
  const byId = new Map<string, TaskEvent>()
  const add = (event: TaskEvent): void => {
    const indexed = byIndex.get(event.index)
    const identified = byId.get(event.id)
    if (indexed) {
      if (indexed.id !== event.id) {
        throw new StorageIntegrityError(
          `Canonical history index ${event.index} has conflicting event identities`,
        )
      }
      if (!sameEvent(indexed, event)) {
        throw new StorageIntegrityError(
          `Canonical history event ${event.id} has conflicting content`,
        )
      }
      return
    }
    if (identified) {
      throw new StorageIntegrityError(
        `Canonical history event ${event.id} has conflicting index or content`,
      )
    }
    byIndex.set(event.index, event)
    byId.set(event.id, event)
  }

  for (const event of durableEvents) {
    const state = matchingSeriesState(event, seriesState)
    if (state) continue
    add(event)
  }
  for (const state of durableSeriesState) add(state.event)

  for (const event of hotEvents) {
    const state = matchingSeriesState(event, seriesState)
    if (state && event.index <= state.throughIndex) continue
    add(event)
  }

  return Array.from(byIndex.values()).sort((left, right) => left.index - right.index)
}

export function applyCanonicalHistoryQuery(
  events: readonly TaskEvent[],
  opts?: EventQueryOptions,
): TaskEvent[] {
  let start = 0
  const since = opts?.since
  if (since?.id) {
    const position = events.findIndex((event) => event.id === since.id)
    if (position >= 0) start = position + 1
  }

  let result = events.slice(start)
  if (!since?.id && since?.index !== undefined) {
    result = result.filter((event) => event.index > since.index!)
  } else if (
    !since?.id &&
    since?.index === undefined &&
    since?.timestamp !== undefined
  ) {
    result = result.filter((event) => event.timestamp > since.timestamp!)
  }
  if (opts?.limit !== undefined) result = result.slice(0, opts.limit)
  return result
}

export function resolveCanonicalSeriesLatest(
  durableState: DurableSeriesState,
  hotEvents: readonly TaskEvent[],
): TaskEvent {
  validateSeriesState(durableState)
  const tail = hotEvents
    .filter(
      (event) =>
        event.taskId === durableState.taskId &&
        event.seriesId === durableState.seriesId &&
        event.seriesMode === durableState.mode &&
        event.index > durableState.throughIndex,
    )
    .sort((left, right) => left.index - right.index)

  if (durableState.mode === 'latest') {
    return tail.at(-1) ?? durableState.event
  }

  const field = durableState.event.seriesAccField ?? 'delta'
  let accumulated = durableState.event
  for (const event of tail) {
    const previousData = jsonObject(accumulated.data)
    const nextData = jsonObject(event.data)
    if (
      previousData &&
      nextData &&
      typeof previousData[field] === 'string' &&
      typeof nextData[field] === 'string'
    ) {
      accumulated = {
        ...event,
        data: {
          ...nextData,
          [field]: previousData[field] + nextData[field],
        },
      }
    } else {
      accumulated = event
    }
  }
  return accumulated
}

function matchingSeriesState(
  event: TaskEvent,
  states: ReadonlyMap<string, DurableSeriesState>,
): DurableSeriesState | undefined {
  if (
    !event.seriesId ||
    (event.seriesMode !== 'latest' && event.seriesMode !== 'accumulate')
  ) {
    return undefined
  }
  const state = states.get(seriesKey(event.taskId, event.seriesId))
  if (!state) return undefined
  if (state.mode !== event.seriesMode) {
    throw new StorageIntegrityError(
      `Canonical history series mode conflicts for ${event.taskId}:${event.seriesId}`,
    )
  }
  return state
}

function validateSeriesState(state: DurableSeriesState): void {
  if (
    state.event.taskId !== state.taskId ||
    state.event.seriesId !== state.seriesId ||
    state.event.seriesMode !== state.mode ||
    state.event.index !== state.throughIndex
  ) {
    throw new StorageIntegrityError(
      `Durable series state is inconsistent for ${state.taskId}:${state.seriesId}`,
    )
  }
}

function sameEvent(left: TaskEvent, right: TaskEvent): boolean {
  return archiveEventRecord(left) === archiveEventRecord(right)
}

function seriesKey(taskId: string, seriesId: string): string {
  return `${taskId}\0${seriesId}`
}

function jsonObject(value: unknown): Record<string, unknown> | null {
  return value !== null && typeof value === 'object' && !Array.isArray(value)
    ? value as Record<string, unknown>
    : null
}
