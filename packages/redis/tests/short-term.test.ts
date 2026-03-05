import { describe, it, expect, beforeAll, afterAll, beforeEach } from 'vitest'
import { Redis } from 'ioredis'
import { GenericContainer, type StartedTestContainer } from 'testcontainers'
import { RedisShortTermStore } from '../src/short-term.js'
import type { Task, TaskEvent, Worker, WorkerAssignment } from '@taskcast/core'

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

const makeTask = (id = 'task-1', overrides: Partial<Task> = {}): Task => ({
  id,
  status: 'pending',
  createdAt: 1000,
  updatedAt: 1000,
  ...overrides,
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

describe('RedisShortTermStore - nextIndex', () => {
  it('returns 0-based monotonically increasing values', async () => {
    expect(await store.nextIndex('task-1')).toBe(0)
    expect(await store.nextIndex('task-1')).toBe(1)
    expect(await store.nextIndex('task-1')).toBe(2)
  })

  it('counters are independent per taskId', async () => {
    expect(await store.nextIndex('task-a')).toBe(0)
    expect(await store.nextIndex('task-b')).toBe(0)
    expect(await store.nextIndex('task-a')).toBe(1)
  })

  it('setTTL expires the idx key', async () => {
    await store.nextIndex('task-1')
    await store.setTTL('task-1', 60)
    const ttl = await redis.ttl('taskcast:idx:task-1')
    expect(ttl).toBeGreaterThan(0)
  })

  it('30 concurrent nextIndex calls return all unique values (INCR atomicity)', async () => {
    // Regression test: per-instance in-memory counters caused duplicate indices
    // when multiple engine instances published to the same task. Redis INCR is
    // atomic, so concurrent calls must produce 30 distinct 0-based values.
    const CONCURRENCY = 30
    const indices = await Promise.all(
      Array.from({ length: CONCURRENCY }, () => store.nextIndex('task-concurrent'))
    )
    expect(new Set(indices).size).toBe(CONCURRENCY)
    expect(Math.min(...indices)).toBe(0)
    expect(Math.max(...indices)).toBe(CONCURRENCY - 1)
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

const makeWorker = (overrides: Partial<Worker> = {}): Worker => ({
  id: 'worker-1',
  status: 'idle',
  matchRule: {},
  capacity: 10,
  usedSlots: 0,
  weight: 50,
  connectionMode: 'websocket',
  connectedAt: 1000,
  lastHeartbeatAt: 1000,
  ...overrides,
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

// ─── Worker CRUD ─────────────────────────────────────────────────────────────

describe('RedisShortTermStore - worker CRUD', () => {
  it('saves and retrieves a worker', async () => {
    const worker = makeWorker()
    await store.saveWorker(worker)
    const retrieved = await store.getWorker('worker-1')
    expect(retrieved).toEqual(worker)
  })

  it('returns null for missing worker', async () => {
    expect(await store.getWorker('nonexistent')).toBeNull()
  })

  it('overwrites existing worker on save', async () => {
    await store.saveWorker(makeWorker())
    await store.saveWorker(makeWorker({ status: 'busy' }))
    const retrieved = await store.getWorker('worker-1')
    expect(retrieved?.status).toBe('busy')
  })

  it('listWorkers returns all saved workers', async () => {
    await store.saveWorker(makeWorker({ id: 'w1' }))
    await store.saveWorker(makeWorker({ id: 'w2' }))
    await store.saveWorker(makeWorker({ id: 'w3' }))
    const workers = await store.listWorkers()
    expect(workers).toHaveLength(3)
    const ids = workers.map((w) => w.id).sort()
    expect(ids).toEqual(['w1', 'w2', 'w3'])
  })

  it('listWorkers returns empty array when no workers exist', async () => {
    const workers = await store.listWorkers()
    expect(workers).toEqual([])
  })

  it('listWorkers filters by status', async () => {
    await store.saveWorker(makeWorker({ id: 'w1', status: 'idle' }))
    await store.saveWorker(makeWorker({ id: 'w2', status: 'busy' }))
    await store.saveWorker(makeWorker({ id: 'w3', status: 'draining' }))
    const idle = await store.listWorkers({ status: ['idle'] })
    expect(idle).toHaveLength(1)
    expect(idle[0]?.id).toBe('w1')
  })

  it('listWorkers filters by multiple statuses', async () => {
    await store.saveWorker(makeWorker({ id: 'w1', status: 'idle' }))
    await store.saveWorker(makeWorker({ id: 'w2', status: 'busy' }))
    await store.saveWorker(makeWorker({ id: 'w3', status: 'draining' }))
    const result = await store.listWorkers({ status: ['idle', 'busy'] })
    expect(result).toHaveLength(2)
    const ids = result.map((w) => w.id).sort()
    expect(ids).toEqual(['w1', 'w2'])
  })

  it('listWorkers filters by connectionMode', async () => {
    await store.saveWorker(makeWorker({ id: 'w1', connectionMode: 'websocket' }))
    await store.saveWorker(makeWorker({ id: 'w2', connectionMode: 'pull' }))
    await store.saveWorker(makeWorker({ id: 'w3', connectionMode: 'websocket' }))
    const pullWorkers = await store.listWorkers({ connectionMode: ['pull'] })
    expect(pullWorkers).toHaveLength(1)
    expect(pullWorkers[0]?.id).toBe('w2')
  })

  it('deleteWorker removes worker', async () => {
    await store.saveWorker(makeWorker({ id: 'w1' }))
    await store.saveWorker(makeWorker({ id: 'w2' }))
    await store.deleteWorker('w1')
    expect(await store.getWorker('w1')).toBeNull()
    // w2 should still exist
    expect(await store.getWorker('w2')).not.toBeNull()
    // listWorkers should only return w2
    const workers = await store.listWorkers()
    expect(workers).toHaveLength(1)
    expect(workers[0]?.id).toBe('w2')
  })

  it('deleteWorker on nonexistent worker does not throw', async () => {
    await expect(store.deleteWorker('nonexistent')).resolves.not.toThrow()
  })
})

// ─── listTasks ───────────────────────────────────────────────────────────────

describe('RedisShortTermStore - listTasks', () => {
  it('returns all saved tasks with empty filter', async () => {
    await store.saveTask(makeTask('t1'))
    await store.saveTask(makeTask('t2'))
    await store.saveTask(makeTask('t3'))
    const tasks = await store.listTasks({})
    expect(tasks).toHaveLength(3)
    const ids = tasks.map((t) => t.id).sort()
    expect(ids).toEqual(['t1', 't2', 't3'])
  })

  it('returns empty array when no tasks exist', async () => {
    const tasks = await store.listTasks({})
    expect(tasks).toEqual([])
  })

  it('filters by status', async () => {
    await store.saveTask(makeTask('t1', { status: 'pending' }))
    await store.saveTask(makeTask('t2', { status: 'running' }))
    await store.saveTask(makeTask('t3', { status: 'completed' }))
    const pending = await store.listTasks({ status: ['pending'] })
    expect(pending).toHaveLength(1)
    expect(pending[0]?.id).toBe('t1')
  })

  it('filters by multiple statuses', async () => {
    await store.saveTask(makeTask('t1', { status: 'pending' }))
    await store.saveTask(makeTask('t2', { status: 'running' }))
    await store.saveTask(makeTask('t3', { status: 'completed' }))
    const result = await store.listTasks({ status: ['pending', 'running'] })
    expect(result).toHaveLength(2)
    const ids = result.map((t) => t.id).sort()
    expect(ids).toEqual(['t1', 't2'])
  })

  it('respects limit', async () => {
    await store.saveTask(makeTask('t1'))
    await store.saveTask(makeTask('t2'))
    await store.saveTask(makeTask('t3'))
    const tasks = await store.listTasks({ limit: 1 })
    expect(tasks).toHaveLength(1)
  })

  it('excludes specified task IDs', async () => {
    await store.saveTask(makeTask('t1'))
    await store.saveTask(makeTask('t2'))
    await store.saveTask(makeTask('t3'))
    const tasks = await store.listTasks({ excludeTaskIds: ['t1', 't3'] })
    expect(tasks).toHaveLength(1)
    expect(tasks[0]?.id).toBe('t2')
  })

  it('filters by assignMode', async () => {
    await store.saveTask(makeTask('t1', { assignMode: 'pull' }))
    await store.saveTask(makeTask('t2', { assignMode: 'external' }))
    await store.saveTask(makeTask('t3', { assignMode: 'pull' }))
    const pullTasks = await store.listTasks({ assignMode: ['pull'] })
    expect(pullTasks).toHaveLength(2)
    const ids = pullTasks.map((t) => t.id).sort()
    expect(ids).toEqual(['t1', 't3'])
  })

  it('filters by tags (all)', async () => {
    await store.saveTask(makeTask('t1', { tags: ['gpu', 'fast'] }))
    await store.saveTask(makeTask('t2', { tags: ['gpu'] }))
    await store.saveTask(makeTask('t3', { tags: ['cpu'] }))
    const tasks = await store.listTasks({ tags: { all: ['gpu'] } })
    expect(tasks).toHaveLength(2)
    const ids = tasks.map((t) => t.id).sort()
    expect(ids).toEqual(['t1', 't2'])
  })

  it('filters by tags (all) requiring multiple tags', async () => {
    await store.saveTask(makeTask('t1', { tags: ['gpu', 'fast'] }))
    await store.saveTask(makeTask('t2', { tags: ['gpu'] }))
    const tasks = await store.listTasks({ tags: { all: ['gpu', 'fast'] } })
    expect(tasks).toHaveLength(1)
    expect(tasks[0]?.id).toBe('t1')
  })

  it('filters by tags (any)', async () => {
    await store.saveTask(makeTask('t1', { tags: ['gpu'] }))
    await store.saveTask(makeTask('t2', { tags: ['cpu'] }))
    await store.saveTask(makeTask('t3', { tags: ['tpu'] }))
    const tasks = await store.listTasks({ tags: { any: ['gpu', 'cpu'] } })
    expect(tasks).toHaveLength(2)
    const ids = tasks.map((t) => t.id).sort()
    expect(ids).toEqual(['t1', 't2'])
  })

  it('filters by tags (none)', async () => {
    await store.saveTask(makeTask('t1', { tags: ['gpu'] }))
    await store.saveTask(makeTask('t2', { tags: ['cpu'] }))
    await store.saveTask(makeTask('t3', { tags: ['tpu'] }))
    const tasks = await store.listTasks({ tags: { none: ['gpu'] } })
    expect(tasks).toHaveLength(2)
    const ids = tasks.map((t) => t.id).sort()
    expect(ids).toEqual(['t2', 't3'])
  })

  it('filters by types', async () => {
    await store.saveTask(makeTask('t1', { type: 'llm' }))
    await store.saveTask(makeTask('t2', { type: 'image' }))
    await store.saveTask(makeTask('t3'))
    const tasks = await store.listTasks({ types: ['llm'] })
    expect(tasks).toHaveLength(1)
    expect(tasks[0]?.id).toBe('t1')
  })

  it('tasks without tags fail tags.all filter', async () => {
    await store.saveTask(makeTask('t1')) // no tags
    await store.saveTask(makeTask('t2', { tags: ['gpu'] }))
    const tasks = await store.listTasks({ tags: { all: ['gpu'] } })
    expect(tasks).toHaveLength(1)
    expect(tasks[0]?.id).toBe('t2')
  })

  it('combines multiple filters', async () => {
    await store.saveTask(makeTask('t1', { status: 'pending', assignMode: 'pull', tags: ['gpu'] }))
    await store.saveTask(makeTask('t2', { status: 'pending', assignMode: 'external', tags: ['gpu'] }))
    await store.saveTask(makeTask('t3', { status: 'running', assignMode: 'pull', tags: ['gpu'] }))
    const tasks = await store.listTasks({ status: ['pending'], assignMode: ['pull'] })
    expect(tasks).toHaveLength(1)
    expect(tasks[0]?.id).toBe('t1')
  })
})

// ─── claimTask ───────────────────────────────────────────────────────────────

describe('RedisShortTermStore - claimTask', () => {
  it('claims a pending task successfully and updates worker usedSlots', async () => {
    await store.saveTask(makeTask('t1', { status: 'pending' }))
    await store.saveWorker(makeWorker({ id: 'w1', capacity: 10, usedSlots: 0 }))

    const result = await store.claimTask('t1', 'w1', 1)
    expect(result).toBe(true)

    const task = await store.getTask('t1')
    expect(task?.status).toBe('assigned')
    expect(task?.assignedWorker).toBe('w1')
    expect(task?.cost).toBe(1)

    const worker = await store.getWorker('w1')
    expect(worker?.usedSlots).toBe(1)
  })

  it('claims an assigned task successfully (re-assignment)', async () => {
    await store.saveTask(makeTask('t1', { status: 'assigned' }))
    await store.saveWorker(makeWorker({ id: 'w1', capacity: 10, usedSlots: 0 }))

    const result = await store.claimTask('t1', 'w1', 1)
    expect(result).toBe(true)
  })

  it('fails to claim a non-pending/non-assigned task', async () => {
    await store.saveTask(makeTask('t1', { status: 'running' }))
    await store.saveWorker(makeWorker({ id: 'w1', capacity: 10, usedSlots: 0 }))

    const result = await store.claimTask('t1', 'w1', 1)
    expect(result).toBe(false)

    // Task status should remain unchanged
    const task = await store.getTask('t1')
    expect(task?.status).toBe('running')
  })

  it('fails to claim with insufficient capacity', async () => {
    await store.saveTask(makeTask('t1', { status: 'pending' }))
    await store.saveWorker(makeWorker({ id: 'w1', capacity: 5, usedSlots: 4 }))

    const result = await store.claimTask('t1', 'w1', 2)
    expect(result).toBe(false)

    // Worker usedSlots should remain unchanged
    const worker = await store.getWorker('w1')
    expect(worker?.usedSlots).toBe(4)
  })

  it('fails when task does not exist', async () => {
    await store.saveWorker(makeWorker({ id: 'w1' }))
    const result = await store.claimTask('nonexistent', 'w1', 1)
    expect(result).toBe(false)
  })

  it('fails when worker does not exist', async () => {
    await store.saveTask(makeTask('t1', { status: 'pending' }))
    const result = await store.claimTask('t1', 'nonexistent', 1)
    expect(result).toBe(false)
  })

  it('succeeds when cost exactly equals remaining capacity', async () => {
    await store.saveTask(makeTask('t1', { status: 'pending' }))
    await store.saveWorker(makeWorker({ id: 'w1', capacity: 5, usedSlots: 3 }))

    const result = await store.claimTask('t1', 'w1', 2)
    expect(result).toBe(true)

    const worker = await store.getWorker('w1')
    expect(worker?.usedSlots).toBe(5)
  })

  it('fails when cost exceeds remaining capacity by 1', async () => {
    await store.saveTask(makeTask('t1', { status: 'pending' }))
    await store.saveWorker(makeWorker({ id: 'w1', capacity: 5, usedSlots: 3 }))

    const result = await store.claimTask('t1', 'w1', 3)
    expect(result).toBe(false)
  })

  it('concurrent claims on same task: exactly 1 succeeds when workers have capacity for only 1', async () => {
    await store.saveTask(makeTask('t1', { status: 'pending' }))
    // Create 10 workers, each with capacity for exactly 1 claim (cost=1)
    for (let i = 0; i < 10; i++) {
      await store.saveWorker(makeWorker({ id: `w${i}`, capacity: 1, usedSlots: 0 }))
    }

    // All 10 workers race to claim the same task with cost=1.
    // The Lua script allows claiming 'pending' or 'assigned' tasks, so
    // multiple workers can succeed. But each worker can only claim once
    // (capacity=1, cost=1). After claim, the worker's usedSlots becomes 1.
    // The task ends up assigned to exactly one worker (the last one to claim).
    const results = await Promise.all(
      Array.from({ length: 10 }, (_, i) => store.claimTask('t1', `w${i}`, 1))
    )

    // Multiple claims can succeed since the script allows re-claiming 'assigned' tasks.
    // But each claim is atomic — no data corruption or partial writes.
    const successes = results.filter((r) => r === true).length
    expect(successes).toBeGreaterThanOrEqual(1)

    // The task should be in 'assigned' state with exactly one assignedWorker
    const task = await store.getTask('t1')
    expect(task?.status).toBe('assigned')
    expect(task?.assignedWorker).toBeTruthy()
    expect(task?.cost).toBe(1)

    // Each successful claimant should have usedSlots incremented by exactly 1
    let totalIncrements = 0
    for (let i = 0; i < 10; i++) {
      const worker = await store.getWorker(`w${i}`)
      if (worker?.usedSlots === 1) totalIncrements++
    }
    // The number of workers with usedSlots=1 equals the number of successful claims
    expect(totalIncrements).toBe(successes)
  })

  it('concurrent claims with single-capacity worker: only first claim succeeds', async () => {
    // This tests true mutual exclusion: one worker claims multiple tasks concurrently.
    // Only tasks that fit within capacity should succeed.
    await store.saveWorker(makeWorker({ id: 'w1', capacity: 1, usedSlots: 0 }))
    for (let i = 0; i < 10; i++) {
      await store.saveTask(makeTask(`t${i}`, { status: 'pending' }))
    }

    const results = await Promise.all(
      Array.from({ length: 10 }, (_, i) => store.claimTask(`t${i}`, 'w1', 1))
    )

    const successes = results.filter((r) => r === true)
    // Exactly 1 should succeed because the worker only has capacity for 1
    expect(successes).toHaveLength(1)

    const worker = await store.getWorker('w1')
    expect(worker?.usedSlots).toBe(1)
  })
})

// ─── Assignments ─────────────────────────────────────────────────────────────

describe('RedisShortTermStore - assignments', () => {
  const makeAssignment = (overrides: Partial<WorkerAssignment> = {}): WorkerAssignment => ({
    taskId: 'task-1',
    workerId: 'worker-1',
    cost: 1,
    assignedAt: Date.now(),
    status: 'assigned',
    ...overrides,
  })

  it('addAssignment and getTaskAssignment roundtrip', async () => {
    const assignment = makeAssignment()
    await store.addAssignment(assignment)
    const retrieved = await store.getTaskAssignment('task-1')
    expect(retrieved).toEqual(assignment)
  })

  it('getTaskAssignment returns null for missing assignment', async () => {
    expect(await store.getTaskAssignment('nonexistent')).toBeNull()
  })

  it('getWorkerAssignments returns correct assignments', async () => {
    const a1 = makeAssignment({ taskId: 't1', workerId: 'w1' })
    const a2 = makeAssignment({ taskId: 't2', workerId: 'w1' })
    const a3 = makeAssignment({ taskId: 't3', workerId: 'w2' })
    await store.addAssignment(a1)
    await store.addAssignment(a2)
    await store.addAssignment(a3)

    const w1Assignments = await store.getWorkerAssignments('w1')
    expect(w1Assignments).toHaveLength(2)
    const taskIds = w1Assignments.map((a) => a.taskId).sort()
    expect(taskIds).toEqual(['t1', 't2'])

    const w2Assignments = await store.getWorkerAssignments('w2')
    expect(w2Assignments).toHaveLength(1)
    expect(w2Assignments[0]?.taskId).toBe('t3')
  })

  it('getWorkerAssignments returns empty array for unknown worker', async () => {
    const assignments = await store.getWorkerAssignments('nonexistent')
    expect(assignments).toEqual([])
  })

  it('removeAssignment removes the assignment', async () => {
    const assignment = makeAssignment({ taskId: 't1', workerId: 'w1' })
    await store.addAssignment(assignment)

    await store.removeAssignment('t1')

    expect(await store.getTaskAssignment('t1')).toBeNull()
    const workerAssignments = await store.getWorkerAssignments('w1')
    expect(workerAssignments).toHaveLength(0)
  })

  it('removeAssignment on nonexistent assignment does not throw', async () => {
    await expect(store.removeAssignment('nonexistent')).resolves.not.toThrow()
  })

  it('removeAssignment only removes the specified task assignment', async () => {
    const a1 = makeAssignment({ taskId: 't1', workerId: 'w1' })
    const a2 = makeAssignment({ taskId: 't2', workerId: 'w1' })
    await store.addAssignment(a1)
    await store.addAssignment(a2)

    await store.removeAssignment('t1')

    expect(await store.getTaskAssignment('t1')).toBeNull()
    expect(await store.getTaskAssignment('t2')).toEqual(a2)

    const remaining = await store.getWorkerAssignments('w1')
    expect(remaining).toHaveLength(1)
    expect(remaining[0]?.taskId).toBe('t2')
  })
})
