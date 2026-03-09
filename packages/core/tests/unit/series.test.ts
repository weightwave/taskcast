import { describe, it, expect, vi } from 'vitest'
import { processSeries } from '../../src/series.js'
import type { TaskEvent, ShortTermStore } from '../../src/types.js'

const makeEvent = (overrides: Partial<TaskEvent> = {}): TaskEvent => ({
  id: 'evt-1',
  taskId: 'task-1',
  index: 0,
  timestamp: 1000,
  type: 'llm.delta',
  level: 'info',
  data: { delta: 'hello' },
  ...overrides,
})

/**
 * Build a mock ShortTermStore.
 * For accumulate mode, accumulateSeries does the work (not getSeriesLatest + setSeriesLatest).
 * The `latestEvent` param controls what getSeriesLatest returns (for non-accumulate uses).
 * The `accumulateFn` param lets tests control what accumulateSeries returns.
 */
const makeStore = (opts?: {
  latestEvent?: TaskEvent
  accumulateFn?: (taskId: string, seriesId: string, event: TaskEvent, field: string) => Promise<TaskEvent>
}): ShortTermStore => ({
  saveTask: vi.fn(),
  getTask: vi.fn(),
  nextIndex: vi.fn(),
  appendEvent: vi.fn(),
  getEvents: vi.fn(),
  setTTL: vi.fn(),
  getSeriesLatest: vi.fn().mockResolvedValue(opts?.latestEvent ?? null),
  setSeriesLatest: vi.fn(),
  accumulateSeries: opts?.accumulateFn
    ? vi.fn().mockImplementation(opts.accumulateFn)
    : vi.fn().mockImplementation(async (_taskId: string, _seriesId: string, event: TaskEvent, _field: string) => event),
  replaceLastSeriesEvent: vi.fn(),
})

describe('processSeries - keep-all', () => {
  it('returns event unchanged in SeriesResult, no store mutation', async () => {
    const store = makeStore()
    const event = makeEvent({ seriesId: 's1', seriesMode: 'keep-all' })
    const result = await processSeries(event, store)
    expect(result.event).toEqual(event)
    expect(result.accumulatedEvent).toBeUndefined()
    expect(store.setSeriesLatest).not.toHaveBeenCalled()
    expect(store.replaceLastSeriesEvent).not.toHaveBeenCalled()
    expect(store.accumulateSeries).not.toHaveBeenCalled()
  })
})

describe('processSeries - accumulate', () => {
  it('calls accumulateSeries and returns both delta and accumulated events', async () => {
    const accumulatedEvent = makeEvent({ data: { delta: 'hello world' }, seriesId: 's1', seriesMode: 'accumulate' })
    const store = makeStore({
      accumulateFn: async () => accumulatedEvent,
    })
    const event = makeEvent({ data: { delta: 'world' }, seriesId: 's1', seriesMode: 'accumulate' })
    const result = await processSeries(event, store)
    // event is the original delta
    expect(result.event).toEqual(event)
    // accumulatedEvent is the accumulated version from store
    expect(result.accumulatedEvent).toEqual(accumulatedEvent)
    expect((result.accumulatedEvent!.data as { delta: string }).delta).toBe('hello world')
    expect(store.accumulateSeries).toHaveBeenCalledWith('task-1', 's1', event, 'delta')
  })

  it('returns delta event unchanged when no previous (accumulateSeries returns same event)', async () => {
    const event = makeEvent({ data: { delta: 'start' }, seriesId: 's1', seriesMode: 'accumulate' })
    const store = makeStore({
      accumulateFn: async (_tid, _sid, evt) => evt,
    })
    const result = await processSeries(event, store)
    expect(result.event).toEqual(event)
    expect(result.accumulatedEvent).toEqual(event)
    expect(store.accumulateSeries).toHaveBeenCalledWith('task-1', 's1', event, 'delta')
  })

  it('does NOT call getSeriesLatest for accumulate mode (accumulateSeries handles it)', async () => {
    const store = makeStore()
    const event = makeEvent({ data: { delta: 'test' }, seriesId: 's1', seriesMode: 'accumulate' })
    await processSeries(event, store)
    expect(store.getSeriesLatest).not.toHaveBeenCalled()
    expect(store.accumulateSeries).toHaveBeenCalled()
  })

  it('does NOT call setSeriesLatest for accumulate mode (accumulateSeries handles it)', async () => {
    const store = makeStore()
    const event = makeEvent({ data: { count: 2 }, seriesId: 's1', seriesMode: 'accumulate' })
    await processSeries(event, store)
    expect(store.setSeriesLatest).not.toHaveBeenCalled()
  })

  it('supports custom seriesAccField', async () => {
    const accumulatedEvent = makeEvent({ data: { content: 'hello world' }, seriesId: 's1', seriesMode: 'accumulate', seriesAccField: 'content' })
    const store = makeStore({
      accumulateFn: async () => accumulatedEvent,
    })
    const event = makeEvent({ data: { content: 'world' }, seriesId: 's1', seriesMode: 'accumulate', seriesAccField: 'content' })
    const result = await processSeries(event, store)
    expect(store.accumulateSeries).toHaveBeenCalledWith('task-1', 's1', event, 'content')
    expect((result.accumulatedEvent!.data as { content: string }).content).toBe('hello world')
  })

  it('supports legacy text field via seriesAccField', async () => {
    const accumulatedEvent = makeEvent({ data: { text: 'hello world' }, seriesId: 's1', seriesMode: 'accumulate', seriesAccField: 'text' })
    const store = makeStore({
      accumulateFn: async () => accumulatedEvent,
    })
    const event = makeEvent({ data: { text: 'world' }, seriesId: 's1', seriesMode: 'accumulate', seriesAccField: 'text' })
    const result = await processSeries(event, store)
    expect(store.accumulateSeries).toHaveBeenCalledWith('task-1', 's1', event, 'text')
    expect((result.accumulatedEvent!.data as { text: string }).text).toBe('hello world')
  })
})

describe('processSeries - latest', () => {
  it('calls replaceLastSeriesEvent with new event', async () => {
    const store = makeStore({ latestEvent: makeEvent({ seriesId: 's1', seriesMode: 'latest', data: { delta: 'old' } }) })
    const event = makeEvent({ seriesId: 's1', seriesMode: 'latest', data: { delta: 'new' } })
    const result = await processSeries(event, store)
    expect(result.event).toEqual(event)
    expect(result.accumulatedEvent).toBeUndefined()
    expect(store.replaceLastSeriesEvent).toHaveBeenCalledWith('task-1', 's1', event)
  })

  it('works with no previous event', async () => {
    const store = makeStore()
    const event = makeEvent({ seriesId: 's1', seriesMode: 'latest', data: { delta: 'first' } })
    await processSeries(event, store)
    expect(store.replaceLastSeriesEvent).toHaveBeenCalledWith('task-1', 's1', event)
  })
})

describe('processSeries - accumulate with non-object data', () => {
  it('handles data: null for new event (no previous)', async () => {
    const event = makeEvent({ data: null, seriesId: 's1', seriesMode: 'accumulate' })
    const store = makeStore({
      accumulateFn: async (_tid, _sid, evt) => evt,
    })
    const result = await processSeries(event, store)
    expect(result.event.data).toBeNull()
    expect(store.accumulateSeries).toHaveBeenCalled()
  })

  it('handles data: "just a string" (not an object)', async () => {
    const event = makeEvent({ data: 'just a string', seriesId: 's1', seriesMode: 'accumulate' })
    const store = makeStore({
      accumulateFn: async (_tid, _sid, evt) => evt,
    })
    const result = await processSeries(event, store)
    expect(result.event.data).toBe('just a string')
    expect(store.accumulateSeries).toHaveBeenCalled()
  })

  it('handles data: "string" with previous string data', async () => {
    const event = makeEvent({ data: 'new string', seriesId: 's1', seriesMode: 'accumulate' })
    const store = makeStore({
      accumulateFn: async (_tid, _sid, evt) => evt,
    })
    const result = await processSeries(event, store)
    expect(result.event.data).toBe('new string')
    expect(store.accumulateSeries).toHaveBeenCalled()
  })

  it('handles data: [1,2,3] (array)', async () => {
    const event = makeEvent({ data: [1, 2, 3], seriesId: 's1', seriesMode: 'accumulate' })
    const store = makeStore({
      accumulateFn: async (_tid, _sid, evt) => evt,
    })
    const result = await processSeries(event, store)
    expect(result.event.data).toEqual([1, 2, 3])
    expect(store.accumulateSeries).toHaveBeenCalled()
  })

  it('handles data: [1,2,3] with previous array data', async () => {
    const event = makeEvent({ data: [1, 2, 3], seriesId: 's1', seriesMode: 'accumulate' })
    const store = makeStore({
      accumulateFn: async (_tid, _sid, evt) => evt,
    })
    const result = await processSeries(event, store)
    expect(result.event.data).toEqual([1, 2, 3])
    expect(store.accumulateSeries).toHaveBeenCalled()
  })

  it('handles data: number (primitive)', async () => {
    const event = makeEvent({ data: 42, seriesId: 's1', seriesMode: 'accumulate' })
    const store = makeStore({
      accumulateFn: async (_tid, _sid, evt) => evt,
    })
    const result = await processSeries(event, store)
    expect(result.event.data).toBe(42)
    expect(store.accumulateSeries).toHaveBeenCalled()
  })

  it('handles data: boolean (primitive)', async () => {
    const event = makeEvent({ data: false, seriesId: 's1', seriesMode: 'accumulate' })
    const store = makeStore({
      accumulateFn: async (_tid, _sid, evt) => evt,
    })
    const result = await processSeries(event, store)
    expect(result.event.data).toBe(false)
    expect(store.accumulateSeries).toHaveBeenCalled()
  })
})

describe('processSeries - no seriesId', () => {
  it('returns event unchanged when no seriesId', async () => {
    const store = makeStore()
    const event = makeEvent()
    const result = await processSeries(event, store)
    expect(result.event).toEqual(event)
    expect(result.accumulatedEvent).toBeUndefined()
    expect(store.getSeriesLatest).not.toHaveBeenCalled()
    expect(store.accumulateSeries).not.toHaveBeenCalled()
  })
})
