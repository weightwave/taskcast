import { describe, it, expect, beforeAll, afterAll } from 'vitest'
import { Redis } from 'ioredis'
import { GenericContainer, type StartedTestContainer } from 'testcontainers'
import { RedisBroadcastProvider } from '../src/broadcast.js'
import type { TaskEvent } from '@taskcast/core'

let container: StartedTestContainer
let redisUrl: string

beforeAll(async () => {
  container = await new GenericContainer('redis:7-alpine')
    .withExposedPorts(6379)
    .start()
  redisUrl = `redis://localhost:${container.getMappedPort(6379)}`
}, 60000)

afterAll(async () => {
  await container?.stop()
})

const makeEvent = (): TaskEvent => ({
  id: 'evt-1',
  taskId: 'task-1',
  index: 0,
  timestamp: Date.now(),
  type: 'llm.delta',
  level: 'info',
  data: { text: 'hello' },
})

describe('RedisBroadcastProvider', () => {
  it('delivers published events to subscribers', async () => {
    const pub = new Redis(redisUrl)
    const sub = new Redis(redisUrl)
    const provider = new RedisBroadcastProvider(pub, sub)

    const received: TaskEvent[] = []
    const unsub = provider.subscribe('task-1', (e) => received.push(e))

    // wait for subscription to be ready
    await new Promise((r) => setTimeout(r, 100))

    const event = makeEvent()
    await provider.publish('task-1', event)

    await new Promise((r) => setTimeout(r, 100))
    expect(received).toHaveLength(1)
    expect(received[0]?.type).toBe('llm.delta')

    unsub()
    pub.disconnect()
    sub.disconnect()
  })

  it('multiple subscribers on same channel all receive events', async () => {
    const pub = new Redis(redisUrl)
    const sub1 = new Redis(redisUrl)
    const sub2 = new Redis(redisUrl)
    const p1 = new RedisBroadcastProvider(pub, sub1)
    const p2 = new RedisBroadcastProvider(new Redis(redisUrl), sub2)

    const r1: TaskEvent[] = []
    const r2: TaskEvent[] = []
    const u1 = p1.subscribe('task-1', (e) => r1.push(e))
    const u2 = p2.subscribe('task-1', (e) => r2.push(e))

    await new Promise((r) => setTimeout(r, 100))
    await p1.publish('task-1', makeEvent())
    await new Promise((r) => setTimeout(r, 100))

    expect(r1).toHaveLength(1)
    expect(r2).toHaveLength(1)

    u1(); u2()
    pub.disconnect(); sub1.disconnect(); sub2.disconnect()
  })

  it('unsubscribe stops delivery', async () => {
    const pub = new Redis(redisUrl)
    const sub = new Redis(redisUrl)
    const provider = new RedisBroadcastProvider(pub, sub)

    const received: TaskEvent[] = []
    const unsub = provider.subscribe('task-1', (e) => received.push(e))
    await new Promise((r) => setTimeout(r, 100))

    await provider.publish('task-1', makeEvent())
    await new Promise((r) => setTimeout(r, 100))
    unsub()

    await provider.publish('task-1', makeEvent())
    await new Promise((r) => setTimeout(r, 100))

    expect(received).toHaveLength(1)
    pub.disconnect(); sub.disconnect()
  })
})
