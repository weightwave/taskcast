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
