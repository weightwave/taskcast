import { Hono } from 'hono'
import { streamSSE } from 'hono/streaming'
import { applyFilteredIndex, matchesFilter } from '@taskcast/core'
import { checkScope } from '../auth.js'
import type { TaskEngine, TaskEvent, SubscribeFilter, SSEEnvelope, Level } from '@taskcast/core'

function parseFilter(query: Record<string, string | undefined>): SubscribeFilter {
  const get = (k: string) => query[k]
  const filter: SubscribeFilter = {}

  const types = get('types')
  if (types !== undefined) filter.types = types.split(',').filter(Boolean)

  const levels = get('levels')
  if (levels !== undefined) filter.levels = levels.split(',').filter(Boolean) as Level[]

  const includeStatus = get('includeStatus')
  if (includeStatus !== undefined) filter.includeStatus = includeStatus !== 'false'

  const wrap = get('wrap')
  if (wrap !== undefined) filter.wrap = wrap !== 'false'

  const sinceId = get('since.id')
  const sinceIndex = get('since.index')
  const sinceTimestamp = get('since.timestamp')
  if (sinceId !== undefined || sinceIndex !== undefined || sinceTimestamp !== undefined) {
    const since: SubscribeFilter['since'] = {}
    if (sinceId !== undefined) since.id = sinceId
    if (sinceIndex !== undefined) since.index = Number(sinceIndex)
    if (sinceTimestamp !== undefined) since.timestamp = Number(sinceTimestamp)
    filter.since = since
  }

  return filter
}

function toEnvelope(event: TaskEvent, filteredIndex: number): SSEEnvelope {
  const env: SSEEnvelope = {
    filteredIndex,
    rawIndex: event.index,
    eventId: event.id,
    taskId: event.taskId,
    type: event.type,
    timestamp: event.timestamp,
    level: event.level,
    data: event.data,
  }
  if (event.seriesId !== undefined) env.seriesId = event.seriesId
  if (event.seriesMode !== undefined) env.seriesMode = event.seriesMode
  return env
}

const TERMINAL = new Set(['completed', 'failed', 'timeout', 'cancelled'])

export function createSSERouter(engine: TaskEngine) {
  const router = new Hono()

  router.get('/:taskId/events', async (c) => {
    const { taskId } = c.req.param()
    const auth = c.get('auth')
    if (!checkScope(auth, 'event:subscribe', taskId)) return c.json({ error: 'Forbidden' }, 403)

    const task = await engine.getTask(taskId)
    if (!task) return c.json({ error: 'Task not found' }, 404)

    const filter = parseFilter(c.req.query() as Record<string, string | undefined>)
    const wrap = filter.wrap !== false // default true

    return streamSSE(c, async (stream) => {
      const sendEvent = async (event: TaskEvent, filteredIndex: number) => {
        const payload = wrap ? toEnvelope(event, filteredIndex) : event
        await stream.writeSSE({
          event: 'taskcast.event',
          data: JSON.stringify(payload),
          id: event.id,
        })
      }

      const sendDone = async (reason: string) => {
        await stream.writeSSE({
          event: 'taskcast.done',
          data: JSON.stringify({ reason }),
        })
      }

      // Replay history
      const history = await engine.getEvents(taskId)
      const filtered = applyFilteredIndex(history, filter)
      for (const { event, filteredIndex } of filtered) {
        await sendEvent(event, filteredIndex)
      }

      // If task is already terminal, send done and close
      if (TERMINAL.has(task.status)) {
        await sendDone(task.status)
        return
      }

      // Subscribe to live events
      let nextFilteredIndex = filtered.length > 0
        ? (filtered[filtered.length - 1]!.filteredIndex + 1)
        : 0

      await new Promise<void>((resolve) => {
        const unsub = engine.subscribe(taskId, async (event) => {
          if (!matchesFilter(event, filter)) return
          await sendEvent(event, nextFilteredIndex++)

          if (event.type === 'taskcast:status') {
            const status = (event.data as { status: string }).status
            if (TERMINAL.has(status)) {
              await sendDone(status)
              unsub()
              resolve()
            }
          }
        })

        stream.onAbort(() => {
          unsub()
          resolve()
        })
      })
    })
  })

  return router
}
