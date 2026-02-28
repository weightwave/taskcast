import { Hono } from 'hono'
import { z } from 'zod'
import { checkScope } from '../auth.js'
import type { TaskEngine, CreateTaskInput, PublishEventInput, SinceCursor, TaskError } from '@taskcast/core'

const CreateTaskSchema = z.object({
  id: z.string().optional(),
  type: z.string().optional(),
  params: z.record(z.unknown()).optional(),
  metadata: z.record(z.unknown()).optional(),
  ttl: z.number().int().positive().optional(),
  webhooks: z.array(z.unknown()).optional(),
  cleanup: z.object({ rules: z.array(z.unknown()) }).optional(),
})

const PublishEventSchema = z.object({
  type: z.string(),
  level: z.enum(['debug', 'info', 'warn', 'error']),
  data: z.unknown(),
  seriesId: z.string().optional(),
  seriesMode: z.enum(['keep-all', 'accumulate', 'latest']).optional(),
})

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
    const schema = z.object({
      status: z.enum(['running', 'completed', 'failed', 'timeout', 'cancelled']),
      result: z.record(z.unknown()).optional(),
      error: z.object({
        code: z.string().optional(),
        message: z.string(),
        details: z.record(z.unknown()).optional(),
      }).optional(),
    })
    const parsed = schema.safeParse(body)
    if (!parsed.success) return c.json({ error: parsed.error.flatten() }, 400)

    try {
      const payload: { result?: Record<string, unknown>; error?: TaskError } = {}
      if (parsed.data.result !== undefined) payload.result = parsed.data.result
      if (parsed.data.error !== undefined) {
        const e = parsed.data.error
        const taskError: TaskError = { message: e.message }
        if (e.code !== undefined) taskError.code = e.code
        if (e.details !== undefined) taskError.details = e.details
        payload.error = taskError
      }
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
