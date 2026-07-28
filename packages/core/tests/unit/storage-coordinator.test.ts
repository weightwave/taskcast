import { describe, expect, it } from 'vitest'
import {
  computeArchiveBatchDigest,
  computeArchiveSourceDigest,
  computeArchiveSourcePageDigest,
  computeSeriesStateDigest,
  MemoryShortTermStore,
  MemoryBroadcastProvider,
  MemoryLongTermStore,
  StorageCoordinator,
  TaskEngine,
  type ArchiveBatch,
  type ArchiveBatchReceipt,
  type ArchiveGeneration,
  type DurableSeriesState,
  type EventQueryOptions,
  type LongTermStore,
  type Task,
  type TaskEvent,
  type TaskStorageMetadata,
  type TaskStorageMetadataCas,
  type StorageReleaseRequest,
  type Worker,
  type WorkerAuditEvent,
} from '../../src/index.js'

const makeTask = (): Task => ({
  id: 'task-1',
  status: 'running',
  createdAt: 1_000,
  updatedAt: 1_000,
})

const makeEvent = (index: number): TaskEvent => ({
  id: `event-${index}`,
  taskId: 'task-1',
  index,
  timestamp: 2_000 + index,
  type: 'llm.delta',
  level: 'info',
  data: { delta: String(index) },
})

class CoordinatorLongTermStore implements LongTermStore {
  readonly supportsHotColdRelease = true
  readonly tasks = new Map<string, Task>()
  readonly events = new Map<string, TaskEvent[]>()
  readonly metadata = new Map<string, TaskStorageMetadata>()
  readonly generations = new Map<string, ArchiveGeneration>()
  readonly batches: ArchiveBatch[] = []
  readonly series = new Map<string, DurableSeriesState[]>()
  readonly creationTokens = new Map<string, string>()
  readonly releaseRequests = new Map<string, StorageReleaseRequest>()
  failArchiveBatch = false
  failMetadataCas = 0

  async saveTask(task: Task): Promise<void> {
    this.tasks.set(task.id, structuredClone(task))
    if (!this.metadata.has(task.id)) {
      this.metadata.set(task.id, {
        taskId: task.id,
        storageState: 'hot',
        storageEpoch: 1,
        activeReleaseGeneration: null,
        archiveWatermark: -1,
        lastEventAt: null,
        coldAt: null,
        executionDeadlineAt: null,
        taskVersion: 1,
      })
    }
  }

  async getTask(taskId: string): Promise<Task | null> {
    return structuredClone(this.tasks.get(taskId) ?? null)
  }

  async claimTaskCreation(task: Task, creationToken: string): Promise<boolean> {
    if (this.tasks.has(task.id)) return false
    await this.saveTask(task)
    this.creationTokens.set(task.id, creationToken)
    return true
  }

  async completeTaskCreation(taskId: string, creationToken: string): Promise<boolean> {
    if (this.creationTokens.get(taskId) !== creationToken) return false
    this.creationTokens.delete(taskId)
    return true
  }

  async abortTaskCreation(taskId: string, creationToken: string): Promise<boolean> {
    if (this.creationTokens.get(taskId) !== creationToken) return false
    this.creationTokens.delete(taskId)
    this.tasks.delete(taskId)
    this.metadata.delete(taskId)
    return true
  }

  async saveEvent(event: TaskEvent): Promise<void> {
    const events = this.events.get(event.taskId) ?? []
    const existing = events.findIndex((candidate) => candidate.index === event.index)
    if (existing >= 0) events[existing] = structuredClone(event)
    else events.push(structuredClone(event))
    events.sort((left, right) => left.index - right.index)
    this.events.set(event.taskId, events)
    const metadata = this.metadata.get(event.taskId)
    if (metadata) metadata.lastEventAt = event.timestamp
  }

  async getEvents(
    taskId: string,
    _opts?: EventQueryOptions,
  ): Promise<TaskEvent[]> {
    return structuredClone(this.events.get(taskId) ?? [])
  }

  async getTaskStorageMetadata(
    taskId: string,
  ): Promise<TaskStorageMetadata | null> {
    return structuredClone(this.metadata.get(taskId) ?? null)
  }

  async persistStorageReleaseRequest(request: StorageReleaseRequest): Promise<boolean> {
    if (!this.tasks.has(request.taskId)) return false
    this.releaseRequests.set(request.taskId, structuredClone(request))
    return true
  }

  async clearStorageReleaseRequest(request: StorageReleaseRequest): Promise<boolean> {
    const current = this.releaseRequests.get(request.taskId)
    if (
      !current ||
      current.requestedAt !== request.requestedAt ||
      current.expectedLastEventIndex !== request.expectedLastEventIndex ||
      current.inactiveSince !== request.inactiveSince
    ) {
      return false
    }
    this.releaseRequests.delete(request.taskId)
    return true
  }

  async listStorageReleaseRequests(limit: number): Promise<StorageReleaseRequest[]> {
    return Array.from(this.releaseRequests.values()).slice(0, limit)
  }

  async compareAndSetTaskStorageMetadata(
    update: TaskStorageMetadataCas,
  ): Promise<boolean> {
    if (this.failMetadataCas > 0) {
      this.failMetadataCas -= 1
      throw new Error('metadata unavailable')
    }
    const current = this.metadata.get(update.taskId)
    if (
      !current ||
      current.storageState !== update.expectedStorageState ||
      current.storageEpoch !== update.expectedStorageEpoch ||
      current.activeReleaseGeneration !== update.expectedReleaseGeneration
    ) {
      return false
    }
    this.metadata.set(update.taskId, structuredClone(update.next))
    return true
  }

  async beginArchive(generation: ArchiveGeneration): Promise<ArchiveGeneration> {
    this.generations.set(generation.generation, structuredClone(generation))
    return structuredClone(generation)
  }

  async archiveBatch(
    _taskId: string,
    _generation: string,
    batch: ArchiveBatch,
  ): Promise<ArchiveBatchReceipt> {
    if (this.failArchiveBatch) throw new Error('archive unavailable')
    this.batches.push(structuredClone(batch))
    for (const event of batch.events) await this.saveEvent(event)
    return structuredClone(batch.receipt)
  }

  async finalizeArchive(
    taskId: string,
    generation: string,
    task: Task,
    seriesLatest: DurableSeriesState[],
  ): Promise<number> {
    const archive = this.generations.get(generation)
    if (!archive) throw new Error('missing archive generation')
    this.tasks.set(taskId, structuredClone(task))
    this.series.set(taskId, structuredClone(seriesLatest))
    this.metadata.get(taskId)!.archiveWatermark = archive.targetWatermark
    return archive.targetWatermark
  }

  async getArchiveWatermark(taskId: string): Promise<number> {
    return this.metadata.get(taskId)?.archiveWatermark ?? -1
  }

  async getLastEventIndex(taskId: string): Promise<number> {
    return this.events.get(taskId)?.at(-1)?.index ?? -1
  }

  async getRecentEvents(taskId: string, limit: number): Promise<TaskEvent[]> {
    return structuredClone((this.events.get(taskId) ?? []).slice(-limit))
  }

  async getDurableSeriesState(taskId: string): Promise<DurableSeriesState[]> {
    return structuredClone(this.series.get(taskId) ?? [])
  }

  async saveWorkerEvent(_event: WorkerAuditEvent): Promise<void> {}

  async getWorkerEvents(
    _workerId: string,
    _opts?: EventQueryOptions,
  ): Promise<WorkerAuditEvent[]> {
    return []
  }
}

class RenewalLossStore extends MemoryShortTermStore {
  renewals = 0

  constructor(private readonly loseOnRenewal: number) {
    super()
  }

  override async renewStorageLock(
    lease: Parameters<MemoryShortTermStore['renewStorageLock']>[0],
    ttlMs: number,
  ): Promise<boolean> {
    this.renewals += 1
    if (this.renewals >= this.loseOnRenewal) return false
    return super.renewStorageLock(lease, ttlMs)
  }
}

class DeleteThenCrashStore extends MemoryShortTermStore {
  override async deleteTaskStorageFenced(
    lease: Parameters<MemoryShortTermStore['deleteTaskStorageFenced']>[0],
    expectedEpoch: number,
  ): Promise<void> {
    await super.deleteTaskStorageFenced(lease, expectedEpoch)
    throw new Error('process crashed after hot deletion')
  }
}

async function seedTask(
  hot: MemoryShortTermStore,
  durable: CoordinatorLongTermStore,
): Promise<void> {
  const task = makeTask()
  await hot.saveTask(task)
  await durable.saveTask(task)
  const token = { taskId: task.id, storageEpoch: 1 }
  for (let index = 0; index < 3; index++) {
    const { index: _ignored, ...input } = makeEvent(index)
    const committed = await hot.commitEventFenced(task.id, input, token)
    await durable.saveEvent(committed.event)
  }
}

describe('StorageCoordinator', () => {
  it('deletes hot storage only after the archive watermark is durable', async () => {
    const hot = new MemoryShortTermStore()
    const durable = new CoordinatorLongTermStore()
    await seedTask(hot, durable)
    const coordinator = new StorageCoordinator({
      shortTermStore: hot,
      longTermStore: durable,
      archiveBatchSize: 2,
      storageLockTtlMs: 5_000,
      generateId: (() => {
        let next = 0
        return () => `generation-${++next}`
      })(),
      now: () => 10_000,
    })

    await expect(
      coordinator.releaseTaskStorage('task-1', {
        expectedLastEventIndex: 2,
        inactiveSince: 3_000,
      }),
    ).resolves.toEqual({
      taskId: 'task-1',
      storageState: 'cold',
      archiveWatermark: 2,
      released: true,
    })

    expect(durable.batches.map((batch) => batch.events.length)).toEqual([2, 1])
    await expect(hot.getTaskStoragePresence('task-1')).resolves.toEqual({
      task: false,
      eventCount: 0,
      nextIndex: false,
      seriesStateCount: 0,
      writeFence: false,
    })
    await expect(durable.getArchiveWatermark('task-1')).resolves.toBe(2)
    await expect(durable.getEvents('task-1')).resolves.toEqual([
      makeEvent(0),
      makeEvent(1),
      makeEvent(2),
    ])
  })

  it('returns an already-cold task idempotently without recreating hot data', async () => {
    const hot = new MemoryShortTermStore()
    const durable = new CoordinatorLongTermStore()
    await durable.saveTask(makeTask())
    durable.metadata.set('task-1', {
      ...durable.metadata.get('task-1')!,
      storageState: 'cold',
      archiveWatermark: 7,
      coldAt: 9_000,
    })
    const coordinator = new StorageCoordinator({
      shortTermStore: hot,
      longTermStore: durable,
    })

    await expect(
      coordinator.releaseTaskStorage('task-1', {
        expectedLastEventIndex: 7,
        inactiveSince: 10_000,
      }),
    ).resolves.toEqual({
      taskId: 'task-1',
      storageState: 'cold',
      archiveWatermark: 7,
      released: false,
    })
    await expect(hot.getTaskStoragePresence('task-1')).resolves.toMatchObject({
      task: false,
      writeFence: false,
    })
  })

  it('reopens a new epoch when the expected index is stale', async () => {
    const hot = new MemoryShortTermStore()
    const durable = new CoordinatorLongTermStore()
    await seedTask(hot, durable)
    const coordinator = new StorageCoordinator({
      shortTermStore: hot,
      longTermStore: durable,
    })

    await expect(
      coordinator.releaseTaskStorage('task-1', {
        expectedLastEventIndex: 1,
        inactiveSince: 3_000,
      }),
    ).rejects.toMatchObject({ code: 'storage_precondition_failed' })
    await expect(hot.getWriteFence('task-1')).resolves.toEqual({
      taskId: 'task-1',
      acceptingWrites: true,
      storageEpoch: 2,
      activeReleaseGeneration: null,
    })
    await expect(durable.getTaskStorageMetadata('task-1')).resolves.toMatchObject({
      storageState: 'hot',
      storageEpoch: 2,
      activeReleaseGeneration: null,
    })
  })

  it('rejects source activity newer than the cutoff even when durable metadata lags', async () => {
    const hot = new MemoryShortTermStore()
    const durable = new CoordinatorLongTermStore()
    await seedTask(hot, durable)
    durable.metadata.get('task-1')!.lastEventAt = null
    const coordinator = new StorageCoordinator({
      shortTermStore: hot,
      longTermStore: durable,
    })

    await expect(
      coordinator.releaseTaskStorage('task-1', {
        expectedLastEventIndex: 2,
        inactiveSince: 2_001,
      }),
    ).rejects.toMatchObject({ code: 'storage_precondition_failed' })
    await expect(hot.getTaskStoragePresence('task-1')).resolves.toMatchObject({
      task: true,
      eventCount: 3,
    })
    await expect(durable.getTaskStorageMetadata('task-1')).resolves.toMatchObject({
      storageState: 'hot',
      storageEpoch: 2,
    })
  })

  it('retains hot data and reopens a new epoch when archive upload fails', async () => {
    const hot = new MemoryShortTermStore()
    const durable = new CoordinatorLongTermStore()
    await seedTask(hot, durable)
    durable.failArchiveBatch = true
    const coordinator = new StorageCoordinator({
      shortTermStore: hot,
      longTermStore: durable,
      archiveBatchSize: 2,
    })

    await expect(
      coordinator.releaseTaskStorage('task-1', {
        expectedLastEventIndex: 2,
        inactiveSince: 3_000,
      }),
    ).rejects.toThrow('archive unavailable')
    await expect(hot.getTaskStoragePresence('task-1')).resolves.toMatchObject({
      task: true,
      eventCount: 3,
      writeFence: true,
    })
    await expect(hot.getWriteFence('task-1')).resolves.toMatchObject({
      acceptingWrites: true,
      storageEpoch: 2,
    })
    await expect(durable.getTaskStorageMetadata('task-1')).resolves.toMatchObject({
      storageState: 'hot',
      storageEpoch: 2,
      activeReleaseGeneration: null,
      archiveWatermark: -1,
    })
  })

  it('performs no cleanup mutation after storage lease renewal is lost', async () => {
    const hot = new RenewalLossStore(3)
    const durable = new CoordinatorLongTermStore()
    await seedTask(hot, durable)
    const coordinator = new StorageCoordinator({
      shortTermStore: hot,
      longTermStore: durable,
      archiveBatchSize: 2,
    })

    await expect(
      coordinator.releaseTaskStorage('task-1', {
        expectedLastEventIndex: 2,
        inactiveSince: 3_000,
      }),
    ).rejects.toMatchObject({ code: 'storage_fence_conflict' })
    await expect(hot.getWriteFence('task-1')).resolves.toMatchObject({
      acceptingWrites: false,
      storageEpoch: 1,
    })
    await expect(durable.getTaskStorageMetadata('task-1')).resolves.toMatchObject({
      storageState: 'releasing',
      storageEpoch: 1,
    })
  })

  it('repairs a reopened epoch after PostgreSQL was unavailable during cleanup', async () => {
    const hot = new MemoryShortTermStore()
    const durable = new CoordinatorLongTermStore()
    await seedTask(hot, durable)
    durable.failMetadataCas = 2
    const coordinator = new StorageCoordinator({
      shortTermStore: hot,
      longTermStore: durable,
    })

    await expect(
      coordinator.releaseTaskStorage('task-1', {
        expectedLastEventIndex: 2,
        inactiveSince: 3_000,
      }),
    ).rejects.toThrow('metadata unavailable')
    await expect(hot.getWriteFence('task-1')).resolves.toMatchObject({
      acceptingWrites: true,
      storageEpoch: 2,
    })
    await expect(durable.getTaskStorageMetadata('task-1')).resolves.toMatchObject({
      storageState: 'hot',
      storageEpoch: 1,
    })

    await expect(coordinator.ensureTaskHotForWrite('task-1')).resolves.toEqual({
      taskId: 'task-1',
      storageEpoch: 2,
    })
    await expect(durable.getTaskStorageMetadata('task-1')).resolves.toMatchObject({
      storageState: 'hot',
      storageEpoch: 2,
    })
  })

  it('repairs durable releasing metadata after Redis already reopened a newer epoch', async () => {
    const hot = new MemoryShortTermStore()
    const durable = new CoordinatorLongTermStore()
    await seedTask(hot, durable)
    const staleLease = (await hot.acquireStorageLock(
      'task-1',
      'stale-lock',
      'stale-generation',
      5_000,
    ))!
    await hot.closeWriteFence(staleLease, 1)
    const metadata = (await durable.getTaskStorageMetadata('task-1'))!
    await durable.compareAndSetTaskStorageMetadata({
      taskId: 'task-1',
      expectedStorageState: 'hot',
      expectedStorageEpoch: 1,
      expectedReleaseGeneration: null,
      next: {
        ...metadata,
        storageState: 'releasing',
        activeReleaseGeneration: 'stale-generation',
      },
    })
    await hot.reopenWriteFence(staleLease, 1)
    await hot.releaseStorageLock(staleLease)

    const coordinator = new StorageCoordinator({
      shortTermStore: hot,
      longTermStore: durable,
    })
    await expect(coordinator.recoverTaskStorage('task-1')).resolves.toEqual({
      taskId: 'task-1',
      storageState: 'hot',
      archiveWatermark: -1,
      released: false,
    })
    await expect(durable.getTaskStorageMetadata('task-1')).resolves.toMatchObject({
      storageState: 'hot',
      storageEpoch: 2,
      activeReleaseGeneration: null,
    })
  })

  it('recovers a crash after hot deletion by proving the durable watermark', async () => {
    const hot = new DeleteThenCrashStore()
    const durable = new CoordinatorLongTermStore()
    await seedTask(hot, durable)
    const ids = ['release-generation', 'release-lock', 'recovery-generation', 'recovery-lock']
    const coordinator = new StorageCoordinator({
      shortTermStore: hot,
      longTermStore: durable,
      archiveBatchSize: 2,
      generateId: () => ids.shift()!,
      now: () => 10_000,
    })

    await expect(
      coordinator.releaseTaskStorage('task-1', {
        expectedLastEventIndex: 2,
        inactiveSince: 3_000,
      }),
    ).rejects.toThrow('process crashed after hot deletion')
    await expect(hot.getTaskStoragePresence('task-1')).resolves.toMatchObject({
      task: false,
      writeFence: false,
    })
    await expect(durable.getTaskStorageMetadata('task-1')).resolves.toMatchObject({
      storageState: 'releasing',
      archiveWatermark: 2,
      activeReleaseGeneration: 'release-generation',
    })

    await expect(coordinator.recoverTaskStorage('task-1')).resolves.toEqual({
      taskId: 'task-1',
      storageState: 'cold',
      archiveWatermark: 2,
      released: true,
    })
    await expect(durable.getTaskStorageMetadata('task-1')).resolves.toMatchObject({
      storageState: 'cold',
      archiveWatermark: 2,
      activeReleaseGeneration: null,
    })
  })

  it('does not mark recovery cold while any hot-storage key class remains', async () => {
    class PartialPresenceStore extends MemoryShortTermStore {
      override async getTaskStoragePresence() {
        return {
          task: false,
          eventCount: 0,
          nextIndex: true,
          seriesStateCount: 0,
          writeFence: false,
        }
      }
    }
    const hot = new PartialPresenceStore()
    const durable = new CoordinatorLongTermStore()
    await durable.saveTask(makeTask())
    durable.metadata.set('task-1', {
      ...durable.metadata.get('task-1')!,
      storageState: 'releasing',
      activeReleaseGeneration: 'stale-generation',
    })
    const coordinator = new StorageCoordinator({
      shortTermStore: hot,
      longTermStore: durable,
    })

    await expect(
      coordinator.recoverTaskStorage('task-1'),
    ).rejects.toMatchObject({ code: 'storage_integrity_error' })
    await expect(durable.getTaskStorageMetadata('task-1')).resolves.toMatchObject({
      storageState: 'releasing',
    })
  })

  it('invalidates a stale executor and reopens retained hot storage', async () => {
    const hot = new MemoryShortTermStore()
    const durable = new CoordinatorLongTermStore()
    await seedTask(hot, durable)
    const staleLease = (await hot.acquireStorageLock(
      'task-1',
      'stale-lock',
      'stale-generation',
      5_000,
    ))!
    await hot.closeWriteFence(staleLease, 1)
    const metadata = (await durable.getTaskStorageMetadata('task-1'))!
    await durable.compareAndSetTaskStorageMetadata({
      taskId: 'task-1',
      expectedStorageState: 'hot',
      expectedStorageEpoch: 1,
      expectedReleaseGeneration: null,
      next: {
        ...metadata,
        storageState: 'releasing',
        activeReleaseGeneration: 'stale-generation',
      },
    })
    await hot.releaseStorageLock(staleLease)

    const ids = ['recovery-generation', 'recovery-lock']
    const coordinator = new StorageCoordinator({
      shortTermStore: hot,
      longTermStore: durable,
      generateId: () => ids.shift()!,
    })
    await expect(coordinator.recoverTaskStorage('task-1')).resolves.toEqual({
      taskId: 'task-1',
      storageState: 'hot',
      archiveWatermark: -1,
      released: false,
    })
    await expect(hot.getWriteFence('task-1')).resolves.toEqual({
      taskId: 'task-1',
      acceptingWrites: true,
      storageEpoch: 2,
      activeReleaseGeneration: null,
    })
    await expect(durable.getTaskStorageMetadata('task-1')).resolves.toMatchObject({
      storageState: 'hot',
      storageEpoch: 2,
      activeReleaseGeneration: null,
    })
    await expect(
      hot.deleteTaskStorageFenced(staleLease, 1),
    ).rejects.toMatchObject({ code: 'storage_fence_conflict' })
  })

  it('routes engine mutations and release through the fenced coordinator', async () => {
    class FencedOnlyStore extends MemoryShortTermStore {
      override async nextIndex(_taskId: string): Promise<number> {
        throw new Error('unfenced nextIndex must not be used')
      }
    }
    const hot = new FencedOnlyStore()
    const durable = new CoordinatorLongTermStore()
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

    await expect(
      engine.releaseTaskStorage('task-1', {
        expectedLastEventIndex: event.index,
        inactiveSince: event.timestamp,
      }),
    ).resolves.toMatchObject({
      taskId: 'task-1',
      storageState: 'cold',
      archiveWatermark: event.index,
      released: true,
    })
  })

  it('recovers an interrupted explicit release before starting it again', async () => {
    const hot = new MemoryShortTermStore()
    const durable = new CoordinatorLongTermStore()
    const first = new TaskEngine({
      shortTermStore: hot,
      longTermStore: durable,
      broadcast: new MemoryBroadcastProvider(),
    })
    await first.createTask({ id: 'task-1' })
    await first.transitionTask('task-1', 'running')
    const canary = await first.publishEvent('task-1', {
      type: 'canary.event',
      level: 'info',
      data: {},
    })
    const preconditions = {
      expectedLastEventIndex: canary.index,
      inactiveSince: canary.timestamp,
    }
    await first.releaseTaskStorage('task-1', preconditions)
    durable.metadata.set('task-1', {
      ...durable.metadata.get('task-1')!,
      storageState: 'releasing',
      activeReleaseGeneration: 'interrupted-generation',
      coldAt: null,
    })
    const second = new TaskEngine({
      shortTermStore: hot,
      longTermStore: durable,
      broadcast: new MemoryBroadcastProvider(),
    })

    await expect(second.releaseTaskStorage('task-1', preconditions)).resolves.toEqual({
      taskId: 'task-1',
      storageState: 'cold',
      archiveWatermark: canary.index,
      released: true,
    })
    expect(durable.releaseRequests).toEqual(new Map())
    await expect(hot.getTaskStoragePresence('task-1')).resolves.toEqual({
      task: false,
      eventCount: 0,
      nextIndex: false,
      seriesStateCount: 0,
      writeFence: false,
    })
  })

  it('commits transition state and status events atomically against release', async () => {
    let unblockCommit!: () => void
    let reportBlocked!: () => void
    const commitBlocked = new Promise<void>((resolve) => {
      reportBlocked = resolve
    })
    const commitGate = new Promise<void>((resolve) => {
      unblockCommit = resolve
    })
    class BlockingCommitStore extends MemoryShortTermStore {
      blockNextCommit = false

      override async commitTaskEventsFenced(
        ...args: Parameters<MemoryShortTermStore['commitTaskEventsFenced']>
      ) {
        if (this.blockNextCommit) {
          this.blockNextCommit = false
          reportBlocked()
          await commitGate
        }
        return super.commitTaskEventsFenced(...args)
      }
    }
    const hot = new BlockingCommitStore()
    const durable = new CoordinatorLongTermStore()
    const engine = new TaskEngine({
      shortTermStore: hot,
      longTermStore: durable,
      broadcast: new MemoryBroadcastProvider(),
    })
    await engine.createTask({ id: 'task-1' })
    hot.blockNextCommit = true
    const transitioning = engine.transitionTask('task-1', 'running')
    await commitBlocked

    await expect(engine.releaseTaskStorage('task-1', {
      expectedLastEventIndex: -1,
      inactiveSince: Date.now(),
    })).resolves.toMatchObject({ storageState: 'cold' })

    unblockCommit()
    await expect(transitioning).rejects.toMatchObject({ code: 'storage_busy' })
    await expect(durable.getTask('task-1')).resolves.toMatchObject({
      status: 'pending',
    })
    await expect(durable.getEvents('task-1')).resolves.toHaveLength(0)
  })

  it('rejects a stale transition after an assigned task is reclaimed without changing status', async () => {
    let unblockCommit!: () => void
    let reportBlocked!: () => void
    const commitBlocked = new Promise<void>((resolve) => {
      reportBlocked = resolve
    })
    const commitGate = new Promise<void>((resolve) => {
      unblockCommit = resolve
    })
    class BlockingCommitStore extends MemoryShortTermStore {
      blockNextCommit = false

      override async commitTaskEventsFenced(
        ...args: Parameters<MemoryShortTermStore['commitTaskEventsFenced']>
      ) {
        if (this.blockNextCommit) {
          this.blockNextCommit = false
          reportBlocked()
          await commitGate
        }
        return super.commitTaskEventsFenced(...args)
      }
    }
    const makeWorker = (id: string): Worker => ({
      id,
      status: 'idle',
      matchRule: {},
      capacity: 2,
      usedSlots: 0,
      weight: 1,
      connectionMode: 'pull',
      connectedAt: 1_000,
      lastHeartbeatAt: 1_000,
    })
    const hot = new BlockingCommitStore()
    const durable = new MemoryLongTermStore()
    const engine = new TaskEngine({
      shortTermStore: hot,
      longTermStore: durable,
      broadcast: new MemoryBroadcastProvider(),
    })
    await engine.createTask({ id: 'task-1', cost: 1 })
    await hot.saveWorker(makeWorker('worker-a'))
    await hot.saveWorker(makeWorker('worker-b'))
    await expect(hot.claimTask('task-1', 'worker-a', 1)).resolves.toBe(true)

    hot.blockNextCommit = true
    const transitioning = engine.transitionTask('task-1', 'running')
    await commitBlocked
    await expect(hot.claimTask('task-1', 'worker-b', 1)).resolves.toBe(true)
    unblockCommit()

    await expect(transitioning).rejects.toBeInstanceOf(Error)
    await expect(hot.getTask('task-1')).resolves.toMatchObject({
      status: 'assigned',
      assignedWorker: 'worker-b',
    })
    await expect(hot.getEvents('task-1')).resolves.toHaveLength(0)
  })

  it('takes over an expired pristine creation claim and completes idempotently', async () => {
    const durable = new MemoryLongTermStore()
    const task: Task = {
      id: 'task-lease',
      status: 'pending',
      createdAt: 1_000,
      updatedAt: 1_000,
    }

    await expect(durable.claimTaskCreation(task, 'token-1', 100)).resolves.toBe(true)
    await expect(durable.claimTaskCreation(task, 'token-2', 100)).resolves.toBe(false)
    await new Promise((resolve) => setTimeout(resolve, 110))
    await expect(durable.claimTaskCreation(task, 'token-2', 30_000)).resolves.toBe(true)
    await expect(durable.completeTaskCreation('task-lease', 'token-2')).resolves.toBe(true)
    await expect(durable.completeTaskCreation('task-lease', 'token-2')).resolves.toBe(true)
    await expect(durable.abortTaskCreation('task-lease', 'token-2')).resolves.toBe(false)
  })

  it('recovers when the first creation-complete response is lost', async () => {
    class LostCompleteResponseStore extends MemoryLongTermStore {
      calls = 0

      override async completeTaskCreation(taskId: string, creationToken: string): Promise<boolean> {
        this.calls += 1
        const completed = await super.completeTaskCreation(taskId, creationToken)
        if (this.calls === 1) throw new Error('connection lost after commit')
        return completed
      }
    }
    const durable = new LostCompleteResponseStore()
    const engine = new TaskEngine({
      shortTermStore: new MemoryShortTermStore(),
      longTermStore: durable,
      broadcast: new MemoryBroadcastProvider(),
    })

    await expect(engine.createTask({ id: 'task-idempotent' })).resolves.toMatchObject({
      id: 'task-idempotent',
    })
    expect(durable.calls).toBe(2)
  })

  it('lets the engine recover an expired claim left before the hot save', async () => {
    const durable = new MemoryLongTermStore()
    const crashedTask: Task = {
      id: 'task-crashed-create',
      status: 'pending',
      createdAt: 1_000,
      updatedAt: 1_000,
    }
    await durable.claimTaskCreation(crashedTask, 'crashed-token', 100)
    await new Promise((resolve) => setTimeout(resolve, 110))
    const hot = new MemoryShortTermStore()
    const engine = new TaskEngine({
      shortTermStore: hot,
      longTermStore: durable,
      broadcast: new MemoryBroadcastProvider(),
    })

    await expect(engine.createTask({ id: 'task-crashed-create' })).resolves.toMatchObject({
      id: 'task-crashed-create',
      status: 'pending',
    })
    await expect(hot.getTask('task-crashed-create')).resolves.toMatchObject({
      id: 'task-crashed-create',
    })
  })

  it('ignores a stale async accumulate write after the archive watermark advances', async () => {
    const durable = new MemoryLongTermStore()
    const task = makeTask()
    await durable.saveTask(task)
    const event: TaskEvent = {
      ...makeEvent(0),
      seriesId: 'output',
      seriesMode: 'accumulate',
      seriesAccField: 'delta',
      data: { delta: 'a' },
    }
    await expect(
      durable.accumulateSeries('task-1', 'output', event, 'delta'),
    ).resolves.toMatchObject({ data: { delta: 'a' } })
    const metadata = await durable.getTaskStorageMetadata('task-1')
    if (!metadata) throw new Error('storage metadata is missing')
    await expect(durable.compareAndSetTaskStorageMetadata({
      taskId: 'task-1',
      expectedStorageState: metadata.storageState,
      expectedStorageEpoch: metadata.storageEpoch,
      expectedReleaseGeneration: metadata.activeReleaseGeneration,
      next: { ...metadata, archiveWatermark: 0 },
    })).resolves.toBe(true)

    await expect(
      durable.accumulateSeries('task-1', 'output', event, 'delta'),
    ).resolves.toMatchObject({ data: { delta: 'a' } })
  })

  it('rejects a publish queued behind an atomic terminal transition', async () => {
    let unblockTransition!: () => void
    let reportBlocked!: () => void
    const transitionBlocked = new Promise<void>((resolve) => {
      reportBlocked = resolve
    })
    const transitionGate = new Promise<void>((resolve) => {
      unblockTransition = resolve
    })
    class BlockingTransitionStore extends MemoryShortTermStore {
      blockNextTransition = false

      override async commitTaskEventsFenced(
        ...args: Parameters<MemoryShortTermStore['commitTaskEventsFenced']>
      ) {
        if (this.blockNextTransition) {
          this.blockNextTransition = false
          reportBlocked()
          await transitionGate
        }
        return super.commitTaskEventsFenced(...args)
      }
    }
    const hot = new BlockingTransitionStore()
    const durable = new MemoryLongTermStore()
    const engine = new TaskEngine({
      shortTermStore: hot,
      longTermStore: durable,
      broadcast: new MemoryBroadcastProvider(),
    })
    await engine.createTask({ id: 'task-1' })
    await engine.transitionTask('task-1', 'running')
    hot.blockNextTransition = true
    const transitioning = engine.transitionTask('task-1', 'completed')
    await transitionBlocked
    const publishing = engine.publishEvent('task-1', {
      type: 'late.event',
      level: 'info',
      data: {},
    })
    unblockTransition()

    await expect(transitioning).resolves.toMatchObject({ status: 'completed' })
    await expect(publishing).rejects.toThrow(
      'Cannot publish to task in terminal status: completed',
    )
    await expect(hot.getEvents('task-1')).resolves.toHaveLength(2)
  })

  it('does not recreate a cold task with the same explicit identity', async () => {
    const hot = new MemoryShortTermStore()
    const durable = new MemoryLongTermStore()
    const engine = new TaskEngine({
      shortTermStore: hot,
      longTermStore: durable,
      broadcast: new MemoryBroadcastProvider(),
    })
    await engine.createTask({ id: 'task-1' })
    await engine.transitionTask('task-1', 'running')
    const [statusEvent] = await hot.getEvents('task-1')
    await engine.releaseTaskStorage('task-1', {
      expectedLastEventIndex: statusEvent!.index,
      inactiveSince: statusEvent!.timestamp,
    })

    await expect(engine.createTask({ id: 'task-1' })).rejects.toMatchObject({
      name: 'TaskConflictError',
    })
    await expect(hot.getTaskStoragePresence('task-1')).resolves.toMatchObject({
      task: false,
      writeFence: false,
    })
  })

  it('claims an explicit task identity once across concurrent creators', async () => {
    const hot = new MemoryShortTermStore()
    const durable = new MemoryLongTermStore()
    const engine = new TaskEngine({
      shortTermStore: hot,
      longTermStore: durable,
      broadcast: new MemoryBroadcastProvider(),
    })

    const results = await Promise.allSettled([
      engine.createTask({ id: 'task-1', metadata: { creator: 1 } }),
      engine.createTask({ id: 'task-1', metadata: { creator: 2 } }),
    ])

    expect(results.filter((result) => result.status === 'fulfilled')).toHaveLength(1)
    const rejected = results.find(
      (result): result is PromiseRejectedResult => result.status === 'rejected',
    )
    expect(rejected?.reason).toMatchObject({ name: 'TaskConflictError' })
  })

  it('allows only one concurrent terminal transition across engine instances', async () => {
    const hot = new MemoryShortTermStore()
    const durable = new MemoryLongTermStore()
    const first = new TaskEngine({
      shortTermStore: hot,
      longTermStore: durable,
      broadcast: new MemoryBroadcastProvider(),
    })
    const second = new TaskEngine({
      shortTermStore: hot,
      longTermStore: durable,
      broadcast: new MemoryBroadcastProvider(),
    })
    await first.createTask({ id: 'task-1' })
    await first.transitionTask('task-1', 'running')

    const results = await Promise.allSettled([
      first.transitionTask('task-1', 'completed'),
      second.transitionTask('task-1', 'failed', {
        error: { code: 'failed', message: 'lost race' },
      }),
    ])

    expect(results.filter((result) => result.status === 'fulfilled')).toHaveLength(1)
    expect(results.filter((result) => result.status === 'rejected')).toHaveLength(1)
    const task = await hot.getTask('task-1')
    const statusEvents = (await hot.getEvents('task-1')).filter(
      (event) => event.type === 'taskcast:status',
    )
    expect(statusEvents).toHaveLength(2)
    expect(statusEvents.at(-1)?.data).toMatchObject({ status: task?.status })
  })

  it('aborts a durable creation claim when the first hot write fails', async () => {
    class FailOnceSaveStore extends MemoryShortTermStore {
      private shouldFail = true

      override async saveTask(task: Task): Promise<void> {
        if (this.shouldFail) {
          this.shouldFail = false
          throw new Error('redis unavailable')
        }
        return super.saveTask(task)
      }
    }

    const hot = new FailOnceSaveStore()
    const durable = new MemoryLongTermStore()
    const engine = new TaskEngine({
      shortTermStore: hot,
      longTermStore: durable,
      broadcast: new MemoryBroadcastProvider(),
    })

    await expect(engine.createTask({ id: 'task-1' })).rejects.toThrow(
      'redis unavailable',
    )
    await expect(durable.getTask('task-1')).resolves.toBeNull()
    await expect(engine.createTask({ id: 'task-1' })).resolves.toMatchObject({
      id: 'task-1',
      status: 'pending',
    })
  })

  it('rejects an archive batch after its durable release generation is superseded', async () => {
    const durable = new MemoryLongTermStore()
    const task = makeTask()
    const event = makeEvent(0)
    await durable.saveTask(task)
    const metadata = (await durable.getTaskStorageMetadata(task.id))!
    const releasing = {
      ...metadata,
      storageState: 'releasing' as const,
      activeReleaseGeneration: 'generation-1',
    }
    await durable.compareAndSetTaskStorageMetadata({
      taskId: task.id,
      expectedStorageState: 'hot',
      expectedStorageEpoch: 1,
      expectedReleaseGeneration: null,
      next: releasing,
    })
    const pageDigest = await computeArchiveSourcePageDigest([event])
    const manifest = {
      priorWatermark: -1,
      targetWatermark: 0,
      sourceEntryCount: 1,
      sourceDigest: await computeArchiveSourceDigest([pageDigest]),
      seriesStateDigest: await computeSeriesStateDigest([]),
      expectedBatchOrdinals: [0],
    }
    await durable.beginArchive({
      taskId: task.id,
      generation: 'generation-1',
      storageEpoch: 1,
      targetWatermark: 0,
      manifest,
      status: 'open',
      createdAt: 1,
      updatedAt: 1,
    })
    await durable.compareAndSetTaskStorageMetadata({
      taskId: task.id,
      expectedStorageState: 'releasing',
      expectedStorageEpoch: 1,
      expectedReleaseGeneration: 'generation-1',
      next: {
        ...releasing,
        activeReleaseGeneration: 'generation-2',
      },
    })
    const receipt = {
      taskId: task.id,
      generation: 'generation-1',
      ordinal: 0,
      previousBatchDigest: null,
      batchDigest: await computeArchiveBatchDigest(null, [event], []),
      eventCount: 1,
      firstIndex: 0,
      lastIndex: 0,
    }

    await expect(
      durable.archiveBatch(task.id, 'generation-1', {
        receipt,
        events: [event],
        seriesLatest: [],
      }),
    ).rejects.toMatchObject({ code: 'storage_integrity_error' })
  })

  it('fences a publisher that obtained a token before release closed writes', async () => {
    let unblockCommit!: () => void
    let reportBlocked!: () => void
    const commitBlocked = new Promise<void>((resolve) => {
      reportBlocked = resolve
    })
    const commitGate = new Promise<void>((resolve) => {
      unblockCommit = resolve
    })
    class BlockingCommitStore extends MemoryShortTermStore {
      blockNextCommit = false

      override async commitEventFenced(
        ...args: Parameters<MemoryShortTermStore['commitEventFenced']>
      ) {
        if (this.blockNextCommit) {
          this.blockNextCommit = false
          reportBlocked()
          await commitGate
        }
        return super.commitEventFenced(...args)
      }
    }
    const hot = new BlockingCommitStore()
    const durable = new CoordinatorLongTermStore()
    const engine = new TaskEngine({
      shortTermStore: hot,
      longTermStore: durable,
      broadcast: new MemoryBroadcastProvider(),
    })
    await engine.createTask({ id: 'task-1' })
    await engine.transitionTask('task-1', 'running')
    const [statusEvent] = await hot.getEvents('task-1')
    hot.blockNextCommit = true
    const publishing = engine.publishEvent('task-1', {
      type: 'llm.delta',
      level: 'info',
      data: { delta: 'racing' },
    })
    await commitBlocked

    const released = await engine.releaseTaskStorage('task-1', {
      expectedLastEventIndex: statusEvent!.index,
      inactiveSince: statusEvent!.timestamp,
    })
    unblockCommit()

    expect(released.storageState).toBe('cold')
    await expect(publishing).rejects.toMatchObject({
      code: 'storage_busy',
      retryable: true,
    })
    await expect(durable.getEvents('task-1')).resolves.toEqual([statusEvent])
  })
})
