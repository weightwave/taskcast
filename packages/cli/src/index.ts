#!/usr/bin/env node
import { Command } from 'commander'
import { Redis } from 'ioredis'
import postgres from 'postgres'
import {
  TaskEngine,
  loadConfigFile,
  MemoryBroadcastProvider,
  MemoryShortTermStore,
} from '@taskcast/core'
import type { BroadcastProvider, ShortTermStore } from '@taskcast/core'
import { createTaskcastApp } from '@taskcast/server'
import { createRedisAdapters } from '@taskcast/redis'
import { PostgresLongTermStore } from '@taskcast/postgres'

const program = new Command()

program
  .name('taskcast')
  .description('Taskcast — unified task tracking and streaming service')
  .version('0.1.0')

program
  .command('start', { isDefault: true })
  .description('Start the taskcast server')
  .option('-c, --config <path>', 'config file path')
  .option('-p, --port <port>', 'port to listen on', '3721')
  .action(async (options: { config?: string; port: string }) => {
    const fileConfig = await loadConfigFile(options.config)

    const port = Number(process.env['TASKCAST_PORT'] ?? options.port ?? fileConfig.port ?? 3721)
    const redisUrl = process.env['TASKCAST_REDIS_URL'] ?? fileConfig.adapters?.broadcast?.url
    const postgresUrl = process.env['TASKCAST_POSTGRES_URL'] ?? fileConfig.adapters?.longTerm?.url

    let shortTerm: ShortTermStore
    let broadcast: BroadcastProvider
    let longTerm: InstanceType<typeof PostgresLongTermStore> | undefined

    if (redisUrl) {
      const pubClient = new Redis(redisUrl)
      const subClient = new Redis(redisUrl)
      const storeClient = new Redis(redisUrl)
      const adapters = createRedisAdapters(pubClient, subClient, storeClient)
      broadcast = adapters.broadcast
      shortTerm = adapters.shortTerm
    } else {
      console.warn('[taskcast] No TASKCAST_REDIS_URL configured — using in-memory adapters')
      broadcast = new MemoryBroadcastProvider()
      shortTerm = new MemoryShortTermStore()
    }

    if (postgresUrl) {
      const sql = postgres(postgresUrl)
      longTerm = new PostgresLongTermStore(sql)
    }

    const engineOpts: ConstructorParameters<typeof TaskEngine>[0] = { shortTerm, broadcast }
    if (longTerm !== undefined) engineOpts.longTerm = longTerm
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

program.parse()
