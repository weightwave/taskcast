import {
  DependencyUnavailableError,
  findDependencyUnavailableError,
  type DependencyErrorKind,
  type DependencyObserver,
} from '@taskcast/core'

export interface RedisOperationOptions {
  /** Enables the managed public-operation contract without affecting raw adapters. */
  managed?: boolean
  observer?: DependencyObserver
}

export function classifyRedisError(
  error: unknown,
): DependencyErrorKind | undefined {
  if (!(error instanceof Error)) return undefined
  const code = (error as NodeJS.ErrnoException).code
  if (code === 'ECONNREFUSED') return 'connection_refused'
  if (code === 'ECONNRESET' || code === 'EPIPE') return 'connection_reset'
  if (code === 'ETIMEDOUT' || code === 'ESOCKETTIMEDOUT') return 'timeout'
  if (code === 'ENOTFOUND' || code === 'EAI_AGAIN') return 'dns'
  if (code === 'NR_CLOSED' || code === 'CONNECTION_CLOSED') {
    return 'connection_closed'
  }

  const message = error.message.toLowerCase()
  if (
    message.includes('wrongpass') ||
    message.includes('noauth') ||
    message.includes('authentication')
  ) {
    return 'authentication'
  }
  if (message.includes('connect timeout') || message.includes('timed out')) {
    return 'timeout'
  }
  if (
    message.includes('connection is closed') ||
    message.includes('connection closed') ||
    message.includes("stream isn't writeable") ||
    message.includes('max retries per request limit')
  ) {
    return 'connection_closed'
  }
  return undefined
}

export async function observeRedisCommand<T>(
  options: RedisOperationOptions,
  operation: () => Promise<T>,
): Promise<T> {
  if (!options.managed) return operation()

  try {
    const result = await operation()
    options.observer?.observe({
      dependency: 'redisCommand',
      state: 'healthy',
    })
    return result
  } catch (error) {
    if (findDependencyUnavailableError(error)) throw error
    const kind = classifyRedisError(error)
    if (!kind) throw error
    options.observer?.observe({
      dependency: 'redisCommand',
      state: 'reconnecting',
      errorKind: kind,
    })
    throw new DependencyUnavailableError('redisCommand', kind, error)
  }
}
