#!/usr/bin/env node
import { Command } from 'commander'
import { Redis } from 'ioredis'
import postgres from 'postgres'
import { createInterface } from 'readline'
import { mkdirSync, writeFileSync, existsSync } from 'fs'
import { join, dirname } from 'path'
import { fileURLToPath } from 'url'
import { homedir } from 'os'
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
import { PostgresLongTermStore, loadMigrationFiles, runMigrations } from '@taskcast/postgres'
import { createSqliteAdapters } from '@taskcast/sqlite'
import { resolvePostgresUrl, formatDisplayUrl } from './migrate-helpers.js'

const DEFAULT_CONFIG_YAML = `# Taskcast configuration
# Docs: https://github.com/weightwave/taskcast

port: 3721

# auth:
#   mode: none  # none | jwt

# adapters:
#   broadcast:
#     provider: memory  # memory | redis
#     # url: redis://localhost:6379
#   shortTermStore:
#     provider: memory  # memory | redis
#     # url: redis://localhost:6379
#   longTermStore:
#     provider: postgres
#     # url: postgresql://localhost:5432/taskcast
`

async function promptCreateGlobalConfig(): Promise<boolean> {
  if (!process.stdin.isTTY) return false

  const globalConfigPath = join(homedir(), '.taskcast', 'taskcast.config.yaml')

  return new Promise((resolve) => {
    const rl = createInterface({ input: process.stdin, output: process.stdout })
    rl.on('close', () => resolve(false))
    rl.question(
      `[taskcast] No config file found.\n? Create a default config at ${globalConfigPath}? (Y/n) `,
      (answer) => {
        const trimmed = answer.trim().toLowerCase()
        resolve(trimmed === '' || trimmed === 'y' || trimmed === 'yes')
        rl.close()
      },
    )
  })
}

async function promptConfirm(message: string): Promise<boolean> {
  if (!process.stdin.isTTY) return false

  return new Promise((resolve) => {
    const rl = createInterface({ input: process.stdin, output: process.stdout })
    rl.on('close', () => resolve(false))
    rl.question(message, (answer) => {
      const trimmed = answer.trim().toLowerCase()
      resolve(trimmed === '' || trimmed === 'y' || trimmed === 'yes')
      rl.close()
    })
  })
}

function createDefaultGlobalConfig(): string | null {
  const globalDir = join(homedir(), '.taskcast')
  const globalConfigPath = join(globalDir, 'taskcast.config.yaml')
  try {
    mkdirSync(globalDir, { recursive: true })
    writeFileSync(globalConfigPath, DEFAULT_CONFIG_YAML)
    console.log(`[taskcast] Created default config at ${globalConfigPath}`)
    return globalConfigPath
  } catch (err) {
    console.warn(`[taskcast] Could not create config at ${globalConfigPath}: ${(err as Error).message}`)
    return null
  }
}

const program = new Command()

program
  .name('taskcast')
  .description('Taskcast — unified task tracking and streaming service')
  .version('0.1.0')

program
  .command('start', { isDefault: true })
  .description('Start the taskcast server in foreground (default)')
  .option('-c, --config <path>', 'config file path')
  .option('-p, --port <port>', 'port to listen on', '3721')
  .option('-s, --storage <type>', 'storage backend: memory | redis | sqlite', 'memory')
  .option('--db-path <path>', 'SQLite database file path (default: ./taskcast.db)')
  .option('--playground', 'serve the interactive playground UI at /_playground/')
  .action(async (options: { config?: string; port: string; storage?: string; dbPath?: string; playground?: boolean }) => {
    let { config: fileConfig, source } = await loadConfigFile(options.config)

    if (source === 'none') {
      const shouldCreate = await promptCreateGlobalConfig()
      if (shouldCreate) {
        const createdPath = createDefaultGlobalConfig()
        if (createdPath) {
          const created = await loadConfigFile(createdPath)
          fileConfig = created.config
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

    if (storage === 'sqlite') {
      const sqliteOpts = options.dbPath ? { path: options.dbPath } : {}
      const adapters = createSqliteAdapters(sqliteOpts)
      broadcast = new MemoryBroadcastProvider()
      shortTermStore = adapters.shortTermStore
      longTermStore = adapters.longTermStore
      console.log(`[taskcast] Using SQLite storage at ${options.dbPath ?? './taskcast.db'}`)
    } else if (storage === 'redis' || redisUrl) {
      const pubClient = new Redis(redisUrl!)
      const subClient = new Redis(redisUrl!)
      const storeClient = new Redis(redisUrl!)
      const adapters = createRedisAdapters(pubClient, subClient, storeClient)
      broadcast = adapters.broadcast
      shortTermStore = adapters.shortTermStore
    } else {
      console.warn('[taskcast] No TASKCAST_REDIS_URL configured — using in-memory adapters')
      broadcast = new MemoryBroadcastProvider()
      shortTermStore = new MemoryShortTermStore()
    }

    if (storage !== 'sqlite' && postgresUrl) {
      const sql = postgres(postgresUrl)
      longTermStore = new PostgresLongTermStore(sql)
    }

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

program
  .command('playground')
  .description('Serve only the playground UI (no engine)')
  .option('-p, --port <port>', 'port to listen on', '5173')
  .action(async (options: { port: string }) => {
    const port = Number(options.port)
    try {
      const require = createRequire(import.meta.url)
      const pkgPath = require.resolve('@taskcast/playground/package.json')
      const distDir = join(dirname(pkgPath), 'dist')
      if (!existsSync(distDir)) {
        console.error('[taskcast] Playground dist not found. Run `pnpm --filter @taskcast/playground build` first.')
        process.exit(1)
      }
      const { OpenAPIHono } = await import('@hono/zod-openapi')
      const app = new OpenAPIHono()
      const { serveStatic } = await import('@hono/node-server/serve-static')
      app.use('/_playground/*', serveStatic({ root: distDir, rewriteRequestPath: (p: string) => p.replace(/^\/_playground/, '') }))
      app.get('/_playground/*', serveStatic({ root: distDir, rewriteRequestPath: () => '/index.html' }))
      app.get('/', (c: { redirect: (url: string) => Response }) => c.redirect('/_playground/'))
      const { serve } = await import('@hono/node-server')
      serve({ fetch: app.fetch, port }, () => {
        console.log(`[taskcast] Playground UI at http://localhost:${port}/_playground/`)
        console.log('[taskcast] Use "External" mode in the UI to connect to a remote server.')
      })
    } catch {
      console.error('[taskcast] @taskcast/playground not available.')
      process.exit(1)
    }
  })

program
  .command('ui')
  .alias('dashboard')
  .description('Start the Taskcast Dashboard web UI')
  .option('-p, --port <port>', 'Dashboard port', '3722')
  .option('-s, --server <url>', 'Taskcast server URL', 'http://localhost:3721')
  .option('--admin-token <token>', 'Admin token for auto-connect')
  .action(async (opts: { port: string; server: string; adminToken?: string }) => {
    const { dashboardDistPath } = await import('@taskcast/dashboard-web/dist-path')
    const { Hono } = await import('hono')
    const { serve } = await import('@hono/node-server')
    const { existsSync, readFileSync, statSync } = await import('fs')
    const { join: joinPath, extname, resolve: resolvePath } = await import('path')

    if (!existsSync(dashboardDistPath)) {
      console.error(
        '[taskcast] Dashboard not built. Run: pnpm --filter @taskcast/dashboard-web build',
      )
      process.exit(1)
    }

    // Exchange admin token for JWT at startup (never expose raw token to browser)
    let dashboardJwt: string | undefined
    if (opts.adminToken) {
      try {
        const tokenRes = await fetch(`${opts.server}/admin/token`, {
          method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify({ adminToken: opts.adminToken }),
        })
        if (tokenRes.ok) {
          const { token } = await tokenRes.json()
          dashboardJwt = token
        } else if (tokenRes.status === 404) {
          // Admin API not enabled — server may be in auth: none mode
          console.log('[taskcast] Admin API not enabled, connecting without JWT')
        } else {
          console.error(`[taskcast] Failed to exchange admin token: ${tokenRes.status}`)
          process.exit(1)
        }
      } catch (err) {
        console.error(`[taskcast] Cannot reach server at ${opts.server}: ${err instanceof Error ? err.message : err}`)
        process.exit(1)
      }
    }

    const MIME_TYPES: Record<string, string> = {
      '.html': 'text/html',
      '.js': 'application/javascript',
      '.css': 'text/css',
      '.json': 'application/json',
      '.png': 'image/png',
      '.jpg': 'image/jpeg',
      '.jpeg': 'image/jpeg',
      '.gif': 'image/gif',
      '.svg': 'image/svg+xml',
      '.ico': 'image/x-icon',
      '.woff': 'font/woff',
      '.woff2': 'font/woff2',
      '.ttf': 'font/ttf',
      '.eot': 'application/vnd.ms-fontobject',
      '.map': 'application/json',
    }

    const app = new Hono()

    // Auto-connect config endpoint (never exposes admin token — only pre-exchanged JWT)
    app.get('/api/config', (c) => {
      return c.json({
        baseUrl: opts.server,
        token: dashboardJwt,
      })
    })

    // Serve static dashboard files with SPA fallback
    app.get('*', (c) => {
      const urlPath = decodeURIComponent(new URL(c.req.url).pathname)
      const resolved = resolvePath(joinPath(dashboardDistPath, urlPath))

      // Path traversal protection: ensure resolved path stays within dashboard dist
      if (!resolved.startsWith(dashboardDistPath)) {
        return c.text('Not Found', 404)
      }

      let filePath = resolved

      // Try the exact file first, then fall back to index.html (SPA)
      const isFile = existsSync(filePath) && statSync(filePath).isFile()
      if (!isFile) {
        filePath = joinPath(dashboardDistPath, 'index.html')
      }

      if (!existsSync(filePath)) {
        return c.text('Not Found', 404)
      }

      const ext = extname(filePath)
      const contentType = MIME_TYPES[ext] ?? 'application/octet-stream'
      const body = readFileSync(filePath)
      return c.body(body, 200, { 'Content-Type': contentType })
    })

    const port = Number(opts.port)
    serve({ fetch: app.fetch, port }, () => {
      console.log(`[taskcast] Dashboard running at http://localhost:${port}`)
      console.log(`[taskcast] Connected to server: ${opts.server}`)
      if (opts.adminToken) {
        console.log(`[taskcast] Admin token provided for auto-connect`)
      }
    })
  })

program
  .command('daemon')
  .description('Start the server as a background service (not yet implemented)')
  .action(() => {
    console.error('[taskcast] daemon mode is not yet implemented, use `taskcast start` for foreground mode')
    process.exit(1)
  })

program
  .command('stop')
  .description('Stop the background service (not yet implemented)')
  .action(() => {
    console.error('[taskcast] stop is not yet implemented')
    process.exit(1)
  })

program
  .command('status')
  .description('Show server status (not yet implemented)')
  .action(() => {
    console.error('[taskcast] status is not yet implemented')
    process.exit(1)
  })

program
  .command('migrate')
  .description('Run pending PostgreSQL migrations')
  .option('--url <url>', 'Postgres connection URL (highest priority)')
  .option('-c, --config <path>', 'config file path')
  .option('-y, --yes', 'skip confirmation prompt')
  .action(async (options: { url?: string; config?: string; yes?: boolean }) => {
    // URL resolution priority: --url flag > env var > config file
    const { config: fileConfig } = await loadConfigFile(options.config)
    const pgUrl = resolvePostgresUrl({
      url: options.url,
      envUrl: process.env['TASKCAST_POSTGRES_URL'],
      configUrl: fileConfig.adapters?.longTermStore?.url,
    })

    if (!pgUrl) {
      console.error('[taskcast] No Postgres URL found. Provide --url, set TASKCAST_POSTGRES_URL, or configure adapters.longTermStore.url in config.')
      process.exit(1)
    }

    const target = formatDisplayUrl(pgUrl)

    // TODO: This path works in the monorepo only. For npm publishing,
    // migrations would need to be bundled with the package.
    const migrationsDir = join(dirname(fileURLToPath(import.meta.url)), '../../../migrations/postgres')

    const sql = postgres(pgUrl)
    try {
      // Load migration files and check what's pending
      const allFiles = loadMigrationFiles(migrationsDir)

      // Ensure the migrations table exists so we can query applied versions
      await sql.unsafe(`
        CREATE TABLE IF NOT EXISTS _sqlx_migrations (
            version BIGINT PRIMARY KEY,
            description TEXT NOT NULL,
            installed_on TIMESTAMPTZ NOT NULL DEFAULT now(),
            success BOOLEAN NOT NULL,
            checksum BYTEA NOT NULL,
            execution_time BIGINT NOT NULL
        )
      `)
      const appliedRows = await sql.unsafe('SELECT version FROM _sqlx_migrations WHERE success = true')
      const appliedVersions = new Set(appliedRows.map((r) => Number(r['version'])))
      const pending = allFiles.filter((f) => !appliedVersions.has(f.version))

      if (pending.length === 0) {
        console.log('[taskcast] Database is up to date.')
        return
      }

      console.log(`[taskcast] Target: ${target}`)
      console.log(`[taskcast] Pending migrations:`)
      for (const file of pending) {
        console.log(`  ${file.filename}`)
      }

      if (!options.yes) {
        if (!process.stdin.isTTY) {
          console.error('[taskcast] No TTY detected. Re-run with --yes (-y) to skip confirmation.')
          process.exit(1)
        }
        const confirmed = await promptConfirm(`Apply ${pending.length} migration(s) to ${target}? (Y/n) `)
        if (!confirmed) {
          console.log('[taskcast] Migration cancelled.')
          return
        }
      }

      const result = await runMigrations(sql, migrationsDir)

      for (const filename of result.applied) {
        console.log(`  Applied ${filename}`)
      }
      console.log(`[taskcast] Applied ${result.applied.length} migration(s) successfully.`)
    } catch (err) {
      console.error(`[taskcast] Migration failed: ${(err as Error).message}`)
      process.exit(1)
    } finally {
      await sql.end()
    }
  })

program.parse()
