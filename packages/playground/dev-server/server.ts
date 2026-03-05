import { serve } from '@hono/node-server'
import { Hono } from 'hono'
import { cors } from 'hono/cors'
import { TaskEngine, MemoryBroadcastProvider, MemoryShortTermStore, WorkerManager } from '@taskcast/core'
import { createTaskcastApp } from '@taskcast/server'
import { createServer } from 'vite'

async function main() {
  const broadcast = new MemoryBroadcastProvider()
  const shortTermStore = new MemoryShortTermStore()

  const engine = new TaskEngine({
    broadcast,
    shortTermStore,
  })

  const workerManager = new WorkerManager({
    engine,
    shortTermStore,
    broadcast,
  })

  const taskcastApp = createTaskcastApp({ engine, workerManager })

  const app = new Hono()
  app.use('*', cors())
  app.route('/taskcast', taskcastApp)

  const taskcastPort = 3721
  serve({ fetch: app.fetch, port: taskcastPort }, (info) => {
    console.log(`Taskcast server running at http://localhost:${info.port}`)
  })

  const vite = await createServer({
    configFile: new URL('../vite.config.ts', import.meta.url).pathname,
    root: new URL('..', import.meta.url).pathname,
    server: { port: 5173 },
  })
  await vite.listen()
  console.log(`Playground UI at http://localhost:5173`)

  process.on('SIGINT', async () => {
    await vite.close()
    process.exit(0)
  })
}

main().catch(console.error)
