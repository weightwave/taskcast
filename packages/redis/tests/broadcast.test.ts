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

  it('uses custom prefix for channels', async () => {
    const pub = new Redis(redisUrl)
    const sub = new Redis(redisUrl)
    const provider = new RedisBroadcastProvider(pub, sub, { prefix: 'myapp' })

    const received: TaskEvent[] = []
    const unsub = provider.subscribe('task-1', (e) => received.push(e))
    await new Promise((r) => setTimeout(r, 100))

    await provider.publish('task-1', makeEvent())
    await new Promise((r) => setTimeout(r, 100))

    expect(received).toHaveLength(1)
    unsub()
    pub.disconnect()
    sub.disconnect()
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

  it('ignores malformed (non-JSON) messages on the channel', async () => {
    const pub = new Redis(redisUrl)
    const sub = new Redis(redisUrl)
    const provider = new RedisBroadcastProvider(pub, sub)

    const received: TaskEvent[] = []
    provider.subscribe('task-1', (e) => received.push(e))
    await new Promise((r) => setTimeout(r, 100))

    // Publish a raw malformed message directly via Redis to trigger the catch branch
    await pub.publish('taskcast:task:task-1', 'not-valid-json{{{{')
    await new Promise((r) => setTimeout(r, 100))

    // No events should have been delivered (error was swallowed)
    expect(received).toHaveLength(0)
    pub.disconnect()
    sub.disconnect()
  })

  it('delivers message when channel does not start with prefix (raw channel name used as taskId)', async () => {
    const pub = new Redis(redisUrl)
    const sub = new Redis(redisUrl)
    const provider = new RedisBroadcastProvider(pub, sub)

    const received: TaskEvent[] = []
    // Subscribe using a taskId, which gets subscribed as 'taskcast:task:task-raw'
    // We'll simulate the message handler receiving a channel WITHOUT the prefix
    // by accessing the private sub event emitter directly
    provider.subscribe('task-raw', (e) => received.push(e))
    await new Promise((r) => setTimeout(r, 100))

    // Now manually emit a message event on sub with a channel that does NOT start with the prefix
    // This exercises the `: channel` branch in the message handler
    const event = makeEvent()
    // Emit a fake Redis message event where channel has no prefix match
    ;(sub as unknown as { emit: (event: string, ...args: unknown[]) => void }).emit(
      'message',
      'task-raw', // does not start with 'taskcast:task:'
      JSON.stringify(event),
    )
    await new Promise((r) => setTimeout(r, 10))

    expect(received).toHaveLength(1)
    pub.disconnect()
    sub.disconnect()
  })

  it('ignores messages on channels with no registered handlers', async () => {
    const pub = new Redis(redisUrl)
    const sub = new Redis(redisUrl)
    const provider = new RedisBroadcastProvider(pub, sub)

    // Subscribe to task-1 then unsubscribe to clear handlers
    const unsub = provider.subscribe('task-1', () => {})
    unsub()
    await new Promise((r) => setTimeout(r, 100))

    // Manually emit a message for a channel with no handlers (handlers map is empty)
    // This exercises the `if (!handlers) return` branch
    ;(sub as unknown as { emit: (event: string, ...args: unknown[]) => void }).emit(
      'message',
      'taskcast:task:task-1',
      JSON.stringify(makeEvent()),
    )
    await new Promise((r) => setTimeout(r, 10))
    // No error thrown, handler is not in the map → silently returns
    pub.disconnect()
    sub.disconnect()
  })

  it('calling unsubscribe twice is safe (set not found guard)', async () => {
    const pub = new Redis(redisUrl)
    const sub = new Redis(redisUrl)
    const provider = new RedisBroadcastProvider(pub, sub)

    const received: TaskEvent[] = []
    const unsub = provider.subscribe('task-1', (e) => received.push(e))
    await new Promise((r) => setTimeout(r, 100))

    // Call unsub once — this deletes the set when it becomes empty
    unsub()
    // Call unsub again — this exercises the `if (!set) return` defensive branch
    unsub()

    pub.disconnect()
    sub.disconnect()
  })
})
