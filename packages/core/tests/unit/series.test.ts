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
  data: { text: 'hello' },
  ...overrides,
})

const makeStore = (latestEvent?: TaskEvent): ShortTermStore => ({
  saveTask: vi.fn(),
  getTask: vi.fn(),
  appendEvent: vi.fn(),
  getEvents: vi.fn(),
  setTTL: vi.fn(),
  getSeriesLatest: vi.fn().mockResolvedValue(latestEvent ?? null),
  setSeriesLatest: vi.fn(),
  replaceLastSeriesEvent: vi.fn(),
})

describe('processSeries - keep-all', () => {
  it('returns event unchanged, no store mutation', async () => {
    const store = makeStore()
    const event = makeEvent({ seriesId: 's1', seriesMode: 'keep-all' })
    const result = await processSeries(event, store)
    expect(result).toEqual(event)
    expect(store.setSeriesLatest).not.toHaveBeenCalled()
    expect(store.replaceLastSeriesEvent).not.toHaveBeenCalled()
  })
})

describe('processSeries - accumulate', () => {
  it('concatenates text when previous exists', async () => {
    const prev = makeEvent({ data: { text: 'hello ' }, seriesId: 's1', seriesMode: 'accumulate' })
    const store = makeStore(prev)
    const event = makeEvent({ data: { text: 'world' }, seriesId: 's1', seriesMode: 'accumulate' })
    const result = await processSeries(event, store)
    expect((result.data as { text: string }).text).toBe('hello world')
    expect(store.setSeriesLatest).toHaveBeenCalledWith('task-1', 's1', result)
  })

  it('returns event unchanged when no previous', async () => {
    const store = makeStore()
    const event = makeEvent({ data: { text: 'start' }, seriesId: 's1', seriesMode: 'accumulate' })
    const result = await processSeries(event, store)
    expect((result.data as { text: string }).text).toBe('start')
    expect(store.setSeriesLatest).toHaveBeenCalledWith('task-1', 's1', result)
  })

  it('handles non-text data gracefully (returns event unchanged)', async () => {
    const prev = makeEvent({ data: { count: 1 }, seriesId: 's1', seriesMode: 'accumulate' })
    const store = makeStore(prev)
    const event = makeEvent({ data: { count: 2 }, seriesId: 's1', seriesMode: 'accumulate' })
    const result = await processSeries(event, store)
    expect(result.data).toEqual({ count: 2 })
    expect(store.setSeriesLatest).toHaveBeenCalled()
  })

  it('treats null prev.data as empty object (no text concat)', async () => {
    const prev = makeEvent({ data: null, seriesId: 's1', seriesMode: 'accumulate' })
    const store = makeStore(prev)
    const event = makeEvent({ data: { text: 'world' }, seriesId: 's1', seriesMode: 'accumulate' })
    const result = await processSeries(event, store)
    // prevData is {} (null -> fallback), newData.text is 'world' but prevData.text is not a string → no concat
    expect((result.data as { text: string }).text).toBe('world')
    expect(store.setSeriesLatest).toHaveBeenCalled()
  })

  it('treats null event.data as empty object (no text concat)', async () => {
    const prev = makeEvent({ data: { text: 'hello ' }, seriesId: 's1', seriesMode: 'accumulate' })
    const store = makeStore(prev)
    const event = makeEvent({ data: null, seriesId: 's1', seriesMode: 'accumulate' })
    const result = await processSeries(event, store)
    // newData is {} (null -> fallback), newData.text is not a string → no concat
    expect(result.data).toBeNull()
    expect(store.setSeriesLatest).toHaveBeenCalled()
  })
})

describe('processSeries - latest', () => {
  it('calls replaceLastSeriesEvent with new event', async () => {
    const prev = makeEvent({ seriesId: 's1', seriesMode: 'latest', data: { text: 'old' } })
    const store = makeStore(prev)
    const event = makeEvent({ seriesId: 's1', seriesMode: 'latest', data: { text: 'new' } })
    const result = await processSeries(event, store)
    expect(result).toEqual(event)
    expect(store.replaceLastSeriesEvent).toHaveBeenCalledWith('task-1', 's1', event)
  })

  it('works with no previous event', async () => {
    const store = makeStore()
    const event = makeEvent({ seriesId: 's1', seriesMode: 'latest', data: { text: 'first' } })
    await processSeries(event, store)
    expect(store.replaceLastSeriesEvent).toHaveBeenCalledWith('task-1', 's1', event)
  })
})

describe('processSeries - no seriesId', () => {
  it('returns event unchanged when no seriesId', async () => {
    const store = makeStore()
    const event = makeEvent()
    const result = await processSeries(event, store)
    expect(result).toEqual(event)
    expect(store.getSeriesLatest).not.toHaveBeenCalled()
  })
})
