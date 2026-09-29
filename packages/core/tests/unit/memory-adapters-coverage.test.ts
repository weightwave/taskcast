import { afterEach, describe, expect, it, vi } from 'vitest'
import { MemoryLongTermStore, MemoryShortTermStore } from '../../src/memory-adapters.js'
import { TaskConflictError } from '../../src/engine.js'
import {
  computeArchiveBatchDigest,
  computeArchiveSourceDigest,
  computeArchiveSourcePageDigest,
  computeSeriesStateDigest,
} from '../../src/storage-digest.js'
import {
  StorageFenceConflictError,
  type ArchiveBatch,
  type ArchiveGeneration,
  type Task,
  type TaskEvent,
  type TaskStorageMetadata,
  type TerminalProjection,
  type WorkerAssignment,
} from '../../src/types.js'

const task = (overrides: Partial<Task> = {}): Task => ({
  id: 'task-1', status: 'running', createdAt: 1_000, updatedAt: 1_000, ...overrides,
})
const event = (index = 0, overrides: Partial<TaskEvent> = {}): TaskEvent => ({
  id: `event-${index}`, taskId: 'task-1', index, timestamp: 1_000 + index,
  type: 'llm.delta', level: 'info', data: { delta: 'hello' }, ...overrides,
})
const assignment = (): WorkerAssignment => ({
  taskId: 'task-1', workerId: 'worker-1', cost: 1, assignedAt: 1_000, status: 'running',
})
const projection = (overrides: Partial<TerminalProjection> = {}): TerminalProjection => ({
  projectionId: 'timeout-projection', task: task({ status: 'timeout', completedAt: 2_000 }),
  event: event(0, { type: 'taskcast:status', data: { status: 'timeout' } }),
  assignment: null, claimToken: 'claim', claimUntil: 100_000, ...overrides,
})

async function updateMetadata(
  store: MemoryLongTermStore,
  change: Partial<TaskStorageMetadata>,
  taskId = 'task-1',
) {
  const current = (await store.getTaskStorageMetadata(taskId))!
  expect(await store.compareAndSetTaskStorageMetadata({
    taskId,
    expectedStorageState: current.storageState,
    expectedStorageEpoch: current.storageEpoch,
    expectedReleaseGeneration: current.activeReleaseGeneration,
    next: { ...current, ...change },
  })).toBe(true)
}

async function archiveFixture(manifestChanges: Partial<ArchiveGeneration['manifest']> = {}) {
  const store = new MemoryLongTermStore()
  await store.saveTask(task())
  await updateMetadata(store, { storageState: 'releasing', activeReleaseGeneration: 'release-1' })
  const pages = [[event(0)], [event(1)]]
  const generation: ArchiveGeneration = {
    taskId: 'task-1', generation: 'release-1', storageEpoch: 1, targetWatermark: 1,
    status: 'open', createdAt: 1_000, updatedAt: 1_000,
    manifest: {
      priorWatermark: -1, targetWatermark: 1, sourceEntryCount: 2,
      sourceDigest: await computeArchiveSourceDigest(
        await Promise.all(pages.map(computeArchiveSourcePageDigest)),
      ),
      seriesStateDigest: await computeSeriesStateDigest([]), expectedBatchOrdinals: [0, 1],
      ...manifestChanges,
    },
  }
  const batches: ArchiveBatch[] = []
  for (const [ordinal, events] of pages.entries()) {
    const previousBatchDigest = batches.at(-1)?.receipt.batchDigest ?? null
    batches.push({
      events, seriesLatest: [],
      receipt: {
        taskId: 'task-1', generation: 'release-1', ordinal, previousBatchDigest,
        batchDigest: await computeArchiveBatchDigest(previousBatchDigest, events, []),
        entryCount: events.length, firstIndex: ordinal, lastIndex: ordinal,
      },
    })
  }
  await store.beginArchive(generation)
  return { store, generation, batches }
}

async function overdueFixture() {
  vi.useFakeTimers()
  vi.setSystemTime(10_000)
  const store = new MemoryLongTermStore()
  await store.saveTask(task({ ttl: 1 }))
  vi.setSystemTime(11_001)
  const [claim] = await store.claimOverdueTasks(1, 1_000)
  return { store, claim: claim!, timeout: task({ status: 'timeout', ttl: 1, completedAt: 11_001 }) }
}

async function cleanupFixture(eventCount = 0, overrides: Partial<Task> = {}) {
  const store = new MemoryLongTermStore()
  const initial = task({
    cleanupPolicyVersion: 1, cleanupResolvedAt: 1_000,
    cleanup: { rules: [{ target: 'all', trigger: {} }] }, ...overrides,
  })
  const context = { creationToken: 'cleanup-creation' }
  await store.claimTaskCreation(initial, context.creationToken, 30_000)
  await store.completeTaskCreation(initial.id, context.creationToken)
  for (let index = 0; index < eventCount; index++) await store.saveEvent(event(index), context)
  const completed = { ...initial, status: 'completed' as const, completedAt: 1_000 }
  await store.saveTask(completed, context)
  await updateMetadata(store, { storageState: 'cold', archiveWatermark: eventCount - 1 })
  return { store, completed, context }
}

async function finalizedArchive(store: MemoryLongTermStore, snapshot: Task, name: string, pages: TaskEvent[][]) {
  const prior = (await store.getTaskStorageMetadata(snapshot.id))!
  const watermark = Math.max(prior.archiveWatermark, ...pages.flat().map(entry => entry.index))
  const generation: ArchiveGeneration = {
    taskId: snapshot.id, generation: name, storageEpoch: prior.storageEpoch, targetWatermark: watermark,
    status: 'open', createdAt: 1_000, updatedAt: 1_000,
    manifest: {
      priorWatermark: prior.archiveWatermark, targetWatermark: watermark,
      sourceEntryCount: pages.flat().length,
      sourceDigest: await computeArchiveSourceDigest(await Promise.all(pages.map(computeArchiveSourcePageDigest))),
      seriesStateDigest: await computeSeriesStateDigest([]), expectedBatchOrdinals: pages.map((_, index) => index),
    },
  }
  await updateMetadata(store, { storageState: 'releasing', activeReleaseGeneration: name }, snapshot.id)
  await store.beginArchive(generation)
  const batches: ArchiveBatch[] = []
  for (const [ordinal, events] of pages.entries()) {
    const previousBatchDigest = batches.at(-1)?.receipt.batchDigest ?? null
    const batch: ArchiveBatch = {
      events, seriesLatest: [], receipt: {
        taskId: snapshot.id, generation: name, ordinal, previousBatchDigest,
        batchDigest: await computeArchiveBatchDigest(previousBatchDigest, events, []),
        entryCount: events.length, firstIndex: events[0]?.index ?? null, lastIndex: events.at(-1)?.index ?? null,
      },
    }
    await store.archiveBatch(snapshot.id, name, batch)
    batches.push(batch)
  }
  await store.finalizeArchive(snapshot.id, name, snapshot, [])
  await updateMetadata(store, { storageState: 'cold', activeReleaseGeneration: null }, snapshot.id)
  return { generation, batches }
}

afterEach(() => vi.useRealTimers())

describe('memory terminal cleanup claims and bounded deletion', () => {
  it('rejects invalid batch, lease, retry and watermark bounds before changing state', async () => {
    const { store } = await cleanupFixture()
    const [claim] = await store.claimCleanupTasks(1, 30_000)
    for (const invalid of [-1, 0.5, Number.NaN, 2_147_483_648]) {
      await expect(store.claimCleanupTasks(invalid, 30_000)).rejects.toMatchObject({ code: 'storage_precondition_failed' })
      await expect(store.renewCleanupClaim(claim!, invalid)).rejects.toMatchObject({ code: 'storage_precondition_failed' })
      await expect(store.deferCleanupClaim(claim!, invalid)).rejects.toMatchObject({ code: 'storage_precondition_failed' })
      await expect(store.deleteTaskCleanupBatch(claim!, invalid)).rejects.toMatchObject({ code: 'storage_precondition_failed' })
    }
    await expect(store.claimCleanupTasks(0, 30_000)).rejects.toThrow('Invalid cleanup batch or lease bound')
    for (const invalid of [-2, 0.5, Number.NaN]) {
      await expect(store.beginTaskCleanup(claim!, 1, invalid)).rejects.toThrow('Invalid cleanup watermark')
    }
    expect(await store.canCleanupTask(claim!)).toBe(true)
    expect((await store.getTask('task-1'))?.historyExpiredAt).toBeUndefined()
  })

  it('chooses events before a later whole-task deadline, then escalates an unstarted retry when all becomes due', async () => {
    vi.useFakeTimers()
    vi.setSystemTime(2_000)
    const { store } = await cleanupFixture(0, { cleanup: { rules: [
      { target: 'events', trigger: { afterMs: 100 } },
      { target: 'all', trigger: { afterMs: 2_000 } },
    ] } })
    const [eventsClaim] = await store.claimCleanupTasks(1, 100)
    expect(eventsClaim?.target).toBe('events')
    await store.deferCleanupClaim(eventsClaim!, 1_100)
    expect(await store.claimCleanupTasks(1, 100)).toEqual([])
    vi.setSystemTime(3_100)
    const [allClaim] = await store.claimCleanupTasks(1, 100)
    expect(allClaim?.target).toBe('all')
    expect(await store.beginTaskCleanup(allClaim!, 1, -1)).toBe(true)
    expect(await store.deleteTaskCleanupBatch(allClaim!, 1)).toEqual({ deletedEvents: 0, complete: true })
    expect(await store.getTask('task-1')).toBeNull()
  })

  it('expires an empty event history while retaining its task result for an events-only policy', async () => {
    const { store } = await cleanupFixture(0, {
      result: { report: 'retained' }, cleanup: { rules: [{ target: 'events', trigger: {} }] },
    })
    const [claim] = await store.claimCleanupTasks(1, 30_000)
    expect(claim?.target).toBe('events')
    expect(await store.renewCleanupClaim(claim!, 30_000)).toBe(true)
    expect(await store.beginTaskCleanup(claim!, 1, -1)).toBe(true)
    expect(await store.deleteTaskCleanupBatch(claim!, 1)).toEqual({ deletedEvents: 0, complete: true })
    expect(await store.getTask('task-1')).toMatchObject({
      status: 'completed', result: { report: 'retained' }, historyExpiredAt: expect.any(Number),
    })
    expect(await store.claimCleanupTasks(1, 30_000)).toEqual([])
  })

  it('refuses expired or forged cleanup ownership without changing a live claim', async () => {
    vi.useFakeTimers()
    vi.setSystemTime(10_000)
    const { store } = await cleanupFixture()
    const [claim] = await store.claimCleanupTasks(1, 100)
    for (const forged of [
      { ...claim!, taskId: 'missing' }, { ...claim!, claimToken: 'other-owner' },
      { ...claim!, creationToken: 'replaced-task' },
    ]) {
      expect(await store.renewCleanupClaim(forged, 100)).toBe(false)
      await store.deferCleanupClaim(forged, 0)
      expect(await store.beginTaskCleanup(forged, 1, -1)).toBe(false)
      expect(await store.canCleanupTask(claim!)).toBe(true)
    }
    vi.setSystemTime(10_101)
    expect(await store.renewCleanupClaim(claim!, 100)).toBe(false)
    expect(await store.beginTaskCleanup(claim!, 1, -1)).toBe(false)
    await expect(store.deleteTaskCleanupBatch(claim!, 1)).rejects.toBeInstanceOf(StorageFenceConflictError)
    expect(await store.getTask('task-1')).not.toBeNull()
  })

  it('defers cleanup while worker settlement or a newer durable event invalidates the snapshot', async () => {
    const { store } = await cleanupFixture(1)
    const [claim] = await store.claimCleanupTasks(1, 30_000)
    await store.saveDurableAssignment(assignment())
    expect(await store.beginTaskCleanup(claim!, 1, 0)).toBe(false)
    await store.deleteDurableAssignment('task-1')
    await updateMetadata(store, { archiveWatermark: -1 })
    expect(await store.beginTaskCleanup(claim!, 1, -1)).toBe(false)
    expect(await store.getEvents('task-1')).toEqual([event()])
    await updateMetadata(store, { archiveWatermark: 0 })
    expect(await store.beginTaskCleanup(claim!, 1, 0)).toBe(true)
  })

  it('rejects deletion after cold-state or settlement is lost and resumes after recovery', async () => {
    const { store } = await cleanupFixture()
    const [claim] = await store.claimCleanupTasks(1, 30_000)
    await expect(store.deleteTaskCleanupBatch(claim!, 1)).rejects.toBeInstanceOf(StorageFenceConflictError)
    expect(await store.beginTaskCleanup(claim!, 1, -1)).toBe(true)
    await updateMetadata(store, { storageState: 'hot' })
    await expect(store.deleteTaskCleanupBatch(claim!, 1)).rejects.toBeInstanceOf(StorageFenceConflictError)
    await updateMetadata(store, { storageState: 'cold' })
    await store.saveDurableAssignment(assignment())
    await expect(store.deleteTaskCleanupBatch(claim!, 1)).rejects.toBeInstanceOf(StorageFenceConflictError)
    await store.deleteDurableAssignment('task-1')
    expect(await store.deleteTaskCleanupBatch(claim!, 1)).toEqual({ deletedEvents: 0, complete: true })
    expect(await store.getTask('task-1')).toBeNull()
  })

  it('retains another task archive while deleting receipt and empty-generation records in bounded batches', async () => {
    const { store, completed } = await cleanupFixture()
    const other = task({ id: 'other' })
    await store.saveTask(other)
    await finalizedArchive(store, other, 'other-release', [[event(0, { taskId: 'other' })]])
    await finalizedArchive(store, completed, 'empty-before', [])
    await finalizedArchive(store, completed, 'with-pages', [[event(0)], [event(1)]])
    await finalizedArchive(store, completed, 'empty-after', [])
    const [claim] = await store.claimCleanupTasks(1, 30_000)
    expect(await store.beginTaskCleanup(claim!, 1, 1)).toBe(true)
    expect(await store.deleteTaskCleanupBatch(claim!, 1)).toEqual({ deletedEvents: 1, complete: false })
    expect(await store.deleteTaskCleanupBatch(claim!, 1)).toEqual({ deletedEvents: 1, complete: false })
    expect(await store.deleteTaskCleanupBatch(claim!, 1)).toEqual({ deletedEvents: 0, complete: false })
    expect(await store.deleteTaskCleanupBatch(claim!, 1)).toEqual({ deletedEvents: 0, complete: true })
    expect(await store.getTask('task-1')).toBeNull()
    await updateMetadata(store, { storageState: 'releasing', activeReleaseGeneration: 'other-release' }, 'other')
    expect(await store.finalizeArchive('other', 'other-release', other, [])).toBe(0)
    expect(await store.getEvents('other')).toEqual([event(0, { taskId: 'other' })])
  })

  it('continues deleting series snapshots until each bounded batch is exhausted', async () => {
    const { store, context } = await cleanupFixture(2)
    for (let index = 0; index < 2; index++) {
      const seriesId = `series-${index}`
      // These are delayed durable series writes that settle before the cleanup fence.
      await updateMetadata(store, { archiveWatermark: -1 })
      await store.replaceLastSeriesEvent('task-1', seriesId, event(index, { seriesId, seriesMode: 'latest' }), context)
    }
    await updateMetadata(store, { archiveWatermark: 1 })
    const [claim] = await store.claimCleanupTasks(1, 30_000)
    expect(await store.beginTaskCleanup(claim!, 1, 1)).toBe(true)
    expect(await store.deleteTaskCleanupBatch(claim!, 1)).toEqual({ deletedEvents: 1, complete: false })
    expect(await store.deleteTaskCleanupBatch(claim!, 1)).toEqual({ deletedEvents: 1, complete: false })
    expect(await store.getDurableSeriesState('task-1')).toHaveLength(1)
    expect(await store.deleteTaskCleanupBatch(claim!, 1)).toEqual({ deletedEvents: 0, complete: true })
    expect(await store.getDurableSeriesState('task-1')).toEqual([])
  })

  it('fences every archive write once cleanup has expired history', async () => {
    const { store, completed } = await cleanupFixture()
    const { generation, batches } = await finalizedArchive(store, completed, 'release', [[event(0)]])
    const [claim] = await store.claimCleanupTasks(1, 30_000)
    expect(await store.beginTaskCleanup(claim!, 1, 0)).toBe(true)
    await expect(store.beginArchive(generation)).rejects.toThrow('Task history has expired')
    await expect(store.archiveBatch('task-1', 'release', batches[0]!)).rejects.toThrow('Task history has expired')
    await expect(store.finalizeArchive('task-1', 'release', completed, [])).rejects.toThrow('Task history has expired')
    expect(await store.getEvents('task-1')).toEqual([event(0)])
  })
})

describe('memory hot storage mutation and recovery guards', () => {
  it('tracks hot series and selects tasks by lifecycle status', async () => {
    const store = new MemoryShortTermStore()
    await store.saveTask(task())
    await store.saveTask(task({ id: 'other', status: 'completed' }))
    const first = event(0, { seriesId: 's', seriesMode: 'accumulate' })
    const second = event(1, { seriesId: 's', seriesMode: 'accumulate', data: { delta: ' world' } })
    await store.accumulateSeries('task-1', 's', first, 'delta')
    expect((await store.accumulateSeries('task-1', 's', second, 'delta')).data).toEqual({ delta: 'hello world' })
    await store.setSeriesLatest('other', 'different', event(0, { taskId: 'other' }))
    expect(await store.getTaskStoragePresence('task-1')).toMatchObject({ task: true, seriesStateCount: 1 })
    expect((await store.listByStatus(['running'])).map(entry => entry.id)).toEqual(['task-1'])
    expect(await store.listByStatus(['pending'])).toEqual([])
  })

  it('rejects archive overwrite until requested and replaces only the restored task series', async () => {
    const store = new MemoryShortTermStore()
    await store.saveTask(task())
    await store.setSeriesLatest('task-1', 'old', event())
    await store.setSeriesLatest('other', 'old', event(0, { taskId: 'other' }))
    const latest = event(4, { seriesId: 'new', seriesMode: 'latest' })
    const data = {
      task: task({ status: 'completed' }), events: [latest], nextIndex: 5,
      seriesLatest: [{ taskId: 'task-1', seriesId: 'new', event: latest }],
    }
    await expect(store.restoreTaskArchive(data)).rejects.toBeInstanceOf(TaskConflictError)
    expect(await store.getSeriesLatest('task-1', 'old')).toEqual(event())
    expect(await store.restoreTaskArchive(data, { overwrite: true })).toEqual({ overwritten: true })
    expect(await store.getSeriesLatest('task-1', 'old')).toBeNull()
    expect(await store.getSeriesLatest('other', 'old')).not.toBeNull()
    expect(await store.getEvents('task-1')).toEqual([latest])
    expect(await store.nextIndex('task-1')).toBe(5)
  })

  it('refreshes a lease for its owner and rejects stale or open-fence lifecycle operations', async () => {
    vi.useFakeTimers()
    vi.setSystemTime(10_000)
    const store = new MemoryShortTermStore()
    await store.saveTask(task())
    const lease = (await store.acquireStorageLock('task-1', 'owner', 'release', 100))!
    vi.setSystemTime(10_050)
    expect(await store.acquireStorageLock('task-1', 'owner', 'release', 200)).toEqual(lease)
    vi.setSystemTime(10_150)
    await expect(store.closeWriteFence(lease, 2)).rejects.toBeInstanceOf(StorageFenceConflictError)
    await expect(store.reopenWriteFence(lease, 1)).rejects.toBeInstanceOf(StorageFenceConflictError)
    await expect(store.deleteTaskStorageFenced(lease, 1)).rejects.toBeInstanceOf(StorageFenceConflictError)
    expect(await store.getTask('task-1')).toEqual(task())
    await store.closeWriteFence(lease, 1)
    await store.reopenWriteFence(lease, 1)
    vi.setSystemTime(10_251)
    expect(await store.renewStorageLock(lease, 100)).toBe(false)
    await expect(store.closeWriteFence(lease, 2)).rejects.toThrow('Storage lease is stale')
  })

  it('saves only with the current hot token and advances the mutation revision', async () => {
    const store = new MemoryShortTermStore()
    await store.saveTask(task())
    const before = (await store.getTaskMutationSnapshot('task-1'))!
    const updated = task({ updatedAt: 2_000, params: { prompt: 'new' } })
    for (const token of [{ taskId: 'other', storageEpoch: 1 }, { taskId: 'task-1', storageEpoch: 2 }]) {
      await expect(store.saveTaskFenced(updated, token)).rejects.toBeInstanceOf(StorageFenceConflictError)
    }
    await store.saveTaskFenced(updated, { taskId: 'task-1', storageEpoch: 1 })
    const after = (await store.getTaskMutationSnapshot('task-1'))!
    expect(after.task).toEqual(updated)
    expect(Number(after.revision)).toBe(Number(before.revision) + 1)
    const lease = (await store.acquireStorageLock('task-1', 'owner', 'release', 10_000))!
    await store.closeWriteFence(lease, 1)
    await expect(store.saveTaskFenced(task(), { taskId: 'task-1', storageEpoch: 1 }))
      .rejects.toBeInstanceOf(StorageFenceConflictError)
    expect(await store.getTask('task-1')).toEqual(updated)
  })

  it('validates a rehydration epoch and clears stale series without affecting other tasks', async () => {
    const store = new MemoryShortTermStore()
    await store.saveTask(task())
    await store.setSeriesLatest('task-1', 'old', event())
    await store.setSeriesLatest('other', 'kept', event(0, { taskId: 'other' }))
    const lease = (await store.acquireStorageLock('task-1', 'owner', 'release', 10_000))!
    const snapshot = {
      task: task(), archiveWatermark: 2, maxEventIndex: 2,
      replayEvents: [event(2)], seriesLatest: [], storageEpoch: 1,
    }
    await expect(store.restoreHotTaskFenced(snapshot, lease, 1)).rejects.toBeInstanceOf(StorageFenceConflictError)
    await expect(store.restoreHotTaskFenced({ ...snapshot, task: task({ id: 'other' }) }, lease, 2))
      .rejects.toBeInstanceOf(StorageFenceConflictError)
    await store.restoreHotTaskFenced(snapshot, lease, 2)
    expect(await store.getSeriesLatest('task-1', 'old')).toBeNull()
    expect(await store.getSeriesLatest('other', 'kept')).not.toBeNull()
    expect(await store.nextIndex('task-1')).toBe(3)
  })

  it('rejects an invalid terminal projection fence and a gap before accepting the next event', async () => {
    const store = new MemoryShortTermStore()
    await store.saveTask(task())
    const lease = (await store.acquireStorageLock('task-1', 'owner', 'release', 10_000))!
    const pending = projection()
    await expect(store.projectTerminalFenced(pending, lease, 1, 2))
      .rejects.toBeInstanceOf(StorageFenceConflictError)
    await store.closeWriteFence(lease, 1)
    await expect(store.projectTerminalFenced(pending, lease, 1, 3))
      .rejects.toBeInstanceOf(StorageFenceConflictError)
    await expect(store.projectTerminalFenced({ ...pending, event: event(2) }, lease, 1, 2))
      .rejects.toThrow('not contiguous')
    expect(await store.getEvents('task-1')).toEqual([])
    expect(await store.projectTerminalFenced(pending, lease, 1, 2)).toMatchObject({ projected: true })
    expect(await store.getTask('task-1')).toEqual(pending.task)
  })

  it('replays an already stored timeout once and refuses different event or assignment contents', async () => {
    const store = new MemoryShortTermStore()
    const pending = projection({ assignment: assignment() })
    await store.saveTask(task())
    await store.appendEvent('task-1', pending.event)
    await store.addAssignment(assignment())
    const lease = (await store.acquireStorageLock('task-1', 'owner', 'release', 10_000))!
    await store.closeWriteFence(lease, 1)
    await expect(store.projectTerminalFenced({ ...pending, event: { ...pending.event, data: 'changed' } }, lease, 1, 2))
      .rejects.toThrow('conflicts with the hot event')
    await expect(store.projectTerminalFenced({ ...pending, assignment: { ...assignment(), workerId: 'other' } }, lease, 1, 2))
      .rejects.toThrow('conflicts with the hot assignment')
    expect(await store.getTaskAssignment('task-1')).toEqual(assignment())
    expect(await store.projectTerminalFenced(pending, lease, 1, 2)).toMatchObject({ projected: false })
    expect(await store.getEvents('task-1')).toEqual([pending.event])
    expect(await store.getTaskAssignment('task-1')).toBeNull()
  })

  it.each([false, true])('leaves hot state unchanged after an assignment conflict (event already present: %s)', async alreadyPresent => {
    const store = new MemoryShortTermStore()
    await store.saveTask(task())
    await store.saveWorker({
      id: 'worker-1', status: 'busy', matchRule: {}, capacity: 1, usedSlots: 1,
      weight: 1, connectionMode: 'pull', connectedAt: 1_000, lastHeartbeatAt: 1_000,
    })
    await store.addAssignment(assignment())
    const pending = projection({ event: event(1, { type: 'taskcast:status' }), assignment: assignment() })
    const token = { taskId: 'task-1', storageEpoch: 1 }
    await store.commitEventFenced('task-1', event(), token)
    if (alreadyPresent) await store.commitEventFenced('task-1', pending.event, token)
    const lease = (await store.acquireStorageLock('task-1', 'owner', 'release', 10_000))!
    await store.closeWriteFence(lease, 1)
    const readState = async () => structuredClone({
      snapshot: await store.getTaskMutationSnapshot('task-1'),
      events: await store.getEvents('task-1'),
      assignment: await store.getTaskAssignment('task-1'),
      worker: await store.getWorker('worker-1'),
      fence: await store.getWriteFence('task-1'),
      presence: await store.getTaskStoragePresence('task-1'),
    })
    const before = await readState()

    await expect(store.projectTerminalFenced({ ...pending, assignment: { ...assignment(), cost: 2 } }, lease, 1, 2))
      .rejects.toThrow('conflicts with the hot assignment')

    expect(await readState()).toEqual(before)
    expect(await store.projectTerminalFenced(pending, lease, 1, 2)).toEqual({
      projected: !alreadyPresent, token: { taskId: 'task-1', storageEpoch: 2 },
    })
    expect(await store.getEvents('task-1')).toEqual([event(), pending.event])
    expect(await store.getTaskAssignment('task-1')).toBeNull()
    expect(await store.getWorker('worker-1')).toMatchObject({ usedSlots: 0, status: 'idle' })
    expect(await store.nextIndex('task-1')).toBe(2)
  })
})

describe('memory durable task, series and query contracts', () => {
  it('creates once, preserves the existing task, and prevents terminal status reversal', async () => {
    const store = new MemoryLongTermStore()
    expect(await store.createTaskIfAbsent(task({ ttl: 10 }))).toBe(true)
    expect(await store.createTaskIfAbsent(task({ status: 'failed' }))).toBe(false)
    expect((await store.getTask('task-1'))?.status).toBe('running')
    await store.saveTask(task({ status: 'completed', result: { done: true } }))
    await expect(store.saveTask(task())).rejects.toThrow('Durable terminal task cannot be overwritten')
    expect((await store.getTask('task-1'))?.result).toEqual({ done: true })
  })

  it('queries durable events by cursor and timestamp and returns independent snapshots', async () => {
    const store = new MemoryLongTermStore()
    await store.saveTask(task())
    for (let index = 0; index < 3; index++) await store.saveEvent(event(index))
    expect(await store.getEvents('task-1', { since: { id: 'event-0' }, limit: 1 })).toEqual([event(1)])
    expect(await store.getEvents('task-1', { since: { id: 'unknown' } })).toHaveLength(3)
    expect(await store.getEvents('task-1', { since: { timestamp: 1_001 } })).toEqual([event(2)])
    expect(await store.getEvents('task-1', { since: { index: 1 } })).toEqual([event(2)])
    const events = await store.getEvents('task-1')
    events[0]!.data = 'mutated'
    expect((await store.getEvents('task-1'))[0]!.data).toEqual({ delta: 'hello' })
  })

  it('deduplicates latest retries and refuses missing tasks or inconsistent series semantics', async () => {
    const store = new MemoryLongTermStore()
    const latest = event(0, { seriesId: 's', seriesMode: 'latest' })
    await expect(store.replaceLastSeriesEvent('task-1', 's', latest)).rejects.toThrow('Series task does not exist')
    await store.saveTask(task())
    await store.replaceLastSeriesEvent('task-1', 's', latest)
    await store.replaceLastSeriesEvent('task-1', 's', { ...latest, data: 'stale retry' })
    await expect(store.replaceLastSeriesEvent('task-1', 's', event(1, {
      seriesId: 's', seriesMode: 'latest', seriesAccField: 'text',
    }))).rejects.toThrow('semantics conflict')
    expect(await store.getEvents('task-1')).toEqual([latest])
    await updateMetadata(store, { archiveWatermark: 0 })
    await store.replaceLastSeriesEvent('task-1', 's', latest)
    await expect(store.replaceLastSeriesEvent('task-1', 'missing', latest))
      .rejects.toThrow('Archived latest series state is missing')
    const next = event(1, { seriesId: 's', seriesMode: 'latest', data: 'new snapshot' })
    await store.replaceLastSeriesEvent('task-1', 's', next)
    expect(await store.getEvents('task-1')).toEqual([next])
  })

  it('retains accumulated retries and rejects missing states, mode changes, and field changes', async () => {
    const store = new MemoryLongTermStore()
    const first = event(0, { seriesId: 's', seriesMode: 'accumulate' })
    await expect(store.accumulateSeries('task-1', 's', first, 'delta'))
      .rejects.toThrow('Series task does not exist')
    await store.saveTask(task())
    await store.accumulateSeries('task-1', 's', first, 'delta')
    await expect(store.replaceLastSeriesEvent('task-1', 's', event(1, { seriesId: 's', seriesMode: 'latest' })))
      .rejects.toThrow('semantics conflict')
    await expect(store.accumulateSeries('task-1', 's', event(1, { seriesAccField: 'text' }), 'text'))
      .rejects.toThrow('semantics conflict')
    await expect(store.accumulateSeries('task-1', 's', event(1, { seriesAccField: 'text' }), 'delta'))
      .rejects.toThrow('semantics conflict')
    expect(await store.accumulateSeries('task-1', 's', { ...first, data: 'stale retry' }, 'delta')).toEqual(first)
    await updateMetadata(store, { archiveWatermark: 0 })
    await expect(store.accumulateSeries('task-1', 'missing', first, 'delta'))
      .rejects.toThrow('Archived accumulate series state is missing')
    expect(await store.getEvents('task-1')).toEqual([first])
  })

  it('keeps non-object accumulation payloads as snapshots and rejects converting a latest series', async () => {
    const store = new MemoryLongTermStore()
    await store.saveTask(task())
    const first = event(0, { seriesId: 's', seriesMode: 'accumulate', data: null })
    const second = event(1, { seriesId: 's', seriesMode: 'accumulate', data: ['item'] })
    await store.accumulateSeries('task-1', 's', first, 'delta')
    expect(await store.accumulateSeries('task-1', 's', second, 'delta')).toEqual(second)
    await store.replaceLastSeriesEvent('task-1', 'latest', event(2, { seriesId: 'latest', seriesMode: 'latest' }))
    await expect(store.accumulateSeries('task-1', 'latest', event(3), 'delta')).rejects.toThrow('semantics conflict')
    expect((await store.getDurableSeriesState('task-1')).map(state => state.seriesId)).toEqual(['latest', 's'])
  })

  it('orders and limits pending releases, and only clears the matching request', async () => {
    const store = new MemoryLongTermStore()
    const request = { taskId: 'task-1', requestedAt: 2_000, expectedLastEventIndex: 0, inactiveSince: 1_000 }
    expect(await store.persistStorageReleaseRequest(request)).toBe(false)
    expect(await store.clearStorageReleaseRequest(request)).toBe(false)
    await store.saveTask(task())
    await store.saveTask(task({ id: 'task-2' }))
    await store.persistStorageReleaseRequest(request)
    const earlier = { ...request, taskId: 'task-2', requestedAt: 1_999 }
    await store.persistStorageReleaseRequest(earlier)
    for (const outdated of [
      { ...request, requestedAt: 1_000 },
      { ...request, expectedLastEventIndex: 1 },
      { ...request, inactiveSince: 999 },
    ]) expect(await store.clearStorageReleaseRequest(outdated)).toBe(false)
    const listed = await store.listStorageReleaseRequests(1)
    expect(listed).toEqual([earlier])
    listed[0]!.requestedAt = 0
    expect(await store.listStorageReleaseRequests(10)).toEqual([earlier, request])
    expect(await store.clearStorageReleaseRequest(request)).toBe(true)
    expect(await store.listStorageReleaseRequests(10)).toEqual([earlier])
  })

  it('rejects metadata CAS after another transition changes the durable fence', async () => {
    const store = new MemoryLongTermStore()
    await store.saveTask(task())
    const current = (await store.getTaskStorageMetadata('task-1'))!
    const update = {
      taskId: 'task-1', expectedStorageState: 'hot' as const, expectedStorageEpoch: 1,
      expectedReleaseGeneration: null, next: { ...current, storageState: 'cold' as const },
    }
    expect(await store.compareAndSetTaskStorageMetadata({ ...update, taskId: 'missing' })).toBe(false)
    expect(await store.compareAndSetTaskStorageMetadata({ ...update, expectedStorageEpoch: 2 })).toBe(false)
    expect(await store.compareAndSetTaskStorageMetadata({ ...update, expectedReleaseGeneration: 'other' })).toBe(false)
    expect(await store.compareAndSetTaskStorageMetadata(update)).toBe(true)
    expect(await store.compareAndSetTaskStorageMetadata(update)).toBe(false)
    expect((await store.getTaskStorageMetadata('task-1'))?.storageState).toBe('cold')
  })

  it('filters worker audit history, isolates workers, and protects stored payloads from callers', async () => {
    const store = new MemoryLongTermStore()
    const audits = [0, 1, 2].map(index => ({
      id: `audit-${index}`, workerId: 'worker-1', timestamp: 1_000 + index,
      action: 'updated' as const, data: { capacity: index },
    }))
    for (const audit of audits) await store.saveWorkerEvent(audit)
    expect(await store.getWorkerEvents('missing')).toEqual([])
    expect(await store.getWorkerEvents('worker-1', { since: { id: 'audit-0' }, limit: 1 })).toEqual([audits[1]])
    expect(await store.getWorkerEvents('worker-1', { since: { id: 'unknown' } })).toEqual(audits)
    expect(await store.getWorkerEvents('worker-1', { since: { timestamp: 1_001 } })).toEqual([audits[2]])
    const all = await store.getWorkerEvents('worker-1')
    all[0]!.data!.capacity = 999
    expect(await store.getWorkerEvents('worker-1')).toEqual(audits)
  })
})

describe('memory durable archive validation and recovery', () => {
  it('allows exact generation replay and refuses changed manifests or lost release ownership', async () => {
    const { store, generation } = await archiveFixture()
    expect(await store.beginArchive(generation)).toEqual(generation)
    await expect(store.beginArchive({ ...generation, targetWatermark: 2 })).rejects.toThrow('replay conflicts')
    await expect(store.beginArchive({ ...generation, manifest: { ...generation.manifest, sourceEntryCount: 99 } }))
      .rejects.toThrow('replay conflicts')
    await updateMetadata(store, { activeReleaseGeneration: 'replacement' })
    await expect(store.beginArchive(generation)).rejects.toThrow('lost its durable release fence')
  })

  it('rejects corrupted, reordered, and conflicting batch retries while allowing recovery', async () => {
    const { store, batches } = await archiveFixture()
    const first = batches[0]!
    await expect(store.archiveBatch('task-1', 'release-1', { ...first, events: [event(0, { data: 'tampered' })] }))
      .rejects.toThrow('digest mismatch')
    await expect(store.archiveBatch('task-1', 'release-1', batches[1]!)).rejects.toThrow('out of order')
    expect(await store.archiveBatch('task-1', 'release-1', first)).toEqual(first.receipt)
    expect(await store.archiveBatch('task-1', 'release-1', first)).toEqual(first.receipt)
    await expect(store.archiveBatch('task-1', 'release-1', {
      ...first, receipt: { ...first.receipt, entryCount: 2 },
    })).rejects.toThrow('replay conflicts')
    await store.archiveBatch('task-1', 'release-1', batches[1]!)
    expect(await store.finalizeArchive('task-1', 'release-1', task(), [])).toBe(1)
    expect(await store.getEvents('task-1')).toEqual([event(0), event(1)])
    await expect(store.archiveBatch('task-1', 'release-1', first)).rejects.toThrow('not open')
    await expect(store.archiveBatch('task-1', 'unknown', first)).rejects.toThrow('not open')
  })

  it('will not finalize missing batches and succeeds once the missing page arrives', async () => {
    const { store, batches } = await archiveFixture()
    await expect(store.finalizeArchive('task-1', 'missing', task(), [])).rejects.toThrow('generation is missing')
    await store.archiveBatch('task-1', 'release-1', batches[0]!)
    await expect(store.finalizeArchive('task-1', 'release-1', task(), [])).rejects.toThrow('missing batches')
    expect(await store.getArchiveWatermark('task-1')).toBe(-1)
    await store.archiveBatch('task-1', 'release-1', batches[1]!)
    expect(await store.finalizeArchive('task-1', 'release-1', task(), [])).toBe(1)
  })

  it.each([
    { sourceEntryCount: 3 },
    { sourceDigest: 'wrong-source-digest' },
    { seriesStateDigest: 'wrong-series-digest' },
  ])('rejects a mismatched manifest %j without advancing the durable watermark', async changes => {
    const { store, batches } = await archiveFixture(changes)
    for (const batch of batches) await store.archiveBatch('task-1', 'release-1', batch)
    await expect(store.finalizeArchive('task-1', 'release-1', task(), []))
      .rejects.toThrow('manifest verification failed')
    expect(await store.getArchiveWatermark('task-1')).toBe(-1)
  })

  it('rejects finalization when a newer release has taken the durable fence', async () => {
    const { store, batches } = await archiveFixture()
    for (const batch of batches) await store.archiveBatch('task-1', 'release-1', batch)
    await updateMetadata(store, { activeReleaseGeneration: 'release-2' })
    await expect(store.finalizeArchive('task-1', 'release-1', task(), []))
      .rejects.toThrow('finalization lost its durable release fence')
    expect(await store.getArchiveWatermark('task-1')).toBe(-1)
  })
})

describe('memory durable TTL and projection recovery', () => {
  it.each([[0, 100], [1, 0], [1.5, 100], [1, Number.NaN]])('rejects invalid overdue claim bounds (%s, %s)', async (limit, ttl) => {
    await expect(new MemoryLongTermStore().claimOverdueTasks(limit, ttl)).rejects.toThrow('TTL claim bounds are invalid')
  })

  it('claims deadlines in order with a stable task-ID tie break', async () => {
    vi.useFakeTimers()
    vi.setSystemTime(1_000)
    const store = new MemoryLongTermStore()
    for (const id of ['b', 'a', 'earlier']) await store.saveTask(task({ id, ttl: id === 'earlier' ? 1 : 2 }))
    vi.setSystemTime(4_000)
    expect((await store.claimOverdueTasks(2, 100)).map(claim => claim.taskId)).toEqual(['earlier', 'a'])
    expect((await store.claimOverdueTasks(2, 100)).map(claim => claim.taskId)).toEqual(['b'])
  })

  it('rejects malformed timeout inputs, assignment races, and event gaps before committing a valid timeout', async () => {
    const { store, claim, timeout } = await overdueFixture()
    const status = event(0, { type: 'taskcast:status' })
    for (const invalid of [task({ status: 'completed' }), task({ id: 'other', status: 'timeout' })]) {
      await expect(store.terminalizeTtlClaim(claim, invalid, status, null)).rejects.toThrow('input is invalid')
    }
    await expect(store.terminalizeTtlClaim(claim, timeout, event(), null)).rejects.toThrow('input is invalid')
    await expect(store.terminalizeTtlClaim(claim, timeout, event(0, { taskId: 'other', type: 'taskcast:status' }), null))
      .rejects.toThrow('input is invalid')
    await store.saveDurableAssignment(assignment())
    await expect(store.terminalizeTtlClaim(claim, timeout, status, null)).rejects.toThrow('assignment changed')
    await expect(store.terminalizeTtlClaim(claim, timeout, { ...status, index: 2 }, assignment()))
      .rejects.toThrow('not contiguous')
    expect((await store.getTask('task-1'))?.status).toBe('running')
    expect(await store.getEvents('task-1')).toEqual([])
    const projected = await store.terminalizeTtlClaim(claim, timeout, status, assignment())
    expect(projected?.assignment).toEqual(assignment())
    expect((await store.getTask('task-1'))?.status).toBe('timeout')
    expect(await store.terminalizeTtlClaim(claim, timeout, status, assignment())).toBeNull()
  })

  it.each([[0, 'owner', 100], [1, '', 100], [1, 'owner', 0], [1.5, 'owner', 100]] as const)(
    'rejects invalid projection claim bounds (%s, %s, %s)', async (limit, owner, ttl) => {
      await expect(new MemoryLongTermStore().claimTerminalProjections(limit, owner, ttl))
        .rejects.toThrow('Terminal projection claim bounds are invalid')
    },
  )

  it('fences expired projection owners, then allows the replacement owner to complete idempotently', async () => {
    const { store, claim, timeout } = await overdueFixture()
    const original = (await store.terminalizeTtlClaim(claim, timeout, event(0, { type: 'taskcast:status' }), null))!
    await expect(store.completeTerminalProjection({ ...original, claimToken: 'stranger' }))
      .rejects.toThrow('claim was lost')
    await expect(store.completeTerminalProjection({ ...original, projectionId: 'missing' }))
      .rejects.toThrow('claim was lost')
    expect(await store.claimTerminalProjections(1, 'recovery', 1_000)).toEqual([])
    vi.setSystemTime(claim.claimUntil + 1)
    await expect(store.completeTerminalProjection(original)).rejects.toThrow('claim was lost')
    const [recovered] = await store.claimTerminalProjections(1, 'recovery', 1_000)
    expect(recovered?.claimToken).toBe('recovery')
    await expect(store.completeTerminalProjection(original)).rejects.toThrow('claim was lost')
    await store.completeTerminalProjection(recovered!)
    await store.completeTerminalProjection(recovered!)
    expect(await store.claimTerminalProjections(1, 'later', 1_000)).toEqual([])
  })

  it('deletes completed TTL projection records as part of terminal whole-task cleanup', async () => {
    vi.useFakeTimers()
    vi.setSystemTime(10_000)
    const store = new MemoryLongTermStore()
    const enrolled = task({ ttl: 1, cleanupPolicyVersion: 1, cleanupResolvedAt: 10_000, cleanup: { rules: [{ target: 'all', trigger: {} }] } })
    await store.claimTaskCreation(enrolled, 'creation', 1_000)
    await store.completeTaskCreation('task-1', 'creation')
    vi.setSystemTime(11_001)
    const [ttlClaim] = await store.claimOverdueTasks(1, 1_000)
    const pending = (await store.terminalizeTtlClaim(ttlClaim!, { ...enrolled, status: 'timeout', completedAt: 11_001 },
      event(0, { type: 'taskcast:status' }), null))!
    await store.completeTerminalProjection(pending)
    await updateMetadata(store, { storageState: 'cold', archiveWatermark: 0, coldAt: 11_001 })
    const [cleanupClaim] = await store.claimCleanupTasks(1, 1_000)
    expect(await store.beginTaskCleanup(cleanupClaim!, 1, 0)).toBe(true)
    expect(await store.deleteTaskCleanupBatch(cleanupClaim!, 1)).toEqual({ deletedEvents: 1, complete: true })
    expect(await store.getTask('task-1')).toBeNull()
    // A removed projection cannot be mistaken for an already completed projection after ID reuse.
    await expect(store.completeTerminalProjection(pending)).rejects.toThrow('claim was lost')
    expect(await store.createTaskIfAbsent(task())).toBe(true)
  })
})
