import type { Hono } from 'hono'
import { OpenAPIHono, createRoute, z } from '@hono/zod-openapi'
import type { Context } from 'hono'
import { streamSSE } from 'hono/streaming'
import { applyFilteredIndex, matchesFilter, matchesType, TERMINAL_STATUSES } from '@taskcast/core'
import { checkScope } from '../auth.js'
import { ErrorSchema } from '../schemas.js'
import type { TaskEngine, TaskEvent, Task, SubscribeFilter, SSEEnvelope, Level } from '@taskcast/core'

// ─── Subscriber Tracking ─────────────────────────────────────────────────────

export type SubscriberCounts = Map<string, number>

export function createSubscriberCounts(): SubscriberCounts {
  return new Map<string, number>()
}

export function getSubscriberCount(counts: SubscriberCounts, taskId: string): number {
  return counts.get(taskId) ?? 0
}

function incrementSubscriberCount(counts: SubscriberCounts, taskId: string): void {
  counts.set(taskId, (counts.get(taskId) ?? 0) + 1)
}

function decrementSubscriberCount(counts: SubscriberCounts, taskId: string): void {
  const count = (counts.get(taskId) ?? 1) - 1
  if (count <= 0) {
    counts.delete(taskId)
  } else {
    counts.set(taskId, count)
  }
}

// ─── Route Definition ──────────────────────────────────────────────────────

const sseRoute = createRoute({
  method: 'get',
  path: '/{taskId}/events',
  tags: ['Events'],
  summary: 'Subscribe to task events via SSE',
  description: 'Server-Sent Events stream. Replays history then streams live. Closes on terminal status.',
  security: [{ Bearer: [] }],
  request: {
    params: z.object({ taskId: z.string() }),
    query: z.object({
      types: z.string().optional().openapi({ description: 'Comma-separated type filter with wildcard support' }),
      levels: z.string().optional().openapi({ description: 'Comma-separated level filter' }),
      includeStatus: z.string().optional().openapi({ description: 'Include taskcast:status events (default: true)' }),
      wrap: z.string().optional().openapi({ description: 'Wrap in SSEEnvelope (default: true)' }),
      seriesFormat: z.string().optional().openapi({ description: 'Series format: delta (default) or accumulated' }),
      'since.id': z.string().optional(),
      'since.index': z.string().optional(),
      'since.timestamp': z.string().optional(),
    }),
  },
  responses: {
    200: { description: 'SSE event stream (text/event-stream)' },
    403: { description: 'Forbidden', content: { 'application/json': { schema: ErrorSchema } } },
    404: { description: 'Task not found', content: { 'application/json': { schema: ErrorSchema } } },
  },
})

// ─── Helpers ───────────────────────────────────────────────────────────────

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

  const seriesFormat = get('seriesFormat')
  if (seriesFormat === 'delta' || seriesFormat === 'accumulated') {
    filter.seriesFormat = seriesFormat
  }

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
  if (event.seriesAccField !== undefined) env.seriesAccField = event.seriesAccField
  if (event.seriesSnapshot !== undefined) env.seriesSnapshot = event.seriesSnapshot
  return env
}

const TERMINAL: Set<string> = new Set(TERMINAL_STATUSES)

// ─── Router Factory ────────────────────────────────────────────────────────

// eslint-disable-next-line @typescript-eslint/no-explicit-any
type OpenAPIRegister = (route: any, handler: (c: Context) => Promise<Response>) => void

export function createSSERouter(engine: TaskEngine, subscriberCounts: SubscriberCounts): Hono {
  const router = new OpenAPIHono()
  const register = router.openapi.bind(router) as OpenAPIRegister

  register(sseRoute, async (c) => {
    const taskId = c.req.param('taskId') as string
    const auth = c.get('auth')
    if (!checkScope(auth, 'event:subscribe', taskId)) return c.json({ error: 'Forbidden' }, 403)

    const task = await engine.getTask(taskId)
    if (!task) return c.json({ error: 'Task not found' }, 404)

    const filter = parseFilter(c.req.query() as Record<string, string | undefined>)
    const wrap = filter.wrap !== false // default true

    return streamSSE(c, async (stream) => {
      incrementSubscriberCount(subscriberCounts, taskId)
      let decremented = false
      const cleanup = () => {
        if (!decremented) { decremented = true; decrementSubscriberCount(subscriberCounts, taskId) }
      }

      const seriesFormat = filter.seriesFormat ?? 'delta'

      const sendEvent = async (event: TaskEvent, filteredIndex: number) => {
        let eventToSend = event
        if (seriesFormat === 'accumulated' && event._accumulatedData !== undefined) {
          eventToSend = { ...event, data: event._accumulatedData }
        }
        // Strip transient field
        const { _accumulatedData: _, ...cleanEvent } = eventToSend as TaskEvent & { _accumulatedData?: unknown }
        const payload = wrap ? toEnvelope(cleanEvent as TaskEvent, filteredIndex) : cleanEvent
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
      let history: TaskEvent[]
      try {
        history = await engine.getEvents(taskId)
      } catch {
        cleanup()
        return
      }

      const hasSinceCursor = !!filter.since
      const accumulateSeriesIds = new Set<string>()
      for (const e of history) {
        if (e.seriesMode === 'accumulate' && e.seriesId) {
          accumulateSeriesIds.add(e.seriesId)
        }
      }

      let replayEvents: TaskEvent[]

      if (accumulateSeriesIds.size > 0 && !hasSinceCursor) {
        // Collapse accumulate series into snapshots
        const snapshots = new Map<string, TaskEvent>()
        for (const sid of accumulateSeriesIds) {
          const latest = await engine.getSeriesLatest(taskId, sid)
          if (latest) {
            snapshots.set(sid, { ...latest, seriesSnapshot: true })
          }
        }

        const emittedSnapshots = new Set<string>()
        replayEvents = []
        for (const event of history) {
          if (event.seriesMode === 'accumulate' && event.seriesId && accumulateSeriesIds.has(event.seriesId)) {
            if (!emittedSnapshots.has(event.seriesId)) {
              const snapshot = snapshots.get(event.seriesId)
              if (snapshot) {
                replayEvents.push(snapshot)
                emittedSnapshots.add(event.seriesId)
              }
            }
            // Skip remaining events in this accumulate series
          } else {
            replayEvents.push(event)
          }
        }
      } else {
        replayEvents = history
      }

      const filtered = applyFilteredIndex(replayEvents, filter)
      for (const { event, filteredIndex } of filtered) {
        await sendEvent(event, filteredIndex)
      }

      // If task is already terminal, send done and close
      if (TERMINAL.has(task.status)) {
        await sendDone(task.status)
        cleanup()
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
              cleanup()
              unsub()
              resolve()
            }
          }
        })

        stream.onAbort(() => {
          cleanup()
          unsub()
          resolve()
        })
      })
    })
  })

  return router as unknown as Hono
}

// ─── Global SSE Route ─────────────────────────────────────────────────────

const globalSSERoute = createRoute({
  method: 'get',
  path: '/',
  tags: ['Events'],
  summary: 'Subscribe to events from all tasks via SSE',
  description: 'Global SSE stream. Streams events from all tasks created after the connection is established. Runs indefinitely until client disconnects.',
  security: [{ Bearer: [] }],
  request: {
    query: z.object({
      types: z.string().optional().openapi({ description: 'Comma-separated type filter with wildcard support' }),
      levels: z.string().optional().openapi({ description: 'Comma-separated level filter' }),
    }),
  },
  responses: {
    200: { description: 'SSE event stream (text/event-stream)' },
    403: { description: 'Forbidden', content: { 'application/json': { schema: ErrorSchema } } },
  },
})

export function createGlobalSSERoute(engine: TaskEngine): Hono {
  const router = new OpenAPIHono()
  const register = router.openapi.bind(router) as OpenAPIRegister

  register(globalSSERoute, async (c) => {
    const auth = c.get('auth')
    if (!checkScope(auth, 'event:subscribe')) return c.json({ error: 'Forbidden' }, 403)

    const typesParam = c.req.query('types')
    const levelsParam = c.req.query('levels')
    const types = typesParam ? typesParam.split(',').filter(Boolean) : undefined
    const levels = levelsParam ? levelsParam.split(',').filter(Boolean) as Level[] : undefined

    return streamSSE(c, async (stream) => {
      const unsubscribes: Array<() => void> = []
      let closed = false

      const creationListener = (task: Task) => {
        const unsub = engine.subscribe(task.id, (event: TaskEvent) => {
          if (closed) return
          if (types && !matchesType(event.type, types)) return
          if (levels && !levels.includes(event.level as Level)) return

          const envelope: SSEEnvelope = {
            filteredIndex: 0,
            rawIndex: event.index,
            eventId: event.id,
            taskId: event.taskId,
            type: event.type,
            timestamp: event.timestamp,
            level: event.level,
            data: event.data,
          }
          if (event.seriesId !== undefined) envelope.seriesId = event.seriesId
          if (event.seriesMode !== undefined) envelope.seriesMode = event.seriesMode
          if (event.seriesAccField !== undefined) envelope.seriesAccField = event.seriesAccField

          stream.writeSSE({
            event: 'taskcast.event',
            data: JSON.stringify(envelope),
            id: event.id,
          }).catch(() => { /* stream may have closed */ })
        })
        unsubscribes.push(unsub)
      }

      engine.addCreationListener(creationListener)

      stream.onAbort(() => {
        closed = true
        for (const unsub of unsubscribes) unsub()
        engine.removeCreationListener(creationListener)
      })

      // Keep alive — send comment periodically to detect client disconnection
      while (!closed) {
        await stream.write(':keepalive\n\n')
        await new Promise((r) => setTimeout(r, 30000))
      }
    })
  })

  return router as unknown as Hono
}
