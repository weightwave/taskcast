import { describe, expect, it } from 'vitest'
import {
  MemoryBroadcastProvider,
  MemoryLongTermStore,
  MemoryShortTermStore,
  TaskEngine,
} from '../../src/index.js'

async function waitForDurableIndex(
  store: MemoryLongTermStore,
  taskId: string,
  expected: number,
): Promise<void> {
  for (let attempt = 0; attempt < 100; attempt++) {
    if (await store.getLastEventIndex(taskId) === expected) return
    await new Promise((resolve) => setTimeout(resolve, 1))
  }
  throw new Error(`durable event index did not reach ${expected}`)
}

describe('01KRK8Y78MA3SV416YNAV3E3KJ hot/cold regression', () => {
  it('releases a pending no-TTL retry storm without truncating history and rehydrates on a later write', async () => {
    const hot = new MemoryShortTermStore()
    const durable = new MemoryLongTermStore()
    const engine = new TaskEngine({
      shortTermStore: hot,
      longTermStore: durable,
      broadcast: new MemoryBroadcastProvider(),
      rehydrateReplayEvents: 1_000,
    })
    const taskId = '01KRK8Y78MA3SV416YNAV3E3KJ'
    const task = await engine.createTask({
      id: taskId,
      type: 'agent.session',
      metadata: { fixture: 'reduced-production-incident' },
    })
    expect(task.status).toBe('pending')
    expect('ttl' in task).toBe(false)

    let lastIndex = -1
    let lastTimestamp = task.createdAt
    for (let cycle = 0; cycle < 2_500; cycle++) {
      const event = await engine.publishEvent(taskId, {
        type: cycle % 5 === 0 ? 'agent.retry' : 'agent.message_update',
        level: cycle % 17 === 0 ? 'warn' : 'info',
        data: { cycle, retry: cycle % 5 === 0 },
      })
      lastIndex = event.index
      lastTimestamp = event.timestamp
    }
    for (let chunk = 0; chunk < 40; chunk++) {
      const event = await engine.publishEvent(taskId, {
        type: 'agent.output',
        level: 'info',
        data: { delta: String(chunk % 10) },
        seriesId: 'assistant-output',
        seriesMode: 'accumulate',
        seriesAccField: 'delta',
      })
      lastIndex = event.index
      lastTimestamp = event.timestamp
    }
    for (let progress = 0; progress < 10; progress++) {
      const event = await engine.publishEvent(taskId, {
        type: 'agent.progress',
        level: 'info',
        data: { progress },
        seriesId: 'progress',
        seriesMode: 'latest',
      })
      lastIndex = event.index
      lastTimestamp = event.timestamp
    }
    await waitForDurableIndex(durable, taskId, lastIndex)

    const historyBeforeRelease = await engine.getEvents(taskId)
    expect(historyBeforeRelease[0]?.index).toBe(0)
    expect(historyBeforeRelease.at(-1)?.index).toBe(lastIndex)
    expect(historyBeforeRelease.filter((event) =>
      event.type === 'agent.message_update'
    ).length).toBe(2_000)

    await expect(engine.releaseTaskStorage(taskId, {
      expectedLastEventIndex: lastIndex,
      inactiveSince: lastTimestamp,
    })).resolves.toMatchObject({
      storageState: 'cold',
      archiveWatermark: lastIndex,
      released: true,
    })
    await expect(hot.getTaskStoragePresence(taskId)).resolves.toEqual({
      task: false,
      eventCount: 0,
      nextIndex: false,
      seriesStateCount: 0,
      writeFence: false,
    })
    await expect(durable.getTaskStorageMetadata(taskId)).resolves.toMatchObject({
      storageState: 'cold',
      archiveWatermark: lastIndex,
    })
    await expect(engine.getEvents(taskId)).resolves.toEqual(historyBeforeRelease)

    const late = await engine.publishEvent(taskId, {
      type: 'agent.owner_reacquired',
      level: 'info',
      data: { reason: 'manual-resume' },
    })
    expect(late.index).toBe(lastIndex + 1)
    await waitForDurableIndex(durable, taskId, late.index)
    await expect(hot.getTaskStoragePresence(taskId)).resolves.toMatchObject({
      task: true,
      eventCount: 1_001,
      nextIndex: true,
      writeFence: true,
    })
    const historyAfterRehydrate = await engine.getEvents(taskId)
    expect(historyAfterRehydrate[0]?.index).toBe(0)
    expect(historyAfterRehydrate.at(-1)).toMatchObject({
      id: late.id,
      index: late.index,
    })
    expect(historyAfterRehydrate.length).toBe(historyBeforeRelease.length + 1)
  }, 60_000)
})
