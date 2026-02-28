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
