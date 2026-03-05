import { Hono } from 'hono'
import { checkScope } from '../auth.js'
import { CreateTaskSchema, PublishEventSchema, TransitionSchema } from '../schemas.js'
import type { TaskEngine, CreateTaskInput, PublishEventInput, SinceCursor, TaskError } from '@taskcast/core'

export function createTasksRouter(engine: TaskEngine) {
  const router = new Hono()

  router.post('/', async (c) => {
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

    const task = await engine.createTask(input)
    return c.json(task, 201)
  })

  router.get('/:taskId', async (c) => {
    const { taskId } = c.req.param()
    const auth = c.get('auth')
    if (!checkScope(auth, 'event:subscribe', taskId)) return c.json({ error: 'Forbidden' }, 403)

    const task = await engine.getTask(taskId)
    if (!task) return c.json({ error: 'Task not found' }, 404)
    return c.json(task)
  })

  router.patch('/:taskId/status', async (c) => {
    const { taskId } = c.req.param()
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
      const task = await engine.transitionTask(taskId, parsed.data.status, payload)
      return c.json(task)
    } catch (err) {
      const msg = err instanceof Error ? err.message : String(err)
      if (msg.toLowerCase().includes('not found')) return c.json({ error: msg }, 404)
      return c.json({ error: msg }, 400)
    }
  })

  router.post('/:taskId/events', async (c) => {
    const { taskId } = c.req.param()
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

  router.get('/:taskId/events/history', async (c) => {
    const { taskId } = c.req.param()
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

  return router
}
