import { describe, it, expect, vi } from 'vitest'
import { TaskEngine } from '../../src/engine.js'
import { MemoryBroadcastProvider, MemoryShortTermStore } from '../../src/memory-adapters.js'
import type { LongTermStore, TaskEvent, TaskcastHooks } from '../../src/types.js'

function makeEngine(opts?: { hooks?: TaskcastHooks; longTermStore?: LongTermStore }) {
  const store = new MemoryShortTermStore()
  const broadcast = new MemoryBroadcastProvider()
  const engine = new TaskEngine({
    shortTermStore: store,
    broadcast,
    ...opts,
  })
  return { engine, store, broadcast }
}

describe('Core integration — full lifecycle', () => {
  it('pending -> running -> completed with hooks in order', async () => {
    const hookOrder: string[] = []
    const hooks: TaskcastHooks = {
      onTaskCreated: () => hookOrder.push('created'),
      onTaskTransitioned: (_task, _from, to) => hookOrder.push(`transitioned:${to}`),
    }
    const { engine } = makeEngine({ hooks })

    const task = await engine.createTask({ type: 'test' })
    expect(task.status).toBe('pending')

    const running = await engine.transitionTask(task.id, 'running')
    expect(running.status).toBe('running')

    await engine.publishEvent(task.id, { type: 'chunk', level: 'info', data: { text: 'hi' } })

    const completed = await engine.transitionTask(task.id, 'completed', {
      result: { answer: 42 },
    })
    expect(completed.status).toBe('completed')
    expect(completed.completedAt).toBeTruthy()
    expect(completed.result).toEqual({ answer: 42 })

    expect(hookOrder).toEqual(['created', 'transitioned:running', 'transitioned:completed'])
  })

  it('series accumulate across lifecycle', async () => {
    const { engine, store } = makeEngine()
    const task = await engine.createTask({})
    await engine.transitionTask(task.id, 'running')

    for (let i = 0; i < 10; i++) {
      await engine.publishEvent(task.id, {
        type: 'token',
        level: 'info',
        data: { text: `word${i} ` },
        seriesId: 'output',
        seriesMode: 'accumulate',
        seriesAccField: 'text',
      })
    }

    // All 10 events are stored in history
    const events = await engine.getEvents(task.id)
    const seriesEvents = events.filter(e => e.seriesId === 'output')
    expect(seriesEvents).toHaveLength(10)

    // But the series latest has the accumulated text
    const latest = await store.getSeriesLatest(task.id, 'output')
    expect(latest).toBeTruthy()
    const text = (latest!.data as { text: string }).text
    for (let i = 0; i < 10; i++) {
      expect(text).toContain(`word${i}`)
    }
  })

  it('result/error persistence through transitions', async () => {
    const { engine } = makeEngine()
    const task = await engine.createTask({})
    await engine.transitionTask(task.id, 'running')
    const failed = await engine.transitionTask(task.id, 'failed', {
      error: { code: 'E001', message: 'boom', details: { stack: 'trace' } },
    })

    expect(failed.error).toEqual({ code: 'E001', message: 'boom', details: { stack: 'trace' } })

    const fetched = await engine.getTask(task.id)
    expect(fetched!.error).toEqual(failed.error)
  })

  it('LongTermStore receives async writes', async () => {
    const longTermEvents: TaskEvent[] = []
    const longTermStore: LongTermStore = {
      saveTask: vi.fn().mockResolvedValue(undefined),
      getTask: vi.fn().mockResolvedValue(null),
      saveEvent: vi.fn().mockImplementation(async (e: TaskEvent) => { longTermEvents.push(e) }),
      getEvents: vi.fn().mockResolvedValue([]),
      saveWorkerEvent: vi.fn().mockResolvedValue(undefined),
      getWorkerEvents: vi.fn().mockResolvedValue([]),
    }

    const { engine } = makeEngine({ longTermStore })
    const task = await engine.createTask({})
    await engine.transitionTask(task.id, 'running')
    await engine.publishEvent(task.id, { type: 'evt', level: 'info', data: null })

    // Allow async longTermStore.saveEvent to complete
    await vi.waitFor(() => {
      expect(longTermEvents.length).toBeGreaterThan(0)
    })
  })
})
