/**
 * Regression test for concurrent event publishing ordering.
 *
 * When multiple events are published to the same task concurrently, they
 * must be stored in the same order as their atomically-assigned indices.
 * Without per-task serialization in `_emit`, async scheduling can cause
 * `appendEvent` calls to interleave, producing storage order that differs
 * from index order.
 */
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

async function createRunningTask(engine: TaskEngine, taskId: string) {
  await engine.createTask({ id: taskId })
  await engine.transitionTask(taskId, 'running')
}

function filterUserEvents(events: TaskEvent[]): TaskEvent[] {
  return events.filter((e) => !e.type.startsWith('taskcast:'))
}

describe('concurrent publish ordering', () => {
  it('concurrent publishes to the same task preserve index order', async () => {
    const { engine } = makeEngine()
    await createRunningTask(engine, 't1')

    const n = 20
    const promises = Array.from({ length: n }, (_, i) =>
      engine.publishEvent('t1', {
        type: `event.${i}`,
        level: 'info',
        data: { order: i },
      }),
    )

    await Promise.all(promises)

    const events = await engine.getEvents('t1')
    const userEvents = filterUserEvents(events)
    expect(userEvents).toHaveLength(n)

    // Verify events are stored in ascending index order
    for (let i = 0; i < userEvents.length - 1; i++) {
      expect(userEvents[i]!.index).toBeLessThan(userEvents[i + 1]!.index)
    }
  })

  it('concurrent latest-series and plain events preserve relative order', async () => {
    const { engine } = makeEngine()
    await createRunningTask(engine, 't1')

    // Simulate: message_updates (latest), message_end, turn_end — all concurrent
    const promises: Promise<TaskEvent>[] = []

    for (let i = 0; i < 5; i++) {
      promises.push(
        engine.publishEvent('t1', {
          type: 'message_update',
          level: 'info',
          data: { content: `v${i}` },
          seriesId: 'msg_content',
          seriesMode: 'latest',
        }),
      )
    }

    promises.push(
      engine.publishEvent('t1', {
        type: 'message_end',
        level: 'info',
        data: { done: true },
      }),
    )

    promises.push(
      engine.publishEvent('t1', {
        type: 'turn_end',
        level: 'info',
        data: { turn: 0 },
      }),
    )

    await Promise.all(promises)

    const events = await engine.getEvents('t1')
    const userEvents = filterUserEvents(events)

    // With the per-task lock, all events are stored in index order.
    for (let i = 0; i < userEvents.length - 1; i++) {
      const a = userEvents[i]!
      const b = userEvents[i + 1]!
      expect(a.index).toBeLessThan(b.index)
    }
  })

  it('concurrent publishes to different tasks are independent', async () => {
    const { engine } = makeEngine()
    await createRunningTask(engine, 't1')
    await createRunningTask(engine, 't2')

    const promises = Array.from({ length: 10 }, (_, i) => {
      const taskId = i % 2 === 0 ? 't1' : 't2'
      return engine.publishEvent(taskId, {
        type: `event.${i}`,
        level: 'info',
        data: { i },
      })
    })

    await Promise.all(promises)

    // Both tasks should have their events in index order
    for (const tid of ['t1', 't2']) {
      const events = await engine.getEvents(tid)
      const userEvents = filterUserEvents(events)
      for (let i = 0; i < userEvents.length - 1; i++) {
        expect(userEvents[i]!.index).toBeLessThan(userEvents[i + 1]!.index)
      }
    }
  })
})
