#!/usr/bin/env node
import { Command } from 'commander'
import { Redis } from 'ioredis'
import postgres from 'postgres'
import { createInterface } from 'readline'
import { mkdirSync, writeFileSync } from 'fs'
import { join } from 'path'
import { homedir } from 'os'
import {
  TaskEngine,
  loadConfigFile,
  MemoryBroadcastProvider,
  MemoryShortTermStore,
} from '@taskcast/core'
import type { BroadcastProvider, ShortTermStore, LongTermStore } from '@taskcast/core'
import { createTaskcastApp } from '@taskcast/server'
import { createRedisAdapters } from '@taskcast/redis'
import { PostgresLongTermStore } from '@taskcast/postgres'
import { createSqliteAdapters } from '@taskcast/sqlite'

const DEFAULT_CONFIG_YAML = `# Taskcast configuration
# Docs: https://github.com/weightwave/taskcast

port: 3721

# auth:
#   mode: none  # none | jwt

# adapters:
#   broadcast:
#     provider: memory  # memory | redis
#     # url: redis://localhost:6379
#   shortTerm:
#     provider: memory  # memory | redis
#     # url: redis://localhost:6379
#   longTerm:
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
        rl.close()
        const trimmed = answer.trim().toLowerCase()
        resolve(trimmed === '' || trimmed === 'y' || trimmed === 'yes')
      },
    )
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
  .action(async (options: { config?: string; port: string; storage?: string; dbPath?: string }) => {
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
    const postgresUrl = process.env['TASKCAST_POSTGRES_URL'] ?? fileConfig.adapters?.longTerm?.url

    let shortTermStore: ShortTermStore
    let broadcast: BroadcastProvider
    let longTermStore: LongTermStore | undefined

    const storage = options.storage ?? process.env['TASKCAST_STORAGE'] ?? (redisUrl ? 'redis' : 'memory')

    if (storage === 'sqlite') {
      const sqliteOpts = options.dbPath ? { path: options.dbPath } : {}
      const adapters = createSqliteAdapters(sqliteOpts)
      broadcast = new MemoryBroadcastProvider()
      shortTermStore = adapters.shortTerm
      longTermStore = adapters.longTerm
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
    const app = createTaskcastApp({
      engine,
      auth: { mode: authMode },
    })

    const { serve } = await import('@hono/node-server')
    serve({ fetch: app.fetch, port }, () => {
      console.log(`[taskcast] Server started on http://localhost:${port}`)
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

program.parse()
