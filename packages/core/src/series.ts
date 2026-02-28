import type { TaskEvent, ShortTermStore } from './types.js'

export async function processSeries(
  event: TaskEvent,
  store: ShortTermStore,
): Promise<TaskEvent> {
  if (!event.seriesId || !event.seriesMode) {
    return event
  }

  const { seriesId, seriesMode, taskId } = event

  if (seriesMode === 'keep-all') {
    return event
  }

  if (seriesMode === 'accumulate') {
    const prev = await store.getSeriesLatest(taskId, seriesId)
    let merged = event

    if (prev !== null) {
      const prevData = (typeof prev.data === 'object' && prev.data !== null)
        ? prev.data as Record<string, unknown>
        : {}
      const newData = (typeof event.data === 'object' && event.data !== null)
        ? event.data as Record<string, unknown>
        : {}
      if (typeof prevData['text'] === 'string' && typeof newData['text'] === 'string') {
        merged = {
          ...event,
          data: { ...newData, text: prevData['text'] + newData['text'] },
        }
      }
    }

    await store.setSeriesLatest(taskId, seriesId, merged)
    return merged
  }

  if (seriesMode === 'latest') {
    await store.replaceLastSeriesEvent(taskId, seriesId, event)
    return event
  }

  return event
}
