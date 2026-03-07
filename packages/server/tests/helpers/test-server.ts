import {
  TaskEngine,
  MemoryBroadcastProvider,
  MemoryShortTermStore,
  WorkerManager,
} from '@taskcast/core'
import type { LongTermStore } from '@taskcast/core'
import { createTaskcastApp } from '../../src/index.js'
import type { AuthConfig } from '../../src/auth.js'

export interface TestServerOptions {
  auth?: AuthConfig
  withWorkerManager?: boolean
  longTermStore?: LongTermStore
}

export interface TestServer {
  app: import('hono').Hono
  engine: TaskEngine
  store: MemoryShortTermStore
  broadcast: MemoryBroadcastProvider
  workerManager?: WorkerManager
  stop: () => void
}

export function createTestServer(opts?: TestServerOptions): TestServer {
  const store = new MemoryShortTermStore()
  const broadcast = new MemoryBroadcastProvider()
  const engineOpts: ConstructorParameters<typeof TaskEngine>[0] = {
    shortTermStore: store,
    broadcast,
  }
  if (opts?.longTermStore) engineOpts.longTermStore = opts.longTermStore
  const engine = new TaskEngine(engineOpts)

  let workerManager: WorkerManager | undefined
  if (opts?.withWorkerManager) {
    workerManager = new WorkerManager({ engine, shortTermStore: store, broadcast })
  }

  const taskcast = createTaskcastApp({
    engine,
    workerManager,
    auth: opts?.auth ?? { mode: 'none' },
  })

  return { app: taskcast.app, engine, store, broadcast, workerManager, stop: taskcast.stop }
}
