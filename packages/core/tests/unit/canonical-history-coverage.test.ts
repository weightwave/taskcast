import { describe, expect, it } from 'vitest'
import { mergeCanonicalHistory, resolveCanonicalSeriesLatest } from '../../src/canonical-history.js'
import type { DurableSeriesState, TaskEvent } from '../../src/types.js'

const event = (index: number, overrides: Partial<TaskEvent> = {}): TaskEvent => ({
  id: `event-${index}`, taskId: 'task', index, timestamp: index,
  type: 'message', level: 'info', seriesId: 'output', seriesMode: 'accumulate',
  data: { delta: 'A' }, ...overrides,
})
const state = (overrides: Partial<DurableSeriesState> = {}): DurableSeriesState => ({
  taskId: 'task', seriesId: 'output', mode: 'accumulate',
  throughIndex: 0, event: event(0), ...overrides,
})

describe('canonical history integrity and series fallback', () => {
  it('rejects duplicate durable series snapshots instead of selecting one arbitrarily', () => {
    expect(() => mergeCanonicalHistory([], [], [state(), state()])).toThrow(/Duplicate durable series state/)
  })

  it.each([
    { taskId: 'other' }, { seriesId: 'other' },
    { mode: 'latest' as const }, { throughIndex: 1 },
  ])('rejects an inconsistent durable snapshot: %s', (overrides) => {
    expect(() => mergeCanonicalHistory([], [], [state(overrides)])).toThrow(/Durable series state is inconsistent/)
    expect(() => resolveCanonicalSeriesLatest(state(overrides), [])).toThrow(/Durable series state is inconsistent/)
  })

  it('rejects a compacted series changing mode across the hot and durable boundary', () => {
    expect(() => mergeCanonicalHistory([], [event(1, { seriesMode: 'latest' })], [state()]))
      .toThrow(/series mode conflicts/)
  })

  it('preserves unarchived series and ignores unrelated tails when resolving latest state', () => {
    const latest = state({ mode: 'latest', event: event(0, { seriesMode: 'latest' }) })
    expect(mergeCanonicalHistory([event(0)], [event(1)], [])).toEqual([event(0), event(1)])
    expect(resolveCanonicalSeriesLatest(latest, [
      event(3, { seriesMode: 'latest', taskId: 'other' }),
      event(2, { seriesMode: 'latest', seriesId: 'other' }),
    ])).toEqual(latest.event)
    expect(resolveCanonicalSeriesLatest(latest, [
      event(3, { seriesMode: 'latest' }), event(1, { seriesMode: 'latest' }),
    ]).index).toBe(3)
  })

  it.each([null, [], 'text', { delta: 12 }])('replaces incompatible accumulate data before continuing a string tail: %s', (data) => {
    const tail = event(1, { data })
    expect(resolveCanonicalSeriesLatest(state(), [tail])).toEqual(tail)
    expect(resolveCanonicalSeriesLatest(state({ event: event(0, { data }) }), [
      event(1, { data: { delta: 'B' } }), event(2, { data: { delta: 'C' } }),
    ])).toMatchObject({ index: 2, data: { delta: 'BC' } })
  })
})
