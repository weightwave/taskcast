export { createAuthMiddleware, checkScope } from './auth.js'
export { createVerboseLogger } from './middleware/verbose-logger.js'
export {
  createHttpFailureLogger,
  parseLogLevel,
  sanitizeErrorMessage,
} from './middleware/http-failure-logger.js'
export type { AuthConfig, AuthContext, JWTConfig, TrustedServiceConfig } from './auth.js'
export type {
  HttpFailureKind,
  HttpFailureLog,
  HttpFailureLogger,
  HttpFailureLoggerOptions,
  LogLevel,
} from './middleware/http-failure-logger.js'
export { createTasksRouter, dependencyErrorResponse } from './routes/tasks.js'
export { createSSERouter, createGlobalSSERoute, createSubscriberCounts, getSubscriberCount } from './routes/sse.js'
export type { SubscriberCounts } from './routes/sse.js'
export { createWorkersRouter, WorkerWSHandler, WorkerWSRegistry } from './routes/workers.js'
export type { WSLike, TaskSummary } from './routes/workers.js'
export { createAdminRouter } from './routes/admin.js'
export type { AdminRouteOptions } from './routes/admin.js'
export { WebhookDelivery } from './webhook.js'
export { DependencyHealthRegistry } from './dependency-health.js'
export type {
  DependencyCheck,
  DependencyHealthLogger,
  DependencySnapshot,
  ReadinessResult,
} from './dependency-health.js'
export {
  TaskSchema, TaskEventSchema, WorkerSchema, ErrorSchema,
  CreateTaskSchema, TransitionSchema, PublishEventSchema,
  TaskArchiveSchema, ImportTaskArchiveSchema, ImportTaskArchiveResultSchema, ServerInfoSchema,
  StorageReleaseRequestSchema, StorageReleaseResultSchema,
} from './schemas.js'
export {
  TASKCAST_API_VERSION,
  TASKCAST_SERVER_NAME,
  TASKCAST_SERVER_VERSION,
  serverInfo,
} from './version.js'

import type { Hono } from 'hono'
import { OpenAPIHono } from '@hono/zod-openapi'
import { cors } from 'hono/cors'
import { apiReference } from '@scalar/hono-api-reference'
import { createAuthMiddleware } from './auth.js'
import { createTasksRouter } from './routes/tasks.js'
import type { StorageReleaseReadiness } from './routes/tasks.js'
import { createSSERouter, createGlobalSSERoute, createSubscriberCounts } from './routes/sse.js'
import { createWorkersRouter } from './routes/workers.js'
import { WorkerWSRegistry } from './routes/worker-ws.js'
import { createAdminRouter } from './routes/admin.js'
import { createVerboseLogger } from './middleware/verbose-logger.js'
import {
  createHttpFailureLogger,
  type HttpFailureLogger,
  type LogLevel,
} from './middleware/http-failure-logger.js'
import { TASKCAST_SERVER_VERSION, serverInfo } from './version.js'
import type { AuthConfig } from './auth.js'
import {
  findDependencyUnavailableError,
  isTerminal,
  matchesWorkerRule,
} from '@taskcast/core'
import type {
  DurableTtlSweepResult,
  ResolvedStorageLifecycleConfig,
  StorageReleaseSweepResult,
  Task,
  TaskEngine,
  WorkerManager,
  ShortTermStore,
  DisconnectPolicy,
  TaskcastConfig,
  StorageWriterRegistration,
} from '@taskcast/core'
import {
  resolveStorageLifecycleConfig,
  StorageUnavailableError,
  TaskScheduler,
} from '@taskcast/core'
import { HeartbeatMonitor } from '@taskcast/core'
import {
  DependencyHealthRegistry,
  type DependencySnapshot,
} from './dependency-health.js'

export interface TaskcastServerOptions {
  engine: TaskEngine
  workerManager?: WorkerManager
  shortTermStore?: ShortTermStore
  auth?: AuthConfig
  config?: TaskcastConfig
  /** Enable verbose HTTP request logging to stdout. */
  verbose?: boolean
  /** Custom logger function for verbose mode (defaults to console.log). Useful for testing. */
  verboseLogger?: (line: string) => void
  /** Minimum server log level. Defaults to info. */
  logLevel?: LogLevel
  /** Structured 5xx log sink. Defaults to one JSON line on stderr. */
  errorLogger?: HttpFailureLogger
  dependencyHealth?: DependencyHealthRegistry
  /** Adapters actually instantiated by a managed runtime (takes precedence over file config). */
  effectiveAdapters?: RuntimeAdapterDescriptors
  cors?: boolean | { origin: string | string[] }
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
  storageLifecycle?: ResolvedStorageLifecycleConfig
}

export interface RuntimeAdapterDescriptors {
  broadcast: string
  shortTermStore: string
  longTermStore?: string
}

export interface TaskcastApp {
  app: Hono
  wsRegistry?: WorkerWSRegistry
  stop(): void
}

const STORAGE_PROTOCOL_VERSION = 2
const STORAGE_WRITER_TTL_MS = 30_000
const STORAGE_WRITER_HEARTBEAT_MS = 10_000

interface StorageReadinessSnapshot {
  releaseReady: boolean
  requiredStorageProtocolVersion: number
  activeWriterCount: number
  incompatibleWriterIds: string[]
}

class StorageWriterHeartbeat implements StorageReleaseReadiness {
  private readonly instanceId = globalThis.crypto?.randomUUID?.()
    ?? `taskcast-${Date.now()}-${Math.random().toString(36).slice(2)}`
  private readonly registration: StorageWriterRegistration
  private readonly timer: ReturnType<typeof setInterval>
  private heartbeatError: unknown = null

  constructor(private readonly engine: TaskEngine) {
    this.registration = {
      instanceId: this.instanceId,
      storageProtocolVersion: STORAGE_PROTOCOL_VERSION,
      build: TASKCAST_SERVER_VERSION,
      expiresAt: 0,
    }
    void this.heartbeat()
    this.timer = setInterval(() => {
      void this.heartbeat()
    }, STORAGE_WRITER_HEARTBEAT_MS)
    ;(this.timer as unknown as { unref?: () => void }).unref?.()
  }

  private async heartbeat(): Promise<void> {
    try {
      await this.engine.registerStorageWriter(this.registration, STORAGE_WRITER_TTL_MS)
      this.heartbeatError = null
    } catch (error) {
      this.heartbeatError = error
    }
  }

  async snapshot(): Promise<StorageReadinessSnapshot> {
    await this.heartbeat()
    let writers: StorageWriterRegistration[]
    try {
      writers = await this.engine.listStorageWriters()
    } catch {
      return {
        releaseReady: false,
        requiredStorageProtocolVersion: STORAGE_PROTOCOL_VERSION,
        activeWriterCount: 0,
        incompatibleWriterIds: [],
      }
    }
    const incompatibleWriterIds = writers
      .filter((writer) => writer.storageProtocolVersion < STORAGE_PROTOCOL_VERSION)
      .map((writer) => writer.instanceId)
      .sort()
    return {
      releaseReady:
        this.engine.supportsStorageRelease() &&
        this.heartbeatError === null &&
        writers.some((writer) => writer.instanceId === this.instanceId) &&
        incompatibleWriterIds.length === 0,
      requiredStorageProtocolVersion: STORAGE_PROTOCOL_VERSION,
      activeWriterCount: writers.length,
      incompatibleWriterIds,
    }
  }

  async ensureReady(): Promise<void> {
    if (!this.engine.supportsStorageRelease()) return
    const readiness = await this.snapshot()
    if (!readiness.releaseReady) {
      const detail = readiness.incompatibleWriterIds.length > 0
        ? `: ${readiness.incompatibleWriterIds.join(', ')}`
        : ''
      throw new StorageUnavailableError(`Storage writer readiness is not satisfied${detail}`)
    }
  }

  stop(): void {
    clearInterval(this.timer)
  }
}

export interface StorageLifecycleWorkerOptions {
  engine: TaskEngine
  shortTermStore: ShortTermStore
  config: ResolvedStorageLifecycleConfig
  readiness?: StorageReleaseReadiness
  logger?: (record: Record<string, unknown>) => void
  now?: () => number
}

export interface StorageLifecycleTickResult {
  ttl: DurableTtlSweepResult
  projection: DurableTtlSweepResult
  releaseRequests: StorageReleaseSweepResult
  retention: {
    eligible: number
    released: number
    failed: number
  }
}

const emptyTtlResult = (): DurableTtlSweepResult => ({
  claimed: 0,
  timedOut: 0,
  raceLost: 0,
  failed: 0,
  projected: 0,
})

const emptyReleaseResult = (): StorageReleaseSweepResult => ({
  claimed: 0,
  released: 0,
  recovered: 0,
  stale: 0,
  deferred: 0,
  failed: 0,
})

export class StorageLifecycleWorker {
  private readonly engine: TaskEngine
  private readonly shortTermStore: ShortTermStore
  private readonly config: ResolvedStorageLifecycleConfig
  private readonly readiness: StorageReleaseReadiness | undefined
  private readonly logger: (record: Record<string, unknown>) => void
  private readonly now: () => number
  private timer: ReturnType<typeof setInterval> | undefined
  private running = false
  private ttlFailureStreak = 0
  private ttlRetryAfter = 0
  private releaseFailureStreak = 0
  private releaseRetryAfter = 0

  constructor(options: StorageLifecycleWorkerOptions) {
    this.engine = options.engine
    this.shortTermStore = options.shortTermStore
    this.config = options.config
    this.readiness = options.readiness
    this.logger = options.logger ?? ((record) => {
      console.log(JSON.stringify({ component: 'storage-lifecycle', ...record }))
    })
    this.now = options.now ?? Date.now
  }

  start(): void {
    if (this.timer) return
    void this.tick()
    this.timer = setInterval(() => {
      void this.tick()
    }, this.config.ttlSweepIntervalSeconds * 1_000)
    ;(this.timer as unknown as { unref?: () => void }).unref?.()
  }

  stop(): void {
    if (this.timer) clearInterval(this.timer)
    this.timer = undefined
  }

  async tick(): Promise<StorageLifecycleTickResult | null> {
    const startedAt = this.now()
    if (this.running) return null
    this.running = true
    const ttl = emptyTtlResult()
    const projection = emptyTtlResult()
    const releaseRequests = emptyReleaseResult()
    const retention = { eligible: 0, released: 0, failed: 0 }
    try {
      const limit = this.config.ttlSweepBatchSize
      const claimTtlMs = this.config.storageLockTtlSeconds * 1_000
      const ttlAttempted =
        this.engine.supportsDurableTtl() && startedAt >= this.ttlRetryAfter
      if (ttlAttempted) {
        try {
          Object.assign(ttl, await this.engine.sweepDurableTtl(limit, claimTtlMs))
        } catch (error) {
          ttl.failed += 1
          this.logError('durable_ttl', error)
        }
        try {
          Object.assign(
            projection,
            await this.engine.sweepTerminalProjections(limit, claimTtlMs),
          )
        } catch (error) {
          projection.failed += 1
          this.logError('terminal_projection', error)
        }
      }

      const releaseAttempted =
        this.engine.supportsStorageRelease() &&
        startedAt >= this.releaseRetryAfter
      if (releaseAttempted) {
        let releaseReady = true
        try {
          await this.readiness?.ensureReady()
        } catch (error) {
          releaseReady = false
          releaseRequests.failed += 1
          this.logError('release_readiness', error)
        }
        if (releaseReady) {
          try {
            Object.assign(
              releaseRequests,
              await this.engine.retryStorageReleaseRequests(
                limit,
                startedAt - this.config.hotRetentionIdleSeconds * 1_000,
              ),
            )
          } catch (error) {
            releaseRequests.failed += 1
            this.logError('release_request_retry', error)
          }

          if (this.config.hotRetentionEnabled) {
            try {
              const candidates = await this.shortTermStore.listTasks({
                status: ['completed', 'failed', 'timeout', 'cancelled'],
                limit,
              })
              const eligibleBefore =
                startedAt - this.config.hotRetentionTerminalSeconds * 1_000
              for (const task of candidates) {
                if (task.updatedAt > eligibleBefore) continue
                retention.eligible += 1
                try {
                  await this.engine.releaseTaskStorageAtCurrentDurableIndex(
                    task.id,
                    startedAt,
                  )
                  retention.released += 1
                } catch (error) {
                  retention.failed += 1
                  this.logError('terminal_retention', error, {
                    taskId: task.id,
                  })
                }
              }
            } catch (error) {
              retention.failed += 1
              this.logError('terminal_retention_scan', error)
            }
          }
        }
      }

      if (ttlAttempted) {
        const ttlFailures = ttl.failed + projection.failed
        if (ttlFailures > 0) {
          this.ttlFailureStreak = Math.min(this.ttlFailureStreak + 1, 6)
          this.ttlRetryAfter = startedAt + this.backoffMs(this.ttlFailureStreak)
        } else {
          this.ttlFailureStreak = 0
          this.ttlRetryAfter = 0
        }
      }
      if (releaseAttempted) {
        const releaseFailures = releaseRequests.failed + retention.failed
        if (releaseFailures > 0) {
          this.releaseFailureStreak = Math.min(
            this.releaseFailureStreak + 1,
            6,
          )
          this.releaseRetryAfter =
            startedAt + this.backoffMs(this.releaseFailureStreak)
        } else {
          this.releaseFailureStreak = 0
          this.releaseRetryAfter = 0
        }
      }
      const result = { ttl, projection, releaseRequests, retention }
      this.logger({
        event: 'storage_lifecycle_tick',
        durationMs: Math.max(0, this.now() - startedAt),
        ...result,
      })
      return result
    } finally {
      this.running = false
    }
  }

  private backoffMs(failureStreak: number): number {
    return Math.min(
      this.config.ttlSweepIntervalSeconds * 1_000 * 2 ** failureStreak,
      5 * 60_000,
    )
  }

  private logError(
    operation: string,
    error: unknown,
    detail: Record<string, unknown> = {},
  ): void {
    this.logger({
      event: 'storage_lifecycle_error',
      operation,
      ...detail,
      error: error instanceof Error ? error.message : String(error),
    })
  }
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
  const storageReadiness = new StorageWriterHeartbeat(opts.engine)

  // Hono's default handler writes the raw error to stderr before middleware
  // can sanitize it. Preserve its response behavior while leaving the single
  // structured emission to the failure logger below.
  app.onError((error, c) => {
    if ('getResponse' in error && typeof error.getResponse === 'function') {
      const response = error.getResponse()
      return c.newResponse(response.body, response)
    }
    const dependency = findDependencyUnavailableError(error)
    if (dependency) {
      return c.json({ error: dependency.message }, 503)
    }
    return c.text('Internal Server Error', 500)
  })

  app.use('*', createHttpFailureLogger({
    logLevel: opts.logLevel ?? 'info',
    ...(opts.errorLogger ? { logger: opts.errorLogger } : {}),
  }))

  // Apply verbose logger before all routes when enabled
  if (opts.verbose) {
    app.use('*', createVerboseLogger(opts.verboseLogger))
  }

  // CORS middleware
  if (opts.cors) {
    const origin = opts.cors === true ? '*' : opts.cors.origin
    app.use('*', cors({ origin }))
  }

  app.get('/', (c) => c.json({
    ...serverInfo(),
    links: {
      health: '/health',
      healthReady: '/health/ready',
      healthDetail: '/health/detail',
      openapi: '/openapi.json',
      docs: '/docs',
    },
  }))

  app.get('/health', (c) => c.json({ ok: true, ...serverInfo() }))

  app.get('/health/ready', async (c) => {
    const result = opts.dependencyHealth
      ? await opts.dependencyHealth.checkReadiness(2_000)
      : { ok: true, dependencies: {} }
    return c.json(result, result.ok ? 200 : 503)
  })

  app.get('/health/detail', async (c) => {
    const uptime = Math.floor((Date.now() - startTime) / 1000)
    const authMode = opts.auth?.mode ?? 'none'
    const broadcastProvider = opts.effectiveAdapters?.broadcast
      ?? opts.config?.adapters?.broadcast?.provider
      ?? 'memory'
    const shortTermProvider = opts.effectiveAdapters?.shortTermStore
      ?? opts.config?.adapters?.shortTermStore?.provider
      ?? 'memory'
    const longTermProvider = opts.effectiveAdapters === undefined
      ? opts.config?.adapters?.longTermStore?.provider
      : opts.effectiveAdapters.longTermStore

    const adapters: Record<string, { provider: string; status: string }> = {
      broadcast: { provider: broadcastProvider, status: 'ok' },
      shortTermStore: { provider: shortTermProvider, status: 'ok' },
    }

    if (longTermProvider) {
      adapters.longTermStore = { provider: longTermProvider, status: 'ok' }
    }

    if (!opts.dependencyHealth) {
      return c.json({
        ok: true,
        ...serverInfo(),
        uptime,
        auth: { mode: authMode },
        adapters,
      })
    }

    const dependencies = opts.dependencyHealth.snapshot()
    const dependencyHealthy = (
      name: keyof typeof dependencies,
    ): boolean => dependencies[name]?.state === 'healthy'
    if (broadcastProvider === 'redis') {
      adapters.broadcast!.status = dependencyHealthy('redisCommand')
        && dependencyHealthy('redisPubSub')
        ? 'ok'
        : 'error'
    }
    if (shortTermProvider === 'redis') {
      adapters.shortTermStore!.status = dependencyHealthy('redisCommand')
        ? 'ok'
        : 'error'
    }
    if (longTermProvider === 'postgres' && adapters.longTermStore) {
      adapters.longTermStore.status = dependencyHealthy('postgres')
        ? 'ok'
        : 'error'
    }

    return c.json({
      ok: Object.values(dependencies).every(
        (dependency: DependencySnapshot | undefined) =>
          dependency?.state === 'healthy',
      ),
      ...serverInfo(),
      uptime,
      auth: { mode: authMode },
      adapters,
      dependencies,
      storage: await storageReadiness.snapshot(),
    })
  })

  // Admin route is mounted BEFORE auth middleware so it bypasses JWT/custom auth.
  // It authenticates via admin token independently.
  if (opts.config) {
    app.route('/admin', createAdminRouter({ config: opts.config, auth: opts.auth }))
  }

  const subscriberCounts = createSubscriberCounts()

  const authMiddleware = createAuthMiddleware(opts.auth ?? { mode: 'none' })
  app.use('/tasks', authMiddleware)
  app.use('/tasks/*', authMiddleware)
  app.use('/events', authMiddleware)
  app.use('/events/*', authMiddleware)
  app.use('/workers', authMiddleware)
  app.use('/workers/*', authMiddleware)

  app.route('/tasks', createTasksRouter(opts.engine, subscriberCounts, storageReadiness))
  app.route('/tasks', createSSERouter(opts.engine, subscriberCounts))
  app.route('/events', createGlobalSSERoute(opts.engine))

  const cleanups: Array<() => void> = []
  cleanups.push(() => storageReadiness.stop())

  if (
    opts.shortTermStore &&
    (opts.engine.supportsDurableTtl() || opts.engine.supportsStorageRelease())
  ) {
    const lifecycle = new StorageLifecycleWorker({
      engine: opts.engine,
      shortTermStore: opts.shortTermStore,
      config:
        opts.storageLifecycle ??
        resolveStorageLifecycleConfig(opts.config ?? {}, {}),
      readiness: storageReadiness,
    })
    lifecycle.start()
    cleanups.push(() => lifecycle.stop())
  }

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
      version: TASKCAST_SERVER_VERSION,
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
