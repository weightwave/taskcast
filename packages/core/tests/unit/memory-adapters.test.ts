import { describe, it, expect } from 'vitest'
import { MemoryBroadcastProvider, MemoryShortTermStore } from '../../src/memory-adapters.js'
import type { Task, TaskEvent, Worker, WorkerAssignment } from '../../src/types.js'

const makeEvent = (index = 0): TaskEvent => ({
  id: `evt-${index}`,
  taskId: 'task-1',
  index,
  timestamp: 1000 + index,
  type: 'llm.delta',
  level: 'info',
  data: null,
})

describe('MemoryBroadcastProvider', () => {
  it('delivers published events to subscribers', async () => {
    const provider = new MemoryBroadcastProvider()
    const received: TaskEvent[] = []
    provider.subscribe('task-1', (e) => received.push(e))
    await provider.publish('task-1', makeEvent())
    expect(received).toHaveLength(1)
    expect(received[0]).toEqual(makeEvent())
  })

  it('unsubscribe stops delivery', async () => {
    const provider = new MemoryBroadcastProvider()
    const received: TaskEvent[] = []
    const unsub = provider.subscribe('task-1', (e) => received.push(e))
    await provider.publish('task-1', makeEvent(0))
    unsub()
    await provider.publish('task-1', makeEvent(1))
    expect(received).toHaveLength(1)
  })

  it('delivers to multiple subscribers on same channel', async () => {
    const provider = new MemoryBroadcastProvider()
    const r1: TaskEvent[] = []
    const r2: TaskEvent[] = []
    provider.subscribe('task-1', (e) => r1.push(e))
    provider.subscribe('task-1', (e) => r2.push(e))
    await provider.publish('task-1', makeEvent())
    expect(r1).toHaveLength(1)
    expect(r2).toHaveLength(1)
  })

  it('does not deliver to subscribers on different channel', async () => {
    const provider = new MemoryBroadcastProvider()
    const received: TaskEvent[] = []
    provider.subscribe('task-1', (e) => received.push(e))
    await provider.publish('task-2', makeEvent())
    expect(received).toHaveLength(0)
  })
})

describe('MemoryShortTermStore', () => {
  it('saves and retrieves a task', async () => {
    const store = new MemoryShortTermStore()
    const task = { id: 'task-1', status: 'pending' as const, createdAt: 1000, updatedAt: 1000 }
    await store.saveTask(task)
    const retrieved = await store.getTask('task-1')
    expect(retrieved).toEqual(task)
  })

  it('returns null for missing task', async () => {
    const store = new MemoryShortTermStore()
    expect(await store.getTask('missing')).toBeNull()
  })

  it('nextIndex returns monotonically increasing values starting from 0', async () => {
    const store = new MemoryShortTermStore()
    expect(await store.nextIndex('task-1')).toBe(0)
    expect(await store.nextIndex('task-1')).toBe(1)
    expect(await store.nextIndex('task-1')).toBe(2)
  })

  it('nextIndex counters are independent per taskId', async () => {
    const store = new MemoryShortTermStore()
    expect(await store.nextIndex('task-a')).toBe(0)
    expect(await store.nextIndex('task-b')).toBe(0)
    expect(await store.nextIndex('task-a')).toBe(1)
  })

  it('appends events in order', async () => {
    const store = new MemoryShortTermStore()
    await store.appendEvent('task-1', makeEvent(0))
    await store.appendEvent('task-1', makeEvent(1))
    const events = await store.getEvents('task-1')
    expect(events).toHaveLength(2)
    expect(events[0]?.index).toBe(0)
    expect(events[1]?.index).toBe(1)
  })

  it('filters events by since.index (returns events with index > since.index)', async () => {
    const store = new MemoryShortTermStore()
    for (let i = 0; i < 5; i++) await store.appendEvent('task-1', makeEvent(i))
    const events = await store.getEvents('task-1', { since: { index: 2 } })
    expect(events.map((e) => e.index)).toEqual([3, 4])
  })

  it('filters events by since.timestamp', async () => {
    const store = new MemoryShortTermStore()
    for (let i = 0; i < 5; i++) await store.appendEvent('task-1', makeEvent(i))
    // timestamps: 1000, 1001, 1002, 1003, 1004
    const events = await store.getEvents('task-1', { since: { timestamp: 1002 } })
    expect(events.map((e) => e.index)).toEqual([3, 4])
  })

  it('getEvents with since.id NOT found returns full list', async () => {
    const store = new MemoryShortTermStore()
    for (let i = 0; i < 3; i++) await store.appendEvent('task-1', makeEvent(i))
    // idx < 0 branch: id not found → return all events
    const events = await store.getEvents('task-1', { since: { id: 'nonexistent-id' } })
    expect(events.map((e) => e.index)).toEqual([0, 1, 2])
  })

  it('getEvents with since.id FOUND returns events after that id', async () => {
    const store = new MemoryShortTermStore()
    for (let i = 0; i < 5; i++) await store.appendEvent('task-1', makeEvent(i))
    // idx >= 0 branch: 'evt-2' found → return events after it
    const events = await store.getEvents('task-1', { since: { id: 'evt-2' } })
    expect(events.map((e) => e.index)).toEqual([3, 4])
  })

  it('getEvents with limit slices result', async () => {
    const store = new MemoryShortTermStore()
    for (let i = 0; i < 5; i++) await store.appendEvent('task-1', makeEvent(i))
    const events = await store.getEvents('task-1', { limit: 2 })
    expect(events).toHaveLength(2)
    expect(events[0]?.index).toBe(0)
    expect(events[1]?.index).toBe(1)
  })

  it('getEvents returns empty list when no events for taskId', async () => {
    const store = new MemoryShortTermStore()
    // taskId has no events at all (tests the ?? [] branch)
    const events = await store.getEvents('task-with-no-events')
    expect(events).toEqual([])
  })

  it('setTTL is a no-op and does not throw', async () => {
    const store = new MemoryShortTermStore()
    await expect(store.setTTL('task-1', 60)).resolves.toBeUndefined()
  })

  it('replaceLastSeriesEvent appends event when no previous series event exists', async () => {
    const store = new MemoryShortTermStore()
    const event = makeEvent(0)
    // prev is null: should append to events list
    await store.replaceLastSeriesEvent('task-1', 's1', event)
    const events = await store.getEvents('task-1')
    expect(events).toHaveLength(1)
    expect(events[0]?.id).toBe(event.id)
  })

  it('getSeriesLatest returns null when no series', async () => {
    const store = new MemoryShortTermStore()
    expect(await store.getSeriesLatest('task-1', 's1')).toBeNull()
  })

  it('setSeriesLatest and getSeriesLatest roundtrip', async () => {
    const store = new MemoryShortTermStore()
    const event = makeEvent()
    await store.setSeriesLatest('task-1', 's1', event)
    expect(await store.getSeriesLatest('task-1', 's1')).toEqual(event)
  })

  it('replaceLastSeriesEvent replaces the event in the list', async () => {
    const store = new MemoryShortTermStore()
    const event1 = makeEvent(0)
    await store.appendEvent('task-1', event1)
    await store.setSeriesLatest('task-1', 's1', event1)

    const event2 = { ...makeEvent(0), id: 'evt-replaced', data: { text: 'replaced' } }
    await store.replaceLastSeriesEvent('task-1', 's1', event2)

    const events = await store.getEvents('task-1')
    expect(events).toHaveLength(1)
    expect(events[0]?.id).toBe('evt-replaced')
  })
})

// ─── Factory helpers ──────────────────────────────────────────────────────────

const makeTask = (overrides: Partial<Task> = {}): Task => ({
  id: 'task-1',
  status: 'pending',
  createdAt: 1000,
  updatedAt: 1000,
  ...overrides,
})

const makeWorker = (overrides: Partial<Worker> = {}): Worker => ({
  id: 'worker-1',
  status: 'idle',
  matchRule: {},
  capacity: 10,
  usedSlots: 0,
  weight: 1,
  connectionMode: 'pull',
  connectedAt: 1000,
  lastHeartbeatAt: 1000,
  ...overrides,
})

const makeAssignment = (overrides: Partial<WorkerAssignment> = {}): WorkerAssignment => ({
  taskId: 'task-1',
  workerId: 'worker-1',
  cost: 1,
  assignedAt: 1000,
  status: 'assigned',
  ...overrides,
})

// ─── listTasks ────────────────────────────────────────────────────────────────

describe('MemoryShortTermStore.listTasks', () => {
  it('returns all tasks when filter is empty', async () => {
    const store = new MemoryShortTermStore()
    await store.saveTask(makeTask({ id: 't1' }))
    await store.saveTask(makeTask({ id: 't2' }))
    const result = await store.listTasks({})
    expect(result).toHaveLength(2)
  })

  it('filters by status', async () => {
    const store = new MemoryShortTermStore()
    await store.saveTask(makeTask({ id: 't1', status: 'pending' }))
    await store.saveTask(makeTask({ id: 't2', status: 'running' }))
    await store.saveTask(makeTask({ id: 't3', status: 'completed' }))
    const result = await store.listTasks({ status: ['pending', 'running'] })
    expect(result.map((t) => t.id).sort()).toEqual(['t1', 't2'])
  })

  it('filters by types', async () => {
    const store = new MemoryShortTermStore()
    await store.saveTask(makeTask({ id: 't1', type: 'llm' }))
    await store.saveTask(makeTask({ id: 't2', type: 'agent' }))
    await store.saveTask(makeTask({ id: 't3' })) // no type
    const result = await store.listTasks({ types: ['llm'] })
    expect(result).toHaveLength(1)
    expect(result[0]!.id).toBe('t1')
  })

  it('filters by tags.all (all tags must be present)', async () => {
    const store = new MemoryShortTermStore()
    await store.saveTask(makeTask({ id: 't1', tags: ['gpu', 'fast'] }))
    await store.saveTask(makeTask({ id: 't2', tags: ['gpu'] }))
    const result = await store.listTasks({ tags: { all: ['gpu', 'fast'] } })
    expect(result).toHaveLength(1)
    expect(result[0]!.id).toBe('t1')
  })

  it('filters by tags.any (at least one tag must be present)', async () => {
    const store = new MemoryShortTermStore()
    await store.saveTask(makeTask({ id: 't1', tags: ['gpu'] }))
    await store.saveTask(makeTask({ id: 't2', tags: ['cpu'] }))
    await store.saveTask(makeTask({ id: 't3', tags: ['disk'] }))
    const result = await store.listTasks({ tags: { any: ['gpu', 'cpu'] } })
    expect(result.map((t) => t.id).sort()).toEqual(['t1', 't2'])
  })

  it('filters by tags.none (none of these tags may be present)', async () => {
    const store = new MemoryShortTermStore()
    await store.saveTask(makeTask({ id: 't1', tags: ['gpu'] }))
    await store.saveTask(makeTask({ id: 't2', tags: ['cpu'] }))
    await store.saveTask(makeTask({ id: 't3' })) // no tags
    const result = await store.listTasks({ tags: { none: ['gpu'] } })
    expect(result.map((t) => t.id).sort()).toEqual(['t2', 't3'])
  })

  it('filters by assignMode', async () => {
    const store = new MemoryShortTermStore()
    await store.saveTask(makeTask({ id: 't1', assignMode: 'pull' }))
    await store.saveTask(makeTask({ id: 't2', assignMode: 'external' }))
    await store.saveTask(makeTask({ id: 't3' })) // no assignMode
    const result = await store.listTasks({ assignMode: ['pull'] })
    expect(result).toHaveLength(1)
    expect(result[0]!.id).toBe('t1')
  })

  it('filters by excludeTaskIds', async () => {
    const store = new MemoryShortTermStore()
    await store.saveTask(makeTask({ id: 't1' }))
    await store.saveTask(makeTask({ id: 't2' }))
    await store.saveTask(makeTask({ id: 't3' }))
    const result = await store.listTasks({ excludeTaskIds: ['t1', 't3'] })
    expect(result).toHaveLength(1)
    expect(result[0]!.id).toBe('t2')
  })

  it('applies limit', async () => {
    const store = new MemoryShortTermStore()
    for (let i = 0; i < 5; i++) await store.saveTask(makeTask({ id: `t${i}` }))
    const result = await store.listTasks({ limit: 2 })
    expect(result).toHaveLength(2)
  })

  it('returns empty array when no tasks match', async () => {
    const store = new MemoryShortTermStore()
    await store.saveTask(makeTask({ id: 't1', status: 'pending' }))
    const result = await store.listTasks({ status: ['completed'] })
    expect(result).toEqual([])
  })
})

// ─── Worker state ─────────────────────────────────────────────────────────────

describe('MemoryShortTermStore.workers', () => {
  it('saveWorker and getWorker roundtrip', async () => {
    const store = new MemoryShortTermStore()
    const worker = makeWorker()
    await store.saveWorker(worker)
    const retrieved = await store.getWorker('worker-1')
    expect(retrieved).toEqual(worker)
  })

  it('getWorker returns null for missing worker', async () => {
    const store = new MemoryShortTermStore()
    expect(await store.getWorker('missing')).toBeNull()
  })

  it('saveWorker overwrites existing worker', async () => {
    const store = new MemoryShortTermStore()
    await store.saveWorker(makeWorker({ usedSlots: 0 }))
    await store.saveWorker(makeWorker({ usedSlots: 5 }))
    const retrieved = await store.getWorker('worker-1')
    expect(retrieved!.usedSlots).toBe(5)
  })

  it('listWorkers returns all workers when no filter', async () => {
    const store = new MemoryShortTermStore()
    await store.saveWorker(makeWorker({ id: 'w1' }))
    await store.saveWorker(makeWorker({ id: 'w2' }))
    const result = await store.listWorkers()
    expect(result).toHaveLength(2)
  })

  it('listWorkers filters by status', async () => {
    const store = new MemoryShortTermStore()
    await store.saveWorker(makeWorker({ id: 'w1', status: 'idle' }))
    await store.saveWorker(makeWorker({ id: 'w2', status: 'busy' }))
    await store.saveWorker(makeWorker({ id: 'w3', status: 'offline' }))
    const result = await store.listWorkers({ status: ['idle', 'busy'] })
    expect(result.map((w) => w.id).sort()).toEqual(['w1', 'w2'])
  })

  it('listWorkers filters by connectionMode', async () => {
    const store = new MemoryShortTermStore()
    await store.saveWorker(makeWorker({ id: 'w1', connectionMode: 'pull' }))
    await store.saveWorker(makeWorker({ id: 'w2', connectionMode: 'websocket' }))
    const result = await store.listWorkers({ connectionMode: ['websocket'] })
    expect(result).toHaveLength(1)
    expect(result[0]!.id).toBe('w2')
  })

  it('deleteWorker removes the worker', async () => {
    const store = new MemoryShortTermStore()
    await store.saveWorker(makeWorker({ id: 'w1' }))
    await store.deleteWorker('w1')
    expect(await store.getWorker('w1')).toBeNull()
  })

  it('deleteWorker is a no-op for missing worker', async () => {
    const store = new MemoryShortTermStore()
    // Should not throw
    await store.deleteWorker('nonexistent')
    expect(await store.listWorkers()).toHaveLength(0)
  })
})

// ─── claimTask ────────────────────────────────────────────────────────────────

describe('MemoryShortTermStore.claimTask', () => {
  it('successfully claims a pending task', async () => {
    const store = new MemoryShortTermStore()
    await store.saveWorker(makeWorker({ id: 'w1', capacity: 10 }))
    await store.saveTask(makeTask({ id: 't1', status: 'pending' }))
    const result = await store.claimTask('t1', 'w1', 2)
    expect(result).toBe(true)
    const task = await store.getTask('t1')
    expect(task!.status).toBe('assigned')
    expect(task!.assignedWorker).toBe('w1')
    expect(task!.cost).toBe(2)
    const worker = await store.getWorker('w1')
    expect(worker!.usedSlots).toBe(2)
  })

  it('successfully claims an assigned task (re-assignment)', async () => {
    const store = new MemoryShortTermStore()
    await store.saveWorker(makeWorker({ id: 'w-new', capacity: 10 }))
    await store.saveTask(makeTask({ id: 't1', status: 'assigned', assignedWorker: 'w-old' }))
    const result = await store.claimTask('t1', 'w-new', 1)
    expect(result).toBe(true)
    const task = await store.getTask('t1')
    expect(task!.assignedWorker).toBe('w-new')
  })

  it('rejects claim when worker capacity exceeded', async () => {
    const store = new MemoryShortTermStore()
    await store.saveWorker(makeWorker({ id: 'w1', capacity: 1, usedSlots: 0 }))
    await store.saveTask(makeTask({ id: 't1', status: 'pending' }))
    const result = await store.claimTask('t1', 'w1', 2)
    expect(result).toBe(false)
  })

  it('rejects claim for unregistered worker', async () => {
    const store = new MemoryShortTermStore()
    await store.saveTask(makeTask({ id: 't1', status: 'pending' }))
    const result = await store.claimTask('t1', 'unknown-worker', 1)
    expect(result).toBe(false)
  })

  it('rejects claim for running task when worker exists', async () => {
    const store = new MemoryShortTermStore()
    await store.saveWorker(makeWorker({ id: 'w1', capacity: 10 }))
    await store.saveTask(makeTask({ id: 't1', status: 'running' }))
    const result = await store.claimTask('t1', 'w1', 1)
    expect(result).toBe(false)
  })

  it('rejects claim for completed task when worker exists', async () => {
    const store = new MemoryShortTermStore()
    await store.saveWorker(makeWorker({ id: 'w1', capacity: 10 }))
    await store.saveTask(makeTask({ id: 't1', status: 'completed' }))
    const result = await store.claimTask('t1', 'w1', 1)
    expect(result).toBe(false)
  })

  it('rejects claim for failed task when worker exists', async () => {
    const store = new MemoryShortTermStore()
    await store.saveWorker(makeWorker({ id: 'w1', capacity: 10 }))
    await store.saveTask(makeTask({ id: 't1', status: 'failed' }))
    const result = await store.claimTask('t1', 'w1', 1)
    expect(result).toBe(false)
  })

  it('rejects claim for missing task', async () => {
    const store = new MemoryShortTermStore()
    const result = await store.claimTask('nonexistent', 'w1', 1)
    expect(result).toBe(false)
  })

  it('updates updatedAt timestamp on successful claim', async () => {
    const store = new MemoryShortTermStore()
    await store.saveWorker(makeWorker({ id: 'w1', capacity: 10 }))
    await store.saveTask(makeTask({ id: 't1', updatedAt: 1000 }))
    await store.claimTask('t1', 'w1', 1)
    const task = await store.getTask('t1')
    expect(task!.updatedAt).toBeGreaterThan(1000)
  })
})

// ─── Worker assignments ───────────────────────────────────────────────────────

describe('MemoryShortTermStore.assignments', () => {
  it('addAssignment and getTaskAssignment roundtrip', async () => {
    const store = new MemoryShortTermStore()
    const assignment = makeAssignment()
    await store.addAssignment(assignment)
    const retrieved = await store.getTaskAssignment('task-1')
    expect(retrieved).toEqual(assignment)
  })

  it('getTaskAssignment returns null for missing task', async () => {
    const store = new MemoryShortTermStore()
    expect(await store.getTaskAssignment('nonexistent')).toBeNull()
  })

  it('getWorkerAssignments returns all assignments for a worker', async () => {
    const store = new MemoryShortTermStore()
    await store.addAssignment(makeAssignment({ taskId: 't1', workerId: 'w1' }))
    await store.addAssignment(makeAssignment({ taskId: 't2', workerId: 'w1' }))
    await store.addAssignment(makeAssignment({ taskId: 't3', workerId: 'w2' }))
    const result = await store.getWorkerAssignments('w1')
    expect(result).toHaveLength(2)
    expect(result.map((a) => a.taskId).sort()).toEqual(['t1', 't2'])
  })

  it('getWorkerAssignments returns empty array for worker with no assignments', async () => {
    const store = new MemoryShortTermStore()
    const result = await store.getWorkerAssignments('w-no-assignments')
    expect(result).toEqual([])
  })

  it('removeAssignment deletes the assignment', async () => {
    const store = new MemoryShortTermStore()
    await store.addAssignment(makeAssignment({ taskId: 't1', workerId: 'w1' }))
    await store.removeAssignment('t1')
    expect(await store.getTaskAssignment('t1')).toBeNull()
  })

  it('removeAssignment is a no-op for missing assignment', async () => {
    const store = new MemoryShortTermStore()
    // Should not throw
    await store.removeAssignment('nonexistent')
  })

  it('addAssignment overwrites existing assignment for same taskId', async () => {
    const store = new MemoryShortTermStore()
    await store.addAssignment(makeAssignment({ taskId: 't1', workerId: 'w1' }))
    await store.addAssignment(makeAssignment({ taskId: 't1', workerId: 'w2' }))
    const retrieved = await store.getTaskAssignment('t1')
    expect(retrieved!.workerId).toBe('w2')
  })
})
