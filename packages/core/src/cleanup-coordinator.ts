import { ulid } from 'ulidx'
import {
  StorageBusyError, StorageFenceConflictError, StoragePreconditionError, StorageReleaseUnsupportedError,
  type CleanupClaim, type LongTermStore, type ShortTermStore, type StorageLease,
} from './types.js'

export interface CleanupSweepResult { claimed: number; completed: number; deferred: number; failed: number; deletedEvents: number }
export const emptyCleanupResult = (): CleanupSweepResult => ({ claimed: 0, completed: 0, deferred: 0, failed: 0, deletedEvents: 0 })

export class CleanupCoordinator {
  constructor(
    private hot: ShortTermStore,
    private durable: LongTermStore,
    private release: (taskId: string) => Promise<unknown>,
  ) {
    if (hot.supportsHotColdRelease !== true || durable.supportsTerminalCleanup !== true || durable.supportsHotColdRelease !== true
      || !['acquireStorageLock', 'renewStorageLock', 'releaseStorageLock', 'getTaskStoragePresence', 'listStorageWriters'].every(k => typeof hot[k as keyof ShortTermStore] === 'function')
      || !['claimTaskCreation', 'completeTaskCreation', 'abortTaskCreation', 'claimCleanupTasks', 'canCleanupTask', 'renewCleanupClaim', 'deferCleanupClaim', 'beginTaskCleanup', 'deleteTaskCleanupBatch', 'getTaskStorageMetadata'].every(k => typeof durable[k as keyof LongTermStore] === 'function')) {
      throw new StorageReleaseUnsupportedError('Terminal cleanup requires complete fenced hot and durable store capabilities')
    }
  }

  async sweep(limit: number, eventBatchSize: number, claimTtlMs: number): Promise<CleanupSweepResult> {
    for (const value of [limit, eventBatchSize, claimTtlMs]) {
      if (!Number.isSafeInteger(value) || value <= 0 || value > 2_147_483_647) throw new StoragePreconditionError('Invalid cleanup sweep bound')
    }
    const claims = await this.durable.claimCleanupTasks!(limit, claimTtlMs)
    const result = emptyCleanupResult()
    result.claimed = claims.length
    for (const claim of claims) {
      try {
        const batch = await this.process(claim, eventBatchSize, claimTtlMs)
        result.deletedEvents += batch.deletedEvents
        if (batch.complete) result.completed++
        else { result.deferred++; await this.durable.deferCleanupClaim!(claim, 5000) }
      } catch (error) {
        if (error instanceof StorageBusyError || error instanceof StorageFenceConflictError || error instanceof StoragePreconditionError) result.deferred++
        else result.failed++
        // A failed dependency cannot prevent later claimed tasks from making progress.
        await this.durable.deferCleanupClaim!(claim, 5000).catch(() => {})
      }
    }
    return result
  }

  private async writersReady(): Promise<void> {
    const writers = await this.hot.listStorageWriters!()
    if (writers.some(w => w.storageProtocolVersion < 3)) throw new StorageBusyError('Terminal cleanup requires storage protocol v3 writers')
  }

  private async process(claim: CleanupClaim, batchSize: number, ttl: number) {
    let lease: StorageLease | null = null
    let lost = false
    let renewal: Promise<void> | undefined
    const renew = async () => {
      if (lost || !await this.durable.renewCleanupClaim!(claim, ttl)
        || (lease && !await this.hot.renewStorageLock!(lease, ttl))) {
        lost = true
        throw new StorageFenceConflictError('Cleanup claim or storage lease was lost')
      }
    }
    const timer = setInterval(() => {
      if (renewal) return
      renewal = renew().catch(() => { lost = true }).finally(() => { renewal = undefined })
    }, Math.max(1, Math.floor(ttl / 3)))
    timer.unref?.()
    try {
      await renew()
      await this.writersReady()
      if (!await this.durable.canCleanupTask!(claim)) throw new StorageBusyError('Task cleanup dependencies are not settled')
      let metadata = await this.durable.getTaskStorageMetadata!(claim.taskId)
      if (!metadata || metadata.creationToken !== claim.creationToken) throw new StorageFenceConflictError()
      if (metadata.storageState !== 'cold') {
        // release acquires its own storage lease. Never nest that lease.
        await this.release(claim.taskId)
      }
      await renew()
      lease = await this.hot.acquireStorageLock!(claim.taskId, ulid(), `cleanup:${claim.claimToken}`, ttl)
      if (!lease) throw new StorageBusyError('Task storage is busy')
      await renew()
      await this.writersReady()
      const task = await this.durable.getTask(claim.taskId)
      metadata = await this.durable.getTaskStorageMetadata!(claim.taskId)
      const presence = await this.hot.getTaskStoragePresence!(claim.taskId)
      if (!task || !metadata || metadata.creationToken !== claim.creationToken || task.completedAt !== claim.completedAt
        || task.cleanupPolicyVersion !== 1 || metadata.storageState !== 'cold'
        || presence.task || presence.eventCount !== 0 || presence.nextIndex || presence.seriesStateCount !== 0 || presence.writeFence) throw new StorageFenceConflictError('Cleanup task generation or cold state changed')
      // Archive finalization may update task_version; generation and completion stay fixed.
      const currentClaim = { ...claim, taskVersion: metadata.taskVersion }
      await renew()
      if (!await this.durable.beginTaskCleanup!(currentClaim, metadata.storageEpoch, metadata.archiveWatermark)) throw new StorageBusyError('Task cleanup preconditions changed')
      await renew()
      return await this.durable.deleteTaskCleanupBatch!(currentClaim, batchSize)
    } finally {
      clearInterval(timer)
      await renewal
      if (lease) await this.hot.releaseStorageLock!(lease).catch(() => false)
    }
  }
}
