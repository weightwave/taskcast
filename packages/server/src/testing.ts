export { createTaskcastApp } from './index.js'
export type { TaskcastServerOptions } from './index.js'

/**
 * Starts a real HTTP server for integration testing.
 * Requires @hono/node-server as a devDependency.
 * Returns baseUrl and a close function.
 */
export async function startTestServer(
  opts: import('./index.js').TaskcastServerOptions & { port?: number },
): Promise<{ baseUrl: string; close: () => void }> {
  const { serve } = await import('@hono/node-server')
  const { createTaskcastApp } = await import('./index.js')
  const taskcast = createTaskcastApp(opts)
  return new Promise((resolve) => {
    const server = serve({ fetch: taskcast.app.fetch, port: opts.port ?? 0 }, (info) => {
      resolve({
        baseUrl: `http://localhost:${(info as { port: number }).port}`,
        close: () => { taskcast.stop(); server.close() },
      })
    })
  })
}
