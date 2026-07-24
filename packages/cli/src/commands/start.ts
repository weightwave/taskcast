import { Command } from 'commander'
import postgres from 'postgres'
import { existsSync } from 'fs'
import { join, dirname } from 'path'
import { createRequire } from 'module'
import {
  TaskEngine,
  WorkerManager,
  loadConfigFile,
  resolveAdminToken,
  MemoryBroadcastProvider,
  MemoryShortTermStore,
} from '@taskcast/core'
import type { BroadcastProvider, ShortTermStore, LongTermStore, TaskcastConfig } from '@taskcast/core'
import { createTaskcastApp, DependencyHealthRegistry, parseLogLevel } from '@taskcast/server'
import type { AuthConfig, JWTConfig } from '@taskcast/server'
import { createManagedRedisAdapters } from '@taskcast/redis'
import type { ManagedRedisAdapters } from '@taskcast/redis'
import { PostgresLongTermStore, postgresCheck } from '@taskcast/postgres'
import { createSqliteAdapters } from '@taskcast/sqlite'
import { promptCreateGlobalConfig, createDefaultGlobalConfig } from '../utils.js'
import { performAutoMigrateIfEnabled } from '../auto-migrate.js'
import { formatDisplayUrl } from '../migrate-helpers.js'

/**
 * Strip credentials from a connection URL (Redis/Postgres/etc.) for logging.
 * Returns `scheme://host:port/path` with no userinfo. Falls back to the raw
 * string only when parsing fails AND the fallback does not contain an `@`
 * (which would indicate embedded credentials). When credentials are present
 * and parsing fails, returns '<redacted>' to avoid leaking secrets.
 */
function formatConnectionUrlForLog(url: string): string {
  try {
    const parsed = new URL(url)
    parsed.username = ''
    parsed.password = ''
    return parsed.toString()
  } catch {
    // Parse failed. Only return the raw string if it clearly contains no creds.
    return url.includes('@') ? '<redacted>' : url
  }
}

function envNonEmpty(key: string): string | undefined {
  const value = process.env[key]
  return value === undefined || value === '' ? undefined : value
}

type StorageMode = 'memory' | 'redis' | 'sqlite'

export function resolveStorageMode(options: {
  cli?: string
  env?: string
  configuredProvider?: string
  hasRedisUrl: boolean
}): StorageMode {
  const value = options.cli ?? options.env ?? options.configuredProvider ?? (options.hasRedisUrl ? 'redis' : 'memory')
  if (value !== 'memory' && value !== 'redis' && value !== 'sqlite') {
    throw new Error(`invalid storage mode "${value}"; expected memory, redis, or sqlite`)
  }
  return value
}

export function parsePostgresMaxConnections(value?: string): number {
  if (value === undefined || value === '') return 10
  if (!/^[1-9]\d*$/.test(value)) {
    throw new Error('TASKCAST_POSTGRES_MAX_CONNECTIONS must be a positive integer')
  }
  const parsed = Number(value)
  if (!Number.isSafeInteger(parsed)) {
    throw new Error('TASKCAST_POSTGRES_MAX_CONNECTIONS must be a positive integer')
  }
  return parsed
}

function configuredStorageProvider(config: TaskcastConfig): string | undefined {
  const shortTerm = config.adapters?.shortTermStore?.provider
  const broadcast = config.adapters?.broadcast?.provider
  if (shortTerm !== undefined && broadcast !== undefined && shortTerm !== broadcast) {
    throw new Error('configured short-term and broadcast providers must match')
  }
  return shortTerm ?? broadcast
}

function configuredRedisUrl(config: TaskcastConfig): string | undefined {
  const broadcast = config.adapters?.broadcast?.url
  const shortTerm = config.adapters?.shortTermStore?.url
  return [broadcast, shortTerm].find((value): value is string => value !== undefined && value !== '')
}

function postgresActivation(options: {
  storageMode: StorageMode
  configuredProvider?: string
  envUrl?: string
  configuredUrl?: string
}): { active: false } | { active: true; url: string } {
  if (options.storageMode === 'sqlite') return { active: false }
  if (options.configuredProvider !== undefined) {
    if (options.configuredProvider !== 'postgres') return { active: false }
    const url = options.envUrl ?? options.configuredUrl
    if (url === undefined || url === '') {
      throw new Error(
        'configured PostgreSQL long-term store requires TASKCAST_POSTGRES_URL or adapters.longTermStore.url'
      )
    }
    return { active: true, url }
  }
  return options.envUrl === undefined ? { active: false } : { active: true, url: options.envUrl }
}

function buildAuthConfig(config: TaskcastConfig): AuthConfig {
  const authMode = (envNonEmpty('TASKCAST_AUTH_MODE') ?? config.auth?.mode ?? 'none') as AuthConfig['mode']
  const auth: AuthConfig = { mode: authMode }

  if (authMode === 'jwt') {
    const fileJwt = config.auth?.jwt
    const jwt: JWTConfig = {
      algorithm: (envNonEmpty('TASKCAST_JWT_ALGORITHM') ?? fileJwt?.algorithm ?? 'HS256') as JWTConfig['algorithm'],
    }
    const secret = envNonEmpty('TASKCAST_JWT_SECRET') ?? fileJwt?.secret
    const publicKey = envNonEmpty('TASKCAST_JWT_PUBLIC_KEY') ?? fileJwt?.publicKey
    const publicKeyFile = envNonEmpty('TASKCAST_JWT_PUBLIC_KEY_FILE') ?? fileJwt?.publicKeyFile
    const issuer = envNonEmpty('TASKCAST_JWT_ISSUER') ?? fileJwt?.issuer
    const audience = envNonEmpty('TASKCAST_JWT_AUDIENCE') ?? fileJwt?.audience

    if (secret !== undefined) jwt.secret = secret
    if (publicKey !== undefined) jwt.publicKey = publicKey
    if (publicKeyFile !== undefined) jwt.publicKeyFile = publicKeyFile
    if (issuer !== undefined) jwt.issuer = issuer
    if (audience !== undefined) jwt.audience = audience
    auth.jwt = jwt
  }

  if (config.trustedServices !== undefined) {
    auth.trustedServices = config.trustedServices.map((service) => ({
      name: service.name,
      key: service.key,
      taskIds: service.taskIds ?? '*',
      scope: service.scope,
    }))
  }

  return auth
}

/**
 * Options for runStart function.
 * Captures all server startup configuration.
 */
export interface RunStartOptions {
  /** Postgres connection instance (optional) */
  postgres?: ReturnType<typeof postgres>
  /** Resolved Postgres URL (for auto-migrate banner log), required if postgres is set */
  postgresUrl?: string
  /** Broadcast provider instance */
  broadcast: BroadcastProvider
  /** Short-term store instance */
  shortTermStore: ShortTermStore
  /** Long-term store instance (optional) */
  longTermStore?: LongTermStore
  /** Port to listen on */
  port: number
  /** Server configuration options */
  config: TaskcastConfig
  /** Verbose logging flag */
  verbose: boolean
  /** Playground flag */
  playground: boolean
  /** File config path for display */
  configPath?: string
  /** Environment variables for auto-migrate */
  env?: Record<string, string | undefined>
  /** Active dependency health registry */
  dependencyHealth?: DependencyHealthRegistry
  /** Idempotent close callback for active Redis/PostgreSQL resources */
  closeDependencies?: () => Promise<void>
}

type CloseableServer = {
  close?: (callback?: (error?: Error) => void) => void
}

class StartupCancelledError extends Error {
  constructor() {
    super('startup cancelled by signal')
    this.name = 'StartupCancelledError'
  }
}

/**
 * Owns startup cancellation and shutdown for the whole command lifetime.
 *
 * Shutdown waits for the startup barrier before closing anything. That lets an
 * in-flight acquisition publish its resource and reach a cancellation
 * checkpoint before the single cleanup pass snapshots the owned resources.
 */
class StartLifecycle {
  private readonly startupBarrier: Promise<void>
  private settleStartupBarrier!: () => void
  private startupSettled = false
  private cancellationRequested = false
  private shutdownPromise: Promise<void> | undefined
  private stopServices: (() => void) | undefined
  private server: CloseableServer | undefined

  private readonly signalHandler = (): Promise<void> => {
    this.cancellationRequested = true
    const shutdown = this.requestShutdown()
    void shutdown.catch((error: unknown) => {
      const message = error instanceof Error ? error.message : String(error)
      console.error(`[taskcast] shutdown failed: ${message}`)
    })
    return shutdown
  }

  constructor(private readonly closeDependencies?: () => Promise<void>) {
    this.startupBarrier = new Promise<void>((resolve) => {
      this.settleStartupBarrier = resolve
    })
    process.on('SIGTERM', this.signalHandler)
    process.on('SIGINT', this.signalHandler)
  }

  get cancelled(): boolean {
    return this.cancellationRequested
  }

  checkpoint(): void {
    if (this.cancellationRequested) {
      throw new StartupCancelledError()
    }
  }

  attachServices(stop: () => void): void {
    this.stopServices = stop
  }

  attachServer(server: CloseableServer): void {
    this.server = server
  }

  settleStartup(): void {
    if (this.startupSettled) return
    this.startupSettled = true
    this.settleStartupBarrier()
  }

  requestShutdown(): Promise<void> {
    if (this.shutdownPromise !== undefined) return this.shutdownPromise
    this.shutdownPromise = (async () => {
      await this.startupBarrier
      try {
        try {
          this.stopServices?.()
        } finally {
          await this.closeServer()
        }
      } finally {
        await this.closeDependencies?.()
      }
    })().finally(() => {
      process.off('SIGTERM', this.signalHandler)
      process.off('SIGINT', this.signalHandler)
    })
    return this.shutdownPromise
  }

  private async closeServer(): Promise<void> {
    const server = this.server
    const close = server?.close
    if (server === undefined || close === undefined) return
    await new Promise<void>((resolve, reject) => {
      try {
        close.call(server, (error?: Error) => {
          if (error) reject(error)
          else resolve()
        })
      } catch (error) {
        reject(error)
      }
    })
  }
}

/**
 * Runs the taskcast server with auto-migrate support.
 *
 * This function:
 * 1. Calls performAutoMigrateIfEnabled() if Postgres is configured
 * 2. Creates and starts the server
 * 3. Sets up SIGTERM/SIGINT handlers
 * 4. Serves playground UI if enabled
 *
 * If auto-migrate fails, the error is re-thrown and server startup is blocked.
 *
 * @param options - Server startup options
 * @throws Error if auto-migrate fails
 */
export async function runStart(options: RunStartOptions): Promise<void> {
  const lifecycle = new StartLifecycle(options.closeDependencies)
  let failed = false
  let startupError: unknown
  let shutdown: Promise<void> | undefined
  try {
    await runStartWithLifecycle(options, lifecycle)
  } catch (error) {
    failed = true
    startupError = error
    shutdown = lifecycle.requestShutdown()
  } finally {
    lifecycle.settleStartup()
  }

  if (failed) {
    await shutdown
    if (startupError instanceof StartupCancelledError || lifecycle.cancelled) return
    throw startupError
  }
}

async function runStartWithLifecycle(options: RunStartOptions, lifecycle: StartLifecycle): Promise<void> {
  lifecycle.checkpoint()
  const logLevel = parseLogLevel(options.env?.['TASKCAST_LOG_LEVEL'])

  // The startup SELECT is performed before entering runStart. Auto-migration
  // therefore cannot run before dependency readiness has been established.
  await performAutoMigrateIfEnabled(options.postgres, options.postgresUrl, options.env)
  lifecycle.checkpoint()

  const engineOpts: ConstructorParameters<typeof TaskEngine>[0] = {
    shortTermStore: options.shortTermStore,
    broadcast: options.broadcast,
  }
  if (options.longTermStore !== undefined) {
    engineOpts.longTermStore = options.longTermStore
  }
  const engine = new TaskEngine(engineOpts)
  const auth = buildAuthConfig(options.config)

  const workersEnabled = options.config.workers?.enabled ?? false
  let workerManager: WorkerManager | undefined
  if (workersEnabled) {
    console.log('[taskcast] Worker assignment system enabled')
    const wmOpts: ConstructorParameters<typeof WorkerManager>[0] = {
      engine,
      shortTermStore: options.shortTermStore,
      broadcast: options.broadcast,
    }
    if (options.longTermStore !== undefined) {
      wmOpts.longTermStore = options.longTermStore
    }
    if (options.config.workers?.defaults) {
      wmOpts.defaults = options.config.workers.defaults
    }
    workerManager = new WorkerManager(wmOpts)
  }

  resolveAdminToken(options.config)

  const serverOpts: Parameters<typeof createTaskcastApp>[0] = {
    engine,
    shortTermStore: options.shortTermStore,
    auth,
    config: options.config,
    verbose: options.verbose,
    logLevel,
  }
  if (workerManager !== undefined) serverOpts.workerManager = workerManager
  if (options.dependencyHealth !== undefined) {
    serverOpts.dependencyHealth = options.dependencyHealth
  }
  const { app, stop } = createTaskcastApp(serverOpts)
  lifecycle.attachServices(stop)
  lifecycle.checkpoint()

  if (options.playground) {
    try {
      const require = createRequire(import.meta.url)
      const pkgPath = require.resolve('@taskcast/playground/package.json')
      const distDir = join(dirname(pkgPath), 'dist')
      if (existsSync(distDir)) {
        const { serveStatic } = await import('@hono/node-server/serve-static')
        lifecycle.checkpoint()
        app.use(
          '/_playground/*',
          serveStatic({
            root: distDir,
            rewriteRequestPath: (p) => p.replace(/^\/_playground/, ''),
          })
        )
        app.get(
          '/_playground/*',
          serveStatic({
            root: distDir,
            rewriteRequestPath: () => '/index.html',
          })
        )
      } else {
        console.warn('[taskcast] Playground dist not found. Run `pnpm --filter @taskcast/playground build` first.')
      }
    } catch (error) {
      if (error instanceof StartupCancelledError) throw error
      console.warn('[taskcast] @taskcast/playground not available, skipping playground UI.')
    }
  }

  lifecycle.checkpoint()
  const { serve } = await import('@hono/node-server')
  lifecycle.checkpoint()
  let server: ReturnType<typeof serve> | undefined
  await new Promise<void>((resolve, reject) => {
    const onStartupError = (error: Error) => reject(error)
    server = serve({ fetch: app.fetch, port: options.port }, () => {
      ;(server as {
        off?: (event: string, listener: (error: Error) => void) => void
      } | undefined)?.off?.('error', onStartupError)
      console.log(`[taskcast] Server started on http://localhost:${options.port}`)
      if (options.playground) {
        console.log(`[taskcast] Playground UI at http://localhost:${options.port}/_playground/`)
      }
      resolve()
    })
    ;(server as {
      once?: (event: string, listener: (error: Error) => void) => void
    }).once?.('error', onStartupError)
  })
  if (server === undefined) {
    throw new Error('HTTP server was not created')
  }
  lifecycle.attachServer(server)
  lifecycle.checkpoint()
}

export function registerStartCommand(program: Command): void {
  program
    .command('start', { isDefault: true })
    .description('Start the taskcast server in foreground (default)')
    .option('-c, --config <path>', 'config file path')
    .option('-p, --port <port>', 'port to listen on', '3721')
    .option('-s, --storage <type>', 'storage backend: memory | redis | sqlite')
    .option('--db-path <path>', 'SQLite database file path (default: ./taskcast.db)')
    .option('--playground', 'serve the interactive playground UI at /_playground/')
    .option('-v, --verbose', 'enable verbose logging')
    .action(
      async (options: {
        config?: string
        port: string
        storage?: string
        dbPath?: string
        playground?: boolean
        verbose?: boolean
      }) => {
        let managedRedis: ManagedRedisAdapters | undefined
        let postgres_: ReturnType<typeof postgres> | undefined
        let cleanupPromise: Promise<void> | undefined
        const closeDependencies = (): Promise<void> => {
          cleanupPromise ??= (async () => {
            const closes: Promise<unknown>[] = []
            if (managedRedis !== undefined) closes.push(managedRedis.close())
            if (postgres_ !== undefined) {
              closes.push(postgres_.end({ timeout: 5 }))
            }
            await Promise.allSettled(closes)
          })()
          return cleanupPromise
        }

        const lifecycle = new StartLifecycle(closeDependencies)
        let failed = false
        let startupError: unknown
        let shutdown: Promise<void> | undefined
        try {
          let { config: fileConfig, source, path: configPath } = await loadConfigFile(options.config)
          lifecycle.checkpoint()

          if (source === 'none') {
            const shouldCreate = await promptCreateGlobalConfig()
            lifecycle.checkpoint()
            if (shouldCreate) {
              const createdPath = createDefaultGlobalConfig()
              if (createdPath) {
                const created = await loadConfigFile(createdPath)
                lifecycle.checkpoint()
                fileConfig = created.config
                source = created.source
                configPath = created.path
              }
            }
          }

          const port = Number(options.port ?? fileConfig.port ?? 3721)
          const envRedisUrl = envNonEmpty('TASKCAST_REDIS_URL')
          const redisUrl = envRedisUrl ?? configuredRedisUrl(fileConfig)
          const envStorage = envNonEmpty('TASKCAST_STORAGE')
          const configuredProvider =
            options.storage === undefined && envStorage === undefined
              ? configuredStorageProvider(fileConfig)
              : undefined
          const storageMode = resolveStorageMode({
            ...(options.storage === undefined ? {} : { cli: options.storage }),
            ...(envStorage === undefined ? {} : { env: envStorage }),
            ...(configuredProvider === undefined ? {} : { configuredProvider }),
            hasRedisUrl: redisUrl !== undefined,
          })
          if (storageMode === 'redis' && redisUrl === undefined) {
            throw new Error('storage mode redis requires TASKCAST_REDIS_URL or a configured Redis URL')
          }

          const dependencyHealth = new DependencyHealthRegistry()
          managedRedis =
            storageMode === 'redis'
              ? await createManagedRedisAdapters(redisUrl!, {
                  observer: dependencyHealth,
                  startupTimeoutMs: 15_000,
                })
              : undefined
          lifecycle.checkpoint()
          if (managedRedis !== undefined) {
            dependencyHealth.register('redisCommand', managedRedis.commandCheck)
            dependencyHealth.register('redisPubSub', managedRedis.pubSubCheck)
          }

          let shortTermStore: ShortTermStore
          let broadcast: BroadcastProvider
          let longTermStore: LongTermStore | undefined
          let shortTermLabel: string
          let longTermLabel = '(none)'

          if (storageMode === 'sqlite') {
            const dbPath = options.dbPath ?? './taskcast.db'
            const sqliteOpts = options.dbPath ? { path: options.dbPath } : {}
            const adapters = createSqliteAdapters(sqliteOpts)
            broadcast = new MemoryBroadcastProvider()
            shortTermStore = adapters.shortTermStore
            longTermStore = adapters.longTermStore
            shortTermLabel = `sqlite @ ${dbPath}`
            longTermLabel = `sqlite @ ${dbPath}`
          } else if (managedRedis !== undefined) {
            broadcast = managedRedis.broadcast
            shortTermStore = managedRedis.shortTermStore
            shortTermLabel = `redis @ ${formatConnectionUrlForLog(redisUrl!)}`
          } else {
            broadcast = new MemoryBroadcastProvider()
            shortTermStore = new MemoryShortTermStore()
            shortTermLabel = 'memory'
          }

          const configuredLongTerm = fileConfig.adapters?.longTermStore
          const envPostgresUrl = envNonEmpty('TASKCAST_POSTGRES_URL')
          const postgresState = postgresActivation({
            storageMode,
            ...(configuredLongTerm?.provider === undefined ? {} : { configuredProvider: configuredLongTerm.provider }),
            ...(envPostgresUrl === undefined ? {} : { envUrl: envPostgresUrl }),
            ...(configuredLongTerm?.url === undefined ? {} : { configuredUrl: configuredLongTerm.url }),
          })
          let postgresUrl: string | undefined
          if (postgresState.active) {
            postgresUrl = postgresState.url
            const max = parsePostgresMaxConnections(process.env['TASKCAST_POSTGRES_MAX_CONNECTIONS'])
            postgres_ = postgres(postgresUrl, {
              max,
              connect_timeout: 5,
            })
            await postgresCheck(postgres_)
            lifecycle.checkpoint()
            dependencyHealth.register('postgres', () => postgresCheck(postgres_!))
            longTermStore = new PostgresLongTermStore(postgres_, dependencyHealth)
            longTermLabel = `postgres @ ${formatDisplayUrl(postgresUrl)}`
          }

          // Print startup configuration summary
          console.log(`[taskcast] Config: ${configPath ?? '(none)'}`)
          console.log(`[taskcast] Short-term store: ${shortTermLabel}`)
          console.log(`[taskcast] Long-term store:  ${longTermLabel}`)

          const runStartOptions: RunStartOptions = {
            broadcast,
            shortTermStore,
            port,
            config: fileConfig,
            verbose: options.verbose ?? false,
            playground: options.playground ?? false,
            env: process.env as Record<string, string | undefined>,
            dependencyHealth,
            closeDependencies,
            ...(postgres_ === undefined ? {} : { postgres: postgres_ }),
            ...(postgresUrl === undefined ? {} : { postgresUrl }),
            ...(longTermStore === undefined ? {} : { longTermStore }),
            ...(configPath === undefined ? {} : { configPath }),
          }

          await runStartWithLifecycle(runStartOptions, lifecycle)
        } catch (err) {
          failed = true
          startupError = err
          shutdown = lifecycle.requestShutdown()
        } finally {
          lifecycle.settleStartup()
        }

        if (failed) {
          await shutdown
          if (startupError instanceof StartupCancelledError || lifecycle.cancelled) return
          const msg = startupError instanceof Error ? startupError.message : String(startupError)
          console.error(`[taskcast] ${msg}`)
          process.exit(1)
        }
      }
    )
}
