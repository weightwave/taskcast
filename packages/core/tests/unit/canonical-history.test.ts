import { describe, expect, it } from 'vitest'
import {
  applyCanonicalHistoryQuery,
  mergeCanonicalHistory,
  resolveCanonicalSeriesLatest,
  type DurableSeriesState,
  type TaskEvent,
} from '../../src/index.js'

const event = (
  index: number,
  overrides: Partial<TaskEvent> = {},
): TaskEvent => ({
  id: `event-${index}`,
  taskId: 'task-1',
  index,
  timestamp: 1_000 + index,
  type: 'message',
  level: 'info',
  data: { index },
  ...overrides,
})

describe('canonical hot/cold history', () => {
  it('uses durable history as the baseline and overlays an identical hot tail', () => {
    const durable = Array.from({ length: 10 }, (_, index) => event(index))
    const hot = [event(8), event(9), event(10)]

    expect(mergeCanonicalHistory(durable, hot, [])).toEqual(
      Array.from({ length: 11 }, (_, index) => event(index)),
    )
  })

  it('rejects conflicting index and event identities', () => {
    expect(() =>
      mergeCanonicalHistory(
        [event(0)],
        [event(0, { id: 'other-id' })],
        [],
      ),
    ).toThrow(/index 0/i)
    expect(() =>
      mergeCanonicalHistory(
        [event(0)],
        [event(0, { data: { changed: true } })],
        [],
      ),
    ).toThrow(/event-0/i)
    expect(() =>
      mergeCanonicalHistory(
        [event(0)],
        [event(1, { id: 'event-0' })],
        [],
      ),
    ).toThrow(/event-0/i)
  })

  it('uses durable series throughIndex to ignore covered deltas and retain the tail', () => {
    const accumulated = event(2, {
      id: 'acc-2',
      seriesId: 'output',
      seriesMode: 'accumulate',
      seriesAccField: 'delta',
      data: { delta: 'ABC' },
    })
    const state: DurableSeriesState = {
      taskId: 'task-1',
      seriesId: 'output',
      mode: 'accumulate',
      event: accumulated,
      throughIndex: 2,
    }
    const hot = ['A', 'B', 'C', 'D'].map((delta, index) =>
      event(index, {
        id: `acc-${index}`,
        seriesId: 'output',
        seriesMode: 'accumulate',
        seriesAccField: 'delta',
        data: { delta },
      }))

    expect(mergeCanonicalHistory([accumulated], hot, [state])).toEqual([
      accumulated,
      hot[3],
    ])
    expect(resolveCanonicalSeriesLatest(state, hot)).toEqual({
      ...hot[3],
      data: { delta: 'ABCD' },
    })
  })

  it('resolves latest-mode state from the highest uncovered hot event', () => {
    const durable = event(2, {
      id: 'progress-2',
      seriesId: 'progress',
      seriesMode: 'latest',
      data: { percent: 50 },
    })
    const state: DurableSeriesState = {
      taskId: 'task-1',
      seriesId: 'progress',
      mode: 'latest',
      event: durable,
      throughIndex: 2,
    }
    const hot = [
      durable,
      event(3, {
        id: 'progress-3',
        seriesId: 'progress',
        seriesMode: 'latest',
        data: { percent: 75 },
      }),
    ]

    expect(resolveCanonicalSeriesLatest(state, hot)).toEqual(hot[1])
  })

  it('applies cursor precedence and limit after canonical assembly', () => {
    const events = Array.from({ length: 11 }, (_, index) => event(index))

    expect(
      applyCanonicalHistoryQuery(events, {
        since: { id: 'event-7', index: 1, timestamp: 1_001 },
        limit: 2,
      }).map(({ index }) => index),
    ).toEqual([8, 9])
    expect(
      applyCanonicalHistoryQuery(events, {
        since: { index: 7 },
        limit: 2,
      }).map(({ index }) => index),
    ).toEqual([8, 9])
    expect(
      applyCanonicalHistoryQuery(events, {
        since: { timestamp: 1_007 },
        limit: 2,
      }).map(({ index }) => index),
    ).toEqual([8, 9])
    expect(
      applyCanonicalHistoryQuery(events, {
        since: { id: 'missing' },
        limit: 2,
      }).map(({ index }) => index),
    ).toEqual([0, 1])
  })
})
