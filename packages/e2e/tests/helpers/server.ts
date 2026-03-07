import { serve } from '@hono/node-server'
import {
  TaskEngine,
  MemoryBroadcastProvider,
  MemoryShortTermStore,
  WorkerManager,
  resolveAdminToken,
} from '@taskcast/core'
import type { ShortTermStore, BroadcastProvider } from '@taskcast/core'
import { createTaskcastApp } from '@taskcast/server'
import type { TaskcastServerOptions } from '@taskcast/server'

export interface TestServer {
  baseUrl: string
  engine: TaskEngine
  workerManager?: WorkerManager
  close: () => void
}

export interface StartServerOptions {
  auth?: 'none' | 'jwt'
  jwtSecret?: string
  adminApi?: boolean
  adminToken?: string
  workers?: boolean
}

export async function startServer(opts: StartServerOptions = {}): Promise<TestServer> {
  const shortTermStore: ShortTermStore = new MemoryShortTermStore()
  const broadcast: BroadcastProvider = new MemoryBroadcastProvider()
  const engine = new TaskEngine({ shortTermStore, broadcast })

  const jwtSecret = opts.jwtSecret ?? 'e2e-test-secret-that-is-long-enough-for-HS256'

  const config = {
    adminApi: opts.adminApi ?? false,
    adminToken: opts.adminToken,
  }
  if (config.adminApi) resolveAdminToken(config)

  const serverOpts: TaskcastServerOptions = {
    engine,
    shortTermStore,
    auth: opts.auth === 'jwt'
      ? { mode: 'jwt', jwt: { algorithm: 'HS256', secret: jwtSecret } }
      : { mode: 'none' },
    config,
  }

  let workerManager: WorkerManager | undefined
  if (opts.workers) {
    workerManager = new WorkerManager({ engine, shortTermStore, broadcast })
    serverOpts.workerManager = workerManager
  }

  const { app, stop } = createTaskcastApp(serverOpts)

  return new Promise((resolve) => {
    // Port 0 = random available port
    const server = serve({ fetch: app.fetch, port: 0 }, (info) => {
      const port = (info as { port: number }).port
      resolve({
        baseUrl: `http://localhost:${port}`,
        engine,
        workerManager,
        close: () => {
          stop()
          server.close()
        },
      })
    })
  })
}