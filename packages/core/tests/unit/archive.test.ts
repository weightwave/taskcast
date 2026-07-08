import { describe, expect, it } from 'vitest'
import type { Task, TaskArchive, TaskEvent } from '../../src/types.js'
import { InvalidTaskArchiveError, buildTaskArchiveRestoreData, normalizeTaskArchive } from '../../src/archive.js'

function makeTask(id = 'task-1'): Task {
  return {
    id,
    status: 'running',
    createdAt: 1000,
    updatedAt: 2000,
    type: 'demo',
  }
}

function makeEvent(id: string, taskId: string, index: number, data: unknown = null): TaskEvent {
  return {
    id,
    taskId,
    index,
    timestamp: 3000 + index,
    type: 'demo.event',
    level: 'info',
    data,
  }
}

function makeArchive(events: TaskEvent[]): TaskArchive {
  return {
    schema: 'taskcast.taskArchive',
    version: 1,
    exportedAt: 5000,
    task: makeTask('task-1'),
    events,
  }
}

describe('normalizeTaskArchive', () => {
  it('rejects malformed archive envelope fields', () => {
    const malformedArchives = [
      { ...makeArchive([]), exportedAt: Number.NaN },
      { ...makeArchive([]), events: undefined },
      { ...makeArchive([]), task: null },
    ] as unknown as TaskArchive[]

    for (const archive of malformedArchives) {
      expect(() => normalizeTaskArchive(archive)).toThrow(InvalidTaskArchiveError)
    }
  })

  it('rejects missing or invalid task required fields', () => {
    const malformedTasks = [
      { status: undefined },
      { status: 'unknown' },
      { createdAt: undefined },
      { createdAt: Number.NaN },
      { updatedAt: undefined },
      { updatedAt: Number.POSITIVE_INFINITY },
    ]

    for (const taskPatch of malformedTasks) {
      const archive = {
        ...makeArchive([]),
        task: { ...makeTask(), ...taskPatch },
      } as unknown as TaskArchive
      expect(() => normalizeTaskArchive(archive)).toThrow(InvalidTaskArchiveError)
    }
  })

  it('rejects events missing required fields or data as an own property', () => {
    const baseEvent = makeEvent('event-1', 'task-1', 0)
    const { data: _data, ...eventWithoutData } = baseEvent
    const malformedEvents = [
      { ...baseEvent, id: undefined },
      { ...baseEvent, timestamp: undefined },
      { ...baseEvent, type: undefined },
      { ...baseEvent, level: undefined },
      eventWithoutData,
    ]

    for (const event of malformedEvents) {
      const archive = makeArchive([event as TaskEvent])
      expect(() => normalizeTaskArchive(archive)).toThrow(InvalidTaskArchiveError)
    }
  })

  it('rejects invalid event level and series mode fields', () => {
    const malformedEvents = [
      { ...makeEvent('event-1', 'task-1', 0), level: 'fatal' },
      { ...makeEvent('event-1', 'task-1', 0), seriesMode: 'first' },
      { ...makeEvent('event-1', 'task-1', 0), seriesId: 123 },
      { ...makeEvent('event-1', 'task-1', 0), seriesAccField: 123 },
    ] as unknown as TaskEvent[]

    for (const event of malformedEvents) {
      expect(() => normalizeTaskArchive(makeArchive([event]))).toThrow(InvalidTaskArchiveError)
    }
  })

  it('sorts events by index without changing event identity fields', () => {
    const archive = makeArchive([
      makeEvent('event-2', 'task-1', 1),
      makeEvent('event-1', 'task-1', 0),
    ])

    const normalized = normalizeTaskArchive(archive)

    expect(normalized.events.map((event) => event.id)).toEqual(['event-1', 'event-2'])
    expect(normalized.events[0]).toMatchObject({
      id: 'event-1',
      taskId: 'task-1',
      index: 0,
      timestamp: 3000,
    })
  })

  it('sanitizes normalized events to archive-persistable fields', () => {
    const archive = makeArchive([
      {
        ...makeEvent('event-1', 'task-1', 0),
        debugOnly: true,
      } as TaskEvent & { debugOnly: true },
    ])

    const normalized = normalizeTaskArchive(archive)

    expect(normalized.events[0]).toEqual(makeEvent('event-1', 'task-1', 0))
    expect('debugOnly' in normalized.events[0]!).toBe(false)
  })

  it('rejects unsupported archive version', () => {
    const archive = { ...makeArchive([]), version: 2 as 1 }
    expect(() => normalizeTaskArchive(archive)).toThrow(InvalidTaskArchiveError)
  })

  it('rejects event taskId mismatch', () => {
    const archive = makeArchive([makeEvent('event-1', 'other-task', 0)])
    expect(() => normalizeTaskArchive(archive)).toThrow(/taskId/)
  })

  it('rejects duplicate event ids', () => {
    const archive = makeArchive([
      makeEvent('event-1', 'task-1', 0),
      makeEvent('event-1', 'task-1', 1),
    ])
    expect(() => normalizeTaskArchive(archive)).toThrow(/duplicate event id/)
  })

  it('rejects duplicate event indexes', () => {
    const archive = makeArchive([
      makeEvent('event-1', 'task-1', 0),
      makeEvent('event-2', 'task-1', 0),
    ])
    expect(() => normalizeTaskArchive(archive)).toThrow(/duplicate event index/)
  })

  it('rejects non-contiguous indexes', () => {
    const archive = makeArchive([
      makeEvent('event-1', 'task-1', 0),
      makeEvent('event-2', 'task-1', 2),
    ])
    expect(() => normalizeTaskArchive(archive)).toThrow(/contiguous/)
  })

  it('rejects series snapshot events', () => {
    const archive = makeArchive([
      {
        ...makeEvent('event-1', 'task-1', 0),
        seriesSnapshot: true,
      },
    ])
    expect(() => normalizeTaskArchive(archive)).toThrow(/seriesSnapshot/)
  })

  it('rejects broadcast accumulated data', () => {
    const archive = makeArchive([
      {
        ...makeEvent('event-1', 'task-1', 0),
        _accumulatedData: { delta: 'hello world' },
      },
    ])
    expect(() => normalizeTaskArchive(archive)).toThrow(/_accumulatedData/)
  })
})

describe('buildTaskArchiveRestoreData', () => {
  it('sets nextIndex to max index plus one', () => {
    const restore = buildTaskArchiveRestoreData(
      makeArchive([
        makeEvent('event-1', 'task-1', 0),
        makeEvent('event-2', 'task-1', 1),
      ]),
    )

    expect(restore.nextIndex).toBe(2)
  })

  it('rebuilds latest series state', () => {
    const latestEvent = {
      ...makeEvent('event-2', 'task-1', 1, { value: 'new' }),
      seriesId: 'series-1',
      seriesMode: 'latest' as const,
    }
    const restore = buildTaskArchiveRestoreData(
      makeArchive([
        {
          ...makeEvent('event-1', 'task-1', 0, { value: 'old' }),
          seriesId: 'series-1',
          seriesMode: 'latest' as const,
        },
        latestEvent,
      ]),
    )

    expect(restore.seriesLatest).toEqual([{ taskId: 'task-1', seriesId: 'series-1', event: latestEvent }])
  })

  it('rebuilds accumulate series state by concatenating the configured field', () => {
    const restore = buildTaskArchiveRestoreData(
      makeArchive([
        {
          ...makeEvent('event-1', 'task-1', 0, { delta: 'hello ' }),
          seriesId: 'series-1',
          seriesMode: 'accumulate' as const,
          seriesAccField: 'delta',
        },
        {
          ...makeEvent('event-2', 'task-1', 1, { delta: 'world' }),
          seriesId: 'series-1',
          seriesMode: 'accumulate' as const,
          seriesAccField: 'delta',
        },
      ]),
    )

    expect(restore.seriesLatest[0]?.event.data).toEqual({ delta: 'hello world' })
  })
})
