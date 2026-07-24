import { describe, it, expect, beforeAll, afterAll, vi } from 'vitest'
import { Redis } from 'ioredis'
import { GenericContainer, type StartedTestContainer } from 'testcontainers'
import { RedisBroadcastProvider } from '../src/broadcast.js'
import type { TaskEvent } from '@taskcast/core'

let container: StartedTestContainer
let redisUrl: string
const clients: Redis[] = []

function createClient(): Redis {
  const c = new Redis(redisUrl)
  c.on('error', () => {
    // Tests intentionally exercise rapid subscriber teardown.
  })
  clients.push(c)
  return c
}

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

async function eventuallySubscribed(
  client: Redis,
  channel: string,
  expected: number,
): Promise<void> {
  await eventually(async () => {
    expect(await client.pubsub('NUMSUB', channel)).toEqual([channel, expected])
  })
}

beforeAll(async () => {
  container = await new GenericContainer('redis:7-alpine')
    .withExposedPorts(6379)
    .start()
  redisUrl = `redis://localhost:${container.getMappedPort(6379)}`
}, 60000)

afterAll(async () => {
  await Promise.all(clients.map((c) => c.quit().catch(() => {})))
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
  it('awaits one wildcard subscription and keeps pattern handlers local', async () => {
    const pub = createClient()
    const sub = createClient()
    const subscribe = vi.spyOn(sub, 'subscribe')
    const unsubscribe = vi.spyOn(sub, 'unsubscribe')
    const psubscribe = vi.spyOn(sub, 'psubscribe')
    const provider = new RedisBroadcastProvider(pub, sub, {
      prefix: 'pattern',
      subscriptionMode: 'pattern',
    })

    await provider.startPatternSubscription()

    expect(psubscribe).toHaveBeenCalledOnce()
    expect(psubscribe).toHaveBeenCalledWith('pattern:task:*')
    expect(provider.isPatternSubscribed()).toBe(true)

    const received: TaskEvent[] = []
    const remove = provider.subscribe('task-1', (event) => received.push(event))
    expect(subscribe).not.toHaveBeenCalled()

    await provider.publish('task-1', makeEvent())
    await eventually(() => expect(received).toHaveLength(1))

    remove()
    expect(unsubscribe).not.toHaveBeenCalled()
  })

  it('delivers published events to subscribers', async () => {
    const pub = createClient()
    const sub = createClient()
    const provider = new RedisBroadcastProvider(pub, sub)

    const received: TaskEvent[] = []
    const unsub = provider.subscribe('task-1', (e) => received.push(e))

    await eventuallySubscribed(pub, 'taskcast:task:task-1', 1)

    const event = makeEvent()
    await provider.publish('task-1', event)

    await eventually(() => expect(received).toHaveLength(1))
    expect(received).toHaveLength(1)
    expect(received[0]?.type).toBe('llm.delta')

    unsub()
  })

  it('multiple subscribers on same channel all receive events', async () => {
    const pub = createClient()
    const sub1 = createClient()
    const sub2 = createClient()
    const p1 = new RedisBroadcastProvider(pub, sub1)
    const p2 = new RedisBroadcastProvider(createClient(), sub2)

    const r1: TaskEvent[] = []
    const r2: TaskEvent[] = []
    const u1 = p1.subscribe('task-1', (e) => r1.push(e))
    const u2 = p2.subscribe('task-1', (e) => r2.push(e))

    await eventuallySubscribed(pub, 'taskcast:task:task-1', 2)
    await p1.publish('task-1', makeEvent())
    await eventually(() => {
      expect(r1).toHaveLength(1)
      expect(r2).toHaveLength(1)
    })

    expect(r1).toHaveLength(1)
    expect(r2).toHaveLength(1)

    u1(); u2()
  })

  it('uses custom prefix for channels', async () => {
    const pub = createClient()
    const sub = createClient()
    const provider = new RedisBroadcastProvider(pub, sub, { prefix: 'myapp' })

    const received: TaskEvent[] = []
    const unsub = provider.subscribe('task-1', (e) => received.push(e))
    await eventuallySubscribed(pub, 'myapp:task:task-1', 1)

    await provider.publish('task-1', makeEvent())
    await eventually(() => expect(received).toHaveLength(1))

    expect(received).toHaveLength(1)
    unsub()
  })

  it('unsubscribe stops delivery', async () => {
    const pub = createClient()
    const sub = createClient()
    const provider = new RedisBroadcastProvider(pub, sub)

    const received: TaskEvent[] = []
    const unsub = provider.subscribe('task-1', (e) => received.push(e))
    await eventuallySubscribed(pub, 'taskcast:task:task-1', 1)

    await provider.publish('task-1', makeEvent())
    await eventually(() => expect(received).toHaveLength(1))
    unsub()
    await eventuallySubscribed(pub, 'taskcast:task:task-1', 0)

    await provider.publish('task-1', makeEvent())
    await new Promise((r) => setTimeout(r, 100))

    expect(received).toHaveLength(1)
  })

  it('ignores malformed (non-JSON) messages on the channel', async () => {
    const pub = createClient()
    const sub = createClient()
    const provider = new RedisBroadcastProvider(pub, sub)

    const received: TaskEvent[] = []
    provider.subscribe('task-1', (e) => received.push(e))
    await eventuallySubscribed(pub, 'taskcast:task:task-1', 1)

    // Publish a raw malformed message directly via Redis to trigger the catch branch
    await pub.publish('taskcast:task:task-1', 'not-valid-json{{{{')
    await new Promise((r) => setTimeout(r, 100))

    // No events should have been delivered (error was swallowed)
    expect(received).toHaveLength(0)
  })

  it('delivers message when channel does not start with prefix (raw channel name used as taskId)', async () => {
    const pub = createClient()
    const sub = createClient()
    const provider = new RedisBroadcastProvider(pub, sub)

    const received: TaskEvent[] = []
    // Subscribe using a taskId, which gets subscribed as 'taskcast:task:task-raw'
    // We'll simulate the message handler receiving a channel WITHOUT the prefix
    // by accessing the private sub event emitter directly
    provider.subscribe('task-raw', (e) => received.push(e))

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
  })

  it('ignores messages on channels with no registered handlers', async () => {
    const pub = createClient()
    const sub = createClient()
    const provider = new RedisBroadcastProvider(pub, sub)

    // Subscribe to task-1 then unsubscribe to clear handlers
    const unsub = provider.subscribe('task-1', () => {})
    unsub()

    // Manually emit a message for a channel with no handlers (handlers map is empty)
    // This exercises the `if (!handlers) return` branch
    ;(sub as unknown as { emit: (event: string, ...args: unknown[]) => void }).emit(
      'message',
      'taskcast:task:task-1',
      JSON.stringify(makeEvent()),
    )
    await new Promise((r) => setTimeout(r, 10))
    // No error thrown, handler is not in the map → silently returns
  })

  it('calling unsubscribe twice is safe (set not found guard)', async () => {
    const pub = createClient()
    const sub = createClient()
    const provider = new RedisBroadcastProvider(pub, sub)

    const received: TaskEvent[] = []
    const unsub = provider.subscribe('task-1', (e) => received.push(e))

    // Call unsub once — this deletes the set when it becomes empty
    unsub()
    // Call unsub again — this exercises the `if (!set) return` defensive branch
    unsub()
  })
})
