import { describe, expect, it, vi } from 'vitest'
import {
  MemoryBroadcastProvider,
  MemoryShortTermStore,
  InvalidTaskArchiveError,
  TaskConflictError,
  TaskEngine,
} from '../../src/index.js'
import type { LongTermStore, TaskArchive, TaskArchiveRestoreData, TaskEvent } from '../../src/types.js'

function makeEngine(broadcast = new MemoryBroadcastProvider(), longTermStore?: LongTermStore) {
  return new TaskEngine({
    broadcast,
    shortTermStore: new MemoryShortTermStore(),
    ...(longTermStore !== undefined ? { longTermStore } : {}),
  })
}

function makeArchive(events: TaskArchive['events'] = []): TaskArchive {
  return {
    schema: 'taskcast.taskArchive',
    version: 1,
    exportedAt: 5000,
    task: { id: 'task-1', status: 'running', createdAt: 1000, updatedAt: 2000 },
    events,
  }
}

function makeLongTermStore(
  overrides: Partial<LongTermStore> & {
    validateTaskArchiveRestore?: (
      data: TaskArchiveRestoreData,
      options?: { overwrite?: boolean },
    ) => Promise<void>
  } = {},
): LongTermStore {
  return {
    saveTask: vi.fn().mockResolvedValue(undefined),
    getTask: vi.fn().mockResolvedValue(null),
    saveEvent: vi.fn().mockResolvedValue(undefined),
    getEvents: vi.fn().mockResolvedValue([]),
    saveWorkerEvent: vi.fn().mockResolvedValue(undefined),
    getWorkerEvents: vi.fn().mockResolvedValue([]),
    ...overrides,
  }
}

describe('TaskEngine archive import/export', () => {
  it('exports task and events preserving original fields', async () => {
    const engine = makeEngine()
    const task = await engine.createTask({ id: 'task-1', type: 'demo' })
    const event = await engine.publishEvent(task.id, {
      type: 'demo.event',
      level: 'info',
      data: { value: 1 },
    })

    const archive = await engine.exportTaskArchive(task.id)

    expect(archive).toMatchObject({
      schema: 'taskcast.taskArchive',
      version: 1,
      task: { id: 'task-1', createdAt: task.createdAt, updatedAt: task.updatedAt },
      events: [{ id: event.id, index: 0, timestamp: event.timestamp }],
    })
  })

  it('exports full long-term latest-series history instead of compacted short-term history', async () => {
    const longTermEvents: TaskEvent[] = []
    const longTermStore = makeLongTermStore({
      saveEvent: vi.fn(async (event: TaskEvent) => {
        longTermEvents.push({ ...event })
      }),
      getEvents: vi.fn(async () => longTermEvents),
    })
    const engine = makeEngine(new MemoryBroadcastProvider(), longTermStore)
    await engine.createTask({ id: 'task-1' })
    const first = await engine.publishEvent('task-1', {
      type: 'task.status',
      level: 'info',
      data: { status: 'starting' },
      seriesId: 'status',
      seriesMode: 'latest',
    })
    const second = await engine.publishEvent('task-1', {
      type: 'task.status',
      level: 'info',
      data: { status: 'ready' },
      seriesId: 'status',
      seriesMode: 'latest',
    })

    await expect(engine.getEvents('task-1')).resolves.toMatchObject([{ index: 1 }])

    const archive = await engine.exportTaskArchive('task-1')

    expect(archive.events.map((event) => event.id)).toEqual([first.id, second.id])
    expect(archive.events.map((event) => event.index)).toEqual([0, 1])
  })

  it('exports raw accumulate deltas instead of long-term accumulated values', async () => {
    const longTermEvents: TaskEvent[] = []
    const longTermStore = makeLongTermStore({
      saveEvent: vi.fn(async (event: TaskEvent) => {
        longTermEvents.push({ ...event })
      }),
      getEvents: vi.fn(async () => longTermEvents),
    })
    const engine = makeEngine(new MemoryBroadcastProvider(), longTermStore)
    await engine.createTask({ id: 'task-1' })
    await engine.publishEvent('task-1', {
      type: 'task.output',
      level: 'info',
      data: { delta: 'hello ' },
      seriesId: 'output',
      seriesMode: 'accumulate',
      seriesAccField: 'delta',
    })
    await engine.publishEvent('task-1', {
      type: 'task.output',
      level: 'info',
      data: { delta: 'world' },
      seriesId: 'output',
      seriesMode: 'accumulate',
      seriesAccField: 'delta',
    })

    expect(longTermEvents.map((event) => event.data)).toEqual([{ delta: 'hello ' }, { delta: 'hello world' }])

    const archive = await engine.exportTaskArchive('task-1')

    expect(archive.events.map((event) => event.data)).toEqual([{ delta: 'hello ' }, { delta: 'world' }])
  })

  it('exports mixed latest and accumulate series as a contiguous raw archive', async () => {
    const longTermEvents: TaskEvent[] = []
    const longTermStore = makeLongTermStore({
      saveEvent: vi.fn(async (event: TaskEvent) => {
        longTermEvents.push({ ...event })
      }),
      getEvents: vi.fn(async () => longTermEvents),
    })
    const engine = makeEngine(new MemoryBroadcastProvider(), longTermStore)
    await engine.createTask({ id: 'task-1' })
    await engine.publishEvent('task-1', {
      type: 'task.status',
      level: 'info',
      data: { status: 'starting' },
      seriesId: 'status',
      seriesMode: 'latest',
    })
    await engine.publishEvent('task-1', {
      type: 'task.status',
      level: 'info',
      data: { status: 'ready' },
      seriesId: 'status',
      seriesMode: 'latest',
    })
    await engine.publishEvent('task-1', {
      type: 'task.output',
      level: 'info',
      data: { delta: 'hello ' },
      seriesId: 'output',
      seriesMode: 'accumulate',
      seriesAccField: 'delta',
    })
    await engine.publishEvent('task-1', {
      type: 'task.output',
      level: 'info',
      data: { delta: 'world' },
      seriesId: 'output',
      seriesMode: 'accumulate',
      seriesAccField: 'delta',
    })

    await expect(engine.getEvents('task-1')).resolves.toMatchObject([{ index: 1 }, { index: 2 }, { index: 3 }])
    expect(longTermEvents.map((event) => event.data)).toEqual([
      { status: 'starting' },
      { status: 'ready' },
      { delta: 'hello ' },
      { delta: 'hello world' },
    ])

    const archive = await engine.exportTaskArchive('task-1')

    expect(archive.events.map((event) => event.index)).toEqual([0, 1, 2, 3])
    expect(archive.events.map((event) => event.data)).toEqual([
      { status: 'starting' },
      { status: 'ready' },
      { delta: 'hello ' },
      { delta: 'world' },
    ])
  })

  it('recovers lagging long-term plain-event tails from short-term history', async () => {
    const longTermEvents: TaskEvent[] = []
    const longTermStore = makeLongTermStore({
      saveEvent: vi.fn(async (event: TaskEvent) => {
        longTermEvents.push({ ...event })
      }),
      getEvents: vi.fn(async () => longTermEvents.slice(0, 1)),
    })
    const engine = makeEngine(new MemoryBroadcastProvider(), longTermStore)
    await engine.createTask({ id: 'task-1' })
    await engine.publishEvent('task-1', { type: 'demo.one', level: 'info', data: { value: 1 } })
    await engine.publishEvent('task-1', { type: 'demo.two', level: 'info', data: { value: 2 } })
    await engine.publishEvent('task-1', { type: 'demo.three', level: 'info', data: { value: 3 } })

    const archive = await engine.exportTaskArchive('task-1')

    expect(archive.events.map((event) => event.index)).toEqual([0, 1, 2])
    expect(archive.events.map((event) => event.type)).toEqual(['demo.one', 'demo.two', 'demo.three'])
  })

  it('rejects lagging latest-series long-term history that short-term compaction cannot recover', async () => {
    const longTermEvents: TaskEvent[] = []
    const longTermStore = makeLongTermStore({
      saveEvent: vi.fn(async (event: TaskEvent) => {
        longTermEvents.push({ ...event })
      }),
      getEvents: vi.fn(async () => longTermEvents.slice(0, 1)),
    })
    const engine = makeEngine(new MemoryBroadcastProvider(), longTermStore)
    await engine.createTask({ id: 'task-1' })
    await engine.publishEvent('task-1', {
      type: 'task.status',
      level: 'info',
      data: { status: 'starting' },
      seriesId: 'status',
      seriesMode: 'latest',
    })
    await engine.publishEvent('task-1', {
      type: 'task.status',
      level: 'info',
      data: { status: 'running' },
      seriesId: 'status',
      seriesMode: 'latest',
    })
    await engine.publishEvent('task-1', {
      type: 'task.status',
      level: 'info',
      data: { status: 'ready' },
      seriesId: 'status',
      seriesMode: 'latest',
    })
    await expect(engine.getEvents('task-1')).resolves.toMatchObject([{ index: 2 }])

    await expect(engine.exportTaskArchive('task-1')).rejects.toThrow(InvalidTaskArchiveError)
  })

  it('does not mask corrupted non-empty long-term history with short-term fallback', async () => {
    const longTermEvents: TaskEvent[] = []
    const longTermStore = makeLongTermStore({
      saveEvent: vi.fn(async (event: TaskEvent) => {
        longTermEvents.push({ ...event })
      }),
      getEvents: vi.fn(async () => [longTermEvents[0]!, longTermEvents[2]!]),
    })
    const engine = makeEngine(new MemoryBroadcastProvider(), longTermStore)
    await engine.createTask({ id: 'task-1' })
    await engine.publishEvent('task-1', { type: 'demo.one', level: 'info', data: null })
    await engine.publishEvent('task-1', { type: 'demo.two', level: 'info', data: null })
    await engine.publishEvent('task-1', { type: 'demo.three', level: 'info', data: null })

    await expect(engine.exportTaskArchive('task-1')).rejects.toThrow(InvalidTaskArchiveError)
  })

  it('rejects long-term accumulate events that have no matching short-term raw event', async () => {
    const longTermStore = makeLongTermStore({
      getEvents: vi.fn(async () => [
        {
          id: 'event-1',
          taskId: 'task-1',
          index: 0,
          timestamp: 3000,
          type: 'task.output',
          level: 'info',
          data: { delta: 'hello world' },
          seriesId: 'output',
          seriesMode: 'accumulate',
          seriesAccField: 'delta',
        },
      ]),
    })
    const engine = makeEngine(new MemoryBroadcastProvider(), longTermStore)
    await engine.createTask({ id: 'task-1' })

    await expect(engine.exportTaskArchive('task-1')).rejects.toThrow(InvalidTaskArchiveError)
  })

  it('imports archive silently and allows publish to continue at the next index', async () => {
    const source = makeEngine()
    await source.createTask({ id: 'task-1' })
    await source.publishEvent('task-1', { type: 'demo.one', level: 'info', data: null })
    const archive = await source.exportTaskArchive('task-1')

    const broadcast = new MemoryBroadcastProvider()
    const handler = vi.fn()
    broadcast.subscribe('task-1', handler)
    const target = makeEngine(broadcast)

    const result = await target.importTaskArchive(archive)
    const next = await target.publishEvent('task-1', { type: 'demo.two', level: 'info', data: null })

    expect(result).toEqual({ taskId: 'task-1', eventCount: 1, overwritten: false })
    expect(handler).toHaveBeenCalledTimes(1)
    expect(handler.mock.calls[0]![0].type).toBe('demo.two')
    expect(next.index).toBe(1)
  })

  it('rejects an existing task unless overwrite is true', async () => {
    const engine = makeEngine()
    await engine.createTask({ id: 'task-1' })

    await expect(engine.importTaskArchive(makeArchive())).rejects.toThrow(TaskConflictError)
  })

  it('overwrite replaces the full old history', async () => {
    const engine = makeEngine()
    await engine.createTask({ id: 'task-1' })
    await engine.publishEvent('task-1', { type: 'old.event', level: 'info', data: null })

    const result = await engine.importTaskArchive(
      makeArchive([
        {
          id: 'imported-event',
          taskId: 'task-1',
          index: 0,
          timestamp: 3000,
          type: 'new.event',
          level: 'info',
          data: null,
        },
      ]),
      { overwrite: true },
    )
    const events = await engine.getEvents('task-1')

    expect(result.overwritten).toBe(true)
    expect(events.map((event) => event.type)).toEqual(['new.event'])
  })

  it('rebuilds latest and accumulate series state so future publishes resume correctly', async () => {
    const target = makeEngine()
    const archive = makeArchive([
      {
        id: 'status-old',
        taskId: 'task-1',
        index: 0,
        timestamp: 3000,
        type: 'task.status',
        level: 'info',
        data: { status: 'starting' },
        seriesId: 'status',
        seriesMode: 'latest',
      },
      {
        id: 'status-new',
        taskId: 'task-1',
        index: 1,
        timestamp: 3001,
        type: 'task.status',
        level: 'info',
        data: { status: 'ready' },
        seriesId: 'status',
        seriesMode: 'latest',
      },
      {
        id: 'output-1',
        taskId: 'task-1',
        index: 2,
        timestamp: 3002,
        type: 'task.output',
        level: 'info',
        data: { delta: 'hello ' },
        seriesId: 'output',
        seriesMode: 'accumulate',
        seriesAccField: 'delta',
      },
      {
        id: 'output-2',
        taskId: 'task-1',
        index: 3,
        timestamp: 3003,
        type: 'task.output',
        level: 'info',
        data: { delta: 'world' },
        seriesId: 'output',
        seriesMode: 'accumulate',
        seriesAccField: 'delta',
      },
    ])

    await target.importTaskArchive(archive)
    const restoredStatus = await target.getSeriesLatest('task-1', 'status')
    const restoredOutput = await target.getSeriesLatest('task-1', 'output')
    const next = await target.publishEvent('task-1', {
      type: 'task.output',
      level: 'info',
      data: { delta: '!' },
      seriesId: 'output',
      seriesMode: 'accumulate',
      seriesAccField: 'delta',
    })
    const resumedLatest = await target.getSeriesLatest('task-1', 'output')

    expect(restoredStatus?.data).toEqual({ status: 'ready' })
    expect(restoredOutput?.data).toEqual({ delta: 'hello world' })
    expect(next.index).toBe(4)
    expect(resumedLatest?.data).toEqual({ delta: 'hello world!' })
  })

  it('export and import do not include or broadcast transient accumulated data', async () => {
    const source = makeEngine()
    await source.createTask({ id: 'task-1' })
    await source.publishEvent('task-1', {
      type: 'task.output',
      level: 'info',
      data: { delta: 'hello ' },
      seriesId: 'output',
      seriesMode: 'accumulate',
    })
    await source.publishEvent('task-1', {
      type: 'task.output',
      level: 'info',
      data: { delta: 'world' },
      seriesId: 'output',
      seriesMode: 'accumulate',
    })

    const archive = await source.exportTaskArchive('task-1')
    const broadcast = new MemoryBroadcastProvider()
    const handler = vi.fn()
    broadcast.subscribe('task-1', handler)
    const target = makeEngine(broadcast)

    await target.importTaskArchive(archive)

    expect(handler).not.toHaveBeenCalled()
    expect(archive.events).toHaveLength(2)
    expect(archive.events.map((event) => event.data)).toEqual([{ delta: 'hello ' }, { delta: 'world' }])
    expect(archive.events.some((event) => '_accumulatedData' in event)).toBe(false)
  })

  it('fails before short-term restore when long-term store cannot restore archives', async () => {
    const shortTermStore = new MemoryShortTermStore()
    const engine = new TaskEngine({
      broadcast: new MemoryBroadcastProvider(),
      shortTermStore,
      longTermStore: makeLongTermStore(),
    })

    await expect(engine.importTaskArchive(makeArchive())).rejects.toThrow(/longTermStore.*restoreTaskArchive/)

    await expect(shortTermStore.getTask('task-1')).resolves.toBeNull()
    await expect(shortTermStore.getEvents('task-1')).resolves.toEqual([])
  })

  it('does not restore long-term when short-term archive restore fails', async () => {
    const shortTermStore = new MemoryShortTermStore()
    const shortRestore = vi
      .spyOn(shortTermStore, 'restoreTaskArchive')
      .mockRejectedValue(new Error('short restore failed'))
    const longRestore = vi.fn().mockResolvedValue({ overwritten: false })
    const engine = new TaskEngine({
      broadcast: new MemoryBroadcastProvider(),
      shortTermStore,
      longTermStore: makeLongTermStore({ restoreTaskArchive: longRestore }),
    })

    await expect(engine.importTaskArchive(makeArchive())).rejects.toThrow('short restore failed')

    expect(shortRestore).toHaveBeenCalledOnce()
    expect(longRestore).not.toHaveBeenCalled()
  })

  it('validates long-term restore before mutating short-term state', async () => {
    const shortTermStore = new MemoryShortTermStore()
    const shortRestore = vi.spyOn(shortTermStore, 'restoreTaskArchive')
    const longValidate = vi.fn().mockRejectedValue(new Error('long preflight failed'))
    const longRestore = vi.fn().mockResolvedValue({ overwritten: false })
    const engine = new TaskEngine({
      broadcast: new MemoryBroadcastProvider(),
      shortTermStore,
      longTermStore: makeLongTermStore({
        validateTaskArchiveRestore: longValidate,
        restoreTaskArchive: longRestore,
      }),
    })

    await expect(engine.importTaskArchive(makeArchive())).rejects.toThrow('long preflight failed')

    expect(longValidate).toHaveBeenCalledOnce()
    expect(shortRestore).not.toHaveBeenCalled()
    expect(longRestore).not.toHaveBeenCalled()
    await expect(shortTermStore.getTask('task-1')).resolves.toBeNull()
    await expect(shortTermStore.getEvents('task-1')).resolves.toEqual([])
  })

  it('uses validated restore-phase overwrite without reporting new imports as overwritten', async () => {
    const shortTermStore = new MemoryShortTermStore()
    const originalRestore = shortTermStore.restoreTaskArchive.bind(shortTermStore)
    const shortValidate = vi.fn().mockResolvedValue(undefined)
    const shortRestore = vi.fn(async (data: TaskArchiveRestoreData, options?: { overwrite?: boolean }) => {
      if (options?.overwrite !== true) throw new Error('restore overwrite option missing')
      await originalRestore(data, options)
      return { overwritten: true }
    })
    shortTermStore.validateTaskArchiveRestore = shortValidate
    shortTermStore.restoreTaskArchive = shortRestore
    const engine = new TaskEngine({
      broadcast: new MemoryBroadcastProvider(),
      shortTermStore,
    })

    const result = await engine.importTaskArchive(makeArchive())

    expect(shortValidate).toHaveBeenCalled()
    expect(shortRestore).toHaveBeenCalledWith(expect.any(Object), { overwrite: true })
    expect(result).toEqual({ taskId: 'task-1', eventCount: 0, overwritten: false })
  })
})
