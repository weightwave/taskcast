import type { TaskEvent, ShortTermStore, SeriesResult } from './types.js'

export async function processSeries(
  event: TaskEvent,
  store: ShortTermStore,
): Promise<SeriesResult> {
  if (!event.seriesId || !event.seriesMode) {
    return { event }
  }

  const { seriesId, seriesMode, taskId } = event

  if (seriesMode === 'keep-all') {
    return { event }
  }

  if (seriesMode === 'accumulate') {
    const field = event.seriesAccField ?? 'delta'
    const accumulatedEvent = await store.accumulateSeries(taskId, seriesId, event, field)
    return { event, accumulatedEvent }
  }

  // 'latest' is the only remaining case; 'keep-all' and 'accumulate' return early above
  await store.replaceLastSeriesEvent(taskId, seriesId, event)
  return { event }
}

/**
 * Collapse accumulate-mode series events into single snapshot events.
 * Used by history endpoint and SSE late-join replay.
 *
 * @param events - Array of events to collapse
 * @param getSeriesLatest - Callback to get latest accumulated value for a series.
 *   For hot tasks, pass engine.getSeriesLatest. If returns null (cold task),
 *   falls back to last event in the events array for that series.
 */
export async function collapseAccumulateSeries(
  events: TaskEvent[],
  getSeriesLatest: (taskId: string, seriesId: string) => Promise<TaskEvent | null>,
): Promise<TaskEvent[]> {
  const accSeriesIds = new Set<string>()
  for (const e of events) {
    if (e.seriesMode === 'accumulate' && e.seriesId) {
      accSeriesIds.add(e.seriesId)
    }
  }

  if (accSeriesIds.size === 0 || events.length === 0) return events

  // Resolve snapshots for each accumulate series
  const snapshots = new Map<string, TaskEvent>()
  const taskId = events[0]!.taskId
  for (const sid of accSeriesIds) {
    const latest = await getSeriesLatest(taskId, sid)
    if (latest) {
      snapshots.set(sid, { ...latest, seriesSnapshot: true })
    } else {
      // Cold path: derive from last event in this series
      for (let i = events.length - 1; i >= 0; i--) {
        const evt = events[i]!
        if (evt.seriesId === sid) {
          snapshots.set(sid, { ...evt, seriesSnapshot: true })
          break
        }
      }
    }
  }

  // Replace series events with snapshots (first occurrence only)
  const emitted = new Set<string>()
  const result: TaskEvent[] = []
  for (const event of events) {
    if (event.seriesMode === 'accumulate' && event.seriesId && accSeriesIds.has(event.seriesId)) {
      if (!emitted.has(event.seriesId)) {
        const snapshot = snapshots.get(event.seriesId)
        if (snapshot) {
          result.push(snapshot)
          emitted.add(event.seriesId)
        }
      }
      // Skip remaining events in this accumulate series
    } else {
      result.push(event)
    }
  }

  return result
}
