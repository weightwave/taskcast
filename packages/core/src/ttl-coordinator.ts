import { ulid } from 'ulidx'
import { isTerminal } from './state-machine.js'
import { StorageCoordinator } from './storage-coordinator.js'
import {
  StorageBusyError,
  StorageFenceConflictError,
  StorageIntegrityError,
  StorageReleaseUnsupportedError,
  type BroadcastProvider,
  type ClosedWriteFence,
  type LongTermStore,
  type ShortTermStore,
  type StorageLease,
  type Task,
  type TaskEvent,
  type TaskStorageMetadata,
  type TerminalProjection,
  type TtlClaim,
} from './types.js'

export interface DurableTtlSweepResult {
  claimed: number
  timedOut: number
  raceLost: number
  failed: number
  projected: number
}

export interface TtlCoordinatorOptions {
  shortTermStore: ShortTermStore
  longTermStore: LongTermStore
  broadcast: BroadcastProvider
  storageCoordinator: StorageCoordinator
  storageLockTtlMs?: number
  generateId?: () => string
  now?: () => number
  onTimeoutProjected?: (task: Task, from: Task['status']) => void
}

type HotTtlMethods = {
  acquireStorageLock: NonNullable<ShortTermStore['acquireStorageLock']>
  renewStorageLock: NonNullable<ShortTermStore['renewStorageLock']>
  releaseStorageLock: NonNullable<ShortTermStore['releaseStorageLock']>
  closeWriteFence: NonNullable<ShortTermStore['closeWriteFence']>
  reopenWriteFence: NonNullable<ShortTermStore['reopenWriteFence']>
  getTaskMutationSnapshot: NonNullable<ShortTermStore['getTaskMutationSnapshot']>
  projectTerminalFenced: NonNullable<ShortTermStore['projectTerminalFenced']>
}

type DurableTtlMethods = {
  claimOverdueTasks: NonNullable<LongTermStore['claimOverdueTasks']>
  terminalizeTtlClaim: NonNullable<LongTermStore['terminalizeTtlClaim']>
  claimTerminalProjections: NonNullable<LongTermStore['claimTerminalProjections']>
  completeTerminalProjection: NonNullable<LongTermStore['completeTerminalProjection']>
  getTaskStorageMetadata: NonNullable<LongTermStore['getTaskStorageMetadata']>
  compareAndSetTaskStorageMetadata: NonNullable<
    LongTermStore['compareAndSetTaskStorageMetadata']
  >
  getLastEventIndex: NonNullable<LongTermStore['getLastEventIndex']>
}

export class TtlCoordinator {
  private readonly shortTermStore: ShortTermStore
  private readonly longTermStore: LongTermStore
  private readonly broadcast: BroadcastProvider
  private readonly storageCoordinator: StorageCoordinator
  private readonly storageLockTtlMs: number
  private readonly generateId: () => string
  private readonly now: () => number
  private readonly onTimeoutProjected:
    | ((task: Task, from: Task['status']) => void)
    | undefined

  constructor(options: TtlCoordinatorOptions) {
    this.shortTermStore = options.shortTermStore
    this.longTermStore = options.longTermStore
    this.broadcast = options.broadcast
    this.storageCoordinator = options.storageCoordinator
    this.storageLockTtlMs = options.storageLockTtlMs ?? 30_000
    this.generateId = options.generateId ?? ulid
    this.now = options.now ?? Date.now
    this.onTimeoutProjected = options.onTimeoutProjected
    if (
      !Number.isSafeInteger(this.storageLockTtlMs) ||
      this.storageLockTtlMs <= 0
    ) {
      throw new StorageIntegrityError('TTL storage lock duration must be positive')
    }
    this.requireHotMethods()
    this.requireDurableMethods()
  }

  async sweepOverdue(
    limit: number,
    claimTtlMs = this.storageLockTtlMs,
  ): Promise<DurableTtlSweepResult> {
    const durable = this.requireDurableMethods()
    const claims = await durable.claimOverdueTasks.call(
      this.longTermStore,
      limit,
      claimTtlMs,
    )
    const result: DurableTtlSweepResult = {
      claimed: claims.length,
      timedOut: 0,
      raceLost: 0,
      failed: 0,
      projected: 0,
    }
    for (const claim of claims) {
      try {
        const outcome = await this.processClaim(claim)
        if (outcome === 'timed-out') {
          result.timedOut += 1
          result.projected += 1
        } else {
          result.raceLost += 1
        }
      } catch {
        result.failed += 1
      }
    }
    return result
  }

  async sweepTerminalProjections(
    limit: number,
    claimTtlMs = this.storageLockTtlMs,
  ): Promise<DurableTtlSweepResult> {
    const durable = this.requireDurableMethods()
    const projections = await durable.claimTerminalProjections.call(
      this.longTermStore,
      limit,
      this.generateId(),
      claimTtlMs,
    )
    const result: DurableTtlSweepResult = {
      claimed: projections.length,
      timedOut: 0,
      raceLost: 0,
      failed: 0,
      projected: 0,
    }
    for (const projection of projections) {
      try {
        if (await this.projectClaimedTerminal(projection)) {
          result.projected += 1
        }
      } catch {
        result.failed += 1
      }
    }
    return result
  }

  private async processClaim(
    claim: TtlClaim,
  ): Promise<'timed-out' | 'race-lost'> {
    const hot = this.requireHotMethods()
    const durable = this.requireDurableMethods()
    const token = await this.storageCoordinator.ensureTaskHotForWrite(claim.taskId)
    const lease = await hot.acquireStorageLock.call(
      this.shortTermStore,
      claim.taskId,
      this.generateId(),
      `ttl:${claim.claimToken}`,
      this.storageLockTtlMs,
    )
    if (!lease) throw new StorageBusyError('TTL task storage is busy')

    let terminalized = false
    let fenceClosed = false
    try {
      await this.renew(lease, hot)
      const closed = await hot.closeWriteFence.call(
        this.shortTermStore,
        lease,
        token.storageEpoch,
      )
      fenceClosed = true
      const prepared = await this.prepareTimeout(claim, closed, hot, durable)
      await this.renew(lease, hot)
      const projection = await durable.terminalizeTtlClaim.call(
        this.longTermStore,
        claim,
        prepared.task,
        prepared.event,
        prepared.assignment,
      )
      if (!projection) {
        await this.reopenAfterRace(lease, token.storageEpoch, hot, durable)
        fenceClosed = false
        return 'race-lost'
      }
      terminalized = true
      await this.projectWithLease(
        projection,
        lease,
        token.storageEpoch,
        hot,
        durable,
      )
      fenceClosed = false
      await this.broadcast.publish(projection.task.id, projection.event)
      await durable.completeTerminalProjection.call(
        this.longTermStore,
        projection,
      )
      this.onTimeoutProjected?.(projection.task, prepared.from)
      return 'timed-out'
    } catch (error) {
      if (!terminalized && fenceClosed) {
        await this.reopenAfterRace(
          lease,
          token.storageEpoch,
          hot,
          durable,
        ).catch(() => {})
      }
      throw error
    } finally {
      await hot.releaseStorageLock.call(
        this.shortTermStore,
        lease,
      ).catch(() => false)
    }
  }

  private async prepareTimeout(
    claim: TtlClaim,
    closed: ClosedWriteFence,
    hot: HotTtlMethods,
    durable: DurableTtlMethods,
  ): Promise<{
    task: Task
    event: TaskEvent
    assignment: Awaited<ReturnType<ShortTermStore['getTaskAssignment']>>
    from: Task['status']
  }> {
    const [snapshot, durableTask, durableLastIndex, assignment, metadata] =
      await Promise.all([
        hot.getTaskMutationSnapshot.call(this.shortTermStore, claim.taskId),
        this.longTermStore.getTask(claim.taskId),
        durable.getLastEventIndex.call(this.longTermStore, claim.taskId),
        this.shortTermStore.getTaskAssignment(claim.taskId),
        durable.getTaskStorageMetadata.call(this.longTermStore, claim.taskId),
      ])
    const hotTask = snapshot?.task
    if (!hotTask || !durableTask || !metadata) {
      throw new StorageIntegrityError(`TTL task is missing: ${claim.taskId}`)
    }
    if (
      isTerminal(hotTask.status) ||
      hotTask.status !== durableTask.status ||
      hotTask.updatedAt !== durableTask.updatedAt ||
      hotTask.assignedWorker !== durableTask.assignedWorker ||
      metadata.taskVersion !== claim.taskVersion ||
      metadata.executionDeadlineAt !== claim.executionDeadlineAt
    ) {
      throw new StorageFenceConflictError(
        `TTL task changed after it was claimed: ${claim.taskId}`,
      )
    }
    if (
      closed.highWatermark !== durableLastIndex ||
      closed.highWatermark >= Number.MAX_SAFE_INTEGER
    ) {
      throw new StorageFenceConflictError(
        `TTL task history is not durably caught up: ${claim.taskId}`,
      )
    }

    const now = this.now()
    const task: Task = {
      ...hotTask,
      status: 'timeout',
      updatedAt: now,
      completedAt: now,
    }
    delete task.assignedWorker
    delete task.reason
    delete task.blockedRequest
    delete task.resumeAt
    const event: TaskEvent = {
      id: this.generateId(),
      taskId: claim.taskId,
      index: closed.highWatermark + 1,
      timestamp: now,
      type: 'taskcast:status',
      level: 'info',
      data: { status: 'timeout' },
    }
    return { task, event, assignment, from: hotTask.status }
  }

  private async projectClaimedTerminal(
    projection: TerminalProjection,
  ): Promise<boolean> {
    const hot = this.requireHotMethods()
    const durable = this.requireDurableMethods()
    if (projection.claimToken === null || projection.claimUntil === null) {
      throw new StorageIntegrityError('Claimed terminal projection has no claim')
    }
    const metadata = await durable.getTaskStorageMetadata.call(
      this.longTermStore,
      projection.task.id,
    )
    if (!metadata) {
      throw new StorageIntegrityError(
        `Terminal projection task metadata is missing: ${projection.task.id}`,
      )
    }
    if (metadata.storageState === 'releasing') {
      throw new StorageBusyError('Terminal projection storage is being released')
    }
    const token = metadata.storageState === 'cold'
      ? await this.storageCoordinator.ensureTaskHotForWrite(projection.task.id)
      : { taskId: projection.task.id, storageEpoch: metadata.storageEpoch }
    const lease = await hot.acquireStorageLock.call(
      this.shortTermStore,
      projection.task.id,
      this.generateId(),
      `terminal:${projection.projectionId}:${projection.claimToken}`,
      this.storageLockTtlMs,
    )
    if (!lease) throw new StorageBusyError('Terminal projection storage is busy')
    try {
      await this.renew(lease, hot)
      await hot.closeWriteFence.call(
        this.shortTermStore,
        lease,
        token.storageEpoch,
      )
      const projected = await this.projectWithLease(
        projection,
        lease,
        token.storageEpoch,
        hot,
        durable,
      )
      await this.broadcast.publish(projection.task.id, projection.event)
      await durable.completeTerminalProjection.call(
        this.longTermStore,
        projection,
      )
      return projected
    } finally {
      await hot.releaseStorageLock.call(
        this.shortTermStore,
        lease,
      ).catch(() => false)
    }
  }

  private async projectWithLease(
    projection: TerminalProjection,
    lease: StorageLease,
    expectedEpoch: number,
    hot: HotTtlMethods,
    durable: DurableTtlMethods,
  ): Promise<boolean> {
    await this.renew(lease, hot)
    const nextEpoch = expectedEpoch + 1
    if (!Number.isSafeInteger(nextEpoch)) {
      throw new StorageIntegrityError('TTL storage epoch exceeds safe bounds')
    }
    const result = await hot.projectTerminalFenced.call(
      this.shortTermStore,
      projection,
      lease,
      expectedEpoch,
      nextEpoch,
    )
    const metadata = await durable.getTaskStorageMetadata.call(
      this.longTermStore,
      projection.task.id,
    )
    if (!metadata) {
      throw new StorageIntegrityError(
        `TTL task storage metadata is missing: ${projection.task.id}`,
      )
    }
    if (metadata.storageEpoch !== nextEpoch) {
      const installed = await durable.compareAndSetTaskStorageMetadata.call(
        this.longTermStore,
        {
          taskId: projection.task.id,
          expectedStorageState: metadata.storageState,
          expectedStorageEpoch: expectedEpoch,
          expectedReleaseGeneration: metadata.activeReleaseGeneration,
          next: {
            ...metadata,
            storageState: 'hot',
            storageEpoch: nextEpoch,
            activeReleaseGeneration: null,
            coldAt: null,
          },
        },
      )
      if (!installed) {
        const current = await durable.getTaskStorageMetadata.call(
          this.longTermStore,
          projection.task.id,
        )
        if (current?.storageEpoch !== nextEpoch) {
          throw new StorageFenceConflictError(
            'TTL terminal projection lost its storage metadata race',
          )
        }
      }
    }
    return result.projected
  }

  private async reopenAfterRace(
    lease: StorageLease,
    expectedEpoch: number,
    hot: HotTtlMethods,
    durable: DurableTtlMethods,
  ): Promise<void> {
    await this.renew(lease, hot)
    const token = await hot.reopenWriteFence.call(
      this.shortTermStore,
      lease,
      expectedEpoch,
    )
    const metadata = await durable.getTaskStorageMetadata.call(
      this.longTermStore,
      lease.taskId,
    )
    if (!metadata || metadata.storageEpoch === token.storageEpoch) return
    await durable.compareAndSetTaskStorageMetadata.call(
      this.longTermStore,
      {
        taskId: lease.taskId,
        expectedStorageState: metadata.storageState,
        expectedStorageEpoch: expectedEpoch,
        expectedReleaseGeneration: metadata.activeReleaseGeneration,
        next: {
          ...metadata,
          storageState: 'hot',
          storageEpoch: token.storageEpoch,
          activeReleaseGeneration: null,
          coldAt: null,
        },
      },
    )
  }

  private async renew(
    lease: StorageLease,
    hot: HotTtlMethods,
  ): Promise<void> {
    const renewed = await hot.renewStorageLock.call(
      this.shortTermStore,
      lease,
      this.storageLockTtlMs,
    )
    if (!renewed) throw new StorageFenceConflictError('TTL storage lease was lost')
  }

  private requireHotMethods(): HotTtlMethods {
    const methods = {
      acquireStorageLock: this.shortTermStore.acquireStorageLock,
      renewStorageLock: this.shortTermStore.renewStorageLock,
      releaseStorageLock: this.shortTermStore.releaseStorageLock,
      closeWriteFence: this.shortTermStore.closeWriteFence,
      reopenWriteFence: this.shortTermStore.reopenWriteFence,
      getTaskMutationSnapshot: this.shortTermStore.getTaskMutationSnapshot,
      projectTerminalFenced: this.shortTermStore.projectTerminalFenced,
    }
    if (
      this.shortTermStore.supportsHotColdRelease !== true ||
      Object.values(methods).some((method) => typeof method !== 'function')
    ) {
      throw new StorageReleaseUnsupportedError(
        'Short-term store does not support durable TTL projection',
      )
    }
    return methods as HotTtlMethods
  }

  private requireDurableMethods(): DurableTtlMethods {
    const methods = {
      claimOverdueTasks: this.longTermStore.claimOverdueTasks,
      terminalizeTtlClaim: this.longTermStore.terminalizeTtlClaim,
      claimTerminalProjections: this.longTermStore.claimTerminalProjections,
      completeTerminalProjection: this.longTermStore.completeTerminalProjection,
      getTaskStorageMetadata: this.longTermStore.getTaskStorageMetadata,
      compareAndSetTaskStorageMetadata:
        this.longTermStore.compareAndSetTaskStorageMetadata,
      getLastEventIndex: this.longTermStore.getLastEventIndex,
    }
    if (
      this.longTermStore.supportsDurableTtl !== true ||
      Object.values(methods).some((method) => typeof method !== 'function')
    ) {
      throw new StorageReleaseUnsupportedError(
        'Long-term store does not support durable TTL claims',
      )
    }
    return methods as DurableTtlMethods
  }
}
