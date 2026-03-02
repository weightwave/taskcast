import Database from 'better-sqlite3'
import { mkdtempSync, rmSync, readFileSync } from 'node:fs'
import { join } from 'node:path'
import { tmpdir } from 'node:os'
import { describe, it, expect, beforeEach, afterEach } from 'vitest'
import type { Task, TaskEvent } from '@taskcast/core'
import { SqliteShortTermStore } from '../src/short-term.js'

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

describe('SqliteShortTermStore', () => {
  let dir: string
  let db: InstanceType<typeof Database>
  let store: SqliteShortTermStore

  beforeEach(() => {
    dir = mkdtempSync(join(tmpdir(), 'taskcast-sqlite-'))
    db = new Database(join(dir, 'test.db'))
    const migration = readFileSync(
      join(import.meta.dirname, '../migrations/001_initial.sql'),
      'utf8',
    )
    db.exec(migration)
    db.pragma('journal_mode = WAL')
    db.pragma('foreign_keys = ON')
    store = new SqliteShortTermStore(db)
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

  it('should preserve optional fields on task round-trip', async () => {
    const task: Task = {
      ...makeTask(),
      type: 'llm',
      result: { answer: 42 },
      error: { message: 'boom', code: 'ERR' },
      metadata: { source: 'test' },
      completedAt: 3000,
      ttl: 60,
    }
    await store.saveTask(task)
    const retrieved = await store.getTask('task-1')
    expect(retrieved).toEqual(task)
  })

  it('should handle task with no optional fields', async () => {
    const task: Task = {
      id: 'minimal',
      status: 'pending',
      createdAt: 1000,
      updatedAt: 1000,
    }
    await store.saveTask(task)
    const retrieved = await store.getTask('minimal')
    expect(retrieved).toEqual(task)
    expect(retrieved!.params).toBeUndefined()
    expect(retrieved!.type).toBeUndefined()
    expect(retrieved!.result).toBeUndefined()
    expect(retrieved!.error).toBeUndefined()
    expect(retrieved!.metadata).toBeUndefined()
    expect(retrieved!.completedAt).toBeUndefined()
    expect(retrieved!.ttl).toBeUndefined()
  })

  // ─── nextIndex ──────────────────────────────────────────────────────────

  it('should generate monotonic indices starting from 0', async () => {
    // Need a task first because of foreign key constraints on events
    await store.saveTask(makeTask())

    const i0 = await store.nextIndex('task-1')
    const i1 = await store.nextIndex('task-1')
    const i2 = await store.nextIndex('task-1')

    expect(i0).toBe(0)
    expect(i1).toBe(1)
    expect(i2).toBe(2)
  })

  it('should maintain separate counters per task', async () => {
    await store.saveTask(makeTask('task-a'))
    await store.saveTask(makeTask('task-b'))

    const a0 = await store.nextIndex('task-a')
    const b0 = await store.nextIndex('task-b')
    const a1 = await store.nextIndex('task-a')

    expect(a0).toBe(0)
    expect(b0).toBe(0)
    expect(a1).toBe(1)
  })

  // ─── appendEvent / getEvents ────────────────────────────────────────────

  it('should append and retrieve events in order', async () => {
    await store.saveTask(makeTask())
    const e0 = makeEvent('task-1', 0)
    const e1 = makeEvent('task-1', 1)
    const e2 = makeEvent('task-1', 2)

    await store.appendEvent('task-1', e0)
    await store.appendEvent('task-1', e1)
    await store.appendEvent('task-1', e2)

    const events = await store.getEvents('task-1')
    expect(events).toEqual([e0, e1, e2])
  })

  it('should return empty array when no events exist', async () => {
    await store.saveTask(makeTask())
    const events = await store.getEvents('task-1')
    expect(events).toEqual([])
  })

  it('should filter events by since.index', async () => {
    await store.saveTask(makeTask())
    for (let i = 0; i < 5; i++) {
      await store.appendEvent('task-1', makeEvent('task-1', i))
    }

    const events = await store.getEvents('task-1', { since: { index: 2 } })
    expect(events).toHaveLength(2)
    expect(events[0]!.index).toBe(3)
    expect(events[1]!.index).toBe(4)
  })

  it('should filter events by since.timestamp', async () => {
    await store.saveTask(makeTask())
    for (let i = 0; i < 5; i++) {
      await store.appendEvent('task-1', makeEvent('task-1', i))
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
      await store.appendEvent('task-1', makeEvent('task-1', i))
    }

    // since.id = evt-task-1-2 means "everything after event with this id"
    const events = await store.getEvents('task-1', { since: { id: 'evt-task-1-2' } })
    expect(events).toHaveLength(2)
    expect(events[0]!.id).toBe('evt-task-1-3')
    expect(events[1]!.id).toBe('evt-task-1-4')
  })

  it('should return all events when since.id is not found', async () => {
    await store.saveTask(makeTask())
    for (let i = 0; i < 3; i++) {
      await store.appendEvent('task-1', makeEvent('task-1', i))
    }

    const events = await store.getEvents('task-1', { since: { id: 'nonexistent-id' } })
    expect(events).toHaveLength(3)
  })

  it('should respect limit parameter', async () => {
    await store.saveTask(makeTask())
    for (let i = 0; i < 10; i++) {
      await store.appendEvent('task-1', makeEvent('task-1', i))
    }

    const events = await store.getEvents('task-1', { limit: 3 })
    expect(events).toHaveLength(3)
    expect(events[0]!.index).toBe(0)
    expect(events[2]!.index).toBe(2)
  })

  it('should apply limit after since filter', async () => {
    await store.saveTask(makeTask())
    for (let i = 0; i < 10; i++) {
      await store.appendEvent('task-1', makeEvent('task-1', i))
    }

    const events = await store.getEvents('task-1', { since: { index: 5 }, limit: 2 })
    expect(events).toHaveLength(2)
    expect(events[0]!.index).toBe(6)
    expect(events[1]!.index).toBe(7)
  })

  // ─── series ─────────────────────────────────────────────────────────────

  it('should manage series latest (set and get)', async () => {
    await store.saveTask(makeTask())
    const event = makeEvent('task-1', 0)
    await store.setSeriesLatest('task-1', 'series-a', event)

    const latest = await store.getSeriesLatest('task-1', 'series-a')
    expect(latest).toEqual(event)
  })

  it('should return null for missing series', async () => {
    const latest = await store.getSeriesLatest('task-1', 'nonexistent')
    expect(latest).toBeNull()
  })

  it('should update series latest on conflict', async () => {
    await store.saveTask(makeTask())
    const e0 = makeEvent('task-1', 0)
    const e1 = makeEvent('task-1', 1)

    await store.setSeriesLatest('task-1', 'series-a', e0)
    await store.setSeriesLatest('task-1', 'series-a', e1)

    const latest = await store.getSeriesLatest('task-1', 'series-a')
    expect(latest).toEqual(e1)
  })

  // ─── replaceLastSeriesEvent ─────────────────────────────────────────────

  it('should replace last series event in event list', async () => {
    await store.saveTask(makeTask())
    const e0 = makeEvent('task-1', 0)
    const e1 = makeEvent('task-1', 1)

    await store.appendEvent('task-1', e0)
    await store.setSeriesLatest('task-1', 'series-a', e0)

    await store.replaceLastSeriesEvent('task-1', 'series-a', e1)

    const events = await store.getEvents('task-1')
    // e0 should have been replaced by e1
    expect(events).toHaveLength(1)
    expect(events[0]).toEqual(e1)

    // series latest should also be updated
    const latest = await store.getSeriesLatest('task-1', 'series-a')
    expect(latest).toEqual(e1)
  })

  it('should append when no previous series event exists', async () => {
    await store.saveTask(makeTask())

    const e0 = makeEvent('task-1', 0)
    await store.replaceLastSeriesEvent('task-1', 'series-a', e0)

    const events = await store.getEvents('task-1')
    expect(events).toHaveLength(1)
    expect(events[0]).toEqual(e0)

    const latest = await store.getSeriesLatest('task-1', 'series-a')
    expect(latest).toEqual(e0)
  })

  it('should only replace the correct series event, not others', async () => {
    await store.saveTask(makeTask())

    const e0 = makeEvent('task-1', 0) // not part of series
    const e1 = { ...makeEvent('task-1', 1), seriesId: 'series-a', seriesMode: 'latest' as const }
    const e2 = makeEvent('task-1', 2) // not part of series

    await store.appendEvent('task-1', e0)
    await store.appendEvent('task-1', e1)
    await store.appendEvent('task-1', e2)
    await store.setSeriesLatest('task-1', 'series-a', e1)

    const replacement = { ...makeEvent('task-1', 3), seriesId: 'series-a', seriesMode: 'latest' as const }
    await store.replaceLastSeriesEvent('task-1', 'series-a', replacement)

    const events = await store.getEvents('task-1')
    // e1 (idx=1) was replaced by replacement (idx=3). Since SQLite orders by idx,
    // the replacement now sorts after e2 (idx=2).
    expect(events).toHaveLength(3)
    expect(events[0]).toEqual(e0)
    expect(events[1]).toEqual(e2)
    expect(events[2]).toEqual(replacement)
  })

  // ─── setTTL ─────────────────────────────────────────────────────────────

  it('should not throw when calling setTTL (no-op)', async () => {
    await store.saveTask(makeTask())
    await expect(store.setTTL('task-1', 60)).resolves.toBeUndefined()
  })

  // ─── event with series fields ───────────────────────────────────────────

  it('should preserve seriesId and seriesMode on events', async () => {
    await store.saveTask(makeTask())
    const event: TaskEvent = {
      ...makeEvent('task-1', 0),
      seriesId: 'my-series',
      seriesMode: 'accumulate',
    }
    await store.appendEvent('task-1', event)
    const events = await store.getEvents('task-1')
    expect(events[0]).toEqual(event)
    expect(events[0]!.seriesId).toBe('my-series')
    expect(events[0]!.seriesMode).toBe('accumulate')
  })
})
