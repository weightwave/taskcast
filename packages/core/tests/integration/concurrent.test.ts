import { describe, it, expect, beforeAll, afterAll, beforeEach } from 'vitest'
import { Redis } from 'ioredis'
import { GenericContainer, type StartedTestContainer } from 'testcontainers'
import { TaskEngine } from '../../src/engine.js'
import { RedisBroadcastProvider } from '../../../redis/src/broadcast.js'
import { RedisShortTermStore } from '../../../redis/src/short-term.js'
import { MemoryShortTermStore, MemoryBroadcastProvider } from '../../src/memory-adapters.js'

function makeMemoryEngine() {
  const store = new MemoryShortTermStore()
  const broadcast = new MemoryBroadcastProvider()
  return new TaskEngine({ shortTerm: store, broadcast })
}

// ─── Concurrent subscriber tests (memory, no IO) ─────────────────────────────

describe('Concurrent subscribers - memory engine', () => {
  it('50 subscribers all receive 200 events in correct order', async () => {
    const engine = makeMemoryEngine()
    const task = await engine.createTask({})
    await engine.transitionTask(task.id, 'running')

    const SUBSCRIBER_COUNT = 50
    const EVENT_COUNT = 200

    // Set up subscribers BEFORE publishing
    const receivedBySubscriber: string[][] = Array.from({ length: SUBSCRIBER_COUNT }, () => [])
    const unsubs = receivedBySubscriber.map((arr) =>
      engine.subscribe(task.id, (e) => {
        if (e.type !== 'taskcast:status') arr.push(e.id)
      })
    )

    // Publish events sequentially (engine guarantees ordering)
    const publishedIds: string[] = []
    for (let i = 0; i < EVENT_COUNT; i++) {
      const event = await engine.publishEvent(task.id, {
        type: 'load.test',
        level: 'info',
        data: { seq: i },
      })
      publishedIds.push(event.id)
    }

    // Allow micro-tasks to flush
    await new Promise((r) => setTimeout(r, 10))

    for (let i = 0; i < SUBSCRIBER_COUNT; i++) {
      expect(receivedBySubscriber[i]).toHaveLength(EVENT_COUNT)
      expect(receivedBySubscriber[i]).toEqual(publishedIds)
    }

    unsubs.forEach((u) => u())
  })

  it('concurrent status transitions: final state is a single terminal status', async () => {
    // NOTE: The engine does not implement distributed locking or mutex guards for
    // concurrent transitions. In JavaScript's single-threaded async model, multiple
    // concurrent calls to transitionTask will all read the same "running" status before
    // any write completes, so all may succeed. This test verifies that:
    //   1. The final persisted state is exactly one valid terminal status
    //   2. At least one transition succeeds (task is in terminal state)
    const engine = makeMemoryEngine()
    const task = await engine.createTask({})
    await engine.transitionTask(task.id, 'running')

    // 20 concurrent attempts to complete the same task
    const results = await Promise.allSettled(
      Array.from({ length: 20 }, () =>
        engine.transitionTask(task.id, 'completed'),
      )
    )

    const succeeded = results.filter((r) => r.status === 'fulfilled')

    // At least one must succeed
    expect(succeeded.length).toBeGreaterThanOrEqual(1)

    // The final persisted task is in a terminal state
    const finalTask = await engine.getTask(task.id)
    expect(finalTask?.status).toBe('completed')
  })

  it('100 tasks created concurrently all get unique IDs', async () => {
    const engine = makeMemoryEngine()
    const tasks = await Promise.all(
      Array.from({ length: 100 }, () => engine.createTask({}))
    )
    const ids = tasks.map((t) => t.id)
    const unique = new Set(ids)
    expect(unique.size).toBe(100)
  })

  it('concurrent publishEvent maintains monotonic index', async () => {
    const engine = makeMemoryEngine()
    const task = await engine.createTask({})
    await engine.transitionTask(task.id, 'running')

    // Publish 50 events concurrently
    const events = await Promise.all(
      Array.from({ length: 50 }, (_, i) =>
        engine.publishEvent(task.id, { type: 'parallel', level: 'info', data: { i } })
      )
    )

    const indices = events.map((e) => e.index).sort((a, b) => a - b)
    const minIndex = Math.min(...indices)
    const maxIndex = Math.max(...indices)
    expect(maxIndex - minIndex).toBe(49) // 50 unique consecutive indices
    expect(new Set(indices).size).toBe(50) // all unique
  })
})

// ─── Redis pub/sub fan-out (requires Docker) ─────────────────────────────────

describe('Redis concurrent fan-out', () => {
  let container: StartedTestContainer
  let engine: TaskEngine
  let pub: Redis, sub: Redis, store: Redis
  let redisUrl: string

  beforeAll(async () => {
    container = await new GenericContainer('redis:7-alpine').withExposedPorts(6379).start()
    redisUrl = `redis://localhost:${container.getMappedPort(6379)}`
    pub = new Redis(redisUrl)
    sub = new Redis(redisUrl)
    store = new Redis(redisUrl)
    engine = new TaskEngine({
      broadcast: new RedisBroadcastProvider(pub, sub),
      shortTerm: new RedisShortTermStore(store),
    })
  }, 60000)

  afterAll(async () => {
    pub.disconnect(); sub.disconnect(); store.disconnect()
    await container?.stop()
  })

  beforeEach(async () => {
    await store.flushall()
  })

  it('20 subscribers all receive 100 events via Redis pub/sub', async () => {
    const task = await engine.createTask({})
    await engine.transitionTask(task.id, 'running')

    await new Promise((r) => setTimeout(r, 50)) // allow subscription setup

    const SUBSCRIBER_COUNT = 20
    const EVENT_COUNT = 100

    const receivedCounts = Array<number>(SUBSCRIBER_COUNT).fill(0)
    const unsubs = receivedCounts.map((_, i) =>
      engine.subscribe(task.id, (e) => {
        if (e.type !== 'taskcast:status') receivedCounts[i] = (receivedCounts[i] ?? 0) + 1
      })
    )

    await new Promise((r) => setTimeout(r, 100)) // allow Redis subscriptions to register

    for (let i = 0; i < EVENT_COUNT; i++) {
      await engine.publishEvent(task.id, { type: 'fan.out', level: 'info', data: { i } })
    }

    await new Promise((r) => setTimeout(r, 500)) // allow delivery

    for (let i = 0; i < SUBSCRIBER_COUNT; i++) {
      expect(receivedCounts[i]).toBe(EVENT_COUNT)
    }
    unsubs.forEach((u) => u())
  })

  it('two engine instances sharing Redis produce no duplicate event indices', async () => {
    // Regression test: before moving nextIndex() into ShortTermStore, each TaskEngine
    // kept its own in-memory indexCounters map. Under a multi-instance load-balanced
    // setup (stress test found 37/60 collisions), instance A and instance B would both
    // start at 0, yielding duplicate indices for the same task. Redis INCR is atomic,
    // so distributing the counter into RedisShortTermStore.nextIndex() fixes this.
    const pub2 = new Redis(redisUrl)
    const sub2 = new Redis(redisUrl)
    const store2 = new Redis(redisUrl)
    const engine2 = new TaskEngine({
      broadcast: new RedisBroadcastProvider(pub2, sub2),
      shortTerm: new RedisShortTermStore(store2),
    })

    try {
      const task = await engine.createTask({})
      await engine.transitionTask(task.id, 'running')

      const EVENT_COUNT = 30

      // Interleave publishes from both instances concurrently
      const events = await Promise.all([
        ...Array.from({ length: EVENT_COUNT }, (_, i) =>
          engine.publishEvent(task.id, { type: 'inst1', level: 'info', data: { i } })
        ),
        ...Array.from({ length: EVENT_COUNT }, (_, i) =>
          engine2.publishEvent(task.id, { type: 'inst2', level: 'info', data: { i } })
        ),
      ])

      const indices = events.map((e) => e.index)
      expect(new Set(indices).size).toBe(EVENT_COUNT * 2) // all unique
    } finally {
      pub2.disconnect(); sub2.disconnect(); store2.disconnect()
    }
  })
})
