import { describe, it, expect, beforeAll, afterAll, beforeEach } from 'vitest'
import postgres from 'postgres'
import { GenericContainer, type StartedTestContainer } from 'testcontainers'
import { PostgresLongTermStore } from '../src/long-term.js'
import { readFileSync } from 'fs'
import { join } from 'path'
import type { Task, TaskEvent } from '@taskcast/core'

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

  // Run migration
  const migration = readFileSync(
    join(import.meta.dirname, '../migrations/001_initial.sql'),
    'utf8'
  )
  await sql.unsafe(migration)
}, 120000)

afterAll(async () => {
  await sql.end()
  await container?.stop()
})

beforeEach(async () => {
  await sql`TRUNCATE taskcast_events, taskcast_tasks CASCADE`
})

const makeTask = (id = 'task-1'): Task => ({
  id,
  status: 'pending',
  params: { prompt: 'hello' },
  createdAt: 1000,
  updatedAt: 1000,
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

describe('PostgresLongTermStore - custom prefix', () => {
  it('uses custom table names when prefix provided', async () => {
    // Create tables with custom prefix
    await sql.unsafe(`
      CREATE TABLE IF NOT EXISTS myapp_tasks (LIKE taskcast_tasks INCLUDING ALL);
      CREATE TABLE IF NOT EXISTS myapp_events (
        LIKE taskcast_events INCLUDING ALL,
        CONSTRAINT myapp_events_task_id_fkey FOREIGN KEY (task_id) REFERENCES myapp_tasks(id) ON DELETE CASCADE
      );
    `)
    const customStore = new PostgresLongTermStore(sql, { prefix: 'myapp' })
    await customStore.saveTask(makeTask('custom-task'))
    const task = await customStore.getTask('custom-task')
    expect(task?.id).toBe('custom-task')
    // Should not appear in default tables
    const defaultTask = await store.getTask('custom-task')
    expect(defaultTask).toBeNull()
    // Cleanup
    await sql.unsafe('DROP TABLE IF EXISTS myapp_events, myapp_tasks')
  })
})
