import type { MiddlewareHandler } from 'hono'

/**
 * Creates a Hono middleware that logs every HTTP request in a human-friendly
 * format.  Designed for `taskcast start --verbose`.
 *
 * ```
 * [2026-03-07 14:32:01] POST   /tasks                    → 201  12ms  (task: 01JXXXXX)
 * [2026-03-07 14:32:02] PATCH  /tasks/01JXXXXX/status    → 200   3ms  (pending → running)
 * [2026-03-07 14:32:02] POST   /tasks/01JXXXXX/events    → 200   2ms  (type: llm.delta)
 * [2026-03-07 14:32:03] GET    /tasks/01JXXXXX/events    → SSE   0ms  (subscriber connected)
 * ```
 */
export function createVerboseLogger(
  logger: (line: string) => void = console.log,
): MiddlewareHandler {
  return async (c, next) => {
    const start = Date.now()
    const method = c.req.method
    const path = c.req.path

    const MAX_BODY_SIZE = 64 * 1024

    // Check Content-Length to decide if we should parse the body
    const contentLengthRaw = c.req.header('content-length')
    const contentLength = contentLengthRaw ? parseInt(contentLengthRaw, 10) : NaN
    const bodyTooLarge = !isNaN(contentLength) && contentLength > MAX_BODY_SIZE

    // Capture request body for context extraction (clone to avoid consuming)
    let requestBody: Record<string, unknown> | undefined
    if (method === 'POST' || method === 'PATCH' || method === 'PUT') {
      if (!bodyTooLarge) {
        try {
          requestBody = await c.req.raw.clone().json()
        } catch {
          // not JSON — ignore
        }
      }
    }

    await next()

    const duration = Date.now() - start
    const status = c.res.status

    // Determine if this is an SSE endpoint
    const isSSE = path.match(/\/tasks\/[^/]+\/events$/) && method === 'GET'
    const isGlobalSSE = path === '/events' && method === 'GET'
    const statusStr = isSSE || isGlobalSSE ? 'SSE' : String(status)

    // Extract contextual info
    const context = extractContext(method, path, status, requestBody, c.res, bodyTooLarge)

    const timestamp = new Date().toISOString().replace('T', ' ').slice(0, 19)

    logger(
      `[${timestamp}] ${method.padEnd(6)} ${path.padEnd(35)} \u2192 ${statusStr.padStart(3)}  ${String(duration).padStart(3)}ms${context ? '  (' + context + ')' : ''}`,
    )
  }
}

function extractContext(
  method: string,
  path: string,
  status: number,
  requestBody: Record<string, unknown> | undefined,
  _res: Response,
  bodyTooLarge = false,
): string {
  // If the body was too large to parse, note it in context
  if (bodyTooLarge && (method === 'POST' || method === 'PATCH' || method === 'PUT')) {
    return 'body too large to log'
  }

  // POST /tasks → show task ID (from response path or status)
  if (method === 'POST' && path === '/tasks' && status === 201) {
    return 'task created'
  }

  // PATCH /tasks/:id/status → show status transition
  if (method === 'PATCH' && path.match(/\/tasks\/[^/]+\/status$/)) {
    const targetStatus = requestBody?.status
    if (targetStatus) {
      return `\u2192 ${targetStatus}`
    }
    return 'status transition'
  }

  // POST /tasks/:id/events → show event type
  if (method === 'POST' && path.match(/\/tasks\/[^/]+\/events$/)) {
    const eventType = requestBody?.type
    if (eventType) {
      return `type: ${eventType}`
    }
    // Batch events
    if (Array.isArray(requestBody)) {
      return `${requestBody.length} events`
    }
    return 'event published'
  }

  // GET /tasks/:id/events → SSE subscriber
  if (method === 'GET' && path.match(/\/tasks\/[^/]+\/events$/)) {
    return 'subscriber connected'
  }

  // GET /events → global SSE
  if (method === 'GET' && path === '/events') {
    return 'global subscriber connected'
  }

  // POST /tasks/:id/resolve → resolve blocked task
  if (method === 'POST' && path.match(/\/tasks\/[^/]+\/resolve$/)) {
    return 'resolve'
  }

  return ''
}
