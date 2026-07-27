import { describe, expect, it, vi } from 'vitest'
import {
  MemoryBroadcastProvider,
  MemoryLongTermStore,
  MemoryShortTermStore,
  TaskEngine,
  type ResolvedStorageLifecycleConfig,
  type Task,
} from '@taskcast/core'
import { StorageLifecycleWorker } from '../src/index.js'

const config: ResolvedStorageLifecycleConfig = {
  hotRetentionEnabled: false,
  hotRetentionTerminalSeconds: 60,
  hotRetentionIdleSeconds: 1,
  rehydrateReplayEvents: 1_000,
  storageLockTtlSeconds: 30,
  ttlSweepIntervalSeconds: 5,
  ttlSweepBatchSize: 10,
}

function makeTask(
  id: string,
  status: Task['status'],
  overrides: Partial<Task> = {},
): Task {
  return {
    id,
    status,
    createdAt: 1_000,
    updatedAt: 1_000,
    ...overrides,
  }
}

async function makeFixture() {
  const hot = new MemoryShortTermStore()
  const durable = new MemoryLongTermStore()
  const engine = new TaskEngine({
    shortTermStore: hot,
    longTermStore: durable,
    broadcast: new MemoryBroadcastProvider(),
  })
  await engine.registerStorageWriter({
    instanceId: 'writer-v2',
    storageProtocolVersion: 2,
    build: 'test',
    expiresAt: 0,
  }, 30_000)
  return { hot, durable, engine }
}

describe('StorageLifecycleWorker', () => {
  it('sweeps durable TTL and retries a persisted release request in one bounded tick', async () => {
    const { hot, durable, engine } = await makeFixture()
    const overdue = makeTask('overdue', 'running', { ttl: 60 })
    await hot.saveTask(overdue)
    await durable.saveTask(overdue)
    const metadata = (await durable.getTaskStorageMetadata('overdue'))!
    await durable.compareAndSetTaskStorageMetadata({
      taskId: 'overdue',
      expectedStorageState: metadata.storageState,
      expectedStorageEpoch: metadata.storageEpoch,
      expectedReleaseGeneration: metadata.activeReleaseGeneration,
      next: { ...metadata, executionDeadlineAt: Date.now() - 1 },
    })

    const releasable = makeTask('explicit-release', 'completed', {
      completedAt: 1_000,
    })
    await hot.saveTask(releasable)
    await durable.saveTask(releasable)
    await durable.persistStorageReleaseRequest({
      taskId: releasable.id,
      requestedAt: 2_000,
      expectedLastEventIndex: -1,
      inactiveSince: Date.now() - 2_000,
    })

    const worker = new StorageLifecycleWorker({
      engine,
      shortTermStore: hot,
      config,
      logger: () => {},
    })
    const result = await worker.tick()

    expect(result?.ttl.timedOut).toBe(1)
    await expect(durable.getTask('overdue')).resolves.toMatchObject({
      status: 'timeout',
    })
    await expect(hot.getTask('explicit-release')).resolves.toBeNull()
    await expect(
      durable.getTaskStorageMetadata('explicit-release'),
    ).resolves.toMatchObject({ storageState: 'cold' })
    await expect(durable.listStorageReleaseRequests(10)).resolves.toEqual([])
  })

  it('does not infer non-terminal or terminal ownership release when retention is disabled', async () => {
    const { hot, durable, engine } = await makeFixture()
    const terminal = makeTask('terminal-hot', 'completed', {
      completedAt: 1_000,
    })
    const pending = makeTask('pending-hot', 'pending')
    await hot.saveTask(terminal)
    await hot.saveTask(pending)
    await durable.saveTask(terminal)
    await durable.saveTask(pending)

    const worker = new StorageLifecycleWorker({
      engine,
      shortTermStore: hot,
      config,
      logger: () => {},
    })
    await worker.tick()

    await expect(hot.getTask('terminal-hot')).resolves.not.toBeNull()
    await expect(hot.getTask('pending-hot')).resolves.not.toBeNull()
  })

  it('automatically releases only terminal tasks after the configured grace', async () => {
    const { hot, durable, engine } = await makeFixture()
    const terminal = makeTask('old-terminal', 'completed', {
      completedAt: 1_000,
      updatedAt: Date.now() - 61_000,
    })
    const pending = makeTask('old-pending', 'pending', {
      updatedAt: Date.now() - 61_000,
    })
    await hot.saveTask(terminal)
    await hot.saveTask(pending)
    await durable.saveTask(terminal)
    await durable.saveTask(pending)

    const worker = new StorageLifecycleWorker({
      engine,
      shortTermStore: hot,
      config: { ...config, hotRetentionEnabled: true },
      logger: () => {},
    })
    await worker.tick()

    await expect(hot.getTask('old-terminal')).resolves.toBeNull()
    await expect(hot.getTask('old-pending')).resolves.not.toBeNull()
  })

  it('keeps sweeping TTL while release readiness is backing off', async () => {
    const { hot, durable, engine } = await makeFixture()
    const worker = new StorageLifecycleWorker({
      engine,
      shortTermStore: hot,
      config,
      readiness: {
        ensureReady: async () => {
          throw new Error('legacy writer is still active')
        },
      },
      logger: () => {},
    })
    await worker.tick()

    const overdue = makeTask('ttl-during-release-backoff', 'running', {
      ttl: 60,
    })
    await hot.saveTask(overdue)
    await durable.saveTask(overdue)
    const metadata = (await durable.getTaskStorageMetadata(overdue.id))!
    await durable.compareAndSetTaskStorageMetadata({
      taskId: overdue.id,
      expectedStorageState: metadata.storageState,
      expectedStorageEpoch: metadata.storageEpoch,
      expectedReleaseGeneration: metadata.activeReleaseGeneration,
      next: { ...metadata, executionDeadlineAt: Date.now() - 1 },
    })

    const result = await worker.tick()

    expect(result?.ttl.timedOut).toBe(1)
    await expect(durable.getTask(overdue.id)).resolves.toMatchObject({
      status: 'timeout',
    })
  })

  it('logs each worker failure without task payloads', async () => {
    const { hot, engine } = await makeFixture()
    vi.spyOn(engine, 'sweepDurableTtl').mockRejectedValueOnce(
      new Error('ttl database unavailable'),
    )
    vi.spyOn(engine, 'sweepTerminalProjections').mockRejectedValueOnce(
      new Error('projection database unavailable'),
    )
    vi.spyOn(engine, 'retryStorageReleaseRequests').mockRejectedValueOnce(
      new Error('release database unavailable'),
    )
    const records: Array<Record<string, unknown>> = []
    const worker = new StorageLifecycleWorker({
      engine,
      shortTermStore: hot,
      config,
      logger: (record) => records.push(record),
    })

    const result = await worker.tick()

    expect(result).toMatchObject({
      ttl: { failed: 1 },
      projection: { failed: 1 },
      releaseRequests: { failed: 1 },
    })
    expect(records).toEqual(expect.arrayContaining([
      expect.objectContaining({
        event: 'storage_lifecycle_error',
        operation: 'durable_ttl',
        error: 'ttl database unavailable',
      }),
      expect.objectContaining({
        event: 'storage_lifecycle_error',
        operation: 'terminal_projection',
        error: 'projection database unavailable',
      }),
      expect.objectContaining({
        event: 'storage_lifecycle_error',
        operation: 'release_request_retry',
        error: 'release database unavailable',
      }),
    ]))
    expect(JSON.stringify(records)).not.toContain('"data"')
  })

  it('contains retention scan failures and backs off instead of rejecting the tick', async () => {
    const { hot, engine } = await makeFixture()
    vi.spyOn(hot, 'listTasks').mockRejectedValueOnce(
      new Error('redis unavailable'),
    )
    const records: Array<Record<string, unknown>> = []
    const worker = new StorageLifecycleWorker({
      engine,
      shortTermStore: hot,
      config: { ...config, hotRetentionEnabled: true },
      logger: (record) => records.push(record),
    })

    await expect(worker.tick()).resolves.toMatchObject({
      retention: { failed: 1 },
    })
    expect(records).toContainEqual(expect.objectContaining({
      event: 'storage_lifecycle_error',
      operation: 'terminal_retention_scan',
      error: 'redis unavailable',
    }))
  })
})
