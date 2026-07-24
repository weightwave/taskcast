import { Redis } from 'ioredis'
import {
  DependencyUnavailableError,
  type DependencyErrorKind,
  type DependencyObserver,
} from '@taskcast/core'
import { equalJitterDelay } from './backoff.js'
import type { RedisAdapterOptions } from './index.js'

export interface ManagedRedisOptions extends RedisAdapterOptions {
  observer?: DependencyObserver
  startupTimeoutMs?: number
  random?: () => number
}

export interface ManagedRedisCommand {
  client: Redis
  check(): Promise<void>
  close(): Promise<void>
}

async function withDeadline<T>(
  operation: () => Promise<T>,
  timeoutMs: number,
): Promise<T> {
  let timer: ReturnType<typeof setTimeout> | undefined
  try {
    return await Promise.race([
      operation(),
      new Promise<never>((_, reject) => {
        timer = setTimeout(
          () => reject(new Error('Redis startup timed out')),
          timeoutMs,
        )
      }),
    ])
  } finally {
    if (timer !== undefined) clearTimeout(timer)
  }
}

function classifyRedisError(error: unknown): DependencyErrorKind | undefined {
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
    message.includes("stream isn't writeable")
  ) {
    return 'connection_closed'
  }
  return undefined
}

function unavailable(error: unknown): DependencyUnavailableError | undefined {
  const kind = classifyRedisError(error)
  return kind === undefined
    ? undefined
    : new DependencyUnavailableError('redisCommand', kind, error)
}

export async function createManagedRedisCommandClient(
  url: string,
  options: ManagedRedisOptions = {},
): Promise<ManagedRedisCommand> {
  let reconnectAttempt = 0
  let closed = false
  const retryDelay = (times: number) =>
    equalJitterDelay(500, 5_000, times - 1, options.random)
  const commandClient = new Redis(url, {
    lazyConnect: true,
    enableReadyCheck: false,
    enableOfflineQueue: false,
    autoResendUnfulfilledCommands: false,
    maxRetriesPerRequest: 0,
    retryStrategy: retryDelay,
  })

  const onReady = () => {
    reconnectAttempt = 0
    options.observer?.observe({
      dependency: 'redisCommand',
      state: 'healthy',
    })
  }
  const onReconnecting = (delay: number) => {
    reconnectAttempt++
    options.observer?.observe({
      dependency: 'redisCommand',
      state: 'reconnecting',
      attempt: reconnectAttempt,
      nextRetryMs: delay,
    })
  }
  const onUnavailable = (error?: unknown) => {
    if (closed) return
    const kind =
      error === undefined ? 'connection_closed' : classifyRedisError(error)
    if (kind === undefined) return
    options.observer?.observe({
      dependency: 'redisCommand',
      state: 'reconnecting',
      errorKind: kind,
    })
  }
  const onClose = () => onUnavailable()
  const onEnd = () => onUnavailable()
  const onError = (error: Error) => onUnavailable(error)

  commandClient.on('ready', onReady)
  commandClient.on('reconnecting', onReconnecting)
  commandClient.on('close', onClose)
  commandClient.on('end', onEnd)
  commandClient.on('error', onError)

  const removeListeners = () => {
    commandClient.off('ready', onReady)
    commandClient.off('reconnecting', onReconnecting)
    commandClient.off('close', onClose)
    commandClient.off('end', onEnd)
    commandClient.off('error', onError)
  }

  const close = async () => {
    if (closed) return
    closed = true
    removeListeners()
    commandClient.disconnect(false)
  }

  try {
    await withDeadline(async () => {
      await commandClient.connect()
      await commandClient.ping()
    }, options.startupTimeoutMs ?? 15_000)
  } catch (error) {
    await close()
    throw unavailable(error) ?? error
  }

  return {
    client: commandClient,
    async check() {
      try {
        await commandClient.ping()
        options.observer?.observe({
          dependency: 'redisCommand',
          state: 'healthy',
        })
      } catch (error) {
        throw unavailable(error) ?? error
      }
    },
    close,
  }
}
