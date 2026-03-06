import type { Hono } from 'hono'
import { OpenAPIHono, createRoute, z } from '@hono/zod-openapi'
import type { Context } from 'hono'
import { checkScope } from '../auth.js'
import { DeclineSchema, WorkerSchema, TaskSchema, ErrorSchema } from '../schemas.js'
import type { WorkerManager, TaskEngine } from '@taskcast/core'

export { WorkerWSHandler, WorkerWSRegistry } from './worker-ws.js'
export type { WSLike, TaskSummary } from './worker-ws.js'

// ─── Route Definitions ─────────────────────────────────────────────────────

const listWorkersRoute = createRoute({
  method: 'get',
  path: '/',
  tags: ['Workers'],
  summary: 'List all workers',
  security: [{ Bearer: [] }],
  responses: {
    200: { description: 'Worker list', content: { 'application/json': { schema: z.array(WorkerSchema) } } },
    403: { description: 'Forbidden', content: { 'application/json': { schema: ErrorSchema } } },
  },
})

const pullTaskRoute = createRoute({
  method: 'get',
  path: '/pull',
  tags: ['Workers'],
  summary: 'Long-poll for a task assignment',
  description: 'Worker long-polls to receive a task assignment. Returns 204 on timeout.',
  security: [{ Bearer: [] }],
  request: {
    query: z.object({
      workerId: z.string().optional().openapi({ description: 'Worker identifier (required)' }),
      weight: z.string().optional().openapi({ description: 'Worker weight' }),
      timeout: z.string().optional().openapi({ description: 'Long-poll timeout in ms (default: 30000)' }),
    }),
  },
  responses: {
    200: { description: 'Assigned task', content: { 'application/json': { schema: TaskSchema } } },
    204: { description: 'No task available (timeout)' },
    400: { description: 'Missing workerId', content: { 'application/json': { schema: ErrorSchema } } },
    403: { description: 'Forbidden', content: { 'application/json': { schema: ErrorSchema } } },
  },
})

const getWorkerRoute = createRoute({
  method: 'get',
  path: '/{workerId}',
  tags: ['Workers'],
  summary: 'Get worker by ID',
  security: [{ Bearer: [] }],
  request: {
    params: z.object({ workerId: z.string() }),
  },
  responses: {
    200: { description: 'Worker details', content: { 'application/json': { schema: WorkerSchema } } },
    404: { description: 'Worker not found', content: { 'application/json': { schema: ErrorSchema } } },
    403: { description: 'Forbidden', content: { 'application/json': { schema: ErrorSchema } } },
  },
})

const deleteWorkerRoute = createRoute({
  method: 'delete',
  path: '/{workerId}',
  tags: ['Workers'],
  summary: 'Force disconnect a worker',
  security: [{ Bearer: [] }],
  request: {
    params: z.object({ workerId: z.string() }),
  },
  responses: {
    204: { description: 'Worker removed' },
    404: { description: 'Worker not found', content: { 'application/json': { schema: ErrorSchema } } },
    403: { description: 'Forbidden', content: { 'application/json': { schema: ErrorSchema } } },
  },
})

const declineTaskRoute = createRoute({
  method: 'post',
  path: '/tasks/{taskId}/decline',
  tags: ['Workers'],
  summary: 'Worker declines a task assignment',
  security: [{ Bearer: [] }],
  request: {
    params: z.object({ taskId: z.string() }),
    body: { content: { 'application/json': { schema: DeclineSchema } } },
  },
  responses: {
    200: { description: 'Task declined', content: { 'application/json': { schema: z.object({ ok: z.boolean() }) } } },
    400: { description: 'Validation error', content: { 'application/json': { schema: ErrorSchema } } },
    403: { description: 'Forbidden', content: { 'application/json': { schema: ErrorSchema } } },
  },
})

// ─── Router Factory ────────────────────────────────────────────────────────

// eslint-disable-next-line @typescript-eslint/no-explicit-any
type OpenAPIRegister = (route: any, handler: (c: Context) => Promise<Response>) => void

export function createWorkersRouter(manager: WorkerManager, engine: TaskEngine): Hono {
  const router = new OpenAPIHono()
  const register = router.openapi.bind(router) as OpenAPIRegister

  // GET / — list all workers
  register(listWorkersRoute, async (c) => {
    const auth = c.get('auth')
    if (!checkScope(auth, 'worker:manage')) return c.json({ error: 'Forbidden' }, 403)
    const workers = await manager.listWorkers()
    return c.json(workers)
  })

  // GET /pull — long-poll for task (worker:connect scope)
  // Must be registered before /{workerId} to avoid being matched as a param
  register(pullTaskRoute, async (c) => {
    const auth = c.get('auth')
    if (!checkScope(auth, 'worker:connect')) return c.json({ error: 'Forbidden' }, 403)
    const workerId = c.req.query('workerId')
    if (!workerId) return c.json({ error: 'workerId query param required' }, 400)
    if (auth.workerId && auth.workerId !== workerId) {
      return c.json({ error: 'Forbidden: worker ID mismatch' }, 403)
    }
    const weight = c.req.query('weight')
    if (weight) await manager.updateWorker(workerId, { weight: Number(weight) })
    await manager.heartbeat(workerId)
    try {
      const controller = new AbortController()
      const timeoutMs = Number(c.req.query('timeout') ?? 30000)
      const timeout = setTimeout(() => controller.abort(), timeoutMs)
      c.req.raw.signal.addEventListener('abort', () => {
        clearTimeout(timeout)
        controller.abort()
      })
      const task = await manager.waitForTask(workerId, controller.signal)
      clearTimeout(timeout)
      return c.json(task)
    } catch {
      return c.body(null, 204)
    }
  })

  // GET /{workerId} — get single worker
  register(getWorkerRoute, async (c) => {
    const workerId = c.req.param('workerId') as string
    const auth = c.get('auth')
    if (!checkScope(auth, 'worker:manage')) return c.json({ error: 'Forbidden' }, 403)
    const worker = await manager.getWorker(workerId)
    if (!worker) return c.json({ error: 'Worker not found' }, 404)
    return c.json(worker)
  })

  // DELETE /{workerId} — force disconnect
  register(deleteWorkerRoute, async (c) => {
    const workerId = c.req.param('workerId') as string
    const auth = c.get('auth')
    if (!checkScope(auth, 'worker:manage')) return c.json({ error: 'Forbidden' }, 403)
    const worker = await manager.getWorker(workerId)
    if (!worker) return c.json({ error: 'Worker not found' }, 404)
    await manager.unregisterWorker(workerId)
    return c.body(null, 204)
  })

  // POST /tasks/{taskId}/decline — worker declines a task
  register(declineTaskRoute, async (c) => {
    const taskId = c.req.param('taskId') as string
    const auth = c.get('auth')
    if (!checkScope(auth, 'worker:connect')) return c.json({ error: 'Forbidden' }, 403)
    const body = await c.req.json()
    const parsed = DeclineSchema.safeParse(body)
    if (!parsed.success) return c.json({ error: parsed.error.flatten() }, 400)
    if (auth.workerId && auth.workerId !== parsed.data.workerId) {
      return c.json({ error: 'Forbidden: worker ID mismatch' }, 403)
    }
    const declineOpts = parsed.data.blacklist !== undefined ? { blacklist: parsed.data.blacklist } : {}
    await manager.declineTask(taskId, parsed.data.workerId, declineOpts)
    return c.json({ ok: true })
  })

  return router as unknown as Hono
}
