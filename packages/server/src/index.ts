export { createAuthMiddleware, checkScope } from './auth.js'
export type { AuthConfig, AuthContext, JWTConfig } from './auth.js'
export { createTasksRouter } from './routes/tasks.js'
export { createSSERouter } from './routes/sse.js'
export { WebhookDelivery } from './webhook.js'

import { Hono } from 'hono'
import { createAuthMiddleware } from './auth.js'
import { createTasksRouter } from './routes/tasks.js'
import { createSSERouter } from './routes/sse.js'
import type { AuthConfig } from './auth.js'
import type { TaskEngine } from '@taskcast/core'

export interface TaskcastServerOptions {
  engine: TaskEngine
  auth?: AuthConfig
}

/**
 * Creates a Hono app with all taskcast routes mounted.
 * Can be used standalone or mounted into an existing Hono app.
 */
export function createTaskcastApp(opts: TaskcastServerOptions): Hono {
  const app = new Hono()
  app.get('/health', (c) => c.json({ ok: true }))
  app.use('*', createAuthMiddleware(opts.auth ?? { mode: 'none' }))
  app.route('/tasks', createTasksRouter(opts.engine))
  app.route('/tasks', createSSERouter(opts.engine))
  return app
}
