export { createAuthMiddleware, checkScope } from './auth.js'
export type { AuthConfig, AuthContext, JWTConfig } from './auth.js'
export { createTasksRouter } from './routes/tasks.js'
export { createSSERouter, createSubscriberCounts, getSubscriberCount } from './routes/sse.js'
export type { SubscriberCounts } from './routes/sse.js'
export { createWorkersRouter, WorkerWSHandler, WorkerWSRegistry } from './routes/workers.js'
export type { WSLike, TaskSummary } from './routes/workers.js'
export { createAdminRouter } from './routes/admin.js'
export type { AdminRouteOptions } from './routes/admin.js'
export { WebhookDelivery } from './webhook.js'
export {
  TaskSchema, TaskEventSchema, WorkerSchema, ErrorSchema,
  CreateTaskSchema, TransitionSchema, PublishEventSchema,
} from './schemas.js'

import type { Hono } from 'hono'
import { OpenAPIHono } from '@hono/zod-openapi'
import { apiReference } from '@scalar/hono-api-reference'
import { createAuthMiddleware } from './auth.js'
import { createTasksRouter } from './routes/tasks.js'
import { createSSERouter, createSubscriberCounts } from './routes/sse.js'
import { createWorkersRouter } from './routes/workers.js'
import { WorkerWSRegistry } from './routes/worker-ws.js'
import { createAdminRouter } from './routes/admin.js'
import type { AuthConfig } from './auth.js'
import { isTerminal, matchesWorkerRule } from '@taskcast/core'
import type {
  Task,
  TaskEngine,
  WorkerManager,
  ShortTermStore,
  DisconnectPolicy,
  TaskcastConfig,
} from '@taskcast/core'
import { TaskScheduler } from '@taskcast/core'
import { HeartbeatMonitor } from '@taskcast/core'

export interface TaskcastServerOptions {
  engine: TaskEngine
  workerManager?: WorkerManager
  shortTermStore?: ShortTermStore
  auth?: AuthConfig
  config?: TaskcastConfig
  scheduler?: {
    enabled?: boolean
    checkIntervalMs?: number
    pausedColdAfterMs?: number
    blockedColdAfterMs?: number
  }
  heartbeat?: {
    enabled?: boolean
    checkIntervalMs?: number
    heartbeatTimeoutMs?: number
    defaultDisconnectPolicy?: DisconnectPolicy
    disconnectGraceMs?: number
  }
}

export interface TaskcastApp {
  app: Hono
  wsRegistry?: WorkerWSRegistry
  stop(): void
}

/**
 * Creates an OpenAPIHono app with all taskcast routes mounted.
 * Can be used standalone or mounted into an existing Hono app.
 *
 * Returns a TaskcastApp with `app` (the Hono instance) and `stop()` to
 * clean up scheduler/heartbeat timers.
 */
export function createTaskcastApp(opts: TaskcastServerOptions): TaskcastApp {
  const startTime = Date.now()
  const app = new OpenAPIHono()
  app.get('/health', (c) => c.json({ ok: true }))

  app.get('/health/detail', (c) => {
    const uptime = Math.floor((Date.now() - startTime) / 1000)
    const authMode = opts.auth?.mode ?? 'none'
    const broadcastProvider = opts.config?.adapters?.broadcast?.provider ?? 'memory'
    const shortTermProvider = opts.config?.adapters?.shortTermStore?.provider ?? 'memory'
    const longTermProvider = opts.config?.adapters?.longTermStore?.provider

    const adapters: Record<string, { provider: string; status: string }> = {
      broadcast: { provider: broadcastProvider, status: 'ok' },
      shortTermStore: { provider: shortTermProvider, status: 'ok' },
    }

    if (longTermProvider) {
      adapters.longTermStore = { provider: longTermProvider, status: 'ok' }
    }

    return c.json({
      ok: true,
      uptime,
      auth: { mode: authMode },
      adapters,
    })
  })

  // Admin route is mounted BEFORE auth middleware so it bypasses JWT/custom auth.
  // It authenticates via admin token independently.
  if (opts.config) {
    app.route('/admin', createAdminRouter({ config: opts.config, auth: opts.auth }))
  }

  const subscriberCounts = createSubscriberCounts()

  app.use('*', createAuthMiddleware(opts.auth ?? { mode: 'none' }))
  app.route('/tasks', createTasksRouter(opts.engine, subscriberCounts))
  app.route('/tasks', createSSERouter(opts.engine, subscriberCounts))

  const cleanups: Array<() => void> = []

  // Wire scheduler
  let scheduler: TaskScheduler | undefined
  if (opts.scheduler?.enabled !== false && opts.shortTermStore) {
    const schedulerOpts: ConstructorParameters<typeof TaskScheduler>[0] = {
      engine: opts.engine,
      shortTermStore: opts.shortTermStore,
    }
    if (opts.scheduler?.checkIntervalMs !== undefined) schedulerOpts.checkIntervalMs = opts.scheduler.checkIntervalMs
    if (opts.scheduler?.pausedColdAfterMs !== undefined) schedulerOpts.pausedColdAfterMs = opts.scheduler.pausedColdAfterMs
    if (opts.scheduler?.blockedColdAfterMs !== undefined) schedulerOpts.blockedColdAfterMs = opts.scheduler.blockedColdAfterMs
    scheduler = new TaskScheduler(schedulerOpts)
    scheduler.start()
    cleanups.push(() => scheduler!.stop())
  }

  // Wire worker manager
  let wsRegistry: WorkerWSRegistry | undefined
  if (opts.workerManager) {
    const wm = opts.workerManager
    wsRegistry = new WorkerWSRegistry()

    // Auto-release capacity on terminal transitions
    opts.engine.addTransitionListener((_task, _from, to) => {
      if (isTerminal(to)) {
        wm.releaseTask(_task.id).catch(() => {})
      }
    })

    // Wire ws-offer/ws-race dispatch on pending transitions
    async function dispatchToWS(task: Task): Promise<void> {
      if (task.assignMode === 'ws-offer') {
        const result = await wm.dispatchTask(task.id)
        if (result.matched && result.workerId) {
          const handler = wsRegistry!.get(result.workerId)
          if (handler) handler.offerTask(task)
        }
      } else if (task.assignMode === 'ws-race') {
        const workers = await wm.listWorkers({ status: ['idle', 'busy'] })
        for (const worker of workers) {
          if (worker.connectionMode !== 'websocket') continue
          if (!matchesWorkerRule(task, worker.matchRule)) continue
          const cost = task.cost ?? 1
          if (worker.usedSlots + cost > worker.capacity) continue
          const handler = wsRegistry!.get(worker.id)
          if (handler) handler.broadcastAvailable(task)
        }
      }
    }

    // Dispatch on initial task creation
    opts.engine.addCreationListener((task) => {
      if (!task.assignMode || (task.assignMode !== 'ws-offer' && task.assignMode !== 'ws-race')) return
      dispatchToWS(task).catch(() => {})
    })

    // Re-dispatch when task transitions back to pending (e.g. after decline)
    opts.engine.addTransitionListener((task, _from, to) => {
      if (to !== 'pending') return
      if (!task.assignMode || (task.assignMode !== 'ws-offer' && task.assignMode !== 'ws-race')) return
      // Fire-and-forget async dispatch
      dispatchToWS(task).catch(() => {})
    })

    app.route('/workers', createWorkersRouter(opts.workerManager, opts.engine))

    // Wire heartbeat monitor
    if (opts.heartbeat?.enabled !== false && opts.shortTermStore) {
      const monitorOpts: ConstructorParameters<typeof HeartbeatMonitor>[0] = {
        workerManager: wm,
        engine: opts.engine,
        shortTermStore: opts.shortTermStore,
      }
      if (opts.heartbeat?.checkIntervalMs !== undefined) monitorOpts.checkIntervalMs = opts.heartbeat.checkIntervalMs
      if (opts.heartbeat?.heartbeatTimeoutMs !== undefined) monitorOpts.heartbeatTimeoutMs = opts.heartbeat.heartbeatTimeoutMs
      if (opts.heartbeat?.defaultDisconnectPolicy !== undefined) monitorOpts.defaultDisconnectPolicy = opts.heartbeat.defaultDisconnectPolicy
      if (opts.heartbeat?.disconnectGraceMs !== undefined) monitorOpts.disconnectGraceMs = opts.heartbeat.disconnectGraceMs
      const monitor = new HeartbeatMonitor(monitorOpts)
      monitor.start()
      cleanups.push(() => monitor.stop())
    }
  }

  // Register security scheme
  app.openAPIRegistry.registerComponent('securitySchemes', 'Bearer', {
    type: 'http',
    scheme: 'bearer',
    bearerFormat: 'JWT',
    description: 'JWT Bearer token. Required scopes vary per endpoint.',
  })

  // OpenAPI spec endpoint
  app.doc('/openapi.json', {
    openapi: '3.1.0',
    info: {
      title: 'Taskcast API',
      version: '0.3.0',
      description: 'Unified long-lifecycle task tracking service for LLM streaming, agents, and async workloads.',
    },
    security: [{ Bearer: [] }],
  })

  // API documentation UI
  app.get('/docs', apiReference({
    url: '/openapi.json',
  }))

  return {
    app: app as unknown as Hono,
    ...(wsRegistry !== undefined && { wsRegistry }),
    stop() {
      for (const fn of cleanups) fn()
    },
  }
}

