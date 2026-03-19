import { Command } from 'commander'
import { Redis } from 'ioredis'
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
import type { BroadcastProvider, ShortTermStore, LongTermStore } from '@taskcast/core'
import { createTaskcastApp } from '@taskcast/server'
import { createRedisAdapters } from '@taskcast/redis'
import { PostgresLongTermStore } from '@taskcast/postgres'
import { createSqliteAdapters } from '@taskcast/sqlite'
import { promptCreateGlobalConfig, createDefaultGlobalConfig } from '../utils.js'

export function registerStartCommand(program: Command): void {
  program
    .command('start', { isDefault: true })
    .description('Start the taskcast server in foreground (default)')
    .option('-c, --config <path>', 'config file path')
    .option('-p, --port <port>', 'port to listen on', '3721')
    .option('-s, --storage <type>', 'storage backend: memory | redis | sqlite', 'memory')
    .option('--db-path <path>', 'SQLite database file path (default: ./taskcast.db)')
    .option('--playground', 'serve the interactive playground UI at /_playground/')
    .option('-v, --verbose', 'enable verbose logging')
    .action(async (options: { config?: string; port: string; storage?: string; dbPath?: string; playground?: boolean; verbose?: boolean }) => {
      let { config: fileConfig, source, path: configPath } = await loadConfigFile(options.config)

      if (source === 'none') {
        const shouldCreate = await promptCreateGlobalConfig()
        if (shouldCreate) {
          const createdPath = createDefaultGlobalConfig()
          if (createdPath) {
            const created = await loadConfigFile(createdPath)
            fileConfig = created.config
            source = created.source
            configPath = created.path
          }
        }
      }

      const port = Number(options.port ?? fileConfig.port ?? 3721)
      const redisUrl = process.env['TASKCAST_REDIS_URL'] ?? fileConfig.adapters?.broadcast?.url
      const postgresUrl = process.env['TASKCAST_POSTGRES_URL'] ?? fileConfig.adapters?.longTermStore?.url

      let shortTermStore: ShortTermStore
      let broadcast: BroadcastProvider
      let longTermStore: LongTermStore | undefined

      const storage = options.storage ?? process.env['TASKCAST_STORAGE'] ?? (redisUrl ? 'redis' : 'memory')

      let shortTermLabel: string
      let longTermLabel: string

      if (storage === 'sqlite') {
        const dbPath = options.dbPath ?? './taskcast.db'
        const sqliteOpts = options.dbPath ? { path: options.dbPath } : {}
        const adapters = createSqliteAdapters(sqliteOpts)
        broadcast = new MemoryBroadcastProvider()
        shortTermStore = adapters.shortTermStore
        longTermStore = adapters.longTermStore
        shortTermLabel = `sqlite @ ${dbPath}`
        longTermLabel = `sqlite @ ${dbPath}`
      } else if (storage === 'redis' || redisUrl) {
        const pubClient = new Redis(redisUrl!)
        const subClient = new Redis(redisUrl!)
        const storeClient = new Redis(redisUrl!)
        const adapters = createRedisAdapters(pubClient, subClient, storeClient)
        broadcast = adapters.broadcast
        shortTermStore = adapters.shortTermStore
        shortTermLabel = `redis @ ${redisUrl}`
        longTermLabel = '(none)'
      } else {
        broadcast = new MemoryBroadcastProvider()
        shortTermStore = new MemoryShortTermStore()
        shortTermLabel = 'memory'
        longTermLabel = '(none)'
      }

      if (storage !== 'sqlite' && postgresUrl) {
        const sql = postgres(postgresUrl)
        longTermStore = new PostgresLongTermStore(sql)
        longTermLabel = `postgres @ ${postgresUrl}`
      }

      // Print startup configuration summary
      console.log(`[taskcast] Config: ${configPath ?? '(none)'}`)
      console.log(`[taskcast] Short-term store: ${shortTermLabel}`)
      console.log(`[taskcast] Long-term store:  ${longTermLabel}`)

      const engineOpts: ConstructorParameters<typeof TaskEngine>[0] = { shortTermStore, broadcast }
      if (longTermStore !== undefined) engineOpts.longTermStore = longTermStore
      const engine = new TaskEngine(engineOpts)

      const authMode = (process.env['TASKCAST_AUTH_MODE'] ?? fileConfig.auth?.mode ?? 'none') as 'none' | 'jwt'

      // Worker assignment system
      const workersEnabled = fileConfig.workers?.enabled ?? false
      let workerManager: WorkerManager | undefined
      if (workersEnabled) {
        console.log('[taskcast] Worker assignment system enabled')
        const wmOpts: ConstructorParameters<typeof WorkerManager>[0] = {
          engine,
          shortTermStore,
          broadcast,
        }
        if (longTermStore !== undefined) wmOpts.longTermStore = longTermStore
        if (fileConfig.workers?.defaults) wmOpts.defaults = fileConfig.workers.defaults
        workerManager = new WorkerManager(wmOpts)
      }

      // Resolve admin token (auto-generate + print if adminApi is enabled)
      resolveAdminToken(fileConfig)

      const serverOpts: Parameters<typeof createTaskcastApp>[0] = {
        engine,
        shortTermStore,
        auth: { mode: authMode },
        config: fileConfig,
        verbose: options.verbose ?? false,
      }
      if (workerManager !== undefined) serverOpts.workerManager = workerManager
      const { app, stop } = createTaskcastApp(serverOpts)

      // Serve playground static files if --playground and dist exists
      if (options.playground) {
        try {
          const require = createRequire(import.meta.url)
          const pkgPath = require.resolve('@taskcast/playground/package.json')
          const distDir = join(dirname(pkgPath), 'dist')
          if (existsSync(distDir)) {
            const { serveStatic } = await import('@hono/node-server/serve-static')
            app.use('/_playground/*', serveStatic({ root: distDir, rewriteRequestPath: (p) => p.replace(/^\/_playground/, '') }))
            // SPA fallback: serve index.html for non-asset paths
            app.get('/_playground/*', serveStatic({ root: distDir, rewriteRequestPath: () => '/index.html' }))
          } else {
            console.warn('[taskcast] Playground dist not found. Run `pnpm --filter @taskcast/playground build` first.')
          }
        } catch {
          console.warn('[taskcast] @taskcast/playground not available, skipping playground UI.')
        }
      }

      const { serve } = await import('@hono/node-server')
      const server = serve({ fetch: app.fetch, port }, () => {
        console.log(`[taskcast] Server started on http://localhost:${port}`)
        if (options.playground) {
          console.log(`[taskcast] Playground UI at http://localhost:${port}/_playground/`)
        }
      })

      // Clean up scheduler/heartbeat on shutdown
      process.on('SIGTERM', () => { stop(); (server as { close?: () => void }).close?.() })
      process.on('SIGINT', () => { stop(); (server as { close?: () => void }).close?.() })
    })
}
