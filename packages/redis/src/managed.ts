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
  readinessTimeoutMs?: number
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

const REDIS_READINESS_TIMEOUT_MS = 2_000

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
  let commandGeneration = 0
  let closed = false
  let readinessCheck: Promise<void> | undefined
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
    commandGeneration += 1
    reconnectAttempt = 0
    options.observer?.observe({
      dependency: 'redisCommand',
      state: 'healthy',
    })
  }
  const onReconnecting = (delay: number) => {
    commandGeneration += 1
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
    commandGeneration += 1
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

  const check = (): Promise<void> => {
    if (readinessCheck !== undefined) return readinessCheck

    const generation = commandGeneration
    const pending = (async () => {
      let timer: ReturnType<typeof setTimeout> | undefined
      try {
        await Promise.race([
          commandClient.ping(),
          new Promise<never>((_, reject) => {
            timer = setTimeout(() => {
              if (!closed && generation === commandGeneration) {
                commandClient.disconnect(true)
              }
              reject(new DependencyUnavailableError(
                'redisCommand',
                'timeout',
                new Error('Redis readiness timed out'),
              ))
            }, options.readinessTimeoutMs ?? REDIS_READINESS_TIMEOUT_MS)
          }),
        ])
        if (closed || generation !== commandGeneration) {
          throw new DependencyUnavailableError(
            'redisCommand',
            'connection_closed',
          )
        }
        options.observer?.observe({
          dependency: 'redisCommand',
          state: 'healthy',
        })
      } catch (error) {
        throw unavailable(error) ?? error
      } finally {
        if (timer !== undefined) {
          clearTimeout(timer)
        }
      }
    })()
    readinessCheck = pending
    void pending.then(
      () => {
        if (readinessCheck === pending) readinessCheck = undefined
      },
      () => {
        if (readinessCheck === pending) readinessCheck = undefined
      },
    )
    return pending
  }

  return {
    client: commandClient,
    check,
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
      autoResubscribe: false,
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

    let restorationGeneration = 0
    let restoration:
      | { generation: number; promise: Promise<void> }
      | undefined
    let cancelRetryWait: (() => void) | undefined

    const cancelSubscriptionRestoration = () => {
      restorationGeneration += 1
      broadcast.markPatternSubscriptionUnavailable()
      cancelRetryWait?.()
      cancelRetryWait = undefined
    }
    const waitForRetry = (
      delay: number,
      generation: number,
    ): Promise<void> =>
      new Promise((resolve) => {
        const cancel = () => {
          clearTimeout(timer)
          resolve()
        }
        const timer = setTimeout(() => {
          if (cancelRetryWait === cancel) cancelRetryWait = undefined
          resolve()
        }, delay)
        cancelRetryWait = cancel
        if (closed || generation !== restorationGeneration) cancel()
      })
    const restoreSubscription = async (generation: number) => {
      let attempt = 0
      while (
        !closed
        && generation === restorationGeneration
        && subscriberClient?.status === 'ready'
      ) {
        try {
          await broadcast.startPatternSubscription()
          if (
            closed
            || generation !== restorationGeneration
            || subscriberClient?.status !== 'ready'
            || !broadcast.isPatternSubscribed()
          ) {
            return
          }
          reconnectAttempt = 0
          options.observer?.observe({
            dependency: 'redisPubSub',
            state: 'healthy',
          })
          return
        } catch (error) {
          if (
            closed
            || generation !== restorationGeneration
            || subscriberClient?.status !== 'ready'
          ) {
            return
          }
          attempt += 1
          const delay = equalJitterDelay(
            500,
            10_000,
            attempt - 1,
            options.random,
          )
          options.observer?.observe({
            dependency: 'redisPubSub',
            state: 'reconnecting',
            errorKind: classifyRedisError(error) ?? 'unavailable',
            attempt,
            nextRetryMs: delay,
          })
          await waitForRetry(delay, generation)
        }
      }
    }
    const beginSubscriptionRestoration = (): Promise<void> => {
      if (
        restoration !== undefined
        && restoration.generation === restorationGeneration
      ) {
        return restoration.promise
      }
      broadcast.markPatternSubscriptionUnavailable()
      restorationGeneration += 1
      const generation = restorationGeneration
      const promise = restoreSubscription(generation)
      restoration = { generation, promise }
      void promise.then(
        () => {
          if (restoration?.promise === promise) restoration = undefined
        },
        () => {
          if (restoration?.promise === promise) restoration = undefined
        },
      )
      return promise
    }
    const onReady = () => {
      void beginSubscriptionRestoration().catch(() => {
        // The restoration loop observes failures and keeps retrying while this
        // socket generation remains ready.
      })
    }
    const onReconnecting = (delay: number) => {
      cancelSubscriptionRestoration()
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
      cancelSubscriptionRestoration()
      options.observer?.observe({
        dependency: 'redisPubSub',
        state: 'reconnecting',
        errorKind:
          error === undefined
            ? 'connection_closed'
            : (classifyRedisError(error) ?? 'unavailable'),
      })
    }
    const onClose = () => onUnavailable()
    const onEnd = () => onUnavailable()
    const onError = (error: Error) => onUnavailable(error)
    subscriberClient.on('ready', onReady)
    subscriberClient.on('reconnecting', onReconnecting)
    subscriberClient.on('close', onClose)
    subscriberClient.on('end', onEnd)
    subscriberClient.on('error', onError)

    const removeSubscriberListeners = () => {
      subscriberClient?.off('ready', onReady)
      subscriberClient?.off('reconnecting', onReconnecting)
      subscriberClient?.off('close', onClose)
      subscriberClient?.off('end', onEnd)
      subscriberClient?.off('error', onError)
    }
    const close = async () => {
      if (closed) return
      closed = true
      cancelSubscriptionRestoration()
      removeSubscriberListeners()
      broadcast.dispose()
      subscriberClient?.disconnect(false)
      await command?.close()
    }

    try {
      await withDeadline(async () => {
        await subscriberClient!.connect()
        await beginSubscriptionRestoration()
      }, remainingStartupMs())
    } catch (error) {
      await close()
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
    if (!closed) {
      closed = true
      subscriberClient?.disconnect(false)
      await command?.close()
    }
    throw error
  }
}
