import { describe, expect, it } from 'vitest'
import {
  MemoryLongTermStore,
  MemoryShortTermStore,
  StorageCoordinator,
  type ArchiveBatch,
  type ArchiveBatchReceipt,
  type ArchiveGeneration,
  type ArchiveSourcePage,
  type ClosedWriteFence,
  type DurableSeriesState,
  type StorageLease,
  type Task,
  type TaskEvent,
} from '@taskcast/core'

const EVENT_COUNT = 600_000
const ARCHIVE_BATCH_SIZE = 1_000

const makeTask = (id: string): Task => ({
  id,
  status: 'pending',
  type: 'agent.session',
  createdAt: 1_000,
  updatedAt: 1_000,
})

const makeEvent = (taskId: string, index: number): TaskEvent => ({
  id: `event-${index}`,
  taskId,
  index,
  timestamp: 2_000 + index,
  type: 'agent.message_update',
  level: 'info',
  data: { index },
})

class StreamingShortTermStore extends MemoryShortTermStore {
  maxRequestedPageSize = 0
  maxReturnedPageSize = 0
  pagesRead = 0

  constructor(
    private readonly taskId: string,
    private readonly eventCount: number,
  ) {
    super()
  }

  override async closeWriteFence(
    lease: StorageLease,
    expectedEpoch: number,
  ): Promise<ClosedWriteFence> {
    const closed = await super.closeWriteFence(lease, expectedEpoch)
    return { ...closed, highWatermark: this.eventCount - 1 }
  }

  override async readArchiveSourcePage(
    taskId: string,
    watermark: number,
    cursor: string | null,
    limit: number,
  ): Promise<ArchiveSourcePage> {
    if (taskId !== this.taskId || watermark !== this.eventCount - 1) {
      throw new Error('unexpected streaming archive request')
    }
    this.maxRequestedPageSize = Math.max(this.maxRequestedPageSize, limit)
    const offset = cursor === null ? 0 : Number(cursor)
    const end = Math.min(offset + limit, this.eventCount)
    const events = Array.from(
      { length: end - offset },
      (_, index) => makeEvent(taskId, offset + index),
    )
    this.maxReturnedPageSize = Math.max(
      this.maxReturnedPageSize,
      events.length,
    )
    this.pagesRead += 1
    return {
      events,
      nextCursor: end < this.eventCount ? String(end) : null,
      done: end >= this.eventCount,
    }
  }
}

class ReceiptOnlyDurableStore extends MemoryLongTermStore {
  maxArchiveBatchSize = 0
  archivedEvents = 0
  private targetWatermark = -1
  private archiveWatermark = -1

  override async beginArchive(
    generation: ArchiveGeneration,
  ): Promise<ArchiveGeneration> {
    this.targetWatermark = generation.targetWatermark
    return generation
  }

  override async archiveBatch(
    _taskId: string,
    _generation: string,
    batch: ArchiveBatch,
  ): Promise<ArchiveBatchReceipt> {
    this.maxArchiveBatchSize = Math.max(
      this.maxArchiveBatchSize,
      batch.events.length,
    )
    this.archivedEvents += batch.events.length
    return batch.receipt
  }

  override async finalizeArchive(
    _taskId: string,
    _generation: string,
    _task: Task,
    _seriesLatest: DurableSeriesState[],
  ): Promise<number> {
    this.archiveWatermark = this.targetWatermark
    return this.archiveWatermark
  }

  override async getArchiveWatermark(_taskId: string): Promise<number> {
    return this.archiveWatermark
  }
}

describe('large PostgreSQL archive release pipeline', () => {
  it('streams 600,000 events in bounded batches while unrelated durable work progresses', async () => {
    const taskId = 'large-release'
    const hot = new StreamingShortTermStore(taskId, EVENT_COUNT)
    const durable = new ReceiptOnlyDurableStore()
    const task = makeTask(taskId)
    await hot.saveTask(task)
    await durable.saveTask(task)
    const coordinator = new StorageCoordinator({
      shortTermStore: hot,
      longTermStore: durable,
      archiveBatchSize: ARCHIVE_BATCH_SIZE,
    })

    const rssBefore = process.memoryUsage().rss
    let releaseFinished = false
    const release = coordinator.releaseTaskStorage(taskId, {
      expectedLastEventIndex: EVENT_COUNT - 1,
      inactiveSince: 2_000 + EVENT_COUNT,
    }).finally(() => {
      releaseFinished = true
    })

    const unrelated = makeTask('unrelated-task')
    await durable.saveTask(unrelated)
    await durable.saveEvent(makeEvent(unrelated.id, 0))
    expect(releaseFinished).toBe(false)
    await expect(durable.getLastEventIndex(unrelated.id)).resolves.toBe(0)

    await expect(release).resolves.toMatchObject({
      storageState: 'cold',
      archiveWatermark: EVENT_COUNT - 1,
      released: true,
    })
    const rssGrowth = process.memoryUsage().rss - rssBefore
    expect(hot.maxRequestedPageSize).toBe(ARCHIVE_BATCH_SIZE)
    expect(hot.maxReturnedPageSize).toBeLessThanOrEqual(ARCHIVE_BATCH_SIZE)
    expect(durable.maxArchiveBatchSize).toBeLessThanOrEqual(
      ARCHIVE_BATCH_SIZE,
    )
    expect(durable.archivedEvents).toBe(EVENT_COUNT)
    expect(hot.pagesRead).toBe((EVENT_COUNT / ARCHIVE_BATCH_SIZE) * 2)
    expect(rssGrowth).toBeLessThan(256 * 1024 * 1024)
  }, 120_000)
})
