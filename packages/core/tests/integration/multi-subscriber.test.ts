import { describe, it, expect } from 'vitest'
import { TaskEngine } from '../../src/engine.js'
import { MemoryBroadcastProvider, MemoryShortTermStore } from '../../src/memory-adapters.js'
import type { TaskEvent } from '../../src/types.js'

function makeEngine() {
  const store = new MemoryShortTermStore()
  const broadcast = new MemoryBroadcastProvider()
  const engine = new TaskEngine({ shortTermStore: store, broadcast })
  return { engine, store, broadcast }
}

describe('Core integration — multi-subscriber', () => {
  it('5 subscribers receive events independently', async () => {
    const { engine } = makeEngine()
    const task = await engine.createTask({})
    await engine.transitionTask(task.id, 'running')

    const received: TaskEvent[][] = Array.from({ length: 5 }, () => [])
    const unsubs = received.map((arr) =>
      engine.subscribe(task.id, (evt) => arr.push(evt))
    )

    await engine.publishEvent(task.id, { type: 'llm.delta', level: 'info', data: { n: 1 } })
    await engine.publishEvent(task.id, { type: 'tool.call', level: 'info', data: { n: 2 } })
    await engine.publishEvent(task.id, { type: 'llm.delta', level: 'info', data: { n: 3 } })

    // All 5 subscribers receive all 3 events
    for (const arr of received) {
      expect(arr).toHaveLength(3)
    }

    unsubs.forEach(fn => fn())
  })

  it('unsubscribed client stops receiving', async () => {
    const { engine } = makeEngine()
    const task = await engine.createTask({})
    await engine.transitionTask(task.id, 'running')

    const received1: TaskEvent[] = []
    const received2: TaskEvent[] = []

    const unsub1 = engine.subscribe(task.id, (evt) => received1.push(evt))
    engine.subscribe(task.id, (evt) => received2.push(evt))

    await engine.publishEvent(task.id, { type: 'first', level: 'info', data: null })

    // Unsubscribe client 1
    unsub1()

    await engine.publishEvent(task.id, { type: 'second', level: 'info', data: null })

    expect(received1).toHaveLength(1)
    expect(received2).toHaveLength(2)
  })

  it('late subscriber only gets events after subscription', async () => {
    const { engine } = makeEngine()
    const task = await engine.createTask({})
    await engine.transitionTask(task.id, 'running')

    // Publish before subscribing
    await engine.publishEvent(task.id, { type: 'before', level: 'info', data: null })

    const received: TaskEvent[] = []
    const unsub = engine.subscribe(task.id, (evt) => received.push(evt))

    await engine.publishEvent(task.id, { type: 'after', level: 'info', data: null })

    expect(received).toHaveLength(1)
    expect(received[0]!.type).toBe('after')

    // But history has both
    const history = await engine.getEvents(task.id)
    const userEvents = history.filter(e => !e.type.startsWith('taskcast:'))
    expect(userEvents).toHaveLength(2)

    unsub()
  })
})
