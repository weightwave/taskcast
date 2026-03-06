import { describe, it, expect, beforeAll, afterAll } from 'vitest'
import { GenericContainer, type StartedTestContainer } from 'testcontainers'
import Redis from 'ioredis'
import { TaskEngine } from '@taskcast/core'
import { RedisBroadcastProvider, RedisShortTermStore } from '@taskcast/redis'
import type { TaskEvent } from '@taskcast/core'

const canDocker = !!process.env.CI || !!process.env.DOCKER_HOST || !!process.env.TESTCONTAINERS

describe.skipIf(!canDocker)('Redis adapter integration — testcontainer', () => {
  let container: StartedTestContainer
  let redisUrl: string
  let pub: Redis
  let sub: Redis
  let store: Redis

  beforeAll(async () => {
    container = await new GenericContainer('redis:7-alpine')
      .withExposedPorts(6379)
      .start()
    redisUrl = `redis://${container.getHost()}:${container.getMappedPort(6379)}`
    pub = new Redis(redisUrl)
    sub = new Redis(redisUrl)
    store = new Redis(redisUrl)
  }, 60000)

  afterAll(async () => {
    pub?.disconnect()
    sub?.disconnect()
    store?.disconnect()
    await container?.stop()
  })

  it('events published on one engine are received by subscriber on another', async () => {
    const broadcast1 = new RedisBroadcastProvider(pub, sub)
    const store1 = new RedisShortTermStore(store)
    const engine1 = new TaskEngine({ shortTermStore: store1, broadcast: broadcast1 })

    const pub2 = new Redis(redisUrl)
    const sub2 = new Redis(redisUrl)
    const store2 = new Redis(redisUrl)
    const broadcast2 = new RedisBroadcastProvider(pub2, sub2)
    const shortTermStore2 = new RedisShortTermStore(store2)
    const engine2 = new TaskEngine({ shortTermStore: shortTermStore2, broadcast: broadcast2 })

    const task = await engine1.createTask({ type: 'test' })
    await engine1.transitionTask(task.id, 'running')

    // Subscribe on engine2
    const received: TaskEvent[] = []
    const unsub = engine2.subscribe(task.id, (evt) => received.push(evt))

    // Publish on engine1
    await engine1.publishEvent(task.id, { type: 'chunk', level: 'info', data: { n: 1 } })

    // Wait for cross-engine delivery
    await new Promise((r) => setTimeout(r, 200))

    expect(received.length).toBeGreaterThan(0)
    expect(received.some(e => e.type === 'chunk')).toBe(true)

    unsub()
    pub2.disconnect()
    sub2.disconnect()
    store2.disconnect()
  })

  it('task state persists across engine instances', async () => {
    const broadcast1 = new RedisBroadcastProvider(pub, sub)
    const store1 = new RedisShortTermStore(store)
    const engine1 = new TaskEngine({ shortTermStore: store1, broadcast: broadcast1 })

    const task = await engine1.createTask({ type: 'persist-test', metadata: { key: 'value' } })
    await engine1.transitionTask(task.id, 'running')

    // Read from a new store client
    const store2 = new Redis(redisUrl)
    const shortTermStore2 = new RedisShortTermStore(store2)
    const fetched = await shortTermStore2.getTask(task.id)

    expect(fetched).toBeTruthy()
    expect(fetched!.status).toBe('running')
    expect(fetched!.type).toBe('persist-test')

    store2.disconnect()
  })
})
