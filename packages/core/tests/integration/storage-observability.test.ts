import { describe, expect, it } from 'vitest'
import {
  MemoryBroadcastProvider,
  MemoryLongTermStore,
  MemoryShortTermStore,
  StoragePreconditionError,
  TaskEngine,
} from '../../src/index.js'

describe('hot-cold storage observability', () => {
  it('reports release, durable history, and bounded rehydration without payloads', async () => {
    const hot = new MemoryShortTermStore()
    const durable = new MemoryLongTermStore()
    const engine = new TaskEngine({
      shortTermStore: hot,
      longTermStore: durable,
      broadcast: new MemoryBroadcastProvider(),
    })
    const observations: Array<Record<string, unknown>> = []
    engine.addStorageLifecycleListener((observation) => {
      observations.push(observation)
    })
    engine.addStorageLifecycleListener(() => {
      throw new Error('observer failure must be isolated')
    })

    await engine.createTask({ id: 'observed-task' })
    await engine.transitionTask('observed-task', 'running')
    const event = await engine.publishEvent('observed-task', {
      type: 'llm.delta',
      level: 'info',
      data: { secretPayload: 'must-not-be-logged' },
    })
    await engine.releaseTaskStorage('observed-task', {
      expectedLastEventIndex: event.index,
      inactiveSince: event.timestamp,
    })
    await engine.getEvents('observed-task')
    await engine.publishEvent('observed-task', {
      type: 'owner.reacquired',
      level: 'info',
      data: { secretPayload: 'still-must-not-be-logged' },
    })

    expect(observations).toEqual(expect.arrayContaining([
      expect.objectContaining({
        event: 'storage_release',
        taskId: 'observed-task',
        outcome: 'released',
        sourceEventCount: event.index + 1,
        storageStateBefore: 'hot',
        storageStateAfter: 'cold',
        archiveWatermark: event.index,
      }),
      expect.objectContaining({
        event: 'storage_history_read',
        taskId: 'observed-task',
        outcome: 'success',
        source: 'durable',
        eventCount: event.index + 1,
      }),
      expect.objectContaining({
        event: 'storage_rehydrate',
        taskId: 'observed-task',
        outcome: 'rehydrated',
        replayEventCount: event.index + 1,
        archiveWatermark: event.index,
        storageStateBefore: 'cold',
        storageStateAfter: 'hot',
      }),
    ]))
    const encoded = JSON.stringify(observations)
    expect(encoded).not.toContain('must-not-be-logged')
    expect(encoded).not.toContain('"data"')
  })

  it('reports release precondition conflicts and supports listener removal', async () => {
    const hot = new MemoryShortTermStore()
    const durable = new MemoryLongTermStore()
    const engine = new TaskEngine({
      shortTermStore: hot,
      longTermStore: durable,
      broadcast: new MemoryBroadcastProvider(),
    })
    const observations: Array<Record<string, unknown>> = []
    const remove = engine.addStorageLifecycleListener((observation) => {
      observations.push(observation)
    })

    await engine.createTask({ id: 'conflict-task' })
    await expect(engine.releaseTaskStorage('conflict-task', {
      expectedLastEventIndex: 99,
      inactiveSince: Date.now(),
    })).rejects.toBeInstanceOf(StoragePreconditionError)

    expect(observations).toContainEqual(expect.objectContaining({
      event: 'storage_release',
      taskId: 'conflict-task',
      outcome: 'failed',
      errorCode: 'storage_precondition_failed',
    }))
    remove()
    const count = observations.length
    await engine.getEvents('conflict-task')
    expect(observations).toHaveLength(count)
  })
})
