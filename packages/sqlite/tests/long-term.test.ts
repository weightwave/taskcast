import Database from 'better-sqlite3'
import { mkdtempSync, rmSync, readFileSync } from 'node:fs'
import { join } from 'node:path'
import { tmpdir } from 'node:os'
import { describe, it, expect, beforeEach, afterEach } from 'vitest'
import type { Task, TaskEvent, WorkerAuditEvent } from '@taskcast/core'
import { SqliteLongTermStore } from '../src/long-term.js'

function makeTask(id = 'task-1'): Task {
  return {
    id,
    status: 'pending',
    params: { prompt: 'hello' },
    createdAt: 1000,
    updatedAt: 1000,
  }
}

function makeEvent(taskId: string, index: number): TaskEvent {
  return {
    id: `evt-${taskId}-${index}`,
    taskId,
    index,
    timestamp: 1000 + index * 100,
    type: 'llm.delta',
    level: 'info',
    data: { text: `msg-${index}` },
  }
}

describe('SqliteLongTermStore', () => {
  let dir: string
  let db: InstanceType<typeof Database>
  let store: SqliteLongTermStore

  beforeEach(() => {
    dir = mkdtempSync(join(tmpdir(), 'taskcast-sqlite-lt-'))
    db = new Database(join(dir, 'test.db'))
    const migration = readFileSync(
      join(import.meta.dirname, '../migrations/001_initial.sql'),
      'utf8',
    )
    db.exec(migration)
    db.pragma('journal_mode = WAL')
    db.pragma('foreign_keys = ON')
    store = new SqliteLongTermStore(db)
  })

  afterEach(() => {
    db.close()
    rmSync(dir, { recursive: true, force: true })
  })

  // ─── saveTask / getTask ─────────────────────────────────────────────────

  it('should save and retrieve a task', async () => {
    const task = makeTask()
    await store.saveTask(task)
    const retrieved = await store.getTask('task-1')
    expect(retrieved).toEqual(task)
  })

  it('should return null for a missing task', async () => {
    const result = await store.getTask('nonexistent')
    expect(result).toBeNull()
  })

  it('should upsert task on conflict (update status)', async () => {
    const task = makeTask()
    await store.saveTask(task)

    const updated: Task = { ...task, status: 'running', updatedAt: 2000 }
    await store.saveTask(updated)

    const retrieved = await store.getTask('task-1')
    expect(retrieved).toEqual(updated)
    expect(retrieved!.status).toBe('running')
  })

  it('should save task with all optional fields', async () => {
    const task: Task = {
      ...makeTask(),
      type: 'llm',
      result: { answer: 42 },
      error: { message: 'boom', code: 'ERR' },
      metadata: { source: 'test' },
      authConfig: {
        rules: [
          {
            match: { scope: ['task:create'] },
            require: { sub: ['user-1'] },
          },
        ],
      },
      webhooks: [
        {
          url: 'https://example.com/hook',
          secret: 'shh',
        },
      ],
      cleanup: {
        rules: [
          {
            trigger: { afterMs: 60000 },
            target: 'events',
          },
        ],
      },
      completedAt: 3000,
      ttl: 60,
      tags: ['gpu', 'priority'],
      assignMode: 'pull',
      cost: 3,
      assignedWorker: 'worker-1',
      disconnectPolicy: 'cancel',
    }
    await store.saveTask(task)
    const retrieved = await store.getTask('task-1')
    expect(retrieved).toEqual(task)
  })

  // ─── saveEvent / getEvents ──────────────────────────────────────────────

  it('should save and retrieve events in order', async () => {
    await store.saveTask(makeTask())
    const e0 = makeEvent('task-1', 0)
    const e1 = makeEvent('task-1', 1)
    const e2 = makeEvent('task-1', 2)

    await store.saveEvent(e0)
    await store.saveEvent(e1)
    await store.saveEvent(e2)

    const events = await store.getEvents('task-1')
    expect(events).toEqual([e0, e1, e2])
  })

  it('should ignore duplicate events (ON CONFLICT DO NOTHING)', async () => {
    await store.saveTask(makeTask())
    const event = makeEvent('task-1', 0)

    await store.saveEvent(event)
    // Saving the same event again should not throw
    await store.saveEvent(event)

    const events = await store.getEvents('task-1')
    expect(events).toHaveLength(1)
    expect(events[0]).toEqual(event)
  })

  it('should return empty array when no events exist', async () => {
    await store.saveTask(makeTask())
    const events = await store.getEvents('task-1')
    expect(events).toEqual([])
  })

  // ─── since filters ────────────────────────────────────────────────────

  it('should filter events by since.index', async () => {
    await store.saveTask(makeTask())
    for (let i = 0; i < 5; i++) {
      await store.saveEvent(makeEvent('task-1', i))
    }

    const events = await store.getEvents('task-1', { since: { index: 2 } })
    expect(events).toHaveLength(2)
    expect(events[0]!.index).toBe(3)
    expect(events[1]!.index).toBe(4)
  })

  it('should filter events by since.timestamp', async () => {
    await store.saveTask(makeTask())
    for (let i = 0; i < 5; i++) {
      await store.saveEvent(makeEvent('task-1', i))
    }
    // Timestamps: 1000, 1100, 1200, 1300, 1400
    const events = await store.getEvents('task-1', { since: { timestamp: 1200 } })
    expect(events).toHaveLength(2)
    expect(events[0]!.timestamp).toBe(1300)
    expect(events[1]!.timestamp).toBe(1400)
  })

  it('should filter events by since.id', async () => {
    await store.saveTask(makeTask())
    for (let i = 0; i < 5; i++) {
      await store.saveEvent(makeEvent('task-1', i))
    }

    const events = await store.getEvents('task-1', { since: { id: 'evt-task-1-2' } })
    expect(events).toHaveLength(2)
    expect(events[0]!.id).toBe('evt-task-1-3')
    expect(events[1]!.id).toBe('evt-task-1-4')
  })

  it('should return all events when since.id is not found', async () => {
    await store.saveTask(makeTask())
    for (let i = 0; i < 3; i++) {
      await store.saveEvent(makeEvent('task-1', i))
    }

    const events = await store.getEvents('task-1', { since: { id: 'nonexistent-id' } })
    expect(events).toHaveLength(3)
  })

  it('should respect limit parameter', async () => {
    await store.saveTask(makeTask())
    for (let i = 0; i < 10; i++) {
      await store.saveEvent(makeEvent('task-1', i))
    }

    const events = await store.getEvents('task-1', { limit: 3 })
    expect(events).toHaveLength(3)
    expect(events[0]!.index).toBe(0)
    expect(events[2]!.index).toBe(2)
  })

  it('should apply limit after since filter', async () => {
    await store.saveTask(makeTask())
    for (let i = 0; i < 10; i++) {
      await store.saveEvent(makeEvent('task-1', i))
    }

    const events = await store.getEvents('task-1', { since: { index: 5 }, limit: 2 })
    expect(events).toHaveLength(2)
    expect(events[0]!.index).toBe(6)
    expect(events[1]!.index).toBe(7)
  })

  // ─── event with series fields ─────────────────────────────────────────

  it('should preserve seriesId and seriesMode on events', async () => {
    await store.saveTask(makeTask())
    const event: TaskEvent = {
      ...makeEvent('task-1', 0),
      seriesId: 'my-series',
      seriesMode: 'accumulate',
    }
    await store.saveEvent(event)
    const events = await store.getEvents('task-1')
    expect(events[0]).toEqual(event)
    expect(events[0]!.seriesId).toBe('my-series')
    expect(events[0]!.seriesMode).toBe('accumulate')
  })

  it('should preserve seriesAccField on events', async () => {
    await store.saveTask(makeTask())
    const event: TaskEvent = {
      ...makeEvent('task-1', 0),
      seriesId: 'acc-series',
      seriesMode: 'accumulate',
      seriesAccField: 'text',
    }
    await store.saveEvent(event)
    const events = await store.getEvents('task-1')
    expect(events[0]).toEqual(event)
    expect(events[0]!.seriesAccField).toBe('text')
  })

  it('should save event with null data', async () => {
    await store.saveTask(makeTask())
    const event: TaskEvent = {
      ...makeEvent('task-1', 0),
      data: null,
    }
    await store.saveEvent(event)
    const events = await store.getEvents('task-1')
    expect(events[0]!.data).toBeNull()
  })
})

// ─── Worker Event helpers ───────────────────────────────────────────────────

function makeWorkerEvent(overrides: Partial<WorkerAuditEvent> = {}): WorkerAuditEvent {
  return {
    id: 'we-1',
    workerId: 'w1',
    timestamp: 5000,
    action: 'connected',
    ...overrides,
  }
}

describe('SqliteLongTermStore - saveWorkerEvent / getWorkerEvents', () => {
  let dir: string
  let db: InstanceType<typeof Database>
  let store: SqliteLongTermStore

  beforeEach(() => {
    dir = mkdtempSync(join(tmpdir(), 'taskcast-sqlite-lt-worker-'))
    db = new Database(join(dir, 'test.db'))
    const migration = readFileSync(
      join(import.meta.dirname, '../migrations/001_initial.sql'),
      'utf8',
    )
    db.exec(migration)
    db.pragma('journal_mode = WAL')
    db.pragma('foreign_keys = ON')
    store = new SqliteLongTermStore(db)
  })

  afterEach(() => {
    db.close()
    rmSync(dir, { recursive: true, force: true })
  })

  // ─── save and retrieve ──────────────────────────────────────────────────

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
  })

  // ─── since.timestamp filter ─────────────────────────────────────────────

  it('filters by since.timestamp', async () => {
    await store.saveWorkerEvent(makeWorkerEvent({ id: 'we-1', workerId: 'w1', timestamp: 1000, action: 'connected' }))
    await store.saveWorkerEvent(makeWorkerEvent({ id: 'we-2', workerId: 'w1', timestamp: 2000, action: 'updated' }))
    await store.saveWorkerEvent(makeWorkerEvent({ id: 'we-3', workerId: 'w1', timestamp: 3000, action: 'disconnected' }))
    const events = await store.getWorkerEvents('w1', { since: { timestamp: 1000 } })
    expect(events).toHaveLength(2)
    expect(events[0]!.id).toBe('we-2')
    expect(events[1]!.id).toBe('we-3')
  })

  // ─── since.id filter ───────────────────────────────────────────────────

  it('filters by since.id', async () => {
    await store.saveWorkerEvent(makeWorkerEvent({ id: 'we-1', workerId: 'w1', timestamp: 1000, action: 'connected' }))
    await store.saveWorkerEvent(makeWorkerEvent({ id: 'we-2', workerId: 'w1', timestamp: 2000, action: 'updated' }))
    await store.saveWorkerEvent(makeWorkerEvent({ id: 'we-3', workerId: 'w1', timestamp: 3000, action: 'disconnected' }))
    const events = await store.getWorkerEvents('w1', { since: { id: 'we-1' } })
    expect(events).toHaveLength(2)
    expect(events[0]!.id).toBe('we-2')
    expect(events[1]!.id).toBe('we-3')
  })

  it('returns all events when since.id is not found', async () => {
    await store.saveWorkerEvent(makeWorkerEvent({ id: 'we-1', workerId: 'w1', timestamp: 1000, action: 'connected' }))
    await store.saveWorkerEvent(makeWorkerEvent({ id: 'we-2', workerId: 'w1', timestamp: 2000, action: 'updated' }))
    const events = await store.getWorkerEvents('w1', { since: { id: 'nonexistent-id' } })
    expect(events).toHaveLength(2)
  })

  // ─── limit ─────────────────────────────────────────────────────────────

  it('respects limit parameter', async () => {
    await store.saveWorkerEvent(makeWorkerEvent({ id: 'we-1', workerId: 'w1', timestamp: 1000, action: 'connected' }))
    await store.saveWorkerEvent(makeWorkerEvent({ id: 'we-2', workerId: 'w1', timestamp: 2000, action: 'updated' }))
    await store.saveWorkerEvent(makeWorkerEvent({ id: 'we-3', workerId: 'w1', timestamp: 3000, action: 'disconnected' }))
    const events = await store.getWorkerEvents('w1', { limit: 2 })
    expect(events).toHaveLength(2)
    expect(events[0]!.id).toBe('we-1')
    expect(events[1]!.id).toBe('we-2')
  })

  // ─── combined filters ─────────────────────────────────────────────────

  it('combines since.id and limit', async () => {
    for (let i = 0; i < 5; i++) {
      await store.saveWorkerEvent(makeWorkerEvent({ id: `we-${i}`, workerId: 'w1', timestamp: 1000 + i * 100, action: 'connected' }))
    }
    const events = await store.getWorkerEvents('w1', { since: { id: 'we-1' }, limit: 2 })
    expect(events).toHaveLength(2)
    expect(events[0]!.id).toBe('we-2')
    expect(events[1]!.id).toBe('we-3')
  })

  it('combines since.timestamp and limit', async () => {
    for (let i = 0; i < 5; i++) {
      await store.saveWorkerEvent(makeWorkerEvent({ id: `we-${i}`, workerId: 'w1', timestamp: 1000 + i * 100, action: 'connected' }))
    }
    const events = await store.getWorkerEvents('w1', { since: { timestamp: 1100 }, limit: 2 })
    expect(events).toHaveLength(2)
    expect(events[0]!.id).toBe('we-2')
    expect(events[1]!.id).toBe('we-3')
  })

  // ─── duplicates ────────────────────────────────────────────────────────

  it('ignores duplicate worker event id (ON CONFLICT DO NOTHING)', async () => {
    const event = makeWorkerEvent({ id: 'we-dup', workerId: 'w1', timestamp: 5000, action: 'connected' })
    await store.saveWorkerEvent(event)
    await store.saveWorkerEvent(event) // should not throw
    const events = await store.getWorkerEvents('w1')
    expect(events).toHaveLength(1)
  })

  // ─── data field ────────────────────────────────────────────────────────

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

  // ─── empty result ─────────────────────────────────────────────────────

  it('returns empty array for unknown worker', async () => {
    const events = await store.getWorkerEvents('unknown-worker')
    expect(events).toEqual([])
  })
})
