import { Redis } from 'ioredis'
import {
  DependencyUnavailableError,
  type DependencyObserver,
} from '@taskcast/core'
import { equalJitterDelay } from './backoff.js'
import { RedisBroadcastProvider } from './broadcast.js'
import { classifyRedisError } from './connectivity.js'
import type { RedisAdapterOptions } from './index.js'
import { RedisShortTermStore } from './short-term.js'

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

export interface ManagedRedisAdapters {
  broadcast: RedisBroadcastProvider
  shortTermStore: RedisShortTermStore
  commandClient: Redis
  subscriberClient: Redis
  commandCheck(): Promise<void>
  pubSubCheck(): Promise<void>
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

export async function createManagedRedisAdapters(
  url: string,
  options: ManagedRedisOptions = {},
): Promise<ManagedRedisAdapters> {
  const deadlineAt = Date.now() + (options.startupTimeoutMs ?? 15_000)
  const remainingStartupMs = () => Math.max(1, deadlineAt - Date.now())
  let command: ManagedRedisCommand | undefined
  let subscriberClient: Redis | undefined
  let closed = false

  try {
    command = await createManagedRedisCommandClient(url, {
      ...options,
      startupTimeoutMs: remainingStartupMs(),
    })

    let reconnectAttempt = 0
    subscriberClient = new Redis(url, {
      lazyConnect: true,
      autoResubscribe: true,
      enableOfflineQueue: false,
      maxRetriesPerRequest: 0,
      retryStrategy: (times) =>
        equalJitterDelay(500, 10_000, times - 1, options.random),
    })
    const broadcast = new RedisBroadcastProvider(
      command.client,
      subscriberClient,
      {
        ...(options.prefix === undefined ? {} : { prefix: options.prefix }),
        subscriptionMode: 'pattern',
        managed: true,
        ...(options.observer === undefined
          ? {}
          : { observer: options.observer }),
      },
    )
    const shortTermStore = new RedisShortTermStore(command.client, {
      ...options,
      managed: true,
    })

    const onReady = () => {
      reconnectAttempt = 0
      options.observer?.observe({
        dependency: 'redisPubSub',
        state: 'healthy',
      })
    }
    const onReconnecting = (delay: number) => {
      reconnectAttempt++
      options.observer?.observe({
        dependency: 'redisPubSub',
        state: 'reconnecting',
        attempt: reconnectAttempt,
        nextRetryMs: delay,
      })
    }
    const onUnavailable = (error?: unknown) => {
      if (closed) return
      options.observer?.observe({
        dependency: 'redisPubSub',
        state: 'reconnecting',
        errorKind:
          error === undefined
            ? 'connection_closed'
            : (classifyRedisError(error) ?? 'unavailable'),
      })
    }
    const onEnd = () => onUnavailable()
    const onError = (error: Error) => onUnavailable(error)
    subscriberClient.on('ready', onReady)
    subscriberClient.on('reconnecting', onReconnecting)
    subscriberClient.on('end', onEnd)
    subscriberClient.on('error', onError)

    const removeSubscriberListeners = () => {
      subscriberClient?.off('ready', onReady)
      subscriberClient?.off('reconnecting', onReconnecting)
      subscriberClient?.off('end', onEnd)
      subscriberClient?.off('error', onError)
    }
    const close = async () => {
      if (closed) return
      closed = true
      removeSubscriberListeners()
      subscriberClient?.disconnect(false)
      await command?.close()
    }

    try {
      await withDeadline(async () => {
        await subscriberClient!.connect()
        await broadcast.startPatternSubscription()
      }, remainingStartupMs())
    } catch (error) {
      throw new DependencyUnavailableError(
        'redisPubSub',
        classifyRedisError(error) ?? 'unavailable',
        error,
      )
    }

    return {
      broadcast,
      shortTermStore,
      commandClient: command.client,
      subscriberClient,
      commandCheck: command.check,
      async pubSubCheck() {
        if (!broadcast.isPatternSubscribed()) {
          throw new DependencyUnavailableError(
            'redisPubSub',
            'connection_closed',
          )
        }
      },
      close,
    }
  } catch (error) {
    closed = true
    subscriberClient?.disconnect(false)
    await command?.close()
    throw error
  }
}
