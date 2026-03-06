export { createAuthMiddleware, checkScope } from './auth.js'
export type { AuthConfig, AuthContext, JWTConfig } from './auth.js'
export { createTasksRouter } from './routes/tasks.js'
export { createSSERouter } from './routes/sse.js'
export { createWorkersRouter, WorkerWSHandler } from './routes/workers.js'
export type { WSLike, TaskSummary } from './routes/workers.js'
export { WebhookDelivery } from './webhook.js'
export {
  TaskSchema, TaskEventSchema, WorkerSchema, ErrorSchema,
  CreateTaskSchema, TransitionSchema, PublishEventSchema,
} from './schemas.js'

import type { Hono } from 'hono'
import { OpenAPIHono } from '@hono/zod-openapi'
import { apiReference } from '@scalar/hono-api-reference'
import { createAuthMiddleware } from './auth.js'
import { createTasksRouter } from './routes/tasks.js'
import { createSSERouter } from './routes/sse.js'
import { createWorkersRouter } from './routes/workers.js'
import type { AuthConfig } from './auth.js'
import type { TaskEngine, WorkerManager } from '@taskcast/core'

export interface TaskcastServerOptions {
  engine: TaskEngine
  workerManager?: WorkerManager
  auth?: AuthConfig
}

/**
 * Creates an OpenAPIHono app with all taskcast routes mounted.
 * Can be used standalone or mounted into an existing Hono app.
 */
export function createTaskcastApp(opts: TaskcastServerOptions): Hono {
  const app = new OpenAPIHono()
  app.get('/health', (c) => c.json({ ok: true }))
  app.use('*', createAuthMiddleware(opts.auth ?? { mode: 'none' }))
  app.route('/tasks', createTasksRouter(opts.engine))
  app.route('/tasks', createSSERouter(opts.engine))
  if (opts.workerManager) {
    app.route('/workers', createWorkersRouter(opts.workerManager, opts.engine))
  }

  // Register security scheme
  app.openAPIRegistry.registerComponent('securitySchemes', 'Bearer', {
    type: 'http',
    scheme: 'bearer',
    bearerFormat: 'JWT',
    description: 'JWT Bearer token. Required scopes vary per endpoint.',
  })

  // OpenAPI spec endpoint
  app.doc('/openapi.json', {
    openapi: '3.1.0',
    info: {
      title: 'Taskcast API',
      version: '0.3.0',
      description: 'Unified long-lifecycle task tracking service for LLM streaming, agents, and async workloads.',
    },
    security: [{ Bearer: [] }],
  })

  // API documentation UI
  app.get('/docs', apiReference({
    url: '/openapi.json',
  }))

  return app as unknown as Hono
}

/**
 * Starts a real HTTP server for integration testing.
 * Returns baseUrl and a close function.
 */
export async function startTestServer(
  opts: TaskcastServerOptions & { port?: number },
): Promise<{ baseUrl: string; close: () => void }> {
  const { serve } = await import('@hono/node-server')
  const app = createTaskcastApp(opts)
  return new Promise((resolve) => {
    const server = serve({ fetch: app.fetch, port: opts.port ?? 0 }, (info) => {
      resolve({
        baseUrl: `http://localhost:${(info as { port: number }).port}`,
        close: () => server.close(),
      })
    })
  })
}
