import { describe, it, expect, beforeAll, afterAll, beforeEach } from 'vitest'
import { Redis } from 'ioredis'
import { GenericContainer, type StartedTestContainer } from 'testcontainers'
import { RedisShortTermStore } from '../src/short-term.js'
import type { Task, TaskEvent } from '@taskcast/core'

let container: StartedTestContainer
let redis: Redis
let store: RedisShortTermStore

beforeAll(async () => {
  container = await new GenericContainer('redis:7-alpine')
    .withExposedPorts(6379)
    .start()
  redis = new Redis(`redis://localhost:${container.getMappedPort(6379)}`)
  store = new RedisShortTermStore(redis)
}, 60000)

afterAll(async () => {
  redis.disconnect()
  await container?.stop()
})

beforeEach(async () => {
  await redis.flushall()
})

const makeTask = (id = 'task-1'): Task => ({
  id,
  status: 'pending',
  createdAt: 1000,
  updatedAt: 1000,
})

const makeEvent = (index = 0): TaskEvent => ({
  id: `evt-${index}`,
  taskId: 'task-1',
  index,
  timestamp: 1000 + index,
  type: 'llm.delta',
  level: 'info',
  data: { text: `msg-${index}` },
})

describe('RedisShortTermStore - task', () => {
  it('saves and retrieves a task', async () => {
    await store.saveTask(makeTask())
    const task = await store.getTask('task-1')
    expect(task?.status).toBe('pending')
  })

  it('returns null for missing task', async () => {
    expect(await store.getTask('missing')).toBeNull()
  })

  it('overwrites existing task on save', async () => {
    await store.saveTask(makeTask())
    await store.saveTask({ ...makeTask(), status: 'running' })
    const task = await store.getTask('task-1')
    expect(task?.status).toBe('running')
  })
})

describe('RedisShortTermStore - events', () => {
  it('appends events in order', async () => {
    await store.appendEvent('task-1', makeEvent(0))
    await store.appendEvent('task-1', makeEvent(1))
    const events = await store.getEvents('task-1')
    expect(events).toHaveLength(2)
    expect(events[0]?.index).toBe(0)
    expect(events[1]?.index).toBe(1)
  })

  it('filters events by since.index', async () => {
    for (let i = 0; i < 5; i++) await store.appendEvent('task-1', makeEvent(i))
    const events = await store.getEvents('task-1', { since: { index: 2 } })
    expect(events.map((e) => e.index)).toEqual([3, 4])
  })

  it('filters events by since.timestamp', async () => {
    for (let i = 0; i < 5; i++) await store.appendEvent('task-1', makeEvent(i))
    // event timestamps are 1000, 1001, 1002, 1003, 1004
    const events = await store.getEvents('task-1', { since: { timestamp: 1002 } })
    expect(events.map((e) => e.index)).toEqual([3, 4])
  })

  it('filters events by since.id', async () => {
    for (let i = 0; i < 5; i++) await store.appendEvent('task-1', makeEvent(i))
    // since.id = 'evt-2' means return events AFTER evt-2
    const events = await store.getEvents('task-1', { since: { id: 'evt-2' } })
    expect(events.map((e) => e.index)).toEqual([3, 4])
  })

  it('returns full list when since.id is not found', async () => {
    for (let i = 0; i < 3; i++) await store.appendEvent('task-1', makeEvent(i))
    // idx < 0 branch: id not found → return all events
    const events = await store.getEvents('task-1', { since: { id: 'nonexistent-id' } })
    expect(events.map((e) => e.index)).toEqual([0, 1, 2])
  })

  it('applies limit to results', async () => {
    for (let i = 0; i < 5; i++) await store.appendEvent('task-1', makeEvent(i))
    const events = await store.getEvents('task-1', { limit: 2 })
    expect(events).toHaveLength(2)
    expect(events[0]?.index).toBe(0)
  })
})

describe('RedisShortTermStore - series', () => {
  it('setSeriesLatest and getSeriesLatest roundtrip', async () => {
    const event = makeEvent()
    await store.setSeriesLatest('task-1', 's1', event)
    const latest = await store.getSeriesLatest('task-1', 's1')
    expect(latest?.id).toBe(event.id)
  })

  it('returns null for missing series', async () => {
    expect(await store.getSeriesLatest('task-1', 'missing')).toBeNull()
  })
})

describe('RedisShortTermStore - TTL', () => {
  it('setTTL sets expiry on task and events keys', async () => {
    await store.saveTask(makeTask())
    await store.appendEvent('task-1', makeEvent(0))
    await store.setSeriesLatest('task-1', 's1', makeEvent(0))
    await store.setTTL('task-1', 60)
    const taskTtl = await redis.ttl('taskcast:task:task-1')
    const eventsTtl = await redis.ttl('taskcast:events:task-1')
    const seriesTtl = await redis.ttl('taskcast:series:task-1:s1')
    expect(taskTtl).toBeGreaterThan(0)
    expect(eventsTtl).toBeGreaterThan(0)
    expect(seriesTtl).toBeGreaterThan(0)
  })
})

describe('RedisShortTermStore - custom prefix', () => {
  it('uses custom prefix for keys', async () => {
    const customStore = new RedisShortTermStore(redis, { prefix: 'myapp' })
    await customStore.saveTask(makeTask('t1'))
    // The task should be under 'myapp:task:t1', not 'taskcast:task:t1'
    const raw = await redis.get('myapp:task:t1')
    expect(raw).not.toBeNull()
    const defaultRaw = await redis.get('taskcast:task:t1')
    expect(defaultRaw).toBeNull()
    // Retrieve through store
    const task = await customStore.getTask('t1')
    expect(task?.id).toBe('t1')
  })
})

describe('RedisShortTermStore - replaceLastSeriesEvent', () => {
  it('appends event when no previous series event exists', async () => {
    const event = makeEvent(0)
    await store.replaceLastSeriesEvent('task-1', 's1', event)
    const events = await store.getEvents('task-1')
    expect(events).toHaveLength(1)
    expect(events[0]?.id).toBe(event.id)
    const latest = await store.getSeriesLatest('task-1', 's1')
    expect(latest?.id).toBe(event.id)
  })

  it('replaces previous series event in the list', async () => {
    const first = makeEvent(0)
    const second = { ...makeEvent(1), id: 'evt-updated' }
    // First event is appended and tracked as series latest
    await store.replaceLastSeriesEvent('task-1', 's1', first)
    // Second call should replace first in the event list
    await store.replaceLastSeriesEvent('task-1', 's1', second)
    const events = await store.getEvents('task-1')
    // The list should have exactly 1 event (replaced, not appended)
    expect(events).toHaveLength(1)
    expect(events[0]?.id).toBe('evt-updated')
    const latest = await store.getSeriesLatest('task-1', 's1')
    expect(latest?.id).toBe('evt-updated')
  })

  it('handles malformed JSON entries in the events list gracefully when replacing', async () => {
    // Insert a malformed JSON entry directly via redis, then a valid event
    // When replaceLastSeriesEvent scans for prev.id, it should skip malformed entries (catch block)
    const first = makeEvent(0)
    // Manually push a malformed JSON entry directly into the events list
    await redis.rpush('taskcast:events:task-1', 'not-valid-json')
    // Then push a valid event and track it as the series latest
    await store.appendEvent('task-1', first)
    await store.setSeriesLatest('task-1', 's1', first)

    const second = { ...makeEvent(1), id: 'evt-updated' }
    // replaceLastSeriesEvent should not throw even though there's a malformed entry
    await store.replaceLastSeriesEvent('task-1', 's1', second)

    const latest = await store.getSeriesLatest('task-1', 's1')
    expect(latest?.id).toBe('evt-updated')
  })
})
