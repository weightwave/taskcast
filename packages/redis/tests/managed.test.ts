import { afterAll, beforeAll, describe, expect, it, vi } from 'vitest'
import { Redis } from 'ioredis'
import { GenericContainer, type StartedTestContainer } from 'testcontainers'
import type { DependencyObservation, Task, TaskEvent } from '@taskcast/core'
import { equalJitterDelay } from '../src/backoff.js'
import {
  createManagedRedisAdapters,
  createManagedRedisCommandClient,
} from '../src/managed.js'
import {
  createRedisAdapters,
  RedisBroadcastProvider,
  RedisShortTermStore,
} from '../src/index.js'
import {
  redisCommandMatcher,
  TcpFaultProxy,
} from './helpers/tcp-fault-proxy.js'

let container: StartedTestContainer
let proxy: TcpFaultProxy
let redisUrl: string

const makeEvent = (): TaskEvent => ({
  id: 'event-1',
  taskId: 'task-1',
  index: 0,
  timestamp: Date.now(),
  type: 'managed.event',
  level: 'info',
  data: { text: 'managed' },
})

const makeTask = (id: string): Task => ({
  id,
  status: 'pending',
  params: { prompt: 'managed recovery' },
  createdAt: Date.now(),
  updatedAt: Date.now(),
})

async function eventually(
  operation: () => void | Promise<void>,
  timeoutMs = 10_000,
): Promise<void> {
  const deadline = Date.now() + timeoutMs
  let lastError: unknown
  while (Date.now() < deadline) {
    try {
      await operation()
      return
    } catch (error) {
      lastError = error
      await new Promise((resolve) => setTimeout(resolve, 25))
    }
  }
  throw lastError
}

async function withDeadline<T>(
  operation: Promise<T>,
  timeoutMs: number,
  message: string,
): Promise<T> {
  let timer: ReturnType<typeof setTimeout> | undefined
  try {
    return await Promise.race([
      operation,
      new Promise<never>((_, reject) => {
        timer = setTimeout(() => reject(new Error(message)), timeoutMs)
      }),
    ])
  } finally {
    if (timer !== undefined) clearTimeout(timer)
  }
}

async function settleBeforeDeadline<T>(
  operation: Promise<T>,
  timeoutMs: number,
  message: string,
): Promise<PromiseSettledResult<T>> {
  return withDeadline(
    operation.then(
      (value): PromiseFulfilledResult<T> => ({ status: 'fulfilled', value }),
      (reason): PromiseRejectedResult => ({ status: 'rejected', reason }),
    ),
    timeoutMs,
    message,
  )
}

async function expectConnectionCountStable(
  proxyUnderTest: TcpFaultProxy,
  expected: number,
  durationMs: number,
): Promise<void> {
  const deadline = Date.now() + durationMs
  while (Date.now() < deadline) {
    expect(proxyUnderTest.acceptedConnections).toBe(expected)
    await new Promise((resolve) => setTimeout(resolve, 25))
  }
}

function patternSubscriptions(client: Redis): string[] {
  return (
    client as unknown as {
      condition: {
        subscriber: {
          channels(kind: 'psubscribe'): string[]
        }
      }
    }
  ).condition.subscriber.channels('psubscribe')
}

describe('TcpFaultProxy request matcher', () => {
  it('matches an exact RESP command across fragmentation and coalescing', () => {
    const ping = Buffer.from('*1\r\n$4\r\nPING\r\n')
    const increment = Buffer.from(
      '*2\r\n$4\r\nINCR\r\n$23\r\ntaskcast:test:no-replay\r\n',
    )
    const matcher = redisCommandMatcher(
      'INCR',
      'taskcast:test:no-replay',
    )
    const coalesced = Buffer.concat([increment, ping])

    expect(matcher(increment.subarray(0, increment.length - 3))).toBe(false)
    expect(matcher(coalesced)).toBe(true)
    expect(matcher(Buffer.concat([ping, increment]))).toBe(false)
    expect(
      matcher(Buffer.from(
        '*2\r\n$4\r\nINCR\r\n$14\r\ntaskcast:other\r\n',
      )),
    ).toBe(false)
  })
})

describe('equalJitterDelay', () => {
  it('uses the lower, upper, and capped equal-jitter bounds', () => {
    expect(equalJitterDelay(500, 5_000, 0, () => 0)).toBe(250)
    expect(equalJitterDelay(500, 5_000, 1, () => 1)).toBe(1_000)
    expect(equalJitterDelay(500, 5_000, 20, () => 1)).toBe(5_000)
    expect(equalJitterDelay(500, 10_000, 20, () => 0)).toBe(5_000)
    expect(equalJitterDelay(500, 10_000, 20, () => 1)).toBe(10_000)
  })
})

describe('managed Redis command client', () => {
  beforeAll(async () => {
    container = await new GenericContainer('redis:7-alpine')
      .withExposedPorts(6379)
      .start()
    proxy = new TcpFaultProxy('127.0.0.1', container.getMappedPort(6379))
    await proxy.open()
    redisUrl = `redis://127.0.0.1:${proxy.port}`
  }, 60_000)

  afterAll(async () => {
    try {
      await proxy?.stop()
    } finally {
      await container?.stop()
    }
  })

  it('configures one shared client, checks readiness, and closes without QUIT', async () => {
    const observations: DependencyObservation[] = []
    const managed = await createManagedRedisCommandClient(redisUrl, {
      observer: { observe: (observation) => observations.push(observation) },
      startupTimeoutMs: 15_000,
      random: () => 0,
    })

    expect(managed.client.options).toMatchObject({
      lazyConnect: true,
      enableReadyCheck: false,
      enableOfflineQueue: false,
      autoResendUnfulfilledCommands: false,
      maxRetriesPerRequest: 0,
    })

    const subscriber = new Redis(redisUrl)
    const adapters = createRedisAdapters(
      managed.client,
      subscriber,
      managed.client,
    )
    expect(adapters.broadcast).toBeInstanceOf(RedisBroadcastProvider)
    expect(adapters.shortTermStore).toBeInstanceOf(RedisShortTermStore)
    expect((adapters.broadcast as unknown as { pub: Redis }).pub).toBe(managed.client)
    expect((adapters.shortTermStore as unknown as { redis: Redis }).redis).toBe(
      managed.client,
    )
    await managed.check()

    const eventNames = ['ready', 'reconnecting', 'close', 'end', 'error'] as const
    expect(eventNames.every((event) => managed.client.listenerCount(event) > 0)).toBe(true)
    const disconnect = vi.spyOn(managed.client, 'disconnect')
    await managed.close()
    await managed.close()
    expect(disconnect).toHaveBeenCalledOnce()
    expect(disconnect).toHaveBeenCalledWith(false)
    expect(eventNames.every((event) => managed.client.listenerCount(event) === 0)).toBe(true)
    disconnect.mockRestore()
    expect(observations.some((observation) => observation.state === 'healthy')).toBe(true)
    expect(
      observations.every((observation) =>
        Object.keys(observation).every((key) =>
          ['dependency', 'state', 'errorKind', 'attempt', 'nextRetryMs'].includes(key),
        ),
      ),
    ).toBe(true)
    subscriber.disconnect(false)
  })

  it('fails the current offline command and recovers without replaying it', async () => {
    await proxy.open()
    const managed = await createManagedRedisCommandClient(redisUrl, {
      random: () => 0,
    })
    await managed.client.set('taskcast:managed:replay', '0')

    await proxy.refuse()
    await expect(managed.client.incr('taskcast:managed:replay')).rejects.toBeDefined()

    await proxy.open()
    await eventually(() => managed.check())
    await expect(managed.client.get('taskcast:managed:replay')).resolves.toBe('0')
    await managed.close()
  }, 20_000)

  it('does not replay an INCR whose upstream response is dropped', async () => {
    await proxy.open()
    const key = 'taskcast:test:no-replay'
    const direct = new Redis(
      `redis://127.0.0.1:${container.getMappedPort(6379)}`,
    )
    let managed:
      | Awaited<ReturnType<typeof createManagedRedisCommandClient>>
      | undefined
    const matchedBefore = proxy.matchedCommands

    try {
      managed = await createManagedRedisCommandClient(redisUrl, {
        random: () => 0,
      })
      await direct.set(key, '0')
      proxy.dropNextResponse(redisCommandMatcher('INCR', key))

      const outcome = await settleBeforeDeadline(
        managed.client.incr(key),
        5_000,
        'ambiguous INCR did not fail before the deadline',
      )
      expect(outcome.status).toBe('rejected')

      await proxy.open()
      await eventually(() => managed.check())
      await expect(direct.get(key)).resolves.toBe('1')
      expect(proxy.matchedCommands - matchedBefore).toBe(1)
    } finally {
      try {
        direct.disconnect(false)
      } finally {
        try {
          await managed?.close()
        } finally {
          await proxy.open()
        }
      }
    }
  }, 20_000)

  it('returns only after one pattern subscription and shares the command client', async () => {
    await proxy.open()
    proxy.resumeNewConnections()
    const managed = await createManagedRedisAdapters(redisUrl, {
      prefix: 'managed-lifecycle',
      random: () => 0,
    })

    expect(managed.subscriberClient).not.toBe(managed.commandClient)
    expect(
      (managed.broadcast as unknown as { pub: Redis }).pub,
    ).toBe(managed.commandClient)
    expect(
      (managed.shortTermStore as unknown as { redis: Redis }).redis,
    ).toBe(managed.commandClient)
    expect(managed.broadcast.isPatternSubscribed()).toBe(true)
    await expect(managed.commandClient.pubsub('NUMPAT')).resolves.toBe(1)
    await expect(managed.commandCheck()).resolves.toBeUndefined()
    await expect(managed.pubSubCheck()).resolves.toBeUndefined()

    await managed.close()
  })

  it('keeps handlers and the single pattern after a forced subscriber reconnect', async () => {
    await proxy.open()
    proxy.resumeNewConnections()
    const observations: DependencyObservation[] = []
    const options = {
      prefix: 'managed-cross-instance',
      random: () => 0,
      observer: {
        observe: (observation: DependencyObservation) =>
          observations.push(observation),
      },
    }
    const first = await createManagedRedisAdapters(redisUrl, options)
    const second = await createManagedRedisAdapters(redisUrl, options)
    const received: string[] = []
    second.broadcast.subscribe('task-1', (event) => received.push(event.id))

    await first.broadcast.publish('task-1', { ...makeEvent(), id: 'before' })
    await eventually(() => expect(received).toEqual(['before']))
    expect(patternSubscriptions(first.subscriberClient)).toEqual([
      'managed-cross-instance:task:*',
    ])
    expect(patternSubscriptions(second.subscriberClient)).toEqual([
      'managed-cross-instance:task:*',
    ])
    await expect(first.commandClient.pubsub('NUMPAT')).resolves.toBe(1)

    proxy.pauseNewConnections()
    proxy.closeLatestConnection()
    await eventually(() =>
      expect(second.broadcast.isPatternSubscribed()).toBe(false),
    )
    await expect(second.pubSubCheck()).rejects.toBeDefined()
    proxy.resumeNewConnections()
    await eventually(() => second.pubSubCheck())

    await first.broadcast.publish('task-1', { ...makeEvent(), id: 'after' })
    await eventually(() => expect(received).toEqual(['before', 'after']))
    expect(patternSubscriptions(first.subscriberClient)).toHaveLength(1)
    expect(patternSubscriptions(second.subscriberClient)).toHaveLength(1)
    await expect(first.commandClient.pubsub('NUMPAT')).resolves.toBe(1)
    expect(
      observations.some(
        ({ dependency, state }) =>
          dependency === 'redisPubSub' && state === 'reconnecting',
      ),
    ).toBe(true)
    expect(
      observations.every((observation) =>
        Object.keys(observation).every((key) =>
          ['dependency', 'state', 'errorKind', 'attempt', 'nextRetryMs'].includes(
            key,
          ),
        ),
      ),
    ).toBe(true)

    await second.close()
    await first.close()
  }, 20_000)

  it('recovers commands, store, and new PubSub messages after a long outage', async () => {
    await proxy.open()
    const observations: DependencyObservation[] = []
    let managed:
      | Awaited<ReturnType<typeof createManagedRedisAdapters>>
      | undefined
    const received: string[] = []
    let unsubscribe: (() => void) | undefined

    try {
      managed = await createManagedRedisAdapters(redisUrl, {
        prefix: 'managed-long-outage',
        random: () => 0,
        observer: {
          observe: (observation) => observations.push(observation),
        },
      })
      unsubscribe = managed.broadcast.subscribe(
        'task-long-outage',
        (event) => received.push(event.id),
      )
      await managed.broadcast.publish('task-long-outage', {
        ...makeEvent(),
        id: 'before-long-outage',
      })
      await eventually(() =>
        expect(received).toEqual(['before-long-outage']),
      )

      const acceptedBeforeOutage = proxy.acceptedConnections
      await proxy.blackhole()
      await eventually(() =>
        expect(managed.commandClient.status).toBe('ready'),
      )

      const interrupted = managed.commandClient.get(
        'taskcast:managed:interrupted',
      )
      await eventually(async () => {
        const queue = managed.commandClient as unknown as {
          commandQueue: { length: number }
        }
        expect(queue.commandQueue.length).toBeGreaterThan(0)
      })
      await proxy.refuse()
      const interruptedOutcome = await settleBeforeDeadline(
        interrupted,
        5_000,
        'blackholed command did not fail after sockets were refused',
      )
      expect(interruptedOutcome.status).toBe('rejected')

      await eventually(() =>
        expect(
          observations.some(
            ({ dependency, attempt }) =>
              dependency === 'redisCommand' && (attempt ?? 0) >= 2,
          ),
        ).toBe(true),
      )
      await eventually(() =>
        expect(
          observations.some(
            ({ dependency, attempt }) =>
              dependency === 'redisPubSub' && (attempt ?? 0) >= 2,
          ),
        ).toBe(true),
      )
      await proxy.open()
      try {
        await eventually(() => managed.commandCheck())
      } catch (error) {
        throw new Error(
          `command recovery timed out: status=${managed.commandClient.status}, `
          + `acceptedDelta=${proxy.acceptedConnections - acceptedBeforeOutage}, `
          + `pubSubSubscribed=${managed.broadcast.isPatternSubscribed()}, `
          + `observations=${JSON.stringify(observations.slice(-20))}`,
          { cause: error },
        )
      }
      try {
        await eventually(() => managed.pubSubCheck())
      } catch (error) {
        throw new Error(
          `PubSub recovery timed out: status=${managed.subscriberClient.status}, `
          + `acceptedDelta=${proxy.acceptedConnections - acceptedBeforeOutage}`,
          { cause: error },
        )
      }

      const recoveredTask = makeTask('task-managed-recovered')
      await managed.shortTermStore.saveTask(recoveredTask)
      await expect(
        managed.shortTermStore.getTask(recoveredTask.id),
      ).resolves.toEqual(recoveredTask)

      await managed.broadcast.publish('task-long-outage', {
        ...makeEvent(),
        id: 'after-long-outage',
      })
      await eventually(() =>
        expect(received).toEqual([
          'before-long-outage',
          'after-long-outage',
        ]),
      )

      // The command manager and PubSub supervisor may each have one
      // transition-race connection plus their successful recovery.
      const reconnectConnections =
        proxy.acceptedConnections - acceptedBeforeOutage
      expect(reconnectConnections).toBeLessThanOrEqual(4)
    } finally {
      unsubscribe?.()
      try {
        await managed?.close()
      } finally {
        await proxy.open()
      }
    }
  }, 20_000)

  it('keeps 50 concurrent callers within the two coordinated reconnect paths', async () => {
    await proxy.open()
    let managed:
      | Awaited<ReturnType<typeof createManagedRedisAdapters>>
      | undefined

    try {
      managed = await createManagedRedisAdapters(redisUrl, {
        prefix: 'managed-connection-bound',
        random: () => 0,
      })
      const acceptedBeforeDrop = proxy.acceptedConnections
      proxy.closeSockets()
      await eventually(() =>
        expect(managed.commandClient.status).not.toBe('ready'),
      )

      const results = await withDeadline(
        Promise.allSettled(
          Array.from({ length: 50 }, () => managed.commandClient.ping()),
        ),
        5_000,
        '50 commands did not settle before the reconnect deadline',
      )
      expect(results.every(({ status }) => status === 'rejected')).toBe(true)

      await eventually(() => managed.commandCheck())
      await eventually(() => managed.pubSubCheck())

      // One command manager and one PubSub supervisor may each have one
      // transition-race attempt plus their successful reconnect.
      expect(
        proxy.acceptedConnections - acceptedBeforeDrop,
      ).toBeLessThanOrEqual(4)
    } finally {
      try {
        await managed?.close()
      } finally {
        await proxy.open()
      }
    }
  }, 20_000)

  it('cancels a pending subscriber retry when closed', async () => {
    await proxy.open()
    proxy.resumeNewConnections()
    const managed = await createManagedRedisAdapters(redisUrl, {
      prefix: 'managed-shutdown',
      random: () => 0,
    })

    proxy.pauseNewConnections()
    proxy.closeLatestConnection()
    await eventually(() =>
      expect(managed.broadcast.isPatternSubscribed()).toBe(false),
    )
    await managed.close()
    const acceptedAfterClose = proxy.acceptedConnections

    proxy.resumeNewConnections()
    await expectConnectionCountStable(proxy, acceptedAfterClose, 750)
  })

  it('bounds unreachable startup by one overall deadline', async () => {
    await proxy.refuse()
    const startedAt = Date.now()

    await expect(
      createManagedRedisAdapters(redisUrl, {
        prefix: 'managed-unreachable',
        random: () => 0,
      }),
    ).rejects.toBeDefined()

    expect(Date.now() - startedAt).toBeLessThanOrEqual(15_500)
  }, 16_000)
})
