import { describe, it, expect, vi } from 'vitest'
import { collapseAccumulateSeries } from '../../src/series.js'
import type { TaskEvent } from '../../src/types.js'

function makeEvent(overrides: Partial<TaskEvent> = {}): TaskEvent {
  return {
    id: 'evt-1',
    taskId: 'task-1',
    index: 0,
    timestamp: 1000,
    type: 'test',
    level: 'info',
    data: { text: 'hello' },
    ...overrides,
  }
}

describe('collapseAccumulateSeries', () => {
  it('returns events unchanged when no accumulate series present', async () => {
    const events = [
      makeEvent({ id: 'e1', index: 0 }),
      makeEvent({ id: 'e2', index: 1 }),
    ]
    const getLatest = vi.fn().mockResolvedValue(null)
    const result = await collapseAccumulateSeries(events, getLatest)
    expect(result).toEqual(events)
    expect(getLatest).not.toHaveBeenCalled()
  })

  it('collapses accumulate series into single snapshot using getSeriesLatest', async () => {
    const events = [
      makeEvent({ id: 'e1', index: 0, seriesId: 's1', seriesMode: 'accumulate', data: { delta: 'A' } }),
      makeEvent({ id: 'e2', index: 1, seriesId: 's1', seriesMode: 'accumulate', data: { delta: 'B' } }),
      makeEvent({ id: 'e3', index: 2, type: 'other', data: { x: 1 } }),
    ]
    const accSnapshot = makeEvent({
      id: 'e2', index: 1, seriesId: 's1', seriesMode: 'accumulate',
      data: { delta: 'AB' },
    })
    const getLatest = vi.fn().mockResolvedValue(accSnapshot)

    const result = await collapseAccumulateSeries(events, getLatest)

    expect(result).toHaveLength(2)
    expect(result[0]).toEqual({ ...accSnapshot, seriesSnapshot: true })
    expect(result[1]).toEqual(events[2])
    expect(getLatest).toHaveBeenCalledWith('task-1', 's1')
  })

  it('falls back to last event in array when getSeriesLatest returns null (cold task)', async () => {
    const events = [
      makeEvent({ id: 'e1', index: 0, seriesId: 's1', seriesMode: 'accumulate', data: { delta: 'A' } }),
      makeEvent({ id: 'e2', index: 1, seriesId: 's1', seriesMode: 'accumulate', data: { delta: 'AB' } }),
    ]
    const getLatest = vi.fn().mockResolvedValue(null)

    const result = await collapseAccumulateSeries(events, getLatest)

    expect(result).toHaveLength(1)
    expect(result[0]).toEqual({ ...events[1], seriesSnapshot: true })
  })

  it('handles multiple accumulate series independently', async () => {
    const events = [
      makeEvent({ id: 'e1', index: 0, seriesId: 's1', seriesMode: 'accumulate' }),
      makeEvent({ id: 'e2', index: 1, seriesId: 's2', seriesMode: 'accumulate' }),
      makeEvent({ id: 'e3', index: 2, seriesId: 's1', seriesMode: 'accumulate' }),
      makeEvent({ id: 'e4', index: 3, seriesId: 's2', seriesMode: 'accumulate' }),
    ]
    const getLatest = vi.fn()
      .mockResolvedValueOnce(makeEvent({ id: 'snap-s1', seriesId: 's1', data: { delta: 'S1-ACC' } }))
      .mockResolvedValueOnce(makeEvent({ id: 'snap-s2', seriesId: 's2', data: { delta: 'S2-ACC' } }))

    const result = await collapseAccumulateSeries(events, getLatest)

    expect(result).toHaveLength(2)
    expect(result[0].seriesSnapshot).toBe(true)
    expect(result[1].seriesSnapshot).toBe(true)
  })

  it('preserves keep-all and latest series events', async () => {
    const events = [
      makeEvent({ id: 'e1', index: 0, seriesId: 'ka', seriesMode: 'keep-all' }),
      makeEvent({ id: 'e2', index: 1, seriesId: 'lt', seriesMode: 'latest' }),
      makeEvent({ id: 'e3', index: 2, seriesId: 'acc', seriesMode: 'accumulate' }),
      makeEvent({ id: 'e4', index: 3, seriesId: 'acc', seriesMode: 'accumulate' }),
    ]
    const snapshot = makeEvent({ id: 'snap', seriesId: 'acc', data: { text: 'collapsed' } })
    const getLatest = vi.fn().mockResolvedValue(snapshot)

    const result = await collapseAccumulateSeries(events, getLatest)

    expect(result).toHaveLength(3)
    expect(result[0].id).toBe('e1') // keep-all preserved
    expect(result[1].id).toBe('e2') // latest preserved
    expect(result[2].seriesSnapshot).toBe(true) // accumulate collapsed
  })

  it('handles empty events array', async () => {
    const getLatest = vi.fn()
    const result = await collapseAccumulateSeries([], getLatest)
    expect(result).toEqual([])
  })
})
