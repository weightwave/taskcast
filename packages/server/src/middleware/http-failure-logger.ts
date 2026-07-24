import type { MiddlewareHandler } from 'hono'

export type LogLevel = 'debug' | 'info' | 'warn' | 'error'
export type HttpFailureKind = 'store' | 'archive' | 'internal'

export interface HttpFailureLog {
  timestamp: string
  level: 'error'
  event: 'http_request_failed'
  method: string
  path: string
  status: number
  errorKind?: HttpFailureKind
  error?: string
}

export type HttpFailureLogger = (record: HttpFailureLog) => void

export interface HttpFailureLoggerOptions {
  logLevel?: LogLevel
  logger?: HttpFailureLogger
}

const MAX_ERROR_SCALARS = 2048
const URL_USERINFO = /([a-z][a-z0-9+.-]*:\/\/)[^@\s/]+@/giu

export function parseLogLevel(value?: string): LogLevel {
  const normalized = value?.trim().toLowerCase() || 'info'
  if (
    normalized === 'debug' ||
    normalized === 'info' ||
    normalized === 'warn' ||
    normalized === 'error'
  ) {
    return normalized
  }
  throw new Error(
    `invalid TASKCAST_LOG_LEVEL "${value}"; expected debug, info, warn, or error`,
  )
}

export function sanitizeErrorMessage(value: string): string | undefined {
  const redacted = value.replace(URL_USERINFO, '$1***@')
  const truncated = Array.from(redacted).slice(0, MAX_ERROR_SCALARS).join('')
  return truncated.length > 0 ? truncated : undefined
}

function inferErrorKind(path: string): HttpFailureKind {
  if (path === '/tasks/import' || /\/tasks\/[^/]+\/archive$/.test(path)) {
    return 'archive'
  }
  if (
    path === '/tasks' ||
    path.startsWith('/tasks/') ||
    path === '/events' ||
    path.startsWith('/workers')
  ) {
    return 'store'
  }
  return 'internal'
}

function errorMessage(error?: Error): string | undefined {
  return error ? sanitizeErrorMessage(error.message) : undefined
}

function defaultLogger(record: HttpFailureLog): void {
  console.error(JSON.stringify(record))
}

export function createHttpFailureLogger(
  options: HttpFailureLoggerOptions = {},
): MiddlewareHandler {
  const logger = options.logger ?? defaultLogger

  function emit(
    method: string,
    path: string,
    status: number,
    error?: unknown,
  ): void {
    const typedError = error instanceof Error ? error : undefined
    const message = errorMessage(typedError)
    const record: HttpFailureLog = {
      timestamp: new Date().toISOString(),
      level: 'error',
      event: 'http_request_failed',
      method,
      path,
      status,
    }
    if (typedError !== undefined) record.errorKind = inferErrorKind(path)
    if (message !== undefined) record.error = message
    logger(record)
  }

  return async (c, next) => {
    const method = c.req.method
    const path = c.req.path
    await next()

    if (c.res.status >= 500 && c.res.status <= 599) {
      emit(method, path, c.res.status, c.error)
    }
  }
}
