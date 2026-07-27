import { describe, expect, it } from 'vitest'
import {
  MemoryBroadcastProvider,
  MemoryLongTermStore,
  MemoryShortTermStore,
  StorageCoordinator,
  TaskEngine,
  type HotWriteToken,
  type Task,
  type TaskEvent,
} from '../../src/index.js'

const makeTask = (id = 'task-1'): Task => ({
  id,
  status: 'running',
  createdAt: 1_000,
  updatedAt: 1_000,
})

const makeEvent = (
  index: number,
  overrides: Partial<Omit<TaskEvent, 'index'>> = {},
): Omit<TaskEvent, 'index'> => ({
  id: `event-${index}`,
  taskId: 'task-1',
  timestamp: 2_000 + index,
  type: 'llm.delta',
  level: 'info',
  data: { delta: String(index) },
  ...overrides,
})

async function seedAndRelease(eventCount: number): Promise<{
  hot: MemoryShortTermStore
  durable: MemoryLongTermStore
  coordinator: StorageCoordinator
}> {
  const hot = new MemoryShortTermStore()
  const durable = new MemoryLongTermStore()
  const task = makeTask()
  await hot.saveTask(task)
  await durable.saveTask(task)
  const token: HotWriteToken = { taskId: task.id, storageEpoch: 1 }
  for (let index = 0; index < eventCount; index++) {
    const committed = await hot.commitEventFenced(task.id, makeEvent(index), token)
    await durable.saveEvent(committed.event)
  }
  const coordinator = new StorageCoordinator({
    shortTermStore: hot,
    longTermStore: durable,
    storageLockTtlMs: 5_000,
  })
  await coordinator.releaseTaskStorage(task.id, {
    expectedLastEventIndex: eventCount - 1,
    inactiveSince: eventCount === 0 ? 1_000 : 2_000 + eventCount,
  })
  return { hot, durable, coordinator }
}

describe('cold task storage rehydration', () => {
  it('keeps reads cold and restores only a bounded replay window before a write', async () => {
    const { hot, durable } = await seedAndRelease(1_005)
    const engine = new TaskEngine({
      shortTermStore: hot,
      longTermStore: durable,
      broadcast: new MemoryBroadcastProvider(),
    })

    await expect(engine.getTask('task-1')).resolves.toMatchObject({ id: 'task-1' })
    await expect(hot.getTaskStoragePresence('task-1')).resolves.toMatchObject({
      task: false,
      eventCount: 0,
      writeFence: false,
    })
    await expect(durable.getTaskStorageMetadata('task-1')).resolves.toMatchObject({
      storageState: 'cold',
      storageEpoch: 1,
    })

    const published = await engine.publishEvent('task-1', {
      type: 'late.event',
      level: 'info',
      data: { late: true },
    })
    expect(published.index).toBe(1_005)
    await expect(hot.getTaskStoragePresence('task-1')).resolves.toMatchObject({
      task: true,
      eventCount: 1_001,
      writeFence: true,
    })
    await expect(durable.getTaskStorageMetadata('task-1')).resolves.toMatchObject({
      storageState: 'hot',
      storageEpoch: 2,
      coldAt: null,
    })
    await expect(
      hot.commitEventFenced('task-1', makeEvent(1_006), {
        taskId: 'task-1',
        storageEpoch: 1,
      }),
    ).rejects.toMatchObject({ code: 'storage_fence_conflict' })
  })

  it('continues latest and accumulated series from durable state', async () => {
    const hot = new MemoryShortTermStore()
    const durable = new MemoryLongTermStore()
    const task = makeTask()
    await hot.saveTask(task)
    await durable.saveTask(task)
    const token: HotWriteToken = { taskId: task.id, storageEpoch: 1 }
    const first = await hot.commitEventFenced(
      task.id,
      makeEvent(0, {
        id: 'delta-a',
        seriesId: 'output',
        seriesMode: 'accumulate',
        seriesAccField: 'delta',
        data: { delta: 'A' },
      }),
      token,
    )
    await durable.accumulateSeries(task.id, 'output', first.event, 'delta')
    const second = await hot.commitEventFenced(
      task.id,
      makeEvent(1, {
        id: 'delta-b',
        seriesId: 'output',
        seriesMode: 'accumulate',
        seriesAccField: 'delta',
        data: { delta: 'B' },
      }),
      token,
    )
    await durable.accumulateSeries(task.id, 'output', second.event, 'delta')
    const latest = await hot.commitEventFenced(
      task.id,
      makeEvent(2, {
        id: 'latest',
        type: 'progress',
        seriesId: 'progress',
        seriesMode: 'latest',
        data: { percent: 50 },
      }),
      token,
    )
    await durable.replaceLastSeriesEvent(task.id, 'progress', latest.event)
    const coordinator = new StorageCoordinator({
      shortTermStore: hot,
      longTermStore: durable,
    })
    await coordinator.releaseTaskStorage(task.id, {
      expectedLastEventIndex: 2,
      inactiveSince: 3_000,
    })
    const engine = new TaskEngine({
      shortTermStore: hot,
      longTermStore: durable,
      broadcast: new MemoryBroadcastProvider(),
    })

    const delta = await engine.publishEvent(task.id, {
      type: 'llm.delta',
      level: 'info',
      data: { delta: 'C' },
      seriesId: 'output',
      seriesMode: 'accumulate',
      seriesAccField: 'delta',
    })
    expect(delta.index).toBe(3)
    await expect(hot.getSeriesLatest(task.id, 'output')).resolves.toMatchObject({
      data: { delta: 'ABC' },
    })
    await expect(hot.getSeriesLatest(task.id, 'progress')).resolves.toMatchObject({
      data: { percent: 50 },
    })
  })

  it('uses one new epoch for concurrent rehydrators', async () => {
    const { coordinator, durable } = await seedAndRelease(0)
    const results = await Promise.allSettled([
      coordinator.ensureTaskHotForWrite('task-1'),
      coordinator.ensureTaskHotForWrite('task-1'),
    ])
    const successful = results
      .filter((result): result is PromiseFulfilledResult<HotWriteToken> =>
        result.status === 'fulfilled')
      .map((result) => result.value)
    expect(successful.length).toBeGreaterThanOrEqual(1)
    expect(successful.every((token) => token.storageEpoch === 2)).toBe(true)
    await expect(coordinator.ensureTaskHotForWrite('task-1')).resolves.toEqual({
      taskId: 'task-1',
      storageEpoch: 2,
    })
    await expect(durable.getTaskStorageMetadata('task-1')).resolves.toMatchObject({
      storageState: 'hot',
      storageEpoch: 2,
    })
  })

  it('adopts an atomically restored epoch after a crash before durable CAS', async () => {
    const { hot, durable, coordinator } = await seedAndRelease(1)
    const lease = await hot.acquireStorageLock(
      'task-1',
      'crashed-lock',
      'crashed-rehydrate',
      5_000,
    )
    if (!lease) throw new Error('failed to acquire rehydrate fixture lease')
    const metadata = await durable.getTaskStorageMetadata('task-1')
    const task = await durable.getTask('task-1')
    if (!metadata || !task) throw new Error('durable fixture is missing')
    await hot.restoreHotTaskFenced(
      {
        task,
        archiveWatermark: metadata.archiveWatermark,
        maxEventIndex: await durable.getLastEventIndex('task-1'),
        replayEvents: await durable.getRecentEvents('task-1', 1_000),
        seriesLatest: await durable.getDurableSeriesState('task-1'),
        storageEpoch: metadata.storageEpoch,
      },
      lease,
      2,
    )
    await hot.releaseStorageLock(lease)

    await expect(coordinator.ensureTaskHotForWrite('task-1')).resolves.toEqual({
      taskId: 'task-1',
      storageEpoch: 2,
    })
    await expect(durable.getTaskStorageMetadata('task-1')).resolves.toMatchObject({
      storageState: 'hot',
      storageEpoch: 2,
    })
  })
})
