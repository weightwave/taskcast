import { describe, expect, it } from 'vitest'
import {
  MemoryBroadcastProvider,
  MemoryLongTermStore,
  MemoryShortTermStore,
  StorageCoordinator,
  TaskEngine,
  type HotWriteToken,
  type LongTermStore,
  type Task,
  type TaskEvent,
} from '../../src/index.js'

const task: Task = {
  id: 'task-1',
  status: 'running',
  createdAt: 1_000,
  updatedAt: 1_000,
}

const event = (
  index: number,
  overrides: Partial<TaskEvent> = {},
): TaskEvent => ({
  id: `event-${index}`,
  taskId: task.id,
  index,
  timestamp: 2_000 + index,
  type: 'message',
  level: 'info',
  data: { index },
  ...overrides,
})

async function makeHistoryEngine(): Promise<{
  hot: MemoryShortTermStore
  durable: MemoryLongTermStore
  engine: TaskEngine
}> {
  const hot = new MemoryShortTermStore()
  const durable = new MemoryLongTermStore()
  await hot.saveTask(task)
  await durable.saveTask(task)
  for (let index = 0; index < 10; index++) {
    await durable.saveEvent(event(index))
  }
  for (let index = 8; index < 11; index++) {
    await hot.appendEvent(task.id, event(index))
  }
  return {
    hot,
    durable,
    engine: new TaskEngine({
      shortTermStore: hot,
      longTermStore: durable,
      broadcast: new MemoryBroadcastProvider(),
    }),
  }
}

describe('TaskEngine canonical hot/cold history', () => {
  it('does not let a non-empty hot replay window hide durable history', async () => {
    const { engine } = await makeHistoryEngine()

    await expect(engine.getEvents(task.id)).resolves.toEqual(
      Array.from({ length: 11 }, (_, index) => event(index)),
    )
    await expect(
      engine.getEvents(task.id, { since: { index: 7 }, limit: 2 }),
    ).resolves.toEqual([event(8), event(9)])
    await expect(
      engine.getEvents(task.id, { since: { id: 'event-9' }, limit: 2 }),
    ).resolves.toEqual([event(10)])
  })

  it('pages past a compacted durable row before proving a bounded result', async () => {
    const compactedRow = event(0, {
      id: 'acc-first',
      seriesId: 'output',
      seriesMode: 'accumulate',
      seriesAccField: 'delta',
      data: { delta: 'ABC' },
    })
    const snapshot = event(100, {
      id: 'acc-snapshot',
      seriesId: 'output',
      seriesMode: 'accumulate',
      seriesAccField: 'delta',
      data: { delta: 'ABC' },
    })
    const rows = [compactedRow, event(1), event(2)]
    let pageReads = 0
    const durable: LongTermStore = {
      supportsHotColdRelease: true,
      async saveTask() {},
      async getTask() { return task },
      async saveEvent() {},
      async getEvents(_taskId, opts) {
        pageReads++
        let result = rows
        if (opts?.since?.index !== undefined) {
          result = result.filter(({ index }) => index > opts.since!.index!)
        }
        return result.slice(0, opts?.limit)
      },
      async getTaskStorageMetadata() {
        return {
          taskId: task.id,
          storageState: 'hot',
          storageEpoch: 1,
          activeReleaseGeneration: null,
          archiveWatermark: -1,
          lastEventAt: null,
          coldAt: null,
          executionDeadlineAt: null,
          taskVersion: 0,
        }
      },
      async getDurableSeriesState() {
        return [{
          taskId: task.id,
          seriesId: 'output',
          mode: 'accumulate',
          event: snapshot,
          throughIndex: 100,
        }]
      },
      async saveWorkerEvent() {},
      async getWorkerEvents() { return [] },
    }
    const engine = new TaskEngine({
      shortTermStore: new MemoryShortTermStore(),
      longTermStore: durable,
      broadcast: new MemoryBroadcastProvider(),
    })

    await expect(engine.getEvents(task.id, { limit: 2 })).resolves.toEqual([
      event(1),
      event(2),
    ])
    expect(pageReads).toBe(2)
  })

  it('surfaces conflicting hot overlap as durable integrity failure', async () => {
    const { hot, engine } = await makeHistoryEngine()
    await hot.replaceLastSeriesEvent(
      task.id,
      'conflict-fixture',
      event(9, {
        id: 'conflicting-event',
        seriesId: 'conflict-fixture',
        seriesMode: 'latest',
      }),
    )

    await expect(engine.getEvents(task.id)).rejects.toMatchObject({
      code: 'storage_integrity_error',
    })
  })

  it('serves cold history without repopulating hot storage', async () => {
    const { hot, durable } = await makeHistoryEngine()
    const coordinator = new StorageCoordinator({
      shortTermStore: hot,
      longTermStore: durable,
    })
    await coordinator.releaseTaskStorage(task.id, {
      expectedLastEventIndex: 10,
      inactiveSince: 3_000,
    })
    const engine = new TaskEngine({
      shortTermStore: hot,
      longTermStore: durable,
      broadcast: new MemoryBroadcastProvider(),
    })

    await expect(engine.getEvents(task.id, { limit: 2 })).resolves.toEqual([
      event(0),
      event(1),
    ])
    await expect(hot.getTaskStoragePresence(task.id)).resolves.toMatchObject({
      task: false,
      eventCount: 0,
      writeFence: false,
    })
  })

  it('returns identical accumulated state before and after release', async () => {
    const hot = new MemoryShortTermStore()
    const durable = new MemoryLongTermStore()
    await hot.saveTask(task)
    await durable.saveTask(task)
    const token: HotWriteToken = { taskId: task.id, storageEpoch: 1 }
    for (const [index, delta] of ['A', 'B', 'C'].entries()) {
      const raw = event(index, {
        id: `delta-${index}`,
        type: 'llm.delta',
        seriesId: 'output',
        seriesMode: 'accumulate',
        seriesAccField: 'delta',
        data: { delta },
      })
      const committed = await hot.commitEventFenced(task.id, raw, token)
      if (index < 2) {
        await durable.accumulateSeries(
          task.id,
          'output',
          committed.event,
          'delta',
        )
      }
    }
    const engine = new TaskEngine({
      shortTermStore: hot,
      longTermStore: durable,
      broadcast: new MemoryBroadcastProvider(),
    })
    const hotLatest = await engine.getSeriesLatest(task.id, 'output')
    expect(hotLatest?.data).toEqual({ delta: 'ABC' })

    await engine.releaseTaskStorage(task.id, {
      expectedLastEventIndex: 2,
      inactiveSince: 3_000,
    })
    await expect(engine.getSeriesLatest(task.id, 'output')).resolves.toEqual(
      hotLatest,
    )
  })
})
