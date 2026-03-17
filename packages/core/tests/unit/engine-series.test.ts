import { describe, it, expect, vi } from 'vitest'
import { TaskEngine } from '../../src/engine.js'
import { MemoryBroadcastProvider, MemoryShortTermStore } from '../../src/memory-adapters.js'
import type { LongTermStore, TaskEvent } from '../../src/types.js'

// ─── Helpers ─────────────────────────────────────────────────────────────────

function makeEngine(opts?: { longTermStore?: LongTermStore }) {
  const shortTermStore = new MemoryShortTermStore()
  const broadcast = new MemoryBroadcastProvider()
  const engine = new TaskEngine({
    shortTermStore,
    broadcast,
    longTermStore: opts?.longTermStore,
  })
  return { engine, shortTermStore, broadcast }
}

async function createRunningTask(engine: TaskEngine, taskId: string) {
  await engine.createTask({ id: taskId })
  await engine.transitionTask(taskId, 'running')
}

/** Filter out internal taskcast:status events, keep only user-published events */
function userEvents(events: TaskEvent[]): TaskEvent[] {
  return events.filter((e) => !e.type.startsWith('taskcast:'))
}

// ─── latest mode ─────────────────────────────────────────────────────────────

describe('engine series: latest mode', () => {
  it('keeps only the latest event after 5 publishes', async () => {
    const { engine, shortTermStore } = makeEngine()
    await createRunningTask(engine, 't1')

    for (let i = 0; i < 5; i++) {
      await engine.publishEvent('t1', {
        type: 'progress',
        level: 'info',
        data: { value: i },
        seriesId: 'pct',
        seriesMode: 'latest',
      })
    }

    const events = await shortTermStore.getEvents('t1')
    const user = userEvents(events)
    expect(user).toHaveLength(1)
    expect(user[0]!.data).toEqual({ value: 4 })
  })

  it('first event is stored exactly once (not duplicated)', async () => {
    const { engine, shortTermStore } = makeEngine()
    await createRunningTask(engine, 't1')

    await engine.publishEvent('t1', {
      type: 'progress',
      level: 'info',
      data: { value: 'first' },
      seriesId: 'pct',
      seriesMode: 'latest',
    })

    const events = await shortTermStore.getEvents('t1')
    const user = userEvents(events)
    expect(user).toHaveLength(1)
    expect(user[0]!.data).toEqual({ value: 'first' })
  })

  it('multiple series each deduplicated independently', async () => {
    const { engine, shortTermStore } = makeEngine()
    await createRunningTask(engine, 't1')

    // Publish 3 events to series A
    for (let i = 0; i < 3; i++) {
      await engine.publishEvent('t1', {
        type: 'progress',
        level: 'info',
        data: { series: 'A', value: i },
        seriesId: 'sA',
        seriesMode: 'latest',
      })
    }

    // Publish 3 events to series B
    for (let i = 0; i < 3; i++) {
      await engine.publishEvent('t1', {
        type: 'progress',
        level: 'info',
        data: { series: 'B', value: i },
        seriesId: 'sB',
        seriesMode: 'latest',
      })
    }

    const events = await shortTermStore.getEvents('t1')
    const user = userEvents(events)
    expect(user).toHaveLength(2)

    const seriesA = user.find((e) => (e.data as Record<string, unknown>).series === 'A')
    const seriesB = user.find((e) => (e.data as Record<string, unknown>).series === 'B')
    expect(seriesA!.data).toEqual({ series: 'A', value: 2 })
    expect(seriesB!.data).toEqual({ series: 'B', value: 2 })
  })

  it('broadcast fires for every published event', async () => {
    const { engine, broadcast } = makeEngine()
    await createRunningTask(engine, 't1')

    const received: TaskEvent[] = []
    broadcast.subscribe('t1', (e) => received.push(e))

    for (let i = 0; i < 5; i++) {
      await engine.publishEvent('t1', {
        type: 'progress',
        level: 'info',
        data: { value: i },
        seriesId: 'pct',
        seriesMode: 'latest',
      })
    }

    // Broadcast should have all 5 events (plus the 1 taskcast:status from running transition)
    const userBroadcasts = userEvents(received)
    expect(userBroadcasts).toHaveLength(5)
  })

  it('longTermStore receives events', async () => {
    const savedEvents: TaskEvent[] = []
    const longTermStore: LongTermStore = {
      saveTask: vi.fn().mockResolvedValue(undefined),
      getTask: vi.fn().mockResolvedValue(null),
      saveEvent: vi.fn().mockImplementation(async (event: TaskEvent) => {
        savedEvents.push(event)
      }),
      getEvents: vi.fn().mockResolvedValue([]),
      saveWorkerEvent: vi.fn().mockResolvedValue(undefined),
      getWorkerEvents: vi.fn().mockResolvedValue([]),
    }

    const { engine } = makeEngine({ longTermStore })
    await createRunningTask(engine, 't1')

    for (let i = 0; i < 3; i++) {
      await engine.publishEvent('t1', {
        type: 'progress',
        level: 'info',
        data: { value: i },
        seriesId: 'pct',
        seriesMode: 'latest',
      })
    }

    // Wait for async long-term store saves
    await new Promise((resolve) => setTimeout(resolve, 50))

    const userSaved = userEvents(savedEvents)
    expect(userSaved).toHaveLength(3)
  })
})

// ─── keep-all mode ───────────────────────────────────────────────────────────

describe('engine series: keep-all mode', () => {
  it('retains all 5 events', async () => {
    const { engine, shortTermStore } = makeEngine()
    await createRunningTask(engine, 't1')

    for (let i = 0; i < 5; i++) {
      await engine.publishEvent('t1', {
        type: 'progress',
        level: 'info',
        data: { value: i },
        seriesId: 'log',
        seriesMode: 'keep-all',
      })
    }

    const events = await shortTermStore.getEvents('t1')
    const user = userEvents(events)
    expect(user).toHaveLength(5)
    for (let i = 0; i < 5; i++) {
      expect(user[i]!.data).toEqual({ value: i })
    }
  })
})

// ─── accumulate mode ─────────────────────────────────────────────────────────

describe('engine series: accumulate mode', () => {
  it('stores all deltas and getSeriesLatest returns accumulated', async () => {
    const { engine, shortTermStore } = makeEngine()
    await createRunningTask(engine, 't1')

    await engine.publishEvent('t1', {
      type: 'progress',
      level: 'info',
      data: { delta: 'hello' },
      seriesId: 'text',
      seriesMode: 'accumulate',
    })
    await engine.publishEvent('t1', {
      type: 'progress',
      level: 'info',
      data: { delta: ' world' },
      seriesId: 'text',
      seriesMode: 'accumulate',
    })

    // All deltas stored
    const events = await shortTermStore.getEvents('t1')
    const user = userEvents(events)
    expect(user).toHaveLength(2)
    expect(user[0]!.data).toEqual({ delta: 'hello' })
    expect(user[1]!.data).toEqual({ delta: ' world' })

    // Series latest is the accumulated value
    const latest = await engine.getSeriesLatest('t1', 'text')
    expect(latest).not.toBeNull()
    expect((latest!.data as Record<string, unknown>).delta).toBe('hello world')
  })
})

// ─── mixed modes ─────────────────────────────────────────────────────────────

describe('engine series: mixed modes', () => {
  it('all 3 modes coexist correctly on the same task', async () => {
    const { engine, shortTermStore } = makeEngine()
    await createRunningTask(engine, 't1')

    // latest: 3 publishes → 1 stored event
    for (let i = 0; i < 3; i++) {
      await engine.publishEvent('t1', {
        type: 'metric',
        level: 'info',
        data: { value: i },
        seriesId: 'metric',
        seriesMode: 'latest',
      })
    }

    // keep-all: 3 publishes → 3 stored events
    for (let i = 0; i < 3; i++) {
      await engine.publishEvent('t1', {
        type: 'log',
        level: 'info',
        data: { msg: `log-${i}` },
        seriesId: 'logs',
        seriesMode: 'keep-all',
      })
    }

    // accumulate: 3 publishes → 3 stored deltas
    for (let i = 0; i < 3; i++) {
      await engine.publishEvent('t1', {
        type: 'text',
        level: 'info',
        data: { delta: String.fromCharCode(97 + i) },
        seriesId: 'text',
        seriesMode: 'accumulate',
      })
    }

    const events = await shortTermStore.getEvents('t1')
    const user = userEvents(events)

    // 1 (latest) + 3 (keep-all) + 3 (accumulate) = 7
    expect(user).toHaveLength(7)

    // Verify latest kept only last value
    const metricEvents = user.filter((e) => e.type === 'metric')
    expect(metricEvents).toHaveLength(1)
    expect(metricEvents[0]!.data).toEqual({ value: 2 })

    // Verify keep-all retained all
    const logEvents = user.filter((e) => e.type === 'log')
    expect(logEvents).toHaveLength(3)

    // Verify accumulate stored all deltas
    const textEvents = user.filter((e) => e.type === 'text')
    expect(textEvents).toHaveLength(3)

    // Verify accumulated series latest
    const latest = await engine.getSeriesLatest('t1', 'text')
    expect((latest!.data as Record<string, unknown>).delta).toBe('abc')
  })
})
