import { ulid } from 'ulidx'
import {
  computeArchiveBatchDigest,
  computeArchiveSourceDigest,
  computeArchiveSourcePageDigest,
  computeSeriesStateDigest,
} from './storage-digest.js'
import {
  StorageBusyError,
  StorageFenceConflictError,
  StorageIntegrityError,
  StoragePreconditionError,
  StorageReleaseUnsupportedError,
  StorageUnavailableError,
  type ArchiveBatch,
  type ArchiveGeneration,
  type ArchiveSourcePage,
  type ArchiveSourceManifest,
  type DurableSeriesState,
  type HotWriteToken,
  type LongTermStore,
  type ReleasePreconditions,
  type ReleaseResult,
  type ShortTermStore,
  type StorageLease,
  type TaskEvent,
  type TaskStorageMetadata,
} from './types.js'

export interface StorageCoordinatorOptions {
  shortTermStore: ShortTermStore
  longTermStore: LongTermStore
  archiveBatchSize?: number
  storageLockTtlMs?: number
  rehydrateReplayEvents?: number
  requiredStorageProtocolVersion?: number
  generateId?: () => string
  now?: () => number
  observe?: (observation: Record<string, unknown>) => void
}

interface LifecycleMethods {
  acquireStorageLock: NonNullable<ShortTermStore['acquireStorageLock']>
  renewStorageLock: NonNullable<ShortTermStore['renewStorageLock']>
  releaseStorageLock: NonNullable<ShortTermStore['releaseStorageLock']>
  getWriteFence: NonNullable<ShortTermStore['getWriteFence']>
  closeWriteFence: NonNullable<ShortTermStore['closeWriteFence']>
  reopenWriteFence: NonNullable<ShortTermStore['reopenWriteFence']>
  restoreHotTaskFenced: NonNullable<ShortTermStore['restoreHotTaskFenced']>
  readArchiveSourcePage: NonNullable<ShortTermStore['readArchiveSourcePage']>
  deleteTaskStorageFenced: NonNullable<ShortTermStore['deleteTaskStorageFenced']>
  getTaskStoragePresence: NonNullable<ShortTermStore['getTaskStoragePresence']>
  listStorageWriters: NonNullable<ShortTermStore['listStorageWriters']>
}

interface DurableMethods {
  getTaskStorageMetadata: NonNullable<LongTermStore['getTaskStorageMetadata']>
  compareAndSetTaskStorageMetadata: NonNullable<
    LongTermStore['compareAndSetTaskStorageMetadata']
  >
  beginArchive: NonNullable<LongTermStore['beginArchive']>
  archiveBatch: NonNullable<LongTermStore['archiveBatch']>
  finalizeArchive: NonNullable<LongTermStore['finalizeArchive']>
  getArchiveWatermark: NonNullable<LongTermStore['getArchiveWatermark']>
  getLastEventIndex: NonNullable<LongTermStore['getLastEventIndex']>
  getRecentEvents: NonNullable<LongTermStore['getRecentEvents']>
  getDurableSeriesState: NonNullable<LongTermStore['getDurableSeriesState']>
}

interface SourceDescription {
  manifest: ArchiveSourceManifest
  seriesLatest: DurableSeriesState[]
  maxEventTimestamp: number | null
}

interface ReleaseObservationProgress {
  sourceEventCount: number
  sourceBytes: number
}

interface RehydrateObservationProgress {
  replayEventCount: number
  archiveWatermark: number | null
  maxEventIndex: number | null
  storageEpoch: number | null
}

export class StorageCoordinator {
  private readonly shortTermStore: ShortTermStore
  private readonly longTermStore: LongTermStore
  private readonly archiveBatchSize: number
  private readonly storageLockTtlMs: number
  private readonly rehydrateReplayEvents: number
  private readonly requiredStorageProtocolVersion: number
  private readonly generateId: () => string
  private readonly now: () => number
  private readonly observe: (observation: Record<string, unknown>) => void

  constructor(options: StorageCoordinatorOptions) {
    this.shortTermStore = options.shortTermStore
    this.longTermStore = options.longTermStore
    this.archiveBatchSize = options.archiveBatchSize ?? 1_000
    this.storageLockTtlMs = options.storageLockTtlMs ?? 30_000
    this.rehydrateReplayEvents = options.rehydrateReplayEvents ?? 1_000
    this.requiredStorageProtocolVersion =
      options.requiredStorageProtocolVersion ?? 2
    this.generateId = options.generateId ?? ulid
    this.now = options.now ?? Date.now
    this.observe = (observation) => {
      try {
        options.observe?.(observation)
      } catch {
        // Observability must never affect storage correctness.
      }
    }

    if (!Number.isSafeInteger(this.archiveBatchSize) || this.archiveBatchSize <= 0) {
      throw new StorageIntegrityError('Archive batch size must be a positive integer')
    }
    if (!Number.isSafeInteger(this.storageLockTtlMs) || this.storageLockTtlMs <= 0) {
      throw new StorageIntegrityError('Storage lock TTL must be a positive integer')
    }
    if (
      !Number.isSafeInteger(this.rehydrateReplayEvents) ||
      this.rehydrateReplayEvents < 0
    ) {
      throw new StorageIntegrityError(
        'Rehydrate replay event count must be a non-negative integer',
      )
    }
  }

  async releaseTaskStorage(
    taskId: string,
    preconditions: ReleasePreconditions,
  ): Promise<ReleaseResult> {
    const startedAt = this.now()
    const progress: ReleaseObservationProgress = {
      sourceEventCount: 0,
      sourceBytes: 0,
    }
    try {
      const result = await this.releaseTaskStorageInner(
        taskId,
        preconditions,
        progress,
      )
      this.observe({
        event: 'storage_release',
        taskId,
        outcome: result.released ? 'released' : 'noop',
        durationMs: Math.max(0, this.now() - startedAt),
        sourceEventCount: progress.sourceEventCount,
        sourceBytes: progress.sourceBytes,
        storageStateBefore: result.released ? 'hot' : 'cold',
        storageStateAfter: result.storageState,
        archiveWatermark: result.archiveWatermark,
      })
      return result
    } catch (error) {
      this.observe({
        event: 'storage_release',
        taskId,
        outcome: 'failed',
        durationMs: Math.max(0, this.now() - startedAt),
        sourceEventCount: progress.sourceEventCount,
        sourceBytes: progress.sourceBytes,
        storageStateBefore: 'hot',
        storageStateAfter: 'hot',
        errorCode: this.errorCode(error),
        error: error instanceof Error ? error.message : String(error),
      })
      throw error
    }
  }

  private async releaseTaskStorageInner(
    taskId: string,
    preconditions: ReleasePreconditions,
    progress: ReleaseObservationProgress,
  ): Promise<ReleaseResult> {
    const hot = this.requireHotLifecycle()
    const durable = this.requireDurableLifecycle()
    let metadata = await durable.getTaskStorageMetadata.call(
      this.longTermStore,
      taskId,
    )
    if (!metadata) {
      throw new StorageIntegrityError(`Task storage metadata does not exist: ${taskId}`)
    }
    if (metadata.storageState === 'cold') {
      return {
        taskId,
        storageState: 'cold',
        archiveWatermark: metadata.archiveWatermark,
        released: false,
      }
    }
    if (metadata.storageState !== 'hot') {
      throw new StorageBusyError('Task storage is already being released')
    }

    const generation = this.generateId()
    const lockToken = this.generateId()
    const lease = await hot.acquireStorageLock.call(
      this.shortTermStore,
      taskId,
      lockToken,
      generation,
      this.storageLockTtlMs,
    )
    if (!lease) throw new StorageBusyError()

    let leaseLost = false
    let fenceClosed = false
    let hotDeleted = false
    const renew = async (): Promise<void> => {
      if (leaseLost) throw new StorageFenceConflictError('Storage lease was lost')
      let owned = false
      try {
        owned = await hot.renewStorageLock.call(
          this.shortTermStore,
          lease,
          this.storageLockTtlMs,
        )
      } catch {
        leaseLost = true
        throw new StorageFenceConflictError('Storage lease renewal failed')
      }
      if (!owned) {
        leaseLost = true
        throw new StorageFenceConflictError('Storage lease was lost')
      }
    }

    try {
      const writers = await hot.listStorageWriters.call(this.shortTermStore)
      const incompatible = writers.filter(
        (writer) =>
          writer.storageProtocolVersion < this.requiredStorageProtocolVersion,
      )
      if (incompatible.length > 0) {
        throw new StorageUnavailableError(
          `Storage release is blocked by incompatible writers: ${incompatible
            .map((writer) => writer.instanceId)
            .join(', ')}`,
        )
      }

      await renew()
      const closed = await hot.closeWriteFence.call(
        this.shortTermStore,
        lease,
        metadata.storageEpoch,
      )
      fenceClosed = true
      if (
        !Number.isSafeInteger(preconditions.expectedLastEventIndex) ||
        preconditions.expectedLastEventIndex < -1 ||
        closed.highWatermark !== preconditions.expectedLastEventIndex
      ) {
        throw new StoragePreconditionError('Task event index changed before storage release')
      }
      if (
        !Number.isFinite(preconditions.inactiveSince) ||
        (metadata.lastEventAt !== null &&
          metadata.lastEventAt > preconditions.inactiveSince)
      ) {
        throw new StoragePreconditionError('Task has activity newer than the release cutoff')
      }

      const releasing: TaskStorageMetadata = {
        ...metadata,
        storageState: 'releasing',
        activeReleaseGeneration: generation,
        coldAt: null,
      }
      await renew()
      const installed = await durable.compareAndSetTaskStorageMetadata.call(
        this.longTermStore,
        {
          taskId,
          expectedStorageState: 'hot',
          expectedStorageEpoch: metadata.storageEpoch,
          expectedReleaseGeneration: null,
          next: releasing,
        },
      )
      if (!installed) {
        throw new StorageBusyError('Task storage metadata changed before release')
      }
      metadata = releasing

      const description = await this.describeArchiveSource(
        taskId,
        closed.highWatermark,
        metadata.archiveWatermark,
        hot,
        renew,
      )
      progress.sourceEventCount = description.manifest.sourceEntryCount
      if (
        description.maxEventTimestamp !== null &&
        description.maxEventTimestamp > preconditions.inactiveSince
      ) {
        throw new StoragePreconditionError(
          'Task source has activity newer than the release cutoff',
        )
      }
      const now = this.now()
      const archive: ArchiveGeneration = {
        taskId,
        generation,
        storageEpoch: metadata.storageEpoch,
        targetWatermark: closed.highWatermark,
        manifest: description.manifest,
        status: 'open',
        createdAt: now,
        updatedAt: now,
      }
      await renew()
      await durable.beginArchive.call(this.longTermStore, archive)

      let ordinal = 0
      let previousBatchDigest: string | null = null
      for await (const events of this.readSourceBatches(
        taskId,
        closed.highWatermark,
        metadata.archiveWatermark,
        hot,
      )) {
        progress.sourceBytes += new TextEncoder().encode(
          JSON.stringify(events),
        ).byteLength
        await renew()
        const batchDigest = await computeArchiveBatchDigest(
          previousBatchDigest,
          events,
          [],
        )
        const batch: ArchiveBatch = {
          receipt: {
            taskId,
            generation,
            ordinal,
            previousBatchDigest,
            batchDigest,
            entryCount: events.length,
            firstIndex: events[0]?.index ?? null,
            lastIndex: events.at(-1)?.index ?? null,
          },
          events,
          seriesLatest: [],
        }
        await durable.archiveBatch.call(
          this.longTermStore,
          taskId,
          generation,
          batch,
        )
        previousBatchDigest = batchDigest
        ordinal += 1
      }
      if (ordinal !== description.manifest.expectedBatchOrdinals.length) {
        throw new StorageIntegrityError('Archive source changed between sealing passes')
      }

      const task = await this.shortTermStore.getTask(taskId)
      if (!task) throw new StorageIntegrityError('Hot task disappeared during release')
      await renew()
      await durable.finalizeArchive.call(
        this.longTermStore,
        taskId,
        generation,
        task,
        description.seriesLatest,
      )
      await renew()
      const watermark = await durable.getArchiveWatermark.call(
        this.longTermStore,
        taskId,
      )
      const current = await durable.getTaskStorageMetadata.call(
        this.longTermStore,
        taskId,
      )
      if (
        watermark < closed.highWatermark ||
        !current ||
        current.storageState !== 'releasing' ||
        current.storageEpoch !== metadata.storageEpoch ||
        current.activeReleaseGeneration !== generation
      ) {
        this.observe({
          event: 'storage_watermark_mismatch',
          operation: 'release',
          taskId,
          expectedWatermark: closed.highWatermark,
          actualWatermark: watermark,
          storageState: current?.storageState ?? null,
          storageEpoch: current?.storageEpoch ?? null,
        })
        throw new StorageIntegrityError('Durable archive read-back did not prove release')
      }

      await renew()
      await hot.deleteTaskStorageFenced.call(
        this.shortTermStore,
        lease,
        metadata.storageEpoch,
      )
      hotDeleted = true
      await renew()
      const cold: TaskStorageMetadata = {
        ...current,
        storageState: 'cold',
        activeReleaseGeneration: null,
        archiveWatermark: watermark,
        coldAt: this.now(),
      }
      const committed = await durable.compareAndSetTaskStorageMetadata.call(
        this.longTermStore,
        {
          taskId,
          expectedStorageState: 'releasing',
          expectedStorageEpoch: current.storageEpoch,
          expectedReleaseGeneration: generation,
          next: cold,
        },
      )
      if (!committed) {
        throw new StorageFenceConflictError('Task storage cold transition lost its fence')
      }
      return {
        taskId,
        storageState: 'cold',
        archiveWatermark: watermark,
        released: true,
      }
    } catch (error) {
      if (!leaseLost && fenceClosed && !hotDeleted) {
        await this.reopenAfterFailure(taskId, lease, metadata, hot, durable)
      }
      throw error
    } finally {
      if (!leaseLost) {
        await hot.releaseStorageLock.call(this.shortTermStore, lease).catch(() => false)
      }
    }
  }

  async ensureTaskHotForWrite(
    taskId: string,
    rehydrateCold = true,
  ): Promise<HotWriteToken> {
    const hot = this.requireHotLifecycle()
    const durable = this.requireDurableLifecycle()
    for (let attempt = 0; attempt < 3; attempt++) {
      const metadata = await durable.getTaskStorageMetadata.call(
        this.longTermStore,
        taskId,
      )
      if (!metadata) {
        throw new StorageIntegrityError(
          `Task storage metadata does not exist: ${taskId}`,
        )
      }
      if (metadata.storageState === 'releasing') {
        throw new StorageBusyError('Task storage lifecycle operation is in progress')
      }
      if (metadata.storageState === 'cold') {
        if (!rehydrateCold) {
          throw new StorageBusyError(
            'Task became cold after the write mutation started',
          )
        }
        return this.rehydrateColdTask(taskId, metadata, hot, durable)
      }
      const fence = await hot.getWriteFence.call(this.shortTermStore, taskId)
      if (
        fence?.acceptingWrites &&
        fence.activeReleaseGeneration === null &&
        fence.storageEpoch === metadata.storageEpoch
      ) {
        return { taskId, storageEpoch: fence.storageEpoch }
      }
      if (
        fence?.acceptingWrites &&
        fence.activeReleaseGeneration === null &&
        metadata.activeReleaseGeneration === null &&
        fence.storageEpoch > metadata.storageEpoch
      ) {
        const repaired = await durable.compareAndSetTaskStorageMetadata.call(
          this.longTermStore,
          {
            taskId,
            expectedStorageState: 'hot',
            expectedStorageEpoch: metadata.storageEpoch,
            expectedReleaseGeneration: null,
            next: { ...metadata, storageEpoch: fence.storageEpoch },
          },
        )
        if (repaired) return { taskId, storageEpoch: fence.storageEpoch }
        continue
      }
      throw new StorageFenceConflictError(
        'Hot task write fence does not match durable metadata',
      )
    }
    throw new StorageFenceConflictError(
      'Hot task write fence repair lost its metadata race',
    )
  }

  private async rehydrateColdTask(
    taskId: string,
    initial: TaskStorageMetadata,
    hot: LifecycleMethods,
    durable: DurableMethods,
  ): Promise<HotWriteToken> {
    const startedAt = this.now()
    const progress: RehydrateObservationProgress = {
      replayEventCount: 0,
      archiveWatermark: initial.archiveWatermark,
      maxEventIndex: null,
      storageEpoch: initial.storageEpoch,
    }
    try {
      const token = await this.rehydrateColdTaskInner(
        taskId,
        initial,
        hot,
        durable,
        progress,
      )
      this.observe({
        event: 'storage_rehydrate',
        taskId,
        outcome: 'rehydrated',
        durationMs: Math.max(0, this.now() - startedAt),
        replayEventCount: progress.replayEventCount,
        archiveWatermark: progress.archiveWatermark,
        maxEventIndex: progress.maxEventIndex,
        storageEpoch: token.storageEpoch,
        storageStateBefore: 'cold',
        storageStateAfter: 'hot',
      })
      return token
    } catch (error) {
      this.observe({
        event: 'storage_rehydrate',
        taskId,
        outcome: 'failed',
        durationMs: Math.max(0, this.now() - startedAt),
        replayEventCount: progress.replayEventCount,
        archiveWatermark: progress.archiveWatermark,
        maxEventIndex: progress.maxEventIndex,
        storageEpoch: progress.storageEpoch,
        storageStateBefore: 'cold',
        storageStateAfter: 'cold',
        errorCode: this.errorCode(error),
        error: error instanceof Error ? error.message : String(error),
      })
      throw error
    }
  }

  private async rehydrateColdTaskInner(
    taskId: string,
    initial: TaskStorageMetadata,
    hot: LifecycleMethods,
    durable: DurableMethods,
    progress: RehydrateObservationProgress,
  ): Promise<HotWriteToken> {
    const generation = this.generateId()
    const lease = await hot.acquireStorageLock.call(
      this.shortTermStore,
      taskId,
      this.generateId(),
      generation,
      this.storageLockTtlMs,
    )
    if (!lease) throw new StorageBusyError('Task storage rehydration is already in progress')

    let leaseLost = false
    const renew = async (): Promise<void> => {
      let owned = false
      try {
        owned = await hot.renewStorageLock.call(
          this.shortTermStore,
          lease,
          this.storageLockTtlMs,
        )
      } catch {
        leaseLost = true
        throw new StorageFenceConflictError('Storage rehydration lease renewal failed')
      }
      if (!owned) {
        leaseLost = true
        throw new StorageFenceConflictError('Storage rehydration lease was lost')
      }
    }

    try {
      await renew()
      const metadata = await durable.getTaskStorageMetadata.call(
        this.longTermStore,
        taskId,
      )
      if (!metadata) {
        throw new StorageIntegrityError(
          `Task storage metadata does not exist: ${taskId}`,
        )
      }
      if (metadata.storageState === 'releasing') {
        throw new StorageBusyError('Task storage lifecycle operation is in progress')
      }
      if (metadata.storageState === 'hot') {
        const fence = await hot.getWriteFence.call(this.shortTermStore, taskId)
        if (
          fence?.acceptingWrites &&
          fence.activeReleaseGeneration === null &&
          fence.storageEpoch === metadata.storageEpoch
        ) {
          return { taskId, storageEpoch: fence.storageEpoch }
        }
        throw new StorageFenceConflictError(
          'Hot task write fence does not match durable metadata',
        )
      }
      if (
        metadata.storageEpoch !== initial.storageEpoch ||
        metadata.activeReleaseGeneration !== null
      ) {
        throw new StorageFenceConflictError(
          'Cold task metadata changed before rehydration',
        )
      }

      const [presence, existingFence] = await Promise.all([
        hot.getTaskStoragePresence.call(this.shortTermStore, taskId),
        hot.getWriteFence.call(this.shortTermStore, taskId),
      ])
      if (
        presence.task &&
        existingFence?.acceptingWrites &&
        existingFence.activeReleaseGeneration === null &&
        existingFence.storageEpoch > metadata.storageEpoch
      ) {
        const adopted = await durable.compareAndSetTaskStorageMetadata.call(
          this.longTermStore,
          {
            taskId,
            expectedStorageState: 'cold',
            expectedStorageEpoch: metadata.storageEpoch,
            expectedReleaseGeneration: null,
            next: {
              ...metadata,
              storageState: 'hot',
              storageEpoch: existingFence.storageEpoch,
              coldAt: null,
            },
          },
        )
        if (!adopted) {
          throw new StorageFenceConflictError(
            'Restored hot epoch lost its metadata recovery race',
          )
        }
        return { taskId, storageEpoch: existingFence.storageEpoch }
      }
      if (
        presence.task ||
        presence.eventCount !== 0 ||
        presence.nextIndex ||
        presence.seriesStateCount !== 0 ||
        presence.writeFence
      ) {
        throw new StorageIntegrityError(
          'Cold task has partial or stale hot storage',
        )
      }

      await renew()
      const [task, maxEventIndex, replayEvents, seriesLatest] = await Promise.all([
        this.longTermStore.getTask(taskId),
        durable.getLastEventIndex.call(this.longTermStore, taskId),
        durable.getRecentEvents.call(
          this.longTermStore,
          taskId,
          this.rehydrateReplayEvents,
        ),
        durable.getDurableSeriesState.call(this.longTermStore, taskId),
      ])
      progress.replayEventCount = replayEvents.length
      progress.maxEventIndex = maxEventIndex
      progress.archiveWatermark = metadata.archiveWatermark
      progress.storageEpoch = metadata.storageEpoch
      if (!task) {
        throw new StorageIntegrityError(`Durable task does not exist: ${taskId}`)
      }
      if (
        !Number.isSafeInteger(maxEventIndex) ||
        maxEventIndex < metadata.archiveWatermark ||
        replayEvents.some(
          (event) =>
            event.taskId !== taskId ||
            !Number.isSafeInteger(event.index) ||
            event.index > maxEventIndex,
        )
      ) {
        this.observe({
          event: 'storage_watermark_mismatch',
          operation: 'rehydrate',
          taskId,
          expectedWatermark: metadata.archiveWatermark,
          actualWatermark: maxEventIndex,
          replayEventCount: replayEvents.length,
          storageEpoch: metadata.storageEpoch,
        })
        throw new StorageIntegrityError('Durable rehydrate snapshot is inconsistent')
      }
      const nextEpoch = metadata.storageEpoch + 1
      if (!Number.isSafeInteger(nextEpoch)) {
        throw new StorageIntegrityError('Task storage epoch exceeds safe bounds')
      }
      await renew()
      const token = await hot.restoreHotTaskFenced.call(
        this.shortTermStore,
        {
          task,
          archiveWatermark: metadata.archiveWatermark,
          maxEventIndex,
          replayEvents,
          seriesLatest,
          storageEpoch: metadata.storageEpoch,
        },
        lease,
        nextEpoch,
      )
      await renew()
      const installed = await durable.compareAndSetTaskStorageMetadata.call(
        this.longTermStore,
        {
          taskId,
          expectedStorageState: 'cold',
          expectedStorageEpoch: metadata.storageEpoch,
          expectedReleaseGeneration: null,
          next: {
            ...metadata,
            storageState: 'hot',
            storageEpoch: nextEpoch,
            activeReleaseGeneration: null,
            coldAt: null,
          },
        },
      )
      if (installed) return token

      const current = await durable.getTaskStorageMetadata.call(
        this.longTermStore,
        taskId,
      )
      if (
        current?.storageState === 'hot' &&
        current.storageEpoch === nextEpoch &&
        current.activeReleaseGeneration === null
      ) {
        return token
      }
      await renew()
      await hot.closeWriteFence.call(this.shortTermStore, lease, nextEpoch)
      await hot.deleteTaskStorageFenced.call(
        this.shortTermStore,
        lease,
        nextEpoch,
      )
      throw new StorageFenceConflictError(
        'Restored hot epoch lost its durable metadata race',
      )
    } finally {
      if (!leaseLost) {
        await hot.releaseStorageLock.call(this.shortTermStore, lease).catch(() => false)
      }
    }
  }

  async recoverTaskStorage(taskId: string): Promise<ReleaseResult> {
    const hot = this.requireHotLifecycle()
    const durable = this.requireDurableLifecycle()
    const initial = await durable.getTaskStorageMetadata.call(
      this.longTermStore,
      taskId,
    )
    if (!initial) {
      throw new StorageIntegrityError(`Task storage metadata does not exist: ${taskId}`)
    }
    if (initial.storageState !== 'releasing') {
      return {
        taskId,
        storageState: initial.storageState,
        archiveWatermark: initial.archiveWatermark,
        released: false,
      }
    }

    const generation = this.generateId()
    const lease = await hot.acquireStorageLock.call(
      this.shortTermStore,
      taskId,
      this.generateId(),
      generation,
      this.storageLockTtlMs,
    )
    if (!lease) throw new StorageBusyError()

    let leaseLost = false
    const renew = async (): Promise<void> => {
      const owned = await hot.renewStorageLock.call(
        this.shortTermStore,
        lease,
        this.storageLockTtlMs,
      )
      if (!owned) {
        leaseLost = true
        throw new StorageFenceConflictError('Storage recovery lease was lost')
      }
    }

    try {
      await renew()
      const presence = await hot.getTaskStoragePresence.call(
        this.shortTermStore,
        taskId,
      )
      const fence = await hot.getWriteFence.call(this.shortTermStore, taskId)
      if (
        initial.storageState === 'releasing' &&
        presence.task &&
        fence?.acceptingWrites &&
        fence.activeReleaseGeneration === null &&
        fence.storageEpoch > initial.storageEpoch
      ) {
        const reopened: TaskStorageMetadata = {
          ...initial,
          storageState: 'hot',
          storageEpoch: fence.storageEpoch,
          activeReleaseGeneration: null,
          coldAt: null,
        }
        await renew()
        const repaired = await durable.compareAndSetTaskStorageMetadata.call(
          this.longTermStore,
          {
            taskId,
            expectedStorageState: 'releasing',
            expectedStorageEpoch: initial.storageEpoch,
            expectedReleaseGeneration: initial.activeReleaseGeneration,
            next: reopened,
          },
        )
        if (!repaired) {
          throw new StorageFenceConflictError(
            'Recovered hot epoch lost its metadata race',
          )
        }
        return {
          taskId,
          storageState: 'hot',
          archiveWatermark: reopened.archiveWatermark,
          released: false,
        }
      }
      if (
        !presence.task &&
        !presence.writeFence &&
        presence.eventCount === 0 &&
        !presence.nextIndex &&
        presence.seriesStateCount === 0
      ) {
        const [watermark, durableLastIndex] = await Promise.all([
          durable.getArchiveWatermark.call(this.longTermStore, taskId),
          durable.getLastEventIndex.call(this.longTermStore, taskId),
        ])
        if (watermark < durableLastIndex) {
          throw new StorageIntegrityError(
            'Missing hot storage is not covered by the durable watermark',
          )
        }
        const adopted: TaskStorageMetadata = {
          ...initial,
          activeReleaseGeneration: generation,
          archiveWatermark: watermark,
        }
        await renew()
        const installed = await durable.compareAndSetTaskStorageMetadata.call(
          this.longTermStore,
          {
            taskId,
            expectedStorageState: 'releasing',
            expectedStorageEpoch: initial.storageEpoch,
            expectedReleaseGeneration: initial.activeReleaseGeneration,
            next: adopted,
          },
        )
        if (!installed) {
          throw new StorageFenceConflictError(
            'Storage recovery generation was not installed',
          )
        }
        const cold: TaskStorageMetadata = {
          ...adopted,
          storageState: 'cold',
          activeReleaseGeneration: null,
          coldAt: this.now(),
        }
        await renew()
        const committed = await durable.compareAndSetTaskStorageMetadata.call(
          this.longTermStore,
          {
            taskId,
            expectedStorageState: 'releasing',
            expectedStorageEpoch: adopted.storageEpoch,
            expectedReleaseGeneration: generation,
            next: cold,
          },
        )
        if (!committed) {
          throw new StorageFenceConflictError(
            'Recovered cold transition lost its generation',
          )
        }
        return {
          taskId,
          storageState: 'cold',
          archiveWatermark: watermark,
          released: true,
        }
      }

      if (!presence.task) {
        throw new StorageIntegrityError(
          'Retained hot storage is missing its task record',
        )
      }
      await renew()
      const closed = await hot.closeWriteFence.call(
        this.shortTermStore,
        lease,
        initial.storageEpoch,
      )
      const adopted: TaskStorageMetadata = {
        ...initial,
        activeReleaseGeneration: generation,
      }
      await renew()
      const installed = await durable.compareAndSetTaskStorageMetadata.call(
        this.longTermStore,
        {
          taskId,
          expectedStorageState: 'releasing',
          expectedStorageEpoch: initial.storageEpoch,
          expectedReleaseGeneration: initial.activeReleaseGeneration,
          next: adopted,
        },
      )
      if (!installed) {
        throw new StorageFenceConflictError(
          'Storage recovery generation was not installed',
        )
      }

      const watermark = await durable.getArchiveWatermark.call(
        this.longTermStore,
        taskId,
      )
      if (watermark >= closed.highWatermark) {
        await renew()
        await hot.deleteTaskStorageFenced.call(
          this.shortTermStore,
          lease,
          adopted.storageEpoch,
        )
        await renew()
        const cold: TaskStorageMetadata = {
          ...adopted,
          storageState: 'cold',
          activeReleaseGeneration: null,
          archiveWatermark: watermark,
          coldAt: this.now(),
        }
        const committed = await durable.compareAndSetTaskStorageMetadata.call(
          this.longTermStore,
          {
            taskId,
            expectedStorageState: 'releasing',
            expectedStorageEpoch: adopted.storageEpoch,
            expectedReleaseGeneration: generation,
            next: cold,
          },
        )
        if (!committed) {
          throw new StorageFenceConflictError(
            'Recovered cold transition lost its generation',
          )
        }
        return {
          taskId,
          storageState: 'cold',
          archiveWatermark: watermark,
          released: true,
        }
      }

      await renew()
      const token: HotWriteToken = await hot.reopenWriteFence.call(
        this.shortTermStore,
        lease,
        adopted.storageEpoch,
      )
      const reopened: TaskStorageMetadata = {
        ...adopted,
        storageState: 'hot',
        storageEpoch: token.storageEpoch,
        activeReleaseGeneration: null,
        coldAt: null,
      }
      const reopenedDurable = await durable.compareAndSetTaskStorageMetadata.call(
        this.longTermStore,
        {
          taskId,
          expectedStorageState: 'releasing',
          expectedStorageEpoch: adopted.storageEpoch,
          expectedReleaseGeneration: generation,
          next: reopened,
        },
      )
      if (!reopenedDurable) {
        throw new StorageFenceConflictError(
          'Recovered hot transition lost its generation',
        )
      }
      return {
        taskId,
        storageState: 'hot',
        archiveWatermark: reopened.archiveWatermark,
        released: false,
      }
    } finally {
      if (!leaseLost) {
        await hot.releaseStorageLock.call(this.shortTermStore, lease).catch(() => false)
      }
    }
  }

  private async describeArchiveSource(
    taskId: string,
    targetWatermark: number,
    priorWatermark: number,
    hot: LifecycleMethods,
    renew: () => Promise<void>,
  ): Promise<SourceDescription> {
    const pageDigests: string[] = []
    const seriesModes = new Map<string, 'latest' | 'accumulate'>()
    let maxEventTimestamp: number | null = null
    let sourceEntryCount = 0
    let batchCount = 0
    for await (const events of this.readSourceBatches(
      taskId,
      targetWatermark,
      priorWatermark,
      hot,
      (event) => {
        if (!Number.isFinite(event.timestamp)) {
          throw new StorageIntegrityError(
            'Archive source contains an invalid event timestamp',
          )
        }
        maxEventTimestamp =
          maxEventTimestamp === null
            ? event.timestamp
            : Math.max(maxEventTimestamp, event.timestamp)
        if (
          event.seriesId &&
          (event.seriesMode === 'latest' || event.seriesMode === 'accumulate')
        ) {
          const existing = seriesModes.get(event.seriesId)
          if (existing && existing !== event.seriesMode) {
            throw new StorageIntegrityError(
              `Series mode changed for ${event.seriesId}`,
            )
          }
          seriesModes.set(event.seriesId, event.seriesMode)
        }
      },
    )) {
      await renew()
      pageDigests.push(await computeArchiveSourcePageDigest(events))
      sourceEntryCount += events.length
      batchCount += 1
    }

    const seriesLatest: DurableSeriesState[] = []
    for (const [seriesId, mode] of seriesModes) {
      const event = await this.shortTermStore.getSeriesLatest(taskId, seriesId)
      if (!event || event.index > targetWatermark) {
        throw new StorageIntegrityError(
          `Series state is missing or exceeds the release watermark: ${seriesId}`,
        )
      }
      seriesLatest.push({
        taskId,
        seriesId,
        mode,
        event,
        throughIndex: event.index,
      })
    }
    return {
      manifest: {
        priorWatermark,
        targetWatermark,
        sourceEntryCount,
        sourceDigest: await computeArchiveSourceDigest(pageDigests),
        seriesStateDigest: await computeSeriesStateDigest(seriesLatest),
        expectedBatchOrdinals: Array.from(
          { length: batchCount },
          (_, ordinal) => ordinal,
        ),
      },
      seriesLatest,
      maxEventTimestamp,
    }
  }

  private async *readSourceBatches(
    taskId: string,
    targetWatermark: number,
    priorWatermark: number,
    hot: LifecycleMethods,
    observe?: (event: TaskEvent) => void,
  ): AsyncGenerator<TaskEvent[]> {
    let cursor: string | null = null
    let previousIndex = -1
    let batch: TaskEvent[] = []
    do {
      const page: ArchiveSourcePage = await hot.readArchiveSourcePage.call(
        this.shortTermStore,
        taskId,
        targetWatermark,
        cursor,
        this.archiveBatchSize,
      )
      for (const event of page.events) {
        if (event.taskId !== taskId || event.index <= previousIndex) {
          throw new StorageIntegrityError('Archive source is not strictly ordered')
        }
        previousIndex = event.index
        observe?.(event)
        if (event.index <= priorWatermark) continue
        if (event.index > targetWatermark) {
          throw new StorageIntegrityError('Archive source exceeds its closed watermark')
        }
        batch.push(event)
        if (batch.length === this.archiveBatchSize) {
          yield batch
          batch = []
        }
      }
      if (!page.done && page.nextCursor === null) {
        throw new StorageIntegrityError('Archive source page omitted its next cursor')
      }
      cursor = page.nextCursor
      if (page.done) break
    } while (cursor !== null)
    if (batch.length > 0) yield batch
  }

  private async reopenAfterFailure(
    taskId: string,
    lease: StorageLease,
    metadata: TaskStorageMetadata,
    hot: LifecycleMethods,
    durable: DurableMethods,
  ): Promise<void> {
    try {
      const owned = await hot.renewStorageLock.call(
        this.shortTermStore,
        lease,
        this.storageLockTtlMs,
      )
      if (!owned) return
      const presence = await hot.getTaskStoragePresence.call(
        this.shortTermStore,
        taskId,
      )
      if (!presence.writeFence) return
      const token = await hot.reopenWriteFence.call(
        this.shortTermStore,
        lease,
        metadata.storageEpoch,
      )
      const current = await durable.getTaskStorageMetadata.call(
        this.longTermStore,
        taskId,
      )
      if (!current) return
      if (
        current.storageEpoch !== metadata.storageEpoch ||
        (current.storageState === 'releasing' &&
          current.activeReleaseGeneration !== lease.generation)
      ) {
        return
      }
      await durable.compareAndSetTaskStorageMetadata.call(this.longTermStore, {
        taskId,
        expectedStorageState: current.storageState,
        expectedStorageEpoch: current.storageEpoch,
        expectedReleaseGeneration: current.activeReleaseGeneration,
        next: {
          ...current,
          storageState: 'hot',
          storageEpoch: token.storageEpoch,
          activeReleaseGeneration: null,
          coldAt: null,
        },
      })
    } catch {
      // Recovery owns convergence if cleanup itself loses its fence.
    }
  }

  private requireHotLifecycle(): LifecycleMethods {
    if (!this.shortTermStore.supportsHotColdRelease) {
      throw new StorageReleaseUnsupportedError()
    }
    const methods = {
      acquireStorageLock: this.shortTermStore.acquireStorageLock,
      renewStorageLock: this.shortTermStore.renewStorageLock,
      releaseStorageLock: this.shortTermStore.releaseStorageLock,
      getWriteFence: this.shortTermStore.getWriteFence,
      closeWriteFence: this.shortTermStore.closeWriteFence,
      reopenWriteFence: this.shortTermStore.reopenWriteFence,
      restoreHotTaskFenced: this.shortTermStore.restoreHotTaskFenced,
      readArchiveSourcePage: this.shortTermStore.readArchiveSourcePage,
      deleteTaskStorageFenced: this.shortTermStore.deleteTaskStorageFenced,
      getTaskStoragePresence: this.shortTermStore.getTaskStoragePresence,
      listStorageWriters: this.shortTermStore.listStorageWriters,
    }
    if (Object.values(methods).some((method) => typeof method !== 'function')) {
      throw new StorageReleaseUnsupportedError()
    }
    return methods as LifecycleMethods
  }

  private errorCode(error: unknown): string {
    if (
      error &&
      typeof error === 'object' &&
      'code' in error &&
      typeof error.code === 'string'
    ) {
      return error.code
    }
    return 'storage_unavailable'
  }

  private requireDurableLifecycle(): DurableMethods {
    if (!this.longTermStore.supportsHotColdRelease) {
      throw new StorageReleaseUnsupportedError()
    }
    const methods = {
      getTaskStorageMetadata: this.longTermStore.getTaskStorageMetadata,
      compareAndSetTaskStorageMetadata:
        this.longTermStore.compareAndSetTaskStorageMetadata,
      beginArchive: this.longTermStore.beginArchive,
      archiveBatch: this.longTermStore.archiveBatch,
      finalizeArchive: this.longTermStore.finalizeArchive,
      getArchiveWatermark: this.longTermStore.getArchiveWatermark,
      getLastEventIndex: this.longTermStore.getLastEventIndex,
      getRecentEvents: this.longTermStore.getRecentEvents,
      getDurableSeriesState: this.longTermStore.getDurableSeriesState,
    }
    if (Object.values(methods).some((method) => typeof method !== 'function')) {
      throw new StorageReleaseUnsupportedError()
    }
    return methods as DurableMethods
  }
}
