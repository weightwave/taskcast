import { describe, it, expect, vi } from 'vitest'
import { TaskEngine } from '../../src/engine.js'
import { MemoryBroadcastProvider, MemoryShortTermStore } from '../../src/memory-adapters.js'
import type { LongTermStore, TaskEvent } from '../../src/types.js'

function makeEngine() {
  const store = new MemoryShortTermStore()
  const broadcast = new MemoryBroadcastProvider()
  const engine = new TaskEngine({ shortTermStore: store, broadcast })
  return { engine, store, broadcast }
}

async function makeRunningTask(engine: TaskEngine) {
  const task = await engine.createTask({ type: 'test' })
  await engine.transitionTask(task.id, 'running')
  return task
}

function filterUserEvents(events: TaskEvent[]) {
  return events.filter(e => !e.type.startsWith('taskcast:'))
}

// ─── seriesMode: latest ───────────────────────────────────────────────────

describe('engine.publishEvent — seriesMode: latest', () => {
  it('keeps only the latest event after multiple publishes', async () => {
    const { engine } = makeEngine()
    const task = await makeRunningTask(engine)

    for (let i = 1; i <= 5; i++) {
      await engine.publishEvent(task.id, {
        type: 'agent.message_update',
        level: 'info',
        data: { content: `v${i}` },
        seriesId: 'msg',
        seriesMode: 'latest',
      })
    }

    const seriesEvents = filterUserEvents(await engine.getEvents(task.id))
      .filter(e => e.seriesId === 'msg')
    expect(seriesEvents).toHaveLength(1)
    expect(seriesEvents[0]!.data).toEqual({ content: 'v5' })
  })

  it('first latest event is stored exactly once', async () => {
    const { engine } = makeEngine()
    const task = await makeRunningTask(engine)

    await engine.publishEvent(task.id, {
      type: 'status',
      level: 'info',
      data: { text: 'only one' },
      seriesId: 's1',
      seriesMode: 'latest',
    })

    const seriesEvents = filterUserEvents(await engine.getEvents(task.id))
      .filter(e => e.seriesId === 's1')
    expect(seriesEvents).toHaveLength(1)
    expect(seriesEvents[0]!.data).toEqual({ text: 'only one' })
  })

  it('multiple independent latest series are each deduplicated', async () => {
    const { engine } = makeEngine()
    const task = await makeRunningTask(engine)

    for (let i = 1; i <= 3; i++) {
      await engine.publishEvent(task.id, {
        type: 'update',
        level: 'info',
        data: { v: i },
        seriesId: 'seriesA',
        seriesMode: 'latest',
      })
      await engine.publishEvent(task.id, {
        type: 'update',
        level: 'info',
        data: { v: i * 10 },
        seriesId: 'seriesB',
        seriesMode: 'latest',
      })
    }

    const userEvents = filterUserEvents(await engine.getEvents(task.id))
    const seriesA = userEvents.filter(e => e.seriesId === 'seriesA')
    const seriesB = userEvents.filter(e => e.seriesId === 'seriesB')

    expect(seriesA).toHaveLength(1)
    expect(seriesA[0]!.data).toEqual({ v: 3 })
    expect(seriesB).toHaveLength(1)
    expect(seriesB[0]!.data).toEqual({ v: 30 })
  })

  it('broadcasts every latest event to subscribers', async () => {
    const { engine } = makeEngine()
    const task = await makeRunningTask(engine)

    const received: TaskEvent[] = []
    engine.subscribe(task.id, (e) => received.push(e))

    for (let i = 1; i <= 3; i++) {
      await engine.publishEvent(task.id, {
        type: 'update',
        level: 'info',
        data: { v: i },
        seriesId: 's1',
        seriesMode: 'latest',
      })
    }

    // Broadcast should fire for every publish, even though storage only keeps the latest
    const seriesBroadcasts = received.filter(e => e.seriesId === 's1')
    expect(seriesBroadcasts).toHaveLength(3)
  })

  it('writes accumulated (latest) event to longTermStore', async () => {
    const store = new MemoryShortTermStore()
    const broadcast = new MemoryBroadcastProvider()
    const longTermStore: LongTermStore = {
      saveTask: vi.fn().mockResolvedValue(undefined),
      getTask: vi.fn().mockResolvedValue(null),
      saveEvent: vi.fn().mockResolvedValue(undefined),
      getEvents: vi.fn().mockResolvedValue([]),
    }
    const engine = new TaskEngine({ shortTermStore: store, broadcast, longTermStore })
    const task = await engine.createTask({ type: 'test' })
    await engine.transitionTask(task.id, 'running')

    await engine.publishEvent(task.id, {
      type: 'update',
      level: 'info',
      data: { v: 1 },
      seriesId: 's1',
      seriesMode: 'latest',
    })
    await engine.publishEvent(task.id, {
      type: 'update',
      level: 'info',
      data: { v: 2 },
      seriesId: 's1',
      seriesMode: 'latest',
    })

    // Wait for async longTermStore writes
    await new Promise(r => setTimeout(r, 50))

    const saveEventCalls = (longTermStore.saveEvent as ReturnType<typeof vi.fn>).mock.calls
    // latest mode events should be forwarded to longTermStore
    const latestCalls = saveEventCalls.filter(
      (c: unknown[]) => (c[0] as TaskEvent).seriesId === 's1',
    )
    expect(latestCalls.length).toBe(2)
  })

  it('getSeriesLatest returns the latest value for latest-mode series', async () => {
    const { engine } = makeEngine()
    const task = await makeRunningTask(engine)

    for (let i = 1; i <= 3; i++) {
      await engine.publishEvent(task.id, {
        type: 'update',
        level: 'info',
        data: { v: i },
        seriesId: 's1',
        seriesMode: 'latest',
      })
    }

    const latest = await engine.getSeriesLatest(task.id, 's1')
    expect(latest).toBeTruthy()
    expect(latest!.data).toEqual({ v: 3 })
  })

  it('indices are unique after latest replacements', async () => {
    const { engine } = makeEngine()
    const task = await makeRunningTask(engine)

    for (let i = 1; i <= 5; i++) {
      await engine.publishEvent(task.id, {
        type: 'update',
        level: 'info',
        data: { v: i },
        seriesId: 's1',
        seriesMode: 'latest',
      })
    }

    // Also publish some non-series events
    await engine.publishEvent(task.id, {
      type: 'plain',
      level: 'info',
      data: { x: 1 },
    })

    const events = filterUserEvents(await engine.getEvents(task.id))
    const indices = events.map(e => e.index)
    expect(new Set(indices).size).toBe(indices.length)
  })

  it('latest events interleaved with non-series events preserves all non-series events', async () => {
    const { engine } = makeEngine()
    const task = await makeRunningTask(engine)

    // Interleave: plain → latest → plain → latest → plain
    await engine.publishEvent(task.id, { type: 'log', level: 'info', data: { n: 1 } })
    await engine.publishEvent(task.id, {
      type: 'update', level: 'info', data: { v: 1 },
      seriesId: 's1', seriesMode: 'latest',
    })
    await engine.publishEvent(task.id, { type: 'log', level: 'info', data: { n: 2 } })
    await engine.publishEvent(task.id, {
      type: 'update', level: 'info', data: { v: 2 },
      seriesId: 's1', seriesMode: 'latest',
    })
    await engine.publishEvent(task.id, { type: 'log', level: 'info', data: { n: 3 } })

    const userEvents = filterUserEvents(await engine.getEvents(task.id))
    const plainEvents = userEvents.filter(e => e.type === 'log')
    const latestEvents = userEvents.filter(e => e.seriesId === 's1')

    // All 3 plain events preserved
    expect(plainEvents).toHaveLength(3)
    expect(plainEvents.map(e => (e.data as { n: number }).n)).toEqual([1, 2, 3])

    // Only the last latest event retained
    expect(latestEvents).toHaveLength(1)
    expect(latestEvents[0]!.data).toEqual({ v: 2 })
  })
})

// ─── seriesMode: keep-all ─────────────────────────────────────────────────

describe('engine.publishEvent — seriesMode: keep-all', () => {
  it('retains every event in history', async () => {
    const { engine } = makeEngine()
    const task = await makeRunningTask(engine)

    for (let i = 1; i <= 5; i++) {
      await engine.publishEvent(task.id, {
        type: 'log',
        level: 'info',
        data: { line: i },
        seriesId: 'logs',
        seriesMode: 'keep-all',
      })
    }

    const seriesEvents = filterUserEvents(await engine.getEvents(task.id))
      .filter(e => e.seriesId === 'logs')
    expect(seriesEvents).toHaveLength(5)
    expect(seriesEvents.map(e => (e.data as { line: number }).line)).toEqual([1, 2, 3, 4, 5])
  })
})

// ─── seriesMode: accumulate ───────────────────────────────────────────────

describe('engine.publishEvent — seriesMode: accumulate', () => {
  it('stores all deltas in history', async () => {
    const { engine } = makeEngine()
    const task = await makeRunningTask(engine)

    await engine.publishEvent(task.id, {
      type: 'stream',
      level: 'info',
      data: { delta: 'Hello' },
      seriesId: 'output',
      seriesMode: 'accumulate',
    })
    await engine.publishEvent(task.id, {
      type: 'stream',
      level: 'info',
      data: { delta: ' world' },
      seriesId: 'output',
      seriesMode: 'accumulate',
    })

    const seriesEvents = filterUserEvents(await engine.getEvents(task.id))
      .filter(e => e.seriesId === 'output')
    // accumulate stores each delta
    expect(seriesEvents).toHaveLength(2)
    expect((seriesEvents[0]!.data as { delta: string }).delta).toBe('Hello')
    expect((seriesEvents[1]!.data as { delta: string }).delta).toBe(' world')
  })

  it('getSeriesLatest returns accumulated value', async () => {
    const { engine } = makeEngine()
    const task = await makeRunningTask(engine)

    await engine.publishEvent(task.id, {
      type: 'stream',
      level: 'info',
      data: { delta: 'Hello' },
      seriesId: 'output',
      seriesMode: 'accumulate',
    })
    await engine.publishEvent(task.id, {
      type: 'stream',
      level: 'info',
      data: { delta: ' world' },
      seriesId: 'output',
      seriesMode: 'accumulate',
    })

    const latest = await engine.getSeriesLatest(task.id, 'output')
    expect(latest).toBeTruthy()
    expect((latest!.data as { delta: string }).delta).toBe('Hello world')
  })
})

// ─── mixed series modes ───────────────────────────────────────────────────

describe('engine.publishEvent — mixed series modes', () => {
  it('different series modes coexist correctly', async () => {
    const { engine } = makeEngine()
    const task = await makeRunningTask(engine)

    // latest series
    for (let i = 1; i <= 3; i++) {
      await engine.publishEvent(task.id, {
        type: 'status',
        level: 'info',
        data: { status: `v${i}` },
        seriesId: 'status',
        seriesMode: 'latest',
      })
    }

    // keep-all series
    for (let i = 1; i <= 3; i++) {
      await engine.publishEvent(task.id, {
        type: 'log',
        level: 'info',
        data: { line: i },
        seriesId: 'logs',
        seriesMode: 'keep-all',
      })
    }

    // accumulate series
    await engine.publishEvent(task.id, {
      type: 'stream',
      level: 'info',
      data: { delta: 'a' },
      seriesId: 'output',
      seriesMode: 'accumulate',
    })
    await engine.publishEvent(task.id, {
      type: 'stream',
      level: 'info',
      data: { delta: 'b' },
      seriesId: 'output',
      seriesMode: 'accumulate',
    })

    // non-series event
    await engine.publishEvent(task.id, {
      type: 'plain',
      level: 'info',
      data: { misc: true },
    })

    const userEvents = filterUserEvents(await engine.getEvents(task.id))

    expect(userEvents.filter(e => e.seriesId === 'status')).toHaveLength(1)
    expect(userEvents.filter(e => e.seriesId === 'logs')).toHaveLength(3)
    expect(userEvents.filter(e => e.seriesId === 'output')).toHaveLength(2)
    expect(userEvents.filter(e => !e.seriesId)).toHaveLength(1)
  })

  it('interleaved series modes produce correct counts and unique indices', async () => {
    const { engine } = makeEngine()
    const task = await makeRunningTask(engine)

    // Round 1
    await engine.publishEvent(task.id, {
      type: 'status', level: 'info', data: { v: 1 },
      seriesId: 'status', seriesMode: 'latest',
    })
    await engine.publishEvent(task.id, {
      type: 'log', level: 'info', data: { line: 1 },
      seriesId: 'logs', seriesMode: 'keep-all',
    })
    await engine.publishEvent(task.id, {
      type: 'stream', level: 'info', data: { delta: 'a' },
      seriesId: 'output', seriesMode: 'accumulate',
    })
    await engine.publishEvent(task.id, {
      type: 'plain', level: 'info', data: { n: 1 },
    })

    // Round 2
    await engine.publishEvent(task.id, {
      type: 'status', level: 'info', data: { v: 2 },
      seriesId: 'status', seriesMode: 'latest',
    })
    await engine.publishEvent(task.id, {
      type: 'log', level: 'info', data: { line: 2 },
      seriesId: 'logs', seriesMode: 'keep-all',
    })
    await engine.publishEvent(task.id, {
      type: 'stream', level: 'info', data: { delta: 'b' },
      seriesId: 'output', seriesMode: 'accumulate',
    })
    await engine.publishEvent(task.id, {
      type: 'plain', level: 'info', data: { n: 2 },
    })

    // Round 3
    await engine.publishEvent(task.id, {
      type: 'status', level: 'info', data: { v: 3 },
      seriesId: 'status', seriesMode: 'latest',
    })
    await engine.publishEvent(task.id, {
      type: 'log', level: 'info', data: { line: 3 },
      seriesId: 'logs', seriesMode: 'keep-all',
    })
    await engine.publishEvent(task.id, {
      type: 'stream', level: 'info', data: { delta: 'c' },
      seriesId: 'output', seriesMode: 'accumulate',
    })

    const userEvents = filterUserEvents(await engine.getEvents(task.id))

    // latest: 1 (only v3), keep-all: 3, accumulate: 3, plain: 2 → total 9
    const latestEvents = userEvents.filter(e => e.seriesId === 'status')
    const keepAllEvents = userEvents.filter(e => e.seriesId === 'logs')
    const accEvents = userEvents.filter(e => e.seriesId === 'output')
    const plainEvents = userEvents.filter(e => !e.seriesId)

    expect(latestEvents).toHaveLength(1)
    expect(latestEvents[0]!.data).toEqual({ v: 3 })
    expect(keepAllEvents).toHaveLength(3)
    expect(accEvents).toHaveLength(3)
    expect(plainEvents).toHaveLength(2)

    // Total event count
    expect(userEvents).toHaveLength(9)

    // All indices unique
    const indices = userEvents.map(e => e.index)
    expect(new Set(indices).size).toBe(indices.length)
  })
})
