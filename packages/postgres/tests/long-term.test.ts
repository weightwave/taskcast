import { describe, it, expect, beforeAll, afterAll, beforeEach } from 'vitest'
import postgres from 'postgres'
import { GenericContainer, type StartedTestContainer } from 'testcontainers'
import { PostgresLongTermStore } from '../src/long-term.js'
import { join } from 'node:path'
import { runMigrations } from '../src/migration-runner.js'
import type { Task, TaskEvent, WorkerAuditEvent } from '@taskcast/core'

let container: StartedTestContainer
let sql: ReturnType<typeof postgres>
let store: PostgresLongTermStore

beforeAll(async () => {
  container = await new GenericContainer('postgres:16-alpine')
    .withEnvironment({
      POSTGRES_USER: 'test',
      POSTGRES_PASSWORD: 'test',
      POSTGRES_DB: 'testdb',
    })
    .withExposedPorts(5432)
    .start()
  const connUri = `postgres://test:test@localhost:${container.getMappedPort(5432)}/testdb`
  sql = postgres(connUri)
  store = new PostgresLongTermStore(sql)

  // Run migrations
  const migrationsDir = join(import.meta.dirname, '../../../migrations/postgres')
  await runMigrations(sql, migrationsDir)
}, 120000)

afterAll(async () => {
  await sql?.end()
  await container?.stop()
})

beforeEach(async () => {
  await sql`TRUNCATE taskcast_events, taskcast_tasks, taskcast_worker_events CASCADE`
})

const makeTask = (id = 'task-1'): Task => ({
  id,
  status: 'pending',
  params: { prompt: 'hello' },
  createdAt: 1000,
  updatedAt: 1000,
})

const makeWorkerEvent = (overrides: Partial<WorkerAuditEvent> = {}): WorkerAuditEvent => ({
  id: 'we-1',
  workerId: 'w1',
  timestamp: Date.now(),
  action: 'connected',
  ...overrides,
})

const makeEvent = (taskId = 'task-1', index = 0): TaskEvent => ({
  id: `evt-${taskId}-${index}`,
  taskId,
  index,
  timestamp: 1000 + index,
  type: 'llm.delta',
  level: 'info',
  data: { text: `msg-${index}` },
})

describe('PostgresLongTermStore - tasks', () => {
  it('saves and retrieves a task', async () => {
    await store.saveTask(makeTask())
    const task = await store.getTask('task-1')
    expect(task?.status).toBe('pending')
    expect(task?.params).toEqual({ prompt: 'hello' })
  })

  it('returns null for missing task', async () => {
    expect(await store.getTask('missing')).toBeNull()
  })

  it('upserts task on conflict', async () => {
    await store.saveTask(makeTask())
    await store.saveTask({ ...makeTask(), status: 'running' })
    const task = await store.getTask('task-1')
    expect(task?.status).toBe('running')
  })
})

describe('PostgresLongTermStore - events', () => {
  it('saves and retrieves events in order', async () => {
    await store.saveTask(makeTask())
    await store.saveEvent(makeEvent('task-1', 0))
    await store.saveEvent(makeEvent('task-1', 1))
    const events = await store.getEvents('task-1')
    expect(events).toHaveLength(2)
    expect(events[0]?.index).toBe(0)
    expect(events[1]?.index).toBe(1)
  })

  it('filters by since.index', async () => {
    await store.saveTask(makeTask())
    for (let i = 0; i < 5; i++) await store.saveEvent(makeEvent('task-1', i))
    const events = await store.getEvents('task-1', { since: { index: 2 } })
    expect(events.map((e) => e.index)).toEqual([3, 4])
  })

  it('filters by since.timestamp', async () => {
    await store.saveTask(makeTask())
    for (let i = 0; i < 5; i++) await store.saveEvent(makeEvent('task-1', i))
    const events = await store.getEvents('task-1', { since: { timestamp: 1002 } })
    expect(events.map((e) => e.index)).toEqual([3, 4])
  })

  it('ignores duplicate event id (upsert)', async () => {
    await store.saveTask(makeTask())
    const event = makeEvent('task-1', 0)
    await store.saveEvent(event)
    await store.saveEvent(event) // should not throw
    const events = await store.getEvents('task-1')
    expect(events).toHaveLength(1)
  })
})

describe('PostgresLongTermStore - since.id cursor', () => {
  it('filters events by since.id', async () => {
    await store.saveTask(makeTask())
    for (let i = 0; i < 4; i++) await store.saveEvent(makeEvent('task-1', i))
    // Get the anchor event (index=1) and filter since it
    const all = await store.getEvents('task-1')
    const anchor = all[1]! // index=1
    const events = await store.getEvents('task-1', { since: { id: anchor.id } })
    // Should return events with index > 1
    expect(events.map((e) => e.index)).toEqual([2, 3])
  })

  it('returns all events when since.id not found', async () => {
    await store.saveTask(makeTask())
    for (let i = 0; i < 3; i++) await store.saveEvent(makeEvent('task-1', i))
    // Nonexistent id results in anchorIdx=-1, so all events returned
    const events = await store.getEvents('task-1', { since: { id: 'nonexistent-id' } })
    expect(events).toHaveLength(3)
  })
})

describe('PostgresLongTermStore - limit parameter', () => {
  it('limits results when limit is specified with no since', async () => {
    await store.saveTask(makeTask())
    for (let i = 0; i < 5; i++) await store.saveEvent(makeEvent('task-1', i))
    const events = await store.getEvents('task-1', { limit: 2 })
    expect(events).toHaveLength(2)
  })

  it('limits results when limit is specified with since.index', async () => {
    await store.saveTask(makeTask())
    for (let i = 0; i < 5; i++) await store.saveEvent(makeEvent('task-1', i))
    const events = await store.getEvents('task-1', { since: { index: 1 }, limit: 2 })
    expect(events).toHaveLength(2)
    expect(events[0]?.index).toBe(2)
  })

  it('limits results when limit is specified with since.timestamp', async () => {
    await store.saveTask(makeTask())
    for (let i = 0; i < 5; i++) await store.saveEvent(makeEvent('task-1', i))
    const events = await store.getEvents('task-1', { since: { timestamp: 1001 }, limit: 2 })
    expect(events).toHaveLength(2)
  })

  it('limits results when limit is specified with since.id', async () => {
    await store.saveTask(makeTask())
    for (let i = 0; i < 5; i++) await store.saveEvent(makeEvent('task-1', i))
    const all = await store.getEvents('task-1')
    const anchor = all[0]!
    const events = await store.getEvents('task-1', { since: { id: anchor.id }, limit: 2 })
    expect(events).toHaveLength(2)
    expect(events[0]?.index).toBe(1)
  })
})

describe('PostgresLongTermStore - full task fields', () => {
  it('saves and retrieves task with all optional fields', async () => {
    const fullTask: Task = {
      id: 'full-task',
      type: 'llm.chat',
      status: 'completed',
      params: { prompt: 'test' },
      result: { answer: 42 },
      error: { message: 'oops', code: 'ERR_TEST' },
      metadata: { userId: 'u1' },
      authConfig: { mode: 'jwt' } as never,
      webhooks: [{ url: 'https://example.com/hook' }] as never,
      cleanup: { rules: [] },
      createdAt: 1000,
      updatedAt: 2000,
      completedAt: 2000,
      ttl: 86400,
    }
    await store.saveTask(fullTask)
    const task = await store.getTask('full-task')
    expect(task?.type).toBe('llm.chat')
    expect(task?.result).toEqual({ answer: 42 })
    expect(task?.error?.code).toBe('ERR_TEST')
    expect(Number(task?.completedAt)).toBe(2000)
    expect(Number(task?.ttl)).toBe(86400)
  })

  it('saves and retrieves event with seriesId and seriesMode', async () => {
    await store.saveTask(makeTask())
    const event: TaskEvent = {
      id: 'evt-series',
      taskId: 'task-1',
      index: 99,
      timestamp: 9999,
      type: 'chunk',
      level: 'info',
      data: null,
      seriesId: 'series-1',
      seriesMode: 'accumulate',
    }
    await store.saveEvent(event)
    const events = await store.getEvents('task-1')
    const found = events.find((e) => e.id === 'evt-series')
    expect(found?.seriesId).toBe('series-1')
    expect(found?.seriesMode).toBe('accumulate')
  })
})

describe('PostgresLongTermStore - saveWorkerEvent / getWorkerEvents', () => {
  it('saves and retrieves a worker event', async () => {
    const event = makeWorkerEvent({ id: 'we-1', workerId: 'w1', timestamp: 5000, action: 'connected' })
    await store.saveWorkerEvent(event)
    const events = await store.getWorkerEvents('w1')
    expect(events).toHaveLength(1)
    expect(events[0]).toEqual(event)
  })

  it('returns multiple events for same worker ordered by timestamp', async () => {
    await store.saveWorkerEvent(makeWorkerEvent({ id: 'we-1', workerId: 'w1', timestamp: 1000, action: 'connected' }))
    await store.saveWorkerEvent(makeWorkerEvent({ id: 'we-2', workerId: 'w1', timestamp: 2000, action: 'updated' }))
    await store.saveWorkerEvent(makeWorkerEvent({ id: 'we-3', workerId: 'w1', timestamp: 3000, action: 'disconnected' }))
    const events = await store.getWorkerEvents('w1')
    expect(events).toHaveLength(3)
    expect(events[0]!.id).toBe('we-1')
    expect(events[1]!.id).toBe('we-2')
    expect(events[2]!.id).toBe('we-3')
    expect(events[0]!.timestamp).toBeLessThan(events[1]!.timestamp)
    expect(events[1]!.timestamp).toBeLessThan(events[2]!.timestamp)
  })

  it('filters by since.timestamp', async () => {
    await store.saveWorkerEvent(makeWorkerEvent({ id: 'we-1', workerId: 'w1', timestamp: 1000, action: 'connected' }))
    await store.saveWorkerEvent(makeWorkerEvent({ id: 'we-2', workerId: 'w1', timestamp: 2000, action: 'updated' }))
    await store.saveWorkerEvent(makeWorkerEvent({ id: 'we-3', workerId: 'w1', timestamp: 3000, action: 'disconnected' }))
    const events = await store.getWorkerEvents('w1', { since: { timestamp: 1000 } })
    expect(events).toHaveLength(2)
    expect(events[0]!.id).toBe('we-2')
    expect(events[1]!.id).toBe('we-3')
  })

  it('filters by since.id', async () => {
    await store.saveWorkerEvent(makeWorkerEvent({ id: 'we-1', workerId: 'w1', timestamp: 1000, action: 'connected' }))
    await store.saveWorkerEvent(makeWorkerEvent({ id: 'we-2', workerId: 'w1', timestamp: 2000, action: 'updated' }))
    await store.saveWorkerEvent(makeWorkerEvent({ id: 'we-3', workerId: 'w1', timestamp: 3000, action: 'disconnected' }))
    const events = await store.getWorkerEvents('w1', { since: { id: 'we-1' } })
    expect(events).toHaveLength(2)
    expect(events[0]!.id).toBe('we-2')
    expect(events[1]!.id).toBe('we-3')
  })

  it('respects limit parameter', async () => {
    await store.saveWorkerEvent(makeWorkerEvent({ id: 'we-1', workerId: 'w1', timestamp: 1000, action: 'connected' }))
    await store.saveWorkerEvent(makeWorkerEvent({ id: 'we-2', workerId: 'w1', timestamp: 2000, action: 'updated' }))
    await store.saveWorkerEvent(makeWorkerEvent({ id: 'we-3', workerId: 'w1', timestamp: 3000, action: 'disconnected' }))
    const events = await store.getWorkerEvents('w1', { limit: 2 })
    expect(events).toHaveLength(2)
    expect(events[0]!.id).toBe('we-1')
    expect(events[1]!.id).toBe('we-2')
  })

  it('returns empty array for unknown worker', async () => {
    const events = await store.getWorkerEvents('unknown-worker')
    expect(events).toEqual([])
  })

  it('saves worker event with data field', async () => {
    const event = makeWorkerEvent({
      id: 'we-data',
      workerId: 'w1',
      timestamp: 5000,
      action: 'task_assigned',
      data: { taskId: 'task-99', reason: 'manual' },
    })
    await store.saveWorkerEvent(event)
    const events = await store.getWorkerEvents('w1')
    expect(events).toHaveLength(1)
    expect(events[0]!.data).toEqual({ taskId: 'task-99', reason: 'manual' })
  })

  it('saves worker event without data field', async () => {
    const event = makeWorkerEvent({ id: 'we-nodata', workerId: 'w1', timestamp: 5000, action: 'connected' })
    await store.saveWorkerEvent(event)
    const events = await store.getWorkerEvents('w1')
    expect(events).toHaveLength(1)
    expect(events[0]!.data).toBeUndefined()
  })

  it('ignores duplicate worker event id (upsert)', async () => {
    const event = makeWorkerEvent({ id: 'we-dup', workerId: 'w1', timestamp: 5000, action: 'connected' })
    await store.saveWorkerEvent(event)
    await store.saveWorkerEvent(event) // should not throw
    const events = await store.getWorkerEvents('w1')
    expect(events).toHaveLength(1)
  })

  it('combines since.id and limit', async () => {
    for (let i = 0; i < 5; i++) {
      await store.saveWorkerEvent(makeWorkerEvent({ id: `we-${i}`, workerId: 'w1', timestamp: 1000 + i, action: 'connected' }))
    }
    const events = await store.getWorkerEvents('w1', { since: { id: 'we-1' }, limit: 2 })
    expect(events).toHaveLength(2)
    expect(events[0]!.id).toBe('we-2')
    expect(events[1]!.id).toBe('we-3')
  })

  it('combines since.timestamp and limit', async () => {
    for (let i = 0; i < 5; i++) {
      await store.saveWorkerEvent(makeWorkerEvent({ id: `we-${i}`, workerId: 'w1', timestamp: 1000 + i, action: 'connected' }))
    }
    const events = await store.getWorkerEvents('w1', { since: { timestamp: 1001 }, limit: 2 })
    expect(events).toHaveLength(2)
    expect(events[0]!.id).toBe('we-2')
    expect(events[1]!.id).toBe('we-3')
  })
})

describe('PostgresLongTermStore - task worker fields persistence', () => {
  it('saves and retrieves task with tags', async () => {
    const task: Task = { ...makeTask('tag-task'), tags: ['gpu', 'us-east'] }
    await store.saveTask(task)
    const retrieved = await store.getTask('tag-task')
    expect(retrieved?.tags).toEqual(['gpu', 'us-east'])
  })

  it('saves and retrieves task with assignMode', async () => {
    const task: Task = { ...makeTask('assign-task'), assignMode: 'ws-offer' }
    await store.saveTask(task)
    const retrieved = await store.getTask('assign-task')
    expect(retrieved?.assignMode).toBe('ws-offer')
  })

  it('saves and retrieves task with cost', async () => {
    const task: Task = { ...makeTask('cost-task'), cost: 5 }
    await store.saveTask(task)
    const retrieved = await store.getTask('cost-task')
    expect(retrieved?.cost).toBe(5)
  })

  it('saves and retrieves task with assignedWorker', async () => {
    const task: Task = { ...makeTask('worker-task'), assignedWorker: 'w1' }
    await store.saveTask(task)
    const retrieved = await store.getTask('worker-task')
    expect(retrieved?.assignedWorker).toBe('w1')
  })

  it('saves and retrieves task with disconnectPolicy', async () => {
    const task: Task = { ...makeTask('dp-task'), disconnectPolicy: 'reassign' }
    await store.saveTask(task)
    const retrieved = await store.getTask('dp-task')
    expect(retrieved?.disconnectPolicy).toBe('reassign')
  })

  it('saves task without new fields — they are absent (not null)', async () => {
    const task = makeTask('plain-task')
    await store.saveTask(task)
    const retrieved = await store.getTask('plain-task')
    expect(retrieved).not.toBeNull()
    expect(retrieved!.tags).toBeUndefined()
    expect(retrieved!.assignMode).toBeUndefined()
    expect(retrieved!.cost).toBeUndefined()
    expect(retrieved!.assignedWorker).toBeUndefined()
    expect(retrieved!.disconnectPolicy).toBeUndefined()
  })

  it('upsert preserves new worker fields', async () => {
    const task: Task = {
      ...makeTask('upsert-task'),
      tags: ['gpu'],
      assignMode: 'pull',
      cost: 3,
      assignedWorker: 'w2',
      disconnectPolicy: 'mark',
    }
    await store.saveTask(task)
    // Upsert with updated status — should preserve worker fields
    await store.saveTask({ ...task, status: 'running', updatedAt: 2000 })
    const retrieved = await store.getTask('upsert-task')
    expect(retrieved?.status).toBe('running')
    expect(retrieved?.tags).toEqual(['gpu'])
    expect(retrieved?.assignMode).toBe('pull')
    expect(retrieved?.cost).toBe(3)
    expect(retrieved?.assignedWorker).toBe('w2')
    expect(retrieved?.disconnectPolicy).toBe('mark')
  })

  it('saves task with all new fields at once', async () => {
    const task: Task = {
      ...makeTask('all-fields-task'),
      tags: ['gpu', 'eu-west', 'high-priority'],
      assignMode: 'ws-race',
      cost: 10,
      assignedWorker: 'w42',
      disconnectPolicy: 'fail',
    }
    await store.saveTask(task)
    const retrieved = await store.getTask('all-fields-task')
    expect(retrieved?.tags).toEqual(['gpu', 'eu-west', 'high-priority'])
    expect(retrieved?.assignMode).toBe('ws-race')
    expect(retrieved?.cost).toBe(10)
    expect(retrieved?.assignedWorker).toBe('w42')
    expect(retrieved?.disconnectPolicy).toBe('fail')
  })

  it('upsert can update worker fields to new values', async () => {
    const task: Task = {
      ...makeTask('update-fields-task'),
      assignedWorker: 'w1',
      cost: 2,
    }
    await store.saveTask(task)
    // Update assigned worker and cost
    await store.saveTask({ ...task, assignedWorker: 'w3', cost: 7, updatedAt: 3000 })
    const retrieved = await store.getTask('update-fields-task')
    expect(retrieved?.assignedWorker).toBe('w3')
    expect(retrieved?.cost).toBe(7)
  })
})
