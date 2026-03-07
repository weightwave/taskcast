import Database from 'better-sqlite3'
import { mkdtempSync, rmSync, readFileSync } from 'node:fs'
import { join } from 'node:path'
import { tmpdir } from 'node:os'
import { describe, it, expect, beforeEach, afterEach } from 'vitest'
import type { Task, TaskEvent, Worker, WorkerAssignment } from '@taskcast/core'
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

  it('should preserve all task optional fields including assignMode, cost, assignedWorker, disconnectPolicy', async () => {
    const task: Task = {
      ...makeTask(),
      type: 'llm',
      result: { answer: 42 },
      error: { message: 'boom' },
      metadata: { source: 'test' },
      completedAt: 3000,
      ttl: 60,
      tags: ['gpu'],
      assignMode: 'pull',
      cost: 2,
      assignedWorker: 'worker-1',
      disconnectPolicy: 'cancel',
    }
    await store.saveTask(task)
    const retrieved = await store.getTask('task-1')
    expect(retrieved).toEqual(task)
    expect(retrieved!.disconnectPolicy).toBe('cancel')
    expect(retrieved!.assignMode).toBe('pull')
    expect(retrieved!.cost).toBe(2)
    expect(retrieved!.assignedWorker).toBe('worker-1')
  })

  it('should handle event with null data', async () => {
    await store.saveTask(makeTask())
    const event: TaskEvent = {
      ...makeEvent('task-1', 0),
      data: null,
    }
    await store.appendEvent('task-1', event)
    const events = await store.getEvents('task-1')
    expect(events[0]!.data).toBeNull()
  })

  it('should preserve seriesAccField on events', async () => {
    await store.saveTask(makeTask())
    const event: TaskEvent = {
      ...makeEvent('task-1', 0),
      seriesId: 'acc-series',
      seriesMode: 'accumulate',
      seriesAccField: 'text',
    }
    await store.appendEvent('task-1', event)
    const events = await store.getEvents('task-1')
    expect(events[0]!.seriesAccField).toBe('text')
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

    await store.appendEvent('task-1', e0)
    await store.setSeriesLatest('task-1', 'series-a', e0)

    const replacement = { ...makeEvent('task-1', 1), data: { text: 'replaced' } }
    await store.replaceLastSeriesEvent('task-1', 'series-a', replacement)

    const events = await store.getEvents('task-1')
    // Original event's position (id, idx) is preserved, content is replaced
    expect(events).toHaveLength(1)
    expect(events[0]!.id).toBe(e0.id) // original id preserved
    expect(events[0]!.index).toBe(0) // original idx preserved
    expect((events[0]!.data as Record<string, unknown>)?.['text']).toBe('replaced')

    // series latest should be updated
    const latest = await store.getSeriesLatest('task-1', 'series-a')
    expect(latest).toEqual(replacement)
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
    // e1 content replaced in-place, position preserved (idx=1 stays between e0 and e2)
    expect(events).toHaveLength(3)
    expect(events[0]).toEqual(e0)
    expect(events[1]!.id).toBe(e1.id) // original id preserved
    expect(events[1]!.index).toBe(1) // original idx preserved
    expect(events[1]!.type).toBe(replacement.type) // content replaced
    expect(events[2]).toEqual(e2)
  })

  // ─── setTTL ─────────────────────────────────────────────────────────────

  it('should not throw when calling setTTL (no-op)', async () => {
    await store.saveTask(makeTask())
    await expect(store.setTTL('task-1', 60)).resolves.toBeUndefined()
  })

  // ─── clearTTL ──────────────────────────────────────────────────────────

  it('should not throw when calling clearTTL (no-op)', async () => {
    await store.saveTask(makeTask())
    await expect(store.clearTTL('task-1')).resolves.toBeUndefined()
  })

  // ─── listByStatus ─────────────────────────────────────────────────────

  describe('listByStatus', () => {
    it('should return empty array when given empty statuses array', async () => {
      await store.saveTask(makeTask('task-1'))
      const result = await store.listByStatus([])
      expect(result).toEqual([])
    })

    it('should return tasks matching given statuses', async () => {
      const t1: Task = { ...makeTask('task-1'), status: 'pending' }
      const t2: Task = { ...makeTask('task-2'), status: 'running' }
      const t3: Task = { ...makeTask('task-3'), status: 'completed' }
      await store.saveTask(t1)
      await store.saveTask(t2)
      await store.saveTask(t3)

      const pending = await store.listByStatus(['pending'])
      expect(pending).toHaveLength(1)
      expect(pending[0]!.id).toBe('task-1')

      const multiple = await store.listByStatus(['pending', 'running'])
      expect(multiple).toHaveLength(2)
      expect(multiple.map(t => t.id).sort()).toEqual(['task-1', 'task-2'])
    })

    it('should return empty array when no tasks match', async () => {
      await store.saveTask({ ...makeTask('task-1'), status: 'pending' })
      const result = await store.listByStatus(['completed'])
      expect(result).toEqual([])
    })
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

  // ─── listTasks ──────────────────────────────────────────────────────────

  describe('listTasks', () => {
    it('should return all tasks with empty filter', async () => {
      await store.saveTask(makeTask('task-1'))
      await store.saveTask(makeTask('task-2'))
      await store.saveTask(makeTask('task-3'))

      const tasks = await store.listTasks({})
      expect(tasks).toHaveLength(3)
    })

    it('should return empty array when no tasks exist', async () => {
      const tasks = await store.listTasks({})
      expect(tasks).toEqual([])
    })

    it('should filter tasks by status', async () => {
      const t1: Task = { ...makeTask('task-1'), status: 'pending' }
      const t2: Task = { ...makeTask('task-2'), status: 'running' }
      const t3: Task = { ...makeTask('task-3'), status: 'completed' }
      await store.saveTask(t1)
      await store.saveTask(t2)
      await store.saveTask(t3)

      const pending = await store.listTasks({ status: ['pending'] })
      expect(pending).toHaveLength(1)
      expect(pending[0]!.id).toBe('task-1')

      const activeStatuses = await store.listTasks({ status: ['pending', 'running'] })
      expect(activeStatuses).toHaveLength(2)
    })

    it('should filter tasks by types', async () => {
      const t1: Task = { ...makeTask('task-1'), type: 'llm' }
      const t2: Task = { ...makeTask('task-2'), type: 'image' }
      const t3: Task = { ...makeTask('task-3'), type: 'llm' }
      await store.saveTask(t1)
      await store.saveTask(t2)
      await store.saveTask(t3)

      const llmTasks = await store.listTasks({ types: ['llm'] })
      expect(llmTasks).toHaveLength(2)
      expect(llmTasks.every((t) => t.type === 'llm')).toBe(true)
    })

    it('should filter tasks by assignMode', async () => {
      const t1: Task = { ...makeTask('task-1'), assignMode: 'pull' }
      const t2: Task = { ...makeTask('task-2'), assignMode: 'ws-offer' }
      const t3: Task = { ...makeTask('task-3'), assignMode: 'external' }
      await store.saveTask(t1)
      await store.saveTask(t2)
      await store.saveTask(t3)

      const pullTasks = await store.listTasks({ assignMode: ['pull'] })
      expect(pullTasks).toHaveLength(1)
      expect(pullTasks[0]!.id).toBe('task-1')
    })

    it('should exclude tasks by excludeTaskIds', async () => {
      await store.saveTask(makeTask('task-1'))
      await store.saveTask(makeTask('task-2'))
      await store.saveTask(makeTask('task-3'))

      const tasks = await store.listTasks({ excludeTaskIds: ['task-1', 'task-3'] })
      expect(tasks).toHaveLength(1)
      expect(tasks[0]!.id).toBe('task-2')
    })

    it('should respect limit parameter', async () => {
      for (let i = 0; i < 5; i++) {
        await store.saveTask(makeTask(`task-${i}`))
      }

      const tasks = await store.listTasks({ limit: 2 })
      expect(tasks).toHaveLength(2)
    })

    it('should filter tasks by tags (all)', async () => {
      const t1: Task = { ...makeTask('task-1'), tags: ['a', 'b', 'c'] }
      const t2: Task = { ...makeTask('task-2'), tags: ['a', 'b'] }
      const t3: Task = { ...makeTask('task-3'), tags: ['a'] }
      await store.saveTask(t1)
      await store.saveTask(t2)
      await store.saveTask(t3)

      const tasks = await store.listTasks({ tags: { all: ['a', 'b'] } })
      expect(tasks).toHaveLength(2)
      expect(tasks.map((t) => t.id).sort()).toEqual(['task-1', 'task-2'])
    })

    it('should filter tasks by tags (any)', async () => {
      const t1: Task = { ...makeTask('task-1'), tags: ['x'] }
      const t2: Task = { ...makeTask('task-2'), tags: ['y'] }
      const t3: Task = { ...makeTask('task-3'), tags: ['z'] }
      await store.saveTask(t1)
      await store.saveTask(t2)
      await store.saveTask(t3)

      const tasks = await store.listTasks({ tags: { any: ['x', 'z'] } })
      expect(tasks).toHaveLength(2)
      expect(tasks.map((t) => t.id).sort()).toEqual(['task-1', 'task-3'])
    })

    it('should filter tasks by tags (none)', async () => {
      const t1: Task = { ...makeTask('task-1'), tags: ['a', 'b'] }
      const t2: Task = { ...makeTask('task-2'), tags: ['c'] }
      const t3: Task = { ...makeTask('task-3'), tags: ['a'] }
      await store.saveTask(t1)
      await store.saveTask(t2)
      await store.saveTask(t3)

      const tasks = await store.listTasks({ tags: { none: ['a'] } })
      expect(tasks).toHaveLength(1)
      expect(tasks[0]!.id).toBe('task-2')
    })

    it('should combine multiple filters', async () => {
      const t1: Task = { ...makeTask('task-1'), status: 'pending', type: 'llm' }
      const t2: Task = { ...makeTask('task-2'), status: 'running', type: 'llm' }
      const t3: Task = { ...makeTask('task-3'), status: 'pending', type: 'image' }
      await store.saveTask(t1)
      await store.saveTask(t2)
      await store.saveTask(t3)

      const tasks = await store.listTasks({ status: ['pending'], types: ['llm'] })
      expect(tasks).toHaveLength(1)
      expect(tasks[0]!.id).toBe('task-1')
    })
  })

  // ─── Worker state ───────────────────────────────────────────────────────

  describe('worker state', () => {
    function makeWorker(id = 'worker-1'): Worker {
      return {
        id,
        status: 'idle',
        matchRule: { taskTypes: ['llm'] },
        capacity: 5,
        usedSlots: 0,
        weight: 1,
        connectionMode: 'pull',
        connectedAt: 1000,
        lastHeartbeatAt: 1000,
      }
    }

    // ─── saveWorker / getWorker ───────────────────────────────────────────

    it('should save and retrieve a worker', async () => {
      const worker = makeWorker()
      await store.saveWorker(worker)
      const retrieved = await store.getWorker('worker-1')
      expect(retrieved).toEqual(worker)
    })

    it('should return null for a missing worker', async () => {
      const result = await store.getWorker('nonexistent')
      expect(result).toBeNull()
    })

    it('should upsert worker on conflict (update status)', async () => {
      const worker = makeWorker()
      await store.saveWorker(worker)

      const updated: Worker = { ...worker, status: 'busy', usedSlots: 3, lastHeartbeatAt: 2000 }
      await store.saveWorker(updated)

      const retrieved = await store.getWorker('worker-1')
      expect(retrieved).toEqual(updated)
      expect(retrieved!.status).toBe('busy')
      expect(retrieved!.usedSlots).toBe(3)
    })

    it('should preserve optional metadata on worker round-trip', async () => {
      const worker: Worker = {
        ...makeWorker(),
        metadata: { region: 'us-east', gpu: true },
      }
      await store.saveWorker(worker)
      const retrieved = await store.getWorker('worker-1')
      expect(retrieved).toEqual(worker)
      expect(retrieved!.metadata).toEqual({ region: 'us-east', gpu: true })
    })

    it('should handle worker with no metadata', async () => {
      const worker = makeWorker()
      await store.saveWorker(worker)
      const retrieved = await store.getWorker('worker-1')
      expect(retrieved!.metadata).toBeUndefined()
    })

    it('should preserve matchRule with tags', async () => {
      const worker: Worker = {
        ...makeWorker(),
        matchRule: {
          taskTypes: ['llm', 'image'],
          tags: { all: ['gpu'], none: ['deprecated'] },
        },
      }
      await store.saveWorker(worker)
      const retrieved = await store.getWorker('worker-1')
      expect(retrieved!.matchRule).toEqual({
        taskTypes: ['llm', 'image'],
        tags: { all: ['gpu'], none: ['deprecated'] },
      })
    })

    it('should preserve websocket connectionMode', async () => {
      const worker: Worker = { ...makeWorker(), connectionMode: 'websocket' }
      await store.saveWorker(worker)
      const retrieved = await store.getWorker('worker-1')
      expect(retrieved!.connectionMode).toBe('websocket')
    })

    // ─── listWorkers ──────────────────────────────────────────────────────

    it('should list all workers with no filter', async () => {
      await store.saveWorker(makeWorker('worker-1'))
      await store.saveWorker(makeWorker('worker-2'))
      await store.saveWorker(makeWorker('worker-3'))

      const workers = await store.listWorkers()
      expect(workers).toHaveLength(3)
    })

    it('should return empty array when no workers exist', async () => {
      const workers = await store.listWorkers()
      expect(workers).toEqual([])
    })

    it('should filter workers by status', async () => {
      await store.saveWorker({ ...makeWorker('worker-1'), status: 'idle' })
      await store.saveWorker({ ...makeWorker('worker-2'), status: 'busy' })
      await store.saveWorker({ ...makeWorker('worker-3'), status: 'draining' })

      const idle = await store.listWorkers({ status: ['idle'] })
      expect(idle).toHaveLength(1)
      expect(idle[0]!.id).toBe('worker-1')

      const idleOrBusy = await store.listWorkers({ status: ['idle', 'busy'] })
      expect(idleOrBusy).toHaveLength(2)
    })

    it('should filter workers by connectionMode', async () => {
      await store.saveWorker({ ...makeWorker('worker-1'), connectionMode: 'pull' })
      await store.saveWorker({ ...makeWorker('worker-2'), connectionMode: 'websocket' })
      await store.saveWorker({ ...makeWorker('worker-3'), connectionMode: 'pull' })

      const pullWorkers = await store.listWorkers({ connectionMode: ['pull'] })
      expect(pullWorkers).toHaveLength(2)
      expect(pullWorkers.every((w) => w.connectionMode === 'pull')).toBe(true)
    })

    it('should combine status and connectionMode filters', async () => {
      await store.saveWorker({ ...makeWorker('worker-1'), status: 'idle', connectionMode: 'pull' })
      await store.saveWorker({ ...makeWorker('worker-2'), status: 'busy', connectionMode: 'websocket' })
      await store.saveWorker({ ...makeWorker('worker-3'), status: 'idle', connectionMode: 'websocket' })

      const result = await store.listWorkers({ status: ['idle'], connectionMode: ['websocket'] })
      expect(result).toHaveLength(1)
      expect(result[0]!.id).toBe('worker-3')
    })

    // ─── deleteWorker ─────────────────────────────────────────────────────

    it('should delete a worker', async () => {
      await store.saveWorker(makeWorker('worker-1'))
      await store.deleteWorker('worker-1')
      const result = await store.getWorker('worker-1')
      expect(result).toBeNull()
    })

    it('should not throw when deleting a nonexistent worker', async () => {
      await expect(store.deleteWorker('nonexistent')).resolves.toBeUndefined()
    })

    it('should only delete the specified worker', async () => {
      await store.saveWorker(makeWorker('worker-1'))
      await store.saveWorker(makeWorker('worker-2'))
      await store.deleteWorker('worker-1')

      expect(await store.getWorker('worker-1')).toBeNull()
      expect(await store.getWorker('worker-2')).not.toBeNull()
    })

    // ─── claimTask ────────────────────────────────────────────────────────

    describe('claimTask', () => {
      it('should successfully claim a pending task', async () => {
        await store.saveTask(makeTask('task-1'))
        await store.saveWorker(makeWorker('worker-1'))

        const result = await store.claimTask('task-1', 'worker-1', 1)
        expect(result).toBe(true)

        const task = await store.getTask('task-1')
        expect(task!.status).toBe('assigned')
        expect(task!.assignedWorker).toBe('worker-1')
        expect(task!.cost).toBe(1)

        const worker = await store.getWorker('worker-1')
        expect(worker!.usedSlots).toBe(1)
      })

      it('should fail to claim when worker does not exist', async () => {
        await store.saveTask(makeTask('task-1'))

        const result = await store.claimTask('task-1', 'nonexistent', 1)
        expect(result).toBe(false)

        // Task should remain unchanged
        const task = await store.getTask('task-1')
        expect(task!.status).toBe('pending')
      })

      it('should fail to claim when task does not exist', async () => {
        await store.saveWorker(makeWorker('worker-1'))

        const result = await store.claimTask('nonexistent', 'worker-1', 1)
        expect(result).toBe(false)
      })

      it('should fail to claim when worker has insufficient capacity', async () => {
        await store.saveTask(makeTask('task-1'))
        await store.saveWorker({ ...makeWorker('worker-1'), capacity: 3, usedSlots: 2 })

        const result = await store.claimTask('task-1', 'worker-1', 2)
        expect(result).toBe(false)

        // Task should remain unchanged
        const task = await store.getTask('task-1')
        expect(task!.status).toBe('pending')
      })

      it('should fail to claim a task that is already running', async () => {
        const task: Task = { ...makeTask('task-1'), status: 'running' }
        await store.saveTask(task)
        await store.saveWorker(makeWorker('worker-1'))

        const result = await store.claimTask('task-1', 'worker-1', 1)
        expect(result).toBe(false)
      })

      it('should fail to claim a completed task', async () => {
        const task: Task = { ...makeTask('task-1'), status: 'completed' }
        await store.saveTask(task)
        await store.saveWorker(makeWorker('worker-1'))

        const result = await store.claimTask('task-1', 'worker-1', 1)
        expect(result).toBe(false)
      })

      it('should allow claiming an already-assigned task (reassignment)', async () => {
        const task: Task = { ...makeTask('task-1'), status: 'assigned', assignedWorker: 'worker-old' }
        await store.saveTask(task)
        await store.saveWorker(makeWorker('worker-1'))

        const result = await store.claimTask('task-1', 'worker-1', 1)
        expect(result).toBe(true)

        const updated = await store.getTask('task-1')
        expect(updated!.assignedWorker).toBe('worker-1')
      })

      it('should claim exactly at capacity boundary', async () => {
        await store.saveTask(makeTask('task-1'))
        await store.saveWorker({ ...makeWorker('worker-1'), capacity: 5, usedSlots: 3 })

        const result = await store.claimTask('task-1', 'worker-1', 2)
        expect(result).toBe(true)

        const worker = await store.getWorker('worker-1')
        expect(worker!.usedSlots).toBe(5)
      })

      it('should fail when cost exceeds remaining capacity by 1', async () => {
        await store.saveTask(makeTask('task-1'))
        await store.saveWorker({ ...makeWorker('worker-1'), capacity: 5, usedSlots: 3 })

        const result = await store.claimTask('task-1', 'worker-1', 3)
        expect(result).toBe(false)
      })

      it('should handle multiple claims on different tasks', async () => {
        await store.saveTask(makeTask('task-1'))
        await store.saveTask(makeTask('task-2'))
        await store.saveWorker({ ...makeWorker('worker-1'), capacity: 10, usedSlots: 0 })

        const r1 = await store.claimTask('task-1', 'worker-1', 3)
        expect(r1).toBe(true)

        const r2 = await store.claimTask('task-2', 'worker-1', 4)
        expect(r2).toBe(true)

        const worker = await store.getWorker('worker-1')
        expect(worker!.usedSlots).toBe(7)
      })
    })

    // ─── Worker assignments ───────────────────────────────────────────────

    describe('worker assignments', () => {
      function makeAssignment(taskId: string, workerId: string): WorkerAssignment {
        return {
          taskId,
          workerId,
          cost: 1,
          assignedAt: 2000,
          status: 'assigned',
        }
      }

      // ─── addAssignment ────────────────────────────────────────────────

      it('should save and retrieve an assignment by task', async () => {
        const assignment = makeAssignment('task-1', 'worker-1')
        await store.addAssignment(assignment)

        const retrieved = await store.getTaskAssignment('task-1')
        expect(retrieved).toEqual(assignment)
      })

      it('should upsert assignment on conflict (same taskId)', async () => {
        const a1 = makeAssignment('task-1', 'worker-1')
        await store.addAssignment(a1)

        const a2: WorkerAssignment = { ...a1, workerId: 'worker-2', cost: 3, status: 'running' }
        await store.addAssignment(a2)

        const retrieved = await store.getTaskAssignment('task-1')
        expect(retrieved!.workerId).toBe('worker-2')
        expect(retrieved!.cost).toBe(3)
        expect(retrieved!.status).toBe('running')
      })

      it('should preserve all assignment statuses', async () => {
        for (const status of ['offered', 'assigned', 'running'] as const) {
          const assignment: WorkerAssignment = {
            taskId: `task-${status}`,
            workerId: 'worker-1',
            cost: 1,
            assignedAt: 2000,
            status,
          }
          await store.addAssignment(assignment)
          const retrieved = await store.getTaskAssignment(`task-${status}`)
          expect(retrieved!.status).toBe(status)
        }
      })

      // ─── getTaskAssignment ────────────────────────────────────────────

      it('should return null for a task with no assignment', async () => {
        const result = await store.getTaskAssignment('nonexistent')
        expect(result).toBeNull()
      })

      // ─── getWorkerAssignments ─────────────────────────────────────────

      it('should return all assignments for a worker', async () => {
        await store.addAssignment(makeAssignment('task-1', 'worker-1'))
        await store.addAssignment(makeAssignment('task-2', 'worker-1'))
        await store.addAssignment(makeAssignment('task-3', 'worker-2'))

        const assignments = await store.getWorkerAssignments('worker-1')
        expect(assignments).toHaveLength(2)
        expect(assignments.map((a) => a.taskId).sort()).toEqual(['task-1', 'task-2'])
      })

      it('should return empty array for a worker with no assignments', async () => {
        const assignments = await store.getWorkerAssignments('nonexistent')
        expect(assignments).toEqual([])
      })

      // ─── removeAssignment ─────────────────────────────────────────────

      it('should remove an assignment by taskId', async () => {
        await store.addAssignment(makeAssignment('task-1', 'worker-1'))
        await store.removeAssignment('task-1')

        const result = await store.getTaskAssignment('task-1')
        expect(result).toBeNull()
      })

      it('should not throw when removing a nonexistent assignment', async () => {
        await expect(store.removeAssignment('nonexistent')).resolves.toBeUndefined()
      })

      it('should only remove the specified assignment', async () => {
        await store.addAssignment(makeAssignment('task-1', 'worker-1'))
        await store.addAssignment(makeAssignment('task-2', 'worker-1'))
        await store.removeAssignment('task-1')

        expect(await store.getTaskAssignment('task-1')).toBeNull()
        expect(await store.getTaskAssignment('task-2')).not.toBeNull()
      })

      it('should reflect removal in getWorkerAssignments', async () => {
        await store.addAssignment(makeAssignment('task-1', 'worker-1'))
        await store.addAssignment(makeAssignment('task-2', 'worker-1'))

        let assignments = await store.getWorkerAssignments('worker-1')
        expect(assignments).toHaveLength(2)

        await store.removeAssignment('task-1')

        assignments = await store.getWorkerAssignments('worker-1')
        expect(assignments).toHaveLength(1)
        expect(assignments[0]!.taskId).toBe('task-2')
      })
    })
  })
})
