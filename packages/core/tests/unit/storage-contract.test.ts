import { describe, expect, it } from 'vitest'
import {
  StorageBusyError,
  StorageFenceConflictError,
  StorageIntegrityError,
  StorageReleaseUnsupportedError,
  canTransition,
} from '../../src/index.js'
import type {
  ArchiveBatchReceipt,
  ArchiveGeneration,
  ArchiveSourceManifest,
  ArchiveSourcePage,
  CanonicalHistoryEntry,
  DurableSeriesState,
  HotWriteToken,
  RehydrateSnapshot,
  ReleasePreconditions,
  ReleaseResult,
  StorageLease,
  Task,
  TaskEvent,
  TaskStorageMetadata,
  TerminalProjection,
  TtlClaim,
} from '../../src/index.js'

const event: TaskEvent = {
  id: 'event-1',
  taskId: 'task-1',
  index: 7,
  timestamp: 1_000,
  type: 'agent.message',
  level: 'info',
  data: { text: 'hello' },
}

const task: Task = {
  id: 'task-1',
  status: 'pending',
  createdAt: 100,
  updatedAt: 200,
}

describe('storage lifecycle contract', () => {
  it('uses camelCase wire fields for public lifecycle values', () => {
    const metadata: TaskStorageMetadata = {
      taskId: 'task-1',
      storageState: 'releasing',
      storageEpoch: 3,
      activeReleaseGeneration: 'generation-1',
      archiveWatermark: 7,
      lastEventAt: 1_000,
      coldAt: null,
      executionDeadlineAt: 2_000,
      taskVersion: 4,
    }
    const release: ReleaseResult = {
      taskId: 'task-1',
      storageState: 'cold',
      archiveWatermark: 7,
      released: true,
    }

    expect(JSON.parse(JSON.stringify(metadata))).toEqual(metadata)
    expect(JSON.parse(JSON.stringify(release))).toEqual(release)
  })

  it('defines release fencing, archive, rehydrate, and TTL payloads', () => {
    const token: HotWriteToken = { taskId: 'task-1', storageEpoch: 3 }
    const lease: StorageLease = {
      taskId: 'task-1',
      lockToken: 'lock-1',
      generation: 'generation-1',
      storageEpoch: 3,
    }
    const preconditions: ReleasePreconditions = {
      expectedLastEventIndex: 7,
      inactiveSince: 1_500,
    }
    const manifest: ArchiveSourceManifest = {
      priorWatermark: -1,
      targetWatermark: 7,
      sourceEntryCount: 1,
      sourceDigest: 'source-digest',
      seriesStateDigest: 'series-digest',
      expectedBatchOrdinals: [0],
    }
    const generation: ArchiveGeneration = {
      taskId: 'task-1',
      generation: 'generation-1',
      storageEpoch: 3,
      targetWatermark: 7,
      manifest,
      status: 'open',
      createdAt: 1_000,
      updatedAt: 1_000,
    }
    const receipt: ArchiveBatchReceipt = {
      taskId: 'task-1',
      generation: 'generation-1',
      ordinal: 0,
      previousBatchDigest: null,
      batchDigest: 'batch-digest',
      entryCount: 1,
      firstIndex: 7,
      lastIndex: 7,
    }
    const page: ArchiveSourcePage = {
      taskId: 'task-1',
      watermark: 7,
      cursor: null,
      nextCursor: null,
      events: [event],
      done: true,
    }
    const series: DurableSeriesState = {
      taskId: 'task-1',
      seriesId: 'answer',
      mode: 'accumulate',
      event,
      throughIndex: 7,
    }
    const snapshot: RehydrateSnapshot = {
      task,
      archiveWatermark: 7,
      maxEventIndex: 7,
      replayEvents: [event],
      seriesLatest: [series],
      storageEpoch: 3,
    }
    const history: CanonicalHistoryEntry = { event, seriesThroughIndex: 7 }
    const ttlClaim: TtlClaim = {
      taskId: 'task-1',
      claimToken: 'ttl-1',
      claimUntil: 3_000,
      taskVersion: 4,
      executionDeadlineAt: 2_000,
    }
    const projection: TerminalProjection = {
      projectionId: 'projection-1',
      task: { ...task, status: 'timeout' },
      event,
      assignment: null,
      claimToken: null,
      claimUntil: null,
    }

    expect({
      token,
      lease,
      preconditions,
      generation,
      receipt,
      page,
      snapshot,
      history,
      ttlClaim,
      projection,
    }).toMatchObject({
      token: { storageEpoch: 3 },
      preconditions: { expectedLastEventIndex: 7 },
      generation: { manifest },
      receipt: { ordinal: 0 },
      page: { done: true },
      snapshot: { maxEventIndex: 7 },
      history: { seriesThroughIndex: 7 },
      ttlClaim: { taskVersion: 4 },
      projection: { assignment: null },
    })
  })

  it('exposes stable typed storage errors', () => {
    expect(new StorageFenceConflictError().code).toBe('storage_fence_conflict')
    expect(new StorageFenceConflictError().retryable).toBe(true)
    expect(new StorageBusyError().code).toBe('storage_busy')
    expect(new StorageIntegrityError().code).toBe('storage_integrity_error')
    expect(new StorageReleaseUnsupportedError().code).toBe('storage_release_unsupported')
  })
})

describe('durable execution timeout transitions', () => {
  it.each(['pending', 'assigned', 'running', 'paused', 'blocked'] as const)(
    'allows %s to transition to timeout',
    (status) => {
      expect(canTransition(status, 'timeout')).toBe(true)
    },
  )

  it.each(['completed', 'failed', 'timeout', 'cancelled'] as const)(
    'keeps terminal status %s terminal',
    (status) => {
      expect(canTransition(status, 'timeout')).toBe(false)
    },
  )
})
