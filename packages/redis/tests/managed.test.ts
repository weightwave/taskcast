import { afterAll, beforeAll, describe, expect, it } from 'vitest'
import { Redis } from 'ioredis'
import { GenericContainer, type StartedTestContainer } from 'testcontainers'
import type { DependencyObservation } from '@taskcast/core'
import { equalJitterDelay } from '../src/backoff.js'
import { createManagedRedisCommandClient } from '../src/managed.js'
import {
  createRedisAdapters,
  RedisBroadcastProvider,
  RedisShortTermStore,
} from '../src/index.js'
import { TcpFaultProxy } from './helpers/tcp-fault-proxy.js'

let container: StartedTestContainer
let proxy: TcpFaultProxy
let redisUrl: string

async function eventually(
  operation: () => Promise<void>,
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

describe('equalJitterDelay', () => {
  it('uses the lower, upper, and capped equal-jitter bounds', () => {
    expect(equalJitterDelay(500, 5_000, 0, () => 0)).toBe(250)
    expect(equalJitterDelay(500, 5_000, 1, () => 1)).toBe(1_000)
    expect(equalJitterDelay(500, 5_000, 20, () => 1)).toBe(5_000)
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
    await proxy?.stop()
    await container?.stop()
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
    await managed.close()
    await managed.close()
    expect(eventNames.every((event) => managed.client.listenerCount(event) === 0)).toBe(true)
    expect(managed.client.status).toBe('end')
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
})
