import { Hono } from 'hono'
import { z } from 'zod'
import { checkScope } from '../auth.js'
import type { WorkerManager, TaskEngine } from '@taskcast/core'

export { WorkerWSHandler } from './worker-ws.js'
export type { WSLike, TaskSummary } from './worker-ws.js'

const DeclineSchema = z.object({
  workerId: z.string(),
  blacklist: z.boolean().optional(),
})

export function createWorkersRouter(manager: WorkerManager, engine: TaskEngine) {
  const router = new Hono()

  // GET / — list all workers
  router.get('/', async (c) => {
    const auth = c.get('auth')
    if (!checkScope(auth, 'worker:manage')) return c.json({ error: 'Forbidden' }, 403)
    const workers = await manager.listWorkers()
    return c.json(workers)
  })

  // GET /pull — long-poll for task (worker:connect scope)
  // Must be registered before /:workerId to avoid being matched as a param
  router.get('/pull', async (c) => {
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

  // GET /:workerId — get single worker
  router.get('/:workerId', async (c) => {
    const { workerId } = c.req.param()
    const auth = c.get('auth')
    if (!checkScope(auth, 'worker:manage')) return c.json({ error: 'Forbidden' }, 403)
    const worker = await manager.getWorker(workerId)
    if (!worker) return c.json({ error: 'Worker not found' }, 404)
    return c.json(worker)
  })

  // DELETE /:workerId — force disconnect
  router.delete('/:workerId', async (c) => {
    const { workerId } = c.req.param()
    const auth = c.get('auth')
    if (!checkScope(auth, 'worker:manage')) return c.json({ error: 'Forbidden' }, 403)
    const worker = await manager.getWorker(workerId)
    if (!worker) return c.json({ error: 'Worker not found' }, 404)
    await manager.unregisterWorker(workerId)
    return c.body(null, 204)
  })

  // POST /tasks/:taskId/decline — worker declines a task
  router.post('/tasks/:taskId/decline', async (c) => {
    const { taskId } = c.req.param()
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

  return router
}
