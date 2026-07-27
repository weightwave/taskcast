import { describe, expect, it } from 'vitest'
import {
  MemoryBroadcastProvider,
  MemoryLongTermStore,
  MemoryShortTermStore,
  TaskEngine,
} from '../../src/index.js'

describe('TaskEngine hot storage release', () => {
  it('archives a live task through its fenced watermark before deleting hot state', async () => {
    const hot = new MemoryShortTermStore()
    const durable = new MemoryLongTermStore()
    const engine = new TaskEngine({
      shortTermStore: hot,
      longTermStore: durable,
      broadcast: new MemoryBroadcastProvider(),
    })
    await engine.createTask({ id: 'task-1' })
    await engine.transitionTask('task-1', 'running')
    const event = await engine.publishEvent('task-1', {
      type: 'llm.delta',
      level: 'info',
      data: { delta: 'hello' },
    })

    const released = await engine.releaseTaskStorage('task-1', {
      expectedLastEventIndex: event.index,
      inactiveSince: event.timestamp,
    })

    expect(released).toMatchObject({
      taskId: 'task-1',
      storageState: 'cold',
      archiveWatermark: event.index,
      released: true,
    })
    await expect(hot.getTaskStoragePresence('task-1')).resolves.toMatchObject({
      task: false,
      eventCount: 0,
      writeFence: false,
    })
    await expect(durable.getArchiveWatermark('task-1')).resolves.toBe(event.index)
    await expect(durable.getEvents('task-1')).resolves.toHaveLength(event.index + 1)
  })
})
