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
