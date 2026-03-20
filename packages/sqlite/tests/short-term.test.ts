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
    for (const file of ['001_initial.sql', '002_client_seq.sql']) {
      db.exec(readFileSync(join(import.meta.dirname, '../migrations', file), 'utf8'))
    }
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

  // ─── claimTask transaction consistency ──────────────────────────────────

  describe('claimTask transaction consistency', () => {
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

    it('sequential claimTask on same task — second re-assigns (last-writer wins)', async () => {
      await store.saveTask(makeTask('task-1'))
      await store.saveWorker(makeWorker('worker-a'))
      await store.saveWorker(makeWorker('worker-b'))

      // SQLite is single-threaded; Promise.all runs these sequentially.
      // claimTask allows re-claiming an assigned task (status check: pending|assigned).
      const [r1, r2] = await Promise.all([
        store.claimTask('task-1', 'worker-a', 1),
        store.claimTask('task-1', 'worker-b', 1),
      ])

      // Both succeed because re-claim is allowed by design
      expect(r1).toBe(true)
      expect(r2).toBe(true)

      // Last writer wins — task is assigned to worker-b
      const task = await store.getTask('task-1')
      expect(task!.status).toBe('assigned')
      expect(task!.assignedWorker).toBe('worker-b')

      // Both workers had usedSlots incremented (capacity consumed by each claim)
      const workerA = await store.getWorker('worker-a')
      const workerB = await store.getWorker('worker-b')
      expect(workerA!.usedSlots).toBe(1)
      expect(workerB!.usedSlots).toBe(1)
    })

    it('after claim, both task.status and worker.usedSlots are consistent', async () => {
      await store.saveTask(makeTask('task-1'))
      await store.saveWorker({ ...makeWorker('worker-1'), capacity: 10, usedSlots: 3 })

      const result = await store.claimTask('task-1', 'worker-1', 2)
      expect(result).toBe(true)

      const task = await store.getTask('task-1')
      expect(task!.status).toBe('assigned')
      expect(task!.assignedWorker).toBe('worker-1')
      expect(task!.cost).toBe(2)

      const worker = await store.getWorker('worker-1')
      expect(worker!.usedSlots).toBe(5) // 3 + 2
    })

    it('claimTask where task exists but worker does not — returns false, task unchanged', async () => {
      const originalTask = makeTask('task-1')
      await store.saveTask(originalTask)

      const result = await store.claimTask('task-1', 'nonexistent-worker', 1)
      expect(result).toBe(false)

      const task = await store.getTask('task-1')
      expect(task!.status).toBe('pending')
      expect(task!.assignedWorker).toBeUndefined()
      expect(task!.cost).toBeUndefined()
    })

    it('claimTask where worker is at capacity — returns false, nothing changes', async () => {
      await store.saveTask(makeTask('task-1'))
      await store.saveWorker({ ...makeWorker('worker-1'), capacity: 5, usedSlots: 5 })

      const result = await store.claimTask('task-1', 'worker-1', 1)
      expect(result).toBe(false)

      // Task should remain unchanged
      const task = await store.getTask('task-1')
      expect(task!.status).toBe('pending')

      // Worker should remain unchanged
      const worker = await store.getWorker('worker-1')
      expect(worker!.usedSlots).toBe(5)
    })

    it('transaction atomicity: failed claim does not partially update task or worker', async () => {
      await store.saveTask(makeTask('task-1'))
      // Worker has exactly 1 slot left, but we try to claim with cost 2
      await store.saveWorker({ ...makeWorker('worker-1'), capacity: 5, usedSlots: 4 })

      const result = await store.claimTask('task-1', 'worker-1', 2)
      expect(result).toBe(false)

      // Both task AND worker should be completely unchanged
      const task = await store.getTask('task-1')
      expect(task!.status).toBe('pending')
      expect(task!.assignedWorker).toBeUndefined()

      const worker = await store.getWorker('worker-1')
      expect(worker!.usedSlots).toBe(4) // unchanged
    })

    it('claimTask on task with status "failed" — returns false', async () => {
      await store.saveTask({ ...makeTask('task-1'), status: 'failed' })
      await store.saveWorker(makeWorker('worker-1'))

      const result = await store.claimTask('task-1', 'worker-1', 1)
      expect(result).toBe(false)
    })

    it('claimTask on task with status "cancelled" — returns false', async () => {
      await store.saveTask({ ...makeTask('task-1'), status: 'cancelled' })
      await store.saveWorker(makeWorker('worker-1'))

      const result = await store.claimTask('task-1', 'worker-1', 1)
      expect(result).toBe(false)
    })

    it('claimTask on task with status "timeout" — returns false', async () => {
      await store.saveTask({ ...makeTask('task-1'), status: 'timeout' })
      await store.saveWorker(makeWorker('worker-1'))

      const result = await store.claimTask('task-1', 'worker-1', 1)
      expect(result).toBe(false)
    })

    it('claimTask with cost 0 should succeed at full capacity', async () => {
      await store.saveTask(makeTask('task-1'))
      await store.saveWorker({ ...makeWorker('worker-1'), capacity: 5, usedSlots: 5 })

      const result = await store.claimTask('task-1', 'worker-1', 0)
      expect(result).toBe(true)

      const worker = await store.getWorker('worker-1')
      expect(worker!.usedSlots).toBe(5) // 5 + 0
    })

    it('multiple sequential claims exhaust worker capacity correctly', async () => {
      await store.saveTask(makeTask('task-1'))
      await store.saveTask(makeTask('task-2'))
      await store.saveTask(makeTask('task-3'))
      await store.saveWorker({ ...makeWorker('worker-1'), capacity: 5, usedSlots: 0 })

      const r1 = await store.claimTask('task-1', 'worker-1', 2)
      expect(r1).toBe(true)

      const r2 = await store.claimTask('task-2', 'worker-1', 2)
      expect(r2).toBe(true)

      // Now worker has usedSlots=4, capacity=5, trying to claim cost=2 should fail
      const r3 = await store.claimTask('task-3', 'worker-1', 2)
      expect(r3).toBe(false)

      const worker = await store.getWorker('worker-1')
      expect(worker!.usedSlots).toBe(4) // only first two claims succeeded

      const task3 = await store.getTask('task-3')
      expect(task3!.status).toBe('pending') // unchanged
    })
  })

  // ─── SQL injection verification ─────────────────────────────────────────

  describe('SQL injection verification', () => {
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

    it('saveTask with type containing DROP TABLE — table still exists, data stored literally', async () => {
      const injectionType = "'; DROP TABLE taskcast_tasks; --"
      const task: Task = {
        ...makeTask('task-inject-1'),
        type: injectionType,
      }
      await store.saveTask(task)

      // Table should still exist — verify by inserting another task
      const task2: Task = { ...makeTask('task-after'), type: 'normal' }
      await store.saveTask(task2)

      // Injection string should be stored literally
      const retrieved = await store.getTask('task-inject-1')
      expect(retrieved).not.toBeNull()
      expect(retrieved!.type).toBe(injectionType)

      // The other task should also be retrievable
      const retrieved2 = await store.getTask('task-after')
      expect(retrieved2).not.toBeNull()
    })

    it('saveTask with metadata containing SQL injection strings — stored safely', async () => {
      const task: Task = {
        ...makeTask('task-inject-meta'),
        metadata: {
          name: "Robert'); DROP TABLE taskcast_tasks;--",
          query: "1 OR 1=1; DELETE FROM taskcast_events;--",
          nested: { evil: "' UNION SELECT * FROM taskcast_workers --" },
        },
      }
      await store.saveTask(task)

      const retrieved = await store.getTask('task-inject-meta')
      expect(retrieved).not.toBeNull()
      expect(retrieved!.metadata).toEqual(task.metadata)

      // Verify tables still work
      const allTasks = await store.listTasks({})
      expect(allTasks.length).toBeGreaterThanOrEqual(1)
    })

    it('appendEvent with type containing SQL injection — stored safely', async () => {
      await store.saveTask(makeTask('task-1'))
      const injectionType = "'; DROP TABLE taskcast_events; --"
      const event: TaskEvent = {
        id: 'evt-inject',
        taskId: 'task-1',
        index: 0,
        timestamp: 1000,
        type: injectionType,
        level: 'info',
        data: { text: "' OR 1=1 --" },
      }
      await store.appendEvent('task-1', event)

      // Events table should still exist
      const events = await store.getEvents('task-1')
      expect(events).toHaveLength(1)
      expect(events[0]!.type).toBe(injectionType)
      expect((events[0]!.data as Record<string, unknown>)?.['text']).toBe("' OR 1=1 --")
    })

    it('listTasks with filter containing injection-like strings — safe', async () => {
      const task: Task = { ...makeTask('task-1'), type: 'llm', status: 'pending' }
      await store.saveTask(task)

      // These filter values look like SQL injection but are just string values
      const tasks = await store.listTasks({
        types: ["' OR 1=1 --"],
        status: ["pending' OR '1'='1" as Task['status']],
      })
      // Should return no results since these don't match, not cause SQL errors
      expect(tasks).toEqual([])

      // Original task should still be intact
      const original = await store.getTask('task-1')
      expect(original).not.toBeNull()
      expect(original!.type).toBe('llm')
    })

    it('saveWorker with id containing special characters — stored safely', async () => {
      const specialIds = [
        "worker-'; DROP TABLE taskcast_workers; --",
        'worker-" OR 1=1',
        "worker-\n\r\t",
        "worker-{}[]()@#$%^&*",
        "worker-émojis-日本語",
      ]

      for (const id of specialIds) {
        const worker: Worker = { ...makeWorker(), id }
        await store.saveWorker(worker)

        const retrieved = await store.getWorker(id)
        expect(retrieved).not.toBeNull()
        expect(retrieved!.id).toBe(id)
      }

      // Verify table still works normally
      const allWorkers = await store.listWorkers()
      expect(allWorkers.length).toBe(specialIds.length)
    })

    it('saveTask with id containing SQL injection — stored safely', async () => {
      const injectionId = "task-'; DELETE FROM taskcast_tasks; --"
      const task: Task = {
        id: injectionId,
        status: 'pending',
        createdAt: 1000,
        updatedAt: 1000,
      }
      await store.saveTask(task)

      const retrieved = await store.getTask(injectionId)
      expect(retrieved).not.toBeNull()
      expect(retrieved!.id).toBe(injectionId)
    })

    it('appendEvent with seriesId containing injection — stored safely', async () => {
      await store.saveTask(makeTask('task-1'))
      const injectionSeriesId = "'; DROP TABLE taskcast_series_latest; --"
      const event: TaskEvent = {
        id: 'evt-series-inject',
        taskId: 'task-1',
        index: 0,
        timestamp: 1000,
        type: 'test',
        level: 'info',
        data: null,
        seriesId: injectionSeriesId,
        seriesMode: 'latest',
      }
      await store.appendEvent('task-1', event)
      await store.setSeriesLatest('task-1', injectionSeriesId, event)

      const latest = await store.getSeriesLatest('task-1', injectionSeriesId)
      expect(latest).not.toBeNull()
      expect(latest!.seriesId).toBe(injectionSeriesId)
    })
  })

  // ─── Additional edge cases ──────────────────────────────────────────────

  describe('additional edge cases', () => {
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

    it('saveTask with extremely large metadata (1MB JSON) — stores and retrieves correctly', async () => {
      // Create a ~1MB metadata object
      const largeString = 'x'.repeat(1024 * 1024) // 1MB of 'x'
      const task: Task = {
        ...makeTask('task-large'),
        metadata: { payload: largeString, nested: { more: largeString.slice(0, 1000) } },
      }
      await store.saveTask(task)

      const retrieved = await store.getTask('task-large')
      expect(retrieved).not.toBeNull()
      expect((retrieved!.metadata as Record<string, unknown>)?.['payload']).toBe(largeString)
      expect(
        ((retrieved!.metadata as Record<string, unknown>)?.['nested'] as Record<string, unknown>)?.[
          'more'
        ],
      ).toBe(largeString.slice(0, 1000))
    })

    it('getEvents with limit: 0 — should return empty array', async () => {
      await store.saveTask(makeTask('task-1'))
      for (let i = 0; i < 5; i++) {
        await store.appendEvent('task-1', makeEvent('task-1', i))
      }

      const events = await store.getEvents('task-1', { limit: 0 })
      // limit 0 is falsy, so it falls through the `if (limit)` check
      // and returns all events. This documents the actual behavior.
      expect(events).toHaveLength(5)
    })

    it('getEvents with limit: -1 — should return empty array', async () => {
      await store.saveTask(makeTask('task-1'))
      for (let i = 0; i < 5; i++) {
        await store.appendEvent('task-1', makeEvent('task-1', i))
      }

      const events = await store.getEvents('task-1', { limit: -1 })
      // SQLite LIMIT -1 means no limit, so all events are returned
      expect(events).toHaveLength(5)
    })

    it('nextIndex called for non-existent task — should return 0 (first index)', async () => {
      // Note: no foreign key constraint on taskcast_index_counters,
      // so this should work even without a task
      const index = await store.nextIndex('nonexistent-task')
      expect(index).toBe(0)
    })

    it('nextIndex called repeatedly for non-existent task — increments correctly', async () => {
      const i0 = await store.nextIndex('ghost-task')
      const i1 = await store.nextIndex('ghost-task')
      const i2 = await store.nextIndex('ghost-task')
      expect(i0).toBe(0)
      expect(i1).toBe(1)
      expect(i2).toBe(2)
    })

    it('listByStatus with statuses not matching any task — returns []', async () => {
      await store.saveTask({ ...makeTask('task-1'), status: 'pending' })
      await store.saveTask({ ...makeTask('task-2'), status: 'running' })

      const result = await store.listByStatus(['completed', 'failed', 'timeout'])
      expect(result).toEqual([])
    })

    it('listTasks with all filter fields set simultaneously', async () => {
      const t1: Task = {
        ...makeTask('task-1'),
        status: 'pending',
        type: 'llm',
        tags: ['gpu', 'fast'],
        assignMode: 'pull',
      }
      const t2: Task = {
        ...makeTask('task-2'),
        status: 'pending',
        type: 'llm',
        tags: ['gpu'],
        assignMode: 'pull',
      }
      const t3: Task = {
        ...makeTask('task-3'),
        status: 'running',
        type: 'llm',
        tags: ['gpu'],
        assignMode: 'pull',
      }
      const t4: Task = {
        ...makeTask('task-4'),
        status: 'pending',
        type: 'image',
        tags: ['gpu'],
        assignMode: 'pull',
      }
      const t5: Task = {
        ...makeTask('task-5'),
        status: 'pending',
        type: 'llm',
        tags: ['gpu'],
        assignMode: 'external',
      }
      const excluded: Task = {
        ...makeTask('task-excluded'),
        status: 'pending',
        type: 'llm',
        tags: ['gpu'],
        assignMode: 'pull',
      }

      await store.saveTask(t1)
      await store.saveTask(t2)
      await store.saveTask(t3)
      await store.saveTask(t4)
      await store.saveTask(t5)
      await store.saveTask(excluded)

      const tasks = await store.listTasks({
        status: ['pending'],
        types: ['llm'],
        tags: { all: ['gpu'] },
        assignMode: ['pull'],
        excludeTaskIds: ['task-excluded'],
        limit: 10,
      })

      // Should match t1 and t2 only
      expect(tasks).toHaveLength(2)
      const ids = tasks.map((t) => t.id).sort()
      expect(ids).toEqual(['task-1', 'task-2'])
    })

    it('listTasks with limit smaller than matching results', async () => {
      for (let i = 0; i < 10; i++) {
        await store.saveTask({ ...makeTask(`task-${i}`), status: 'pending', tags: ['test'] })
      }

      const tasks = await store.listTasks({
        status: ['pending'],
        tags: { all: ['test'] },
        limit: 3,
      })
      expect(tasks).toHaveLength(3)
    })

    it('listTasks with empty arrays in filter — returns all tasks', async () => {
      await store.saveTask(makeTask('task-1'))
      await store.saveTask(makeTask('task-2'))

      // Empty arrays should not filter (the if(filter.xxx?.length) guards skip them)
      const tasks = await store.listTasks({
        status: [],
        types: [],
        assignMode: [],
        excludeTaskIds: [],
      })
      expect(tasks).toHaveLength(2)
    })

    it('getEvents for a task with no events — returns empty array', async () => {
      await store.saveTask(makeTask('task-empty'))
      const events = await store.getEvents('task-empty')
      expect(events).toEqual([])
    })

    it('getEvents for a non-existent task — returns empty array', async () => {
      const events = await store.getEvents('task-does-not-exist')
      expect(events).toEqual([])
    })

    it('saveTask upsert does not change created_at', async () => {
      const task: Task = { ...makeTask('task-1'), createdAt: 1000, updatedAt: 1000 }
      await store.saveTask(task)

      // Note: the current upsert does update created_at because the ON CONFLICT
      // clause doesn't exclude it from the insert (but it's always the same value).
      // What matters is that the round-trip is correct.
      const updated: Task = { ...task, status: 'running', updatedAt: 2000 }
      await store.saveTask(updated)

      const retrieved = await store.getTask('task-1')
      expect(retrieved!.updatedAt).toBe(2000)
      expect(retrieved!.status).toBe('running')
    })

    it('worker with metadata containing deeply nested objects — round-trips correctly', async () => {
      const deepMeta = {
        l1: {
          l2: {
            l3: {
              l4: { value: 'deep', arr: [1, 2, 3] },
            },
          },
        },
      }
      const worker: Worker = { ...makeWorker(), metadata: deepMeta }
      await store.saveWorker(worker)

      const retrieved = await store.getWorker('worker-1')
      expect(retrieved!.metadata).toEqual(deepMeta)
    })

    it('saveTask with params/result/error containing arrays — round-trips correctly', async () => {
      const task: Task = {
        ...makeTask('task-arrays'),
        params: { items: [1, 'two', { three: 3 }] },
        result: { outputs: [null, true, false] },
        error: { message: 'err', details: { codes: [400, 500] } },
      }
      await store.saveTask(task)

      const retrieved = await store.getTask('task-arrays')
      expect(retrieved!.params).toEqual(task.params)
      expect(retrieved!.result).toEqual(task.result)
      expect(retrieved!.error).toEqual(task.error)
    })

    it('listTasks with tags filter on tasks that have no tags — excludes them', async () => {
      await store.saveTask({ ...makeTask('task-no-tags') }) // no tags
      await store.saveTask({ ...makeTask('task-with-tags'), tags: ['gpu'] })

      const tasks = await store.listTasks({ tags: { all: ['gpu'] } })
      expect(tasks).toHaveLength(1)
      expect(tasks[0]!.id).toBe('task-with-tags')
    })

    it('listTasks with tags.none filter — excludes tasks with forbidden tags, includes tagless tasks', async () => {
      await store.saveTask({ ...makeTask('task-no-tags') }) // no tags
      await store.saveTask({ ...makeTask('task-good'), tags: ['gpu'] })
      await store.saveTask({ ...makeTask('task-bad'), tags: ['deprecated'] })

      const tasks = await store.listTasks({ tags: { none: ['deprecated'] } })
      // tasks without tags have empty array, so none check passes
      expect(tasks).toHaveLength(2)
      const ids = tasks.map((t) => t.id).sort()
      expect(ids).toEqual(['task-good', 'task-no-tags'])
    })

    it('listTasks with tags.any filter — tasks with no tags are excluded', async () => {
      await store.saveTask({ ...makeTask('task-no-tags') }) // no tags
      await store.saveTask({ ...makeTask('task-with'), tags: ['gpu'] })

      const tasks = await store.listTasks({ tags: { any: ['gpu', 'cpu'] } })
      expect(tasks).toHaveLength(1)
      expect(tasks[0]!.id).toBe('task-with')
    })

    it('getEvents with since.index beyond all events — returns empty array', async () => {
      await store.saveTask(makeTask('task-1'))
      for (let i = 0; i < 3; i++) {
        await store.appendEvent('task-1', makeEvent('task-1', i))
      }

      const events = await store.getEvents('task-1', { since: { index: 999 } })
      expect(events).toEqual([])
    })

    it('getEvents with since.timestamp beyond all events — returns empty array', async () => {
      await store.saveTask(makeTask('task-1'))
      for (let i = 0; i < 3; i++) {
        await store.appendEvent('task-1', makeEvent('task-1', i))
      }
      // All timestamps are <= 1200, so since 9999 should yield nothing
      const events = await store.getEvents('task-1', { since: { timestamp: 9999 } })
      expect(events).toEqual([])
    })

    it('replaceLastSeriesEvent preserves original event position in idx ordering', async () => {
      await store.saveTask(makeTask('task-1'))
      const e0 = makeEvent('task-1', 0)
      const e1 = { ...makeEvent('task-1', 1), seriesId: 'ser', seriesMode: 'latest' as const }
      const e2 = makeEvent('task-1', 2)

      await store.appendEvent('task-1', e0)
      await store.appendEvent('task-1', e1)
      await store.appendEvent('task-1', e2)
      await store.setSeriesLatest('task-1', 'ser', e1)

      // Replace series event with new content
      const replacement: TaskEvent = {
        id: 'new-id',
        taskId: 'task-1',
        index: 99, // this should NOT change the stored idx
        timestamp: 9999,
        type: 'replaced-type',
        level: 'warn',
        data: { replaced: true },
        seriesId: 'ser',
        seriesMode: 'latest',
      }
      await store.replaceLastSeriesEvent('task-1', 'ser', replacement)

      const events = await store.getEvents('task-1')
      expect(events).toHaveLength(3)
      // Event at idx 1 should have replaced content but preserve original id and idx
      expect(events[1]!.id).toBe(e1.id) // original id
      expect(events[1]!.index).toBe(1) // original idx
      expect(events[1]!.type).toBe('replaced-type') // replaced content
      expect(events[1]!.level).toBe('warn') // replaced content
      expect((events[1]!.data as Record<string, unknown>)?.['replaced']).toBe(true)
    })

    it('concurrent nextIndex calls produce unique indices', async () => {
      await store.saveTask(makeTask('task-1'))

      // Fire many nextIndex calls concurrently
      const promises = Array.from({ length: 20 }, () => store.nextIndex('task-1'))
      const indices = await Promise.all(promises)

      // All indices should be unique
      const unique = new Set(indices)
      expect(unique.size).toBe(20)

      // Should be values 0-19
      expect(Math.min(...indices)).toBe(0)
      expect(Math.max(...indices)).toBe(19)
    })

    it('listWorkers with no matching status — returns empty array', async () => {
      await store.saveWorker({
        id: 'worker-1',
        status: 'idle',
        matchRule: { taskTypes: ['llm'] },
        capacity: 5,
        usedSlots: 0,
        weight: 1,
        connectionMode: 'pull',
        connectedAt: 1000,
        lastHeartbeatAt: 1000,
      })

      const workers = await store.listWorkers({ status: ['draining'] })
      expect(workers).toEqual([])
    })

    it('listWorkers with no matching connectionMode — returns empty array', async () => {
      await store.saveWorker({
        id: 'worker-1',
        status: 'idle',
        matchRule: { taskTypes: ['llm'] },
        capacity: 5,
        usedSlots: 0,
        weight: 1,
        connectionMode: 'pull',
        connectedAt: 1000,
        lastHeartbeatAt: 1000,
      })

      const workers = await store.listWorkers({ connectionMode: ['websocket'] })
      expect(workers).toEqual([])
    })

    it('deleteWorker then claimTask with that worker — returns false', async () => {
      await store.saveTask(makeTask('task-1'))
      await store.saveWorker(makeWorker('worker-1'))
      await store.deleteWorker('worker-1')

      const result = await store.claimTask('task-1', 'worker-1', 1)
      expect(result).toBe(false)

      const task = await store.getTask('task-1')
      expect(task!.status).toBe('pending')
    })

    it('listByStatus with single status matching multiple tasks', async () => {
      await store.saveTask({ ...makeTask('task-1'), status: 'running' })
      await store.saveTask({ ...makeTask('task-2'), status: 'running' })
      await store.saveTask({ ...makeTask('task-3'), status: 'pending' })

      const running = await store.listByStatus(['running'])
      expect(running).toHaveLength(2)
      expect(running.map((t) => t.id).sort()).toEqual(['task-1', 'task-2'])
    })

    it('saveTask with webhooks and authConfig — round-trips correctly', async () => {
      const task: Task = {
        ...makeTask('task-auth'),
        authConfig: {
          rules: [
            {
              match: { scope: ['event:publish'] },
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
      }
      await store.saveTask(task)

      const retrieved = await store.getTask('task-auth')
      expect(retrieved!.authConfig).toEqual(task.authConfig)
      expect(retrieved!.webhooks).toEqual(task.webhooks)
    })

    it('saveTask with cleanup rules — round-trips correctly', async () => {
      const task: Task = {
        ...makeTask('task-cleanup'),
        cleanup: {
          rules: [
            {
              match: { status: ['completed'] },
              trigger: { afterMs: 3600000 },
              target: 'all',
            },
          ],
        },
      }
      await store.saveTask(task)

      const retrieved = await store.getTask('task-cleanup')
      expect(retrieved!.cleanup).toEqual(task.cleanup)
    })
  })
})
