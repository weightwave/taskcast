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

const makeStore = (latestEvent?: TaskEvent): ShortTermStore => ({
  saveTask: vi.fn(),
  getTask: vi.fn(),
  nextIndex: vi.fn(),
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
  it('concatenates delta field when previous exists', async () => {
    const prev = makeEvent({ data: { delta: 'hello ' }, seriesId: 's1', seriesMode: 'accumulate' })
    const store = makeStore(prev)
    const event = makeEvent({ data: { delta: 'world' }, seriesId: 's1', seriesMode: 'accumulate' })
    const result = await processSeries(event, store)
    expect((result.data as { delta: string }).delta).toBe('hello world')
    expect(store.setSeriesLatest).toHaveBeenCalledWith('task-1', 's1', result)
  })

  it('returns event unchanged when no previous', async () => {
    const store = makeStore()
    const event = makeEvent({ data: { delta: 'start' }, seriesId: 's1', seriesMode: 'accumulate' })
    const result = await processSeries(event, store)
    expect((result.data as { delta: string }).delta).toBe('start')
    expect(store.setSeriesLatest).toHaveBeenCalledWith('task-1', 's1', result)
  })

  it('handles non-delta data gracefully (returns event unchanged)', async () => {
    const prev = makeEvent({ data: { count: 1 }, seriesId: 's1', seriesMode: 'accumulate' })
    const store = makeStore(prev)
    const event = makeEvent({ data: { count: 2 }, seriesId: 's1', seriesMode: 'accumulate' })
    const result = await processSeries(event, store)
    expect(result.data).toEqual({ count: 2 })
    expect(store.setSeriesLatest).toHaveBeenCalled()
  })

  it('treats null prev.data as empty object (no concat)', async () => {
    const prev = makeEvent({ data: null, seriesId: 's1', seriesMode: 'accumulate' })
    const store = makeStore(prev)
    const event = makeEvent({ data: { delta: 'world' }, seriesId: 's1', seriesMode: 'accumulate' })
    const result = await processSeries(event, store)
    expect((result.data as { delta: string }).delta).toBe('world')
    expect(store.setSeriesLatest).toHaveBeenCalled()
  })

  it('treats null event.data as empty object (no concat)', async () => {
    const prev = makeEvent({ data: { delta: 'hello ' }, seriesId: 's1', seriesMode: 'accumulate' })
    const store = makeStore(prev)
    const event = makeEvent({ data: null, seriesId: 's1', seriesMode: 'accumulate' })
    const result = await processSeries(event, store)
    expect(result.data).toBeNull()
    expect(store.setSeriesLatest).toHaveBeenCalled()
  })

  it('supports custom seriesAccField', async () => {
    const prev = makeEvent({ data: { content: 'hello ' }, seriesId: 's1', seriesMode: 'accumulate', seriesAccField: 'content' })
    const store = makeStore(prev)
    const event = makeEvent({ data: { content: 'world' }, seriesId: 's1', seriesMode: 'accumulate', seriesAccField: 'content' })
    const result = await processSeries(event, store)
    expect((result.data as { content: string }).content).toBe('hello world')
  })

  it('supports legacy text field via seriesAccField', async () => {
    const prev = makeEvent({ data: { text: 'hello ' }, seriesId: 's1', seriesMode: 'accumulate', seriesAccField: 'text' })
    const store = makeStore(prev)
    const event = makeEvent({ data: { text: 'world' }, seriesId: 's1', seriesMode: 'accumulate', seriesAccField: 'text' })
    const result = await processSeries(event, store)
    expect((result.data as { text: string }).text).toBe('hello world')
  })
})

describe('processSeries - latest', () => {
  it('calls replaceLastSeriesEvent with new event', async () => {
    const prev = makeEvent({ seriesId: 's1', seriesMode: 'latest', data: { delta: 'old' } })
    const store = makeStore(prev)
    const event = makeEvent({ seriesId: 's1', seriesMode: 'latest', data: { delta: 'new' } })
    const result = await processSeries(event, store)
    expect(result).toEqual(event)
    expect(store.replaceLastSeriesEvent).toHaveBeenCalledWith('task-1', 's1', event)
  })

  it('works with no previous event', async () => {
    const store = makeStore()
    const event = makeEvent({ seriesId: 's1', seriesMode: 'latest', data: { delta: 'first' } })
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
