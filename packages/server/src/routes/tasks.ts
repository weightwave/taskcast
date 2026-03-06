import type { Hono } from 'hono'
import { OpenAPIHono, createRoute, z } from '@hono/zod-openapi'
import type { Context } from 'hono'
import { checkScope } from '../auth.js'
import { getSubscriberCount } from './sse.js'
import type { SubscriberCounts } from './sse.js'
import {
  CreateTaskSchema,
  PublishEventSchema,
  TransitionSchema,
  TaskSchema,
  TaskEventSchema,
  ErrorSchema,
} from '../schemas.js'
import type { TaskEngine, CreateTaskInput, PublishEventInput, SinceCursor, TaskError, BlockedRequest, TaskFilter, TaskStatus } from '@taskcast/core'

// ─── Route Definitions ─────────────────────────────────────────────────────

const createTaskRoute = createRoute({
  method: 'post',
  path: '/',
  tags: ['Tasks'],
  summary: 'Create a new task',
  security: [{ Bearer: [] }],
  request: {
    body: { content: { 'application/json': { schema: CreateTaskSchema } } },
  },
  responses: {
    201: { description: 'Task created', content: { 'application/json': { schema: TaskSchema } } },
    400: { description: 'Validation error', content: { 'application/json': { schema: ErrorSchema } } },
    403: { description: 'Forbidden', content: { 'application/json': { schema: ErrorSchema } } },
  },
})

const listTasksRoute = createRoute({
  method: 'get',
  path: '/',
  tags: ['Tasks'],
  summary: 'List tasks',
  description: 'List tasks with optional status and type filters.',
  security: [{ Bearer: [] }],
  request: {
    query: z.object({
      status: z.string().optional().openapi({ description: 'Comma-separated status filter' }),
      type: z.string().optional().openapi({ description: 'Task type filter' }),
    }),
  },
  responses: {
    200: { description: 'Task list', content: { 'application/json': { schema: z.object({ tasks: z.array(TaskSchema) }) } } },
    403: { description: 'Forbidden', content: { 'application/json': { schema: ErrorSchema } } },
  },
})

const getTaskRoute = createRoute({
  method: 'get',
  path: '/{taskId}',
  tags: ['Tasks'],
  summary: 'Get task by ID',
  security: [{ Bearer: [] }],
  request: {
    params: z.object({ taskId: z.string() }),
  },
  responses: {
    200: { description: 'Task details', content: { 'application/json': { schema: TaskSchema } } },
    404: { description: 'Task not found', content: { 'application/json': { schema: ErrorSchema } } },
    403: { description: 'Forbidden', content: { 'application/json': { schema: ErrorSchema } } },
  },
})

const transitionRoute = createRoute({
  method: 'patch',
  path: '/{taskId}/status',
  tags: ['Tasks'],
  summary: 'Transition task status',
  security: [{ Bearer: [] }],
  request: {
    params: z.object({ taskId: z.string() }),
    body: { content: { 'application/json': { schema: TransitionSchema } } },
  },
  responses: {
    200: { description: 'Updated task', content: { 'application/json': { schema: TaskSchema } } },
    400: { description: 'Invalid transition', content: { 'application/json': { schema: ErrorSchema } } },
    404: { description: 'Task not found', content: { 'application/json': { schema: ErrorSchema } } },
    403: { description: 'Forbidden', content: { 'application/json': { schema: ErrorSchema } } },
  },
})

const publishEventsRoute = createRoute({
  method: 'post',
  path: '/{taskId}/events',
  tags: ['Events'],
  summary: 'Publish events to a task',
  description: 'Supports single event or batch (array) publishing.',
  security: [{ Bearer: [] }],
  request: {
    params: z.object({ taskId: z.string() }),
    body: { content: { 'application/json': { schema: z.union([PublishEventSchema, z.array(PublishEventSchema)]) } } },
  },
  responses: {
    201: { description: 'Events published', content: { 'application/json': { schema: z.union([TaskEventSchema, z.array(TaskEventSchema)]) } } },
    400: { description: 'Validation error', content: { 'application/json': { schema: ErrorSchema } } },
    404: { description: 'Task not found', content: { 'application/json': { schema: ErrorSchema } } },
    403: { description: 'Forbidden', content: { 'application/json': { schema: ErrorSchema } } },
  },
})

const eventHistoryRoute = createRoute({
  method: 'get',
  path: '/{taskId}/events/history',
  tags: ['Events'],
  summary: 'Query event history',
  security: [{ Bearer: [] }],
  request: {
    params: z.object({ taskId: z.string() }),
    query: z.object({
      'since.id': z.string().optional(),
      'since.index': z.string().optional(),
      'since.timestamp': z.string().optional(),
      types: z.string().optional(),
      levels: z.string().optional(),
    }),
  },
  responses: {
    200: { description: 'Event list', content: { 'application/json': { schema: z.array(TaskEventSchema) } } },
    404: { description: 'Task not found', content: { 'application/json': { schema: ErrorSchema } } },
    403: { description: 'Forbidden', content: { 'application/json': { schema: ErrorSchema } } },
  },
})

// ─── Router Factory ────────────────────────────────────────────────────────

// Engine types (Task, TaskEvent) are structurally compatible with the Zod schema
// output types but not identical. @hono/zod-openapi enforces strict return-type
// checking that cannot be satisfied without duplicating core types as Zod schemas.
// We bind openapi() with a relaxed signature to retain runtime route-spec
// registration and request validation while avoiding redundant type gymnastics.
// eslint-disable-next-line @typescript-eslint/no-explicit-any
type OpenAPIRegister = (route: any, handler: (c: Context) => Promise<Response>) => void

export function createTasksRouter(engine: TaskEngine, subscriberCounts: SubscriberCounts): Hono {
  const router = new OpenAPIHono()
  const register = router.openapi.bind(router) as OpenAPIRegister

  register(createTaskRoute, async (c) => {
    const auth = c.get('auth')
    if (!checkScope(auth, 'task:create')) return c.json({ error: 'Forbidden' }, 403)

    const body = await c.req.json()
    const parsed = CreateTaskSchema.safeParse(body)
    if (!parsed.success) return c.json({ error: parsed.error.flatten() }, 400)

    const d = parsed.data
    const input: CreateTaskInput = {}
    if (d.id !== undefined) input.id = d.id
    if (d.type !== undefined) input.type = d.type
    if (d.params !== undefined) input.params = d.params
    if (d.metadata !== undefined) input.metadata = d.metadata
    if (d.ttl !== undefined) input.ttl = d.ttl
    if (d.tags !== undefined) input.tags = d.tags
    if (d.assignMode !== undefined) input.assignMode = d.assignMode
    if (d.cost !== undefined) input.cost = d.cost
    if (d.disconnectPolicy !== undefined) input.disconnectPolicy = d.disconnectPolicy
    if (d.webhooks !== undefined) input.webhooks = d.webhooks as CreateTaskInput['webhooks']
    if (d.cleanup !== undefined) input.cleanup = d.cleanup as CreateTaskInput['cleanup']
    if (d.authConfig !== undefined) input.authConfig = d.authConfig as unknown as CreateTaskInput['authConfig']

    const task = await engine.createTask(input)
    return c.json(task, 201)
  })

  register(listTasksRoute, async (c) => {
    const auth = c.get('auth')
    if (!checkScope(auth, 'event:subscribe')) return c.json({ error: 'Forbidden' }, 403)

    const status = c.req.query('status')
    const type = c.req.query('type')

    const filter: TaskFilter = {}
    if (status) filter.status = status.split(',').filter(Boolean) as TaskStatus[]
    if (type) filter.types = [type]

    const tasks = await engine.listTasks(filter)
    const enriched = tasks.map(t => ({
      ...t,
      hot: true,
      subscriberCount: getSubscriberCount(subscriberCounts, t.id),
    }))

    return c.json({ tasks: enriched })
  })

  register(getTaskRoute, async (c) => {
    const taskId = c.req.param('taskId') as string
    const auth = c.get('auth')
    if (!checkScope(auth, 'event:subscribe', taskId)) return c.json({ error: 'Forbidden' }, 403)

    const task = await engine.getTask(taskId)
    if (!task) return c.json({ error: 'Task not found' }, 404)
    return c.json({ ...task, hot: true, subscriberCount: getSubscriberCount(subscriberCounts, taskId) })
  })

  register(transitionRoute, async (c) => {
    const taskId = c.req.param('taskId') as string
    const auth = c.get('auth')
    if (!checkScope(auth, 'task:manage', taskId)) return c.json({ error: 'Forbidden' }, 403)

    const body = await c.req.json()
    const parsed = TransitionSchema.safeParse(body)
    if (!parsed.success) return c.json({ error: parsed.error.flatten() }, 400)

    try {
      const payload: {
        result?: Record<string, unknown>
        error?: TaskError
        reason?: string
        ttl?: number
        resumeAfterMs?: number
        blockedRequest?: BlockedRequest
      } = {}
      if (parsed.data.result !== undefined) payload.result = parsed.data.result
      if (parsed.data.error !== undefined) {
        const e = parsed.data.error
        const taskError: TaskError = { message: e.message }
        if (e.code !== undefined) taskError.code = e.code
        if (e.details !== undefined) taskError.details = e.details
        payload.error = taskError
      }
      if (parsed.data.reason !== undefined) payload.reason = parsed.data.reason
      if (parsed.data.ttl !== undefined) payload.ttl = parsed.data.ttl
      if (parsed.data.resumeAfterMs !== undefined) payload.resumeAfterMs = parsed.data.resumeAfterMs
      if (parsed.data.blockedRequest !== undefined) {
        payload.blockedRequest = {
          type: parsed.data.blockedRequest.type,
          data: parsed.data.blockedRequest.data,
        }
      }
      const task = await engine.transitionTask(taskId, parsed.data.status, payload)
      return c.json(task)
    } catch (err) {
      const msg = err instanceof Error ? err.message : String(err)
      if (msg.toLowerCase().includes('not found')) return c.json({ error: msg }, 404)
      return c.json({ error: msg }, 400)
    }
  })

  register(publishEventsRoute, async (c) => {
    const taskId = c.req.param('taskId') as string
    const auth = c.get('auth')
    if (!checkScope(auth, 'event:publish', taskId)) return c.json({ error: 'Forbidden' }, 403)

    const body = await c.req.json()
    const isBatch = Array.isArray(body)
    const inputs = isBatch ? body : [body]

    const events = []
    for (const input of inputs) {
      const parsed = PublishEventSchema.safeParse(input)
      if (!parsed.success) return c.json({ error: parsed.error.flatten() }, 400)
      try {
        const d = parsed.data
        const eventInput: PublishEventInput = { type: d.type, level: d.level, data: d.data }
        if (d.seriesId !== undefined) eventInput.seriesId = d.seriesId
        if (d.seriesMode !== undefined) eventInput.seriesMode = d.seriesMode
        if (d.seriesAccField !== undefined) eventInput.seriesAccField = d.seriesAccField
        const event = await engine.publishEvent(taskId, eventInput)
        events.push(event)
      } catch (err) {
        const msg = err instanceof Error ? err.message : String(err)
        if (msg.toLowerCase().includes('not found')) return c.json({ error: msg }, 404)
        return c.json({ error: msg }, 400)
      }
    }

    return c.json(isBatch ? events : events[0], 201)
  })

  register(eventHistoryRoute, async (c) => {
    const taskId = c.req.param('taskId') as string
    const auth = c.get('auth')
    if (!checkScope(auth, 'event:history', taskId)) return c.json({ error: 'Forbidden' }, 403)

    const task = await engine.getTask(taskId)
    if (!task) return c.json({ error: 'Task not found' }, 404)

    const sinceIndex = c.req.query('since.index')
    const sinceTimestamp = c.req.query('since.timestamp')
    const sinceId = c.req.query('since.id')

    let since: SinceCursor | undefined
    if (sinceId !== undefined || sinceIndex !== undefined || sinceTimestamp !== undefined) {
      since = {}
      if (sinceId !== undefined) since.id = sinceId
      if (sinceIndex !== undefined) since.index = Number(sinceIndex)
      if (sinceTimestamp !== undefined) since.timestamp = Number(sinceTimestamp)
    }

    const events = await engine.getEvents(taskId, since !== undefined ? { since } : undefined)
    return c.json(events)
  })

  // ─── POST /tasks/:taskId/resolve — Resolve a blocked task ─────────────────
  router.post('/:taskId/resolve', async (c: Context) => {
    const taskId = c.req.param('taskId') as string
    const auth = c.get('auth')
    if (!checkScope(auth, 'task:resolve', taskId)) return c.json({ error: 'Forbidden' }, 403)

    const task = await engine.getTask(taskId)
    if (!task) return c.json({ error: 'Task not found' }, 404)
    if (task.status !== 'blocked') return c.json({ error: 'Task is not blocked' }, 400)

    const body = await c.req.json()
    const schema = z.object({ data: z.unknown() })
    const parsed = schema.safeParse(body)
    if (!parsed.success) return c.json({ error: parsed.error.flatten() }, 400)

    try {
      const updated = await engine.transitionTask(taskId, 'running', {
        result: typeof parsed.data.data === 'object' && parsed.data.data !== null
          ? parsed.data.data as Record<string, unknown>
          : { resolution: parsed.data.data },
      })
      return c.json(updated)
    } catch (err) {
      const msg = err instanceof Error ? err.message : String(err)
      return c.json({ error: msg }, 400)
    }
  })

  // ─── GET /tasks/:taskId/request — Get blocked request ────────────────────
  router.get('/:taskId/request', async (c: Context) => {
    const taskId = c.req.param('taskId') as string
    const auth = c.get('auth')
    if (!checkScope(auth, 'task:resolve', taskId)) return c.json({ error: 'Forbidden' }, 403)

    const task = await engine.getTask(taskId)
    if (!task) return c.json({ error: 'Task not found' }, 404)
    if (task.status !== 'blocked' || !task.blockedRequest) {
      return c.json({ error: 'No blocked request' }, 404)
    }

    return c.json(task.blockedRequest)
  })

  return router as unknown as Hono
}
