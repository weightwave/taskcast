import { describe, it, expect } from 'vitest'
import { MemoryBroadcastProvider, MemoryShortTermStore } from '../../src/memory-adapters.js'
import type { TaskEvent } from '../../src/types.js'

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
