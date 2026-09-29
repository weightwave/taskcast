import { describe, expect, it, vi } from 'vitest'
import { MemoryLongTermStore, MemoryShortTermStore } from '../../src/memory-adapters.js'
import { StorageCoordinator, type StorageCoordinatorOptions } from '../../src/storage-coordinator.js'
import type { ArchiveSourcePage, LongTermStore, ShortTermStore, Task, TaskEvent, TaskStorageMetadata } from '../../src/types.js'

const task: Task = { id: 'task', status: 'running', createdAt: 1, updatedAt: 1 }
const event = (index: number, overrides: Partial<TaskEvent> = {}): TaskEvent => ({
  id: `event-${index}`, taskId: task.id, index, timestamp: 10 + index,
  type: 'message', level: 'info', data: { delta: `${index}` }, ...overrides,
})
const preconditions = { expectedLastEventIndex: 1, inactiveSince: 100 }
const sourcePage = (events: TaskEvent[], done = true): ArchiveSourcePage => ({
  taskId: task.id, watermark: preconditions.expectedLastEventIndex, cursor: null,
  events, done, nextCursor: null,
})

async function setMetadata(durable: MemoryLongTermStore, change: Partial<TaskStorageMetadata>) {
  const current = (await durable.getTaskStorageMetadata(task.id))!
  expect(await durable.compareAndSetTaskStorageMetadata({
    taskId: task.id, expectedStorageState: current.storageState,
    expectedStorageEpoch: current.storageEpoch,
    expectedReleaseGeneration: current.activeReleaseGeneration,
    next: { ...current, ...change },
  })).toBe(true)
}

async function fixture(options: Partial<StorageCoordinatorOptions> = {}, creationToken?: string) {
  const hot = new MemoryShortTermStore()
  const durable = new MemoryLongTermStore()
  await hot.saveTask(task)
  if (creationToken) {
    await durable.claimTaskCreation(task, creationToken, 30_000)
    await durable.completeTaskCreation(task.id, creationToken)
  } else {
    await durable.saveTask(task)
  }
  for (let index = 0; index < 2; index++) {
    const { index: _, ...input } = event(index)
    const result = await hot.commitEventFenced(task.id, input, { taskId: task.id, storageEpoch: 1 })
    await durable.saveEvent(result.event, creationToken ? { creationToken } : undefined)
  }
  const observations: Record<string, unknown>[] = []
  const coordinator = new StorageCoordinator({
    shortTermStore: hot, longTermStore: durable,
    observe: (value) => observations.push(value), ...options,
  })
  return { hot, durable, coordinator, observations }
}

async function coldFixture(options: Partial<StorageCoordinatorOptions> = {}, creationToken?: string) {
  const result = await fixture(options, creationToken)
  await result.coordinator.releaseTaskStorage(task.id, preconditions)
  return result
}

async function interruptedFixture({ archived = false, deleted = false, reopened = false } = {}) {
  const result = archived ? await coldFixture() : await fixture()
  const { hot, durable } = result
  if (archived && !deleted) {
    await result.coordinator.ensureTaskHotForWrite(task.id)
  }
  const metadata = (await durable.getTaskStorageMetadata(task.id))!
  if (!deleted) {
    const lease = (await hot.acquireStorageLock(task.id, 'interrupted-lock', 'interrupted-generation', 30_000))!
    await hot.closeWriteFence(lease, metadata.storageEpoch)
    if (reopened) await hot.reopenWriteFence(lease, metadata.storageEpoch)
    await hot.releaseStorageLock(lease)
  }
  await setMetadata(durable, { storageState: 'releasing', activeReleaseGeneration: 'interrupted-generation' })
  return result
}

async function expectRetained(hot: MemoryShortTermStore) {
  expect(await hot.getTaskStoragePresence(task.id)).toMatchObject({ task: true, eventCount: 2 })
}

describe('storage coordinator capability and input checks', () => {
  it.each([
    { archiveBatchSize: 0 }, { storageLockTtlMs: 0 }, { rehydrateReplayEvents: -1 },
  ])('rejects invalid lifecycle limits before touching storage: %s', (options) => {
    expect(() => new StorageCoordinator({
      shortTermStore: new MemoryShortTermStore(), longTermStore: new MemoryLongTermStore(), ...options,
    })).toThrow(/must be/)
  })

  it.each(['hot flag', 'hot method', 'durable flag', 'durable method'])('rejects an incomplete adapter contract: %s', async (kind) => {
    const hot = new MemoryShortTermStore()
    const durable = new MemoryLongTermStore()
    if (kind === 'hot flag') Object.defineProperty(hot, 'supportsHotColdRelease', { value: false })
    if (kind === 'hot method') Object.defineProperty(hot, 'restoreHotTaskFenced', { value: undefined })
    if (kind === 'durable flag') Object.defineProperty(durable, 'supportsHotColdRelease', { value: false })
    if (kind === 'durable method') Object.defineProperty(durable, 'getRecentEvents', { value: undefined })
    const coordinator = new StorageCoordinator({ shortTermStore: hot as ShortTermStore, longTermStore: durable as LongTermStore })
    await expect(coordinator.releaseTaskStorage(task.id, preconditions)).rejects.toMatchObject({ code: 'storage_release_unsupported' })
  })

  it.each(['release', 'write', 'recover'])('rejects absent durable identity during %s', async (operation) => {
    const coordinator = new StorageCoordinator({ shortTermStore: new MemoryShortTermStore(), longTermStore: new MemoryLongTermStore() })
    const result = operation === 'release' ? coordinator.releaseTaskStorage(task.id, preconditions)
      : operation === 'write' ? coordinator.ensureTaskHotForWrite(task.id) : coordinator.recoverTaskStorage(task.id)
    await expect(result).rejects.toThrow(/metadata does not exist/)
  })

  it('rejects release and writes while another generation is releasing', async () => {
    const { hot, coordinator } = await interruptedFixture()
    await expect(coordinator.releaseTaskStorage(task.id, preconditions)).rejects.toMatchObject({ code: 'storage_busy' })
    await expect(coordinator.ensureTaskHotForWrite(task.id)).rejects.toMatchObject({ code: 'storage_busy' })
    await expectRetained(hot)
  })

  it('blocks release while a writer still uses the old storage protocol', async () => {
    const { hot, coordinator } = await fixture()
    vi.spyOn(hot, 'listStorageWriters').mockResolvedValue([
      { instanceId: 'old-worker', storageProtocolVersion: 1, build: 'legacy', expiresAt: Date.now() + 30_000 },
    ])
    const close = vi.spyOn(hot, 'closeWriteFence')
    await expect(coordinator.releaseTaskStorage(task.id, preconditions)).rejects.toThrow(/old-worker/)
    expect(close).not.toHaveBeenCalled()
    await expectRetained(hot)
  })

  it('retains hot data when durable activity is newer than the cutoff', async () => {
    const { hot, durable, coordinator } = await fixture()
    await setMetadata(durable, { lastEventAt: 101 })
    await expect(coordinator.releaseTaskStorage(task.id, preconditions)).rejects.toThrow(/activity newer/)
    await expectRetained(hot)
    expect(await hot.getWriteFence(task.id)).toMatchObject({ acceptingWrites: true, storageEpoch: 2 })
  })

  it('does not change the release outcome when observers or lock cleanup throw', async () => {
    const { hot, coordinator } = await fixture({ observe: () => { throw new Error('observer unavailable') } })
    vi.spyOn(hot, 'releaseStorageLock').mockRejectedValue(new Error('lock cleanup unavailable'))
    await expect(coordinator.releaseTaskStorage(task.id, preconditions)).resolves.toMatchObject({ released: true })
    expect(await hot.getTask(task.id)).toBeNull()
  })
})

describe('release lease and archive consistency', () => {
  it('does not reopen or unlock after a lease renewal transport failure', async () => {
    const { hot, coordinator } = await fixture()
    vi.spyOn(hot, 'renewStorageLock').mockRejectedValue(new Error('redis unavailable'))
    const reopen = vi.spyOn(hot, 'reopenWriteFence')
    const unlock = vi.spyOn(hot, 'releaseStorageLock')
    await expect(coordinator.releaseTaskStorage(task.id, preconditions)).rejects.toThrow(/lease renewal failed/)
    expect(reopen).not.toHaveBeenCalled()
    expect(unlock).not.toHaveBeenCalled()
    await expectRetained(hot)
  })

  it('reopens hot storage if installing the release generation loses its CAS', async () => {
    const { hot, durable, coordinator } = await fixture()
    vi.spyOn(durable, 'compareAndSetTaskStorageMetadata').mockResolvedValueOnce(false)
    await expect(coordinator.releaseTaskStorage(task.id, preconditions)).rejects.toThrow(/metadata changed before release/)
    expect(await hot.getWriteFence(task.id)).toMatchObject({ acceptingWrites: true, storageEpoch: 2 })
    await expectRetained(hot)
  })

  it('rejects a source that changes its batch count after the manifest is sealed', async () => {
    const { hot, coordinator } = await fixture()
    vi.spyOn(hot, 'readArchiveSourcePage')
      .mockResolvedValueOnce(sourcePage([event(0), event(1)]))
      .mockResolvedValueOnce(sourcePage([]))
    await expect(coordinator.releaseTaskStorage(task.id, preconditions)).rejects.toThrow(/changed between sealing passes/)
    await expectRetained(hot)
  })

  it('does not finalize an archive after its hot task disappears', async () => {
    const { hot, durable, coordinator } = await fixture()
    vi.spyOn(hot, 'getTask').mockResolvedValue(null)
    const finalize = vi.spyOn(durable, 'finalizeArchive')
    await expect(coordinator.releaseTaskStorage(task.id, preconditions)).rejects.toThrow(/Hot task disappeared/)
    expect(finalize).not.toHaveBeenCalled()
  })

  it.each(['watermark', 'missing metadata', 'epoch', 'generation', 'state'])('requires archive read-back proof: %s', async (failure) => {
    const { hot, durable, coordinator, observations } = await fixture()
    let finalized = false
    const getMetadata = durable.getTaskStorageMetadata.bind(durable)
    const getWatermark = durable.getArchiveWatermark.bind(durable)
    vi.spyOn(durable, 'getTaskStorageMetadata').mockImplementation((taskId) =>
      finalized && failure === 'missing metadata' ? Promise.resolve(null) : getMetadata(taskId))
    vi.spyOn(durable, 'getArchiveWatermark').mockImplementation((taskId) =>
      finalized && failure === 'watermark' ? Promise.resolve(-1) : getWatermark(taskId))
    const finalize = durable.finalizeArchive.bind(durable)
    vi.spyOn(durable, 'finalizeArchive').mockImplementation(async (...args) => {
      const watermark = await finalize(...args)
      if (failure !== 'watermark' && failure !== 'missing metadata') {
        await setMetadata(durable, failure === 'epoch' ? { storageEpoch: 2 }
          : failure === 'generation' ? { activeReleaseGeneration: 'new-owner' } : { storageState: 'cold' })
      }
      finalized = true
      return watermark
    })
    await expect(coordinator.releaseTaskStorage(task.id, preconditions)).rejects.toThrow(/read-back did not prove release/)
    await expectRetained(hot)
    expect(observations).toContainEqual(expect.objectContaining({ event: 'storage_watermark_mismatch', operation: 'release' }))
  })

  it('leaves a failed final cold CAS recoverable after hot deletion', async () => {
    const { hot, durable, coordinator } = await fixture()
    const cas = durable.compareAndSetTaskStorageMetadata.bind(durable)
    vi.spyOn(durable, 'compareAndSetTaskStorageMetadata').mockImplementation((update) =>
      update.next.storageState === 'cold' ? Promise.resolve(false) : cas(update))
    await expect(coordinator.releaseTaskStorage(task.id, preconditions)).rejects.toThrow(/cold transition lost its fence/)
    expect(await hot.getTask(task.id)).toBeNull()
    expect(await durable.getTaskStorageMetadata(task.id)).toMatchObject({ storageState: 'releasing', archiveWatermark: 1 })
  })

  it.each([
    { events: [event(0, { timestamp: NaN })], message: /invalid event timestamp/ },
    { events: [event(0), event(0)], message: /not strictly ordered/ },
    { events: [event(0, { taskId: 'foreign-task' })], message: /not strictly ordered/ },
    { events: [event(2)], message: /exceeds its closed watermark/ },
    { events: [event(0, { seriesId: 'output', seriesMode: 'latest' }), event(1, { seriesId: 'output', seriesMode: 'accumulate' })], message: /Series mode changed/ },
  ])('retains hot storage when the archive source violates $message', async ({ events, message }) => {
    const { hot, coordinator } = await fixture()
    vi.spyOn(hot, 'readArchiveSourcePage').mockResolvedValue(sourcePage(events))
    await expect(coordinator.releaseTaskStorage(task.id, preconditions)).rejects.toThrow(message)
    await expectRetained(hot)
  })

  it('rejects an incomplete page without a continuation cursor', async () => {
    const { hot, coordinator } = await fixture()
    vi.spyOn(hot, 'readArchiveSourcePage').mockResolvedValue(sourcePage([event(0)], false))
    await expect(coordinator.releaseTaskStorage(task.id, preconditions)).rejects.toThrow(/omitted its next cursor/)
    await expectRetained(hot)
  })

  it.each([null, event(2, { seriesId: 'output', seriesMode: 'latest' })])('requires series state within the sealed watermark: %s', async (latest) => {
    const { hot, coordinator } = await fixture()
    vi.spyOn(hot, 'readArchiveSourcePage').mockResolvedValue(sourcePage([
      event(0, { seriesId: 'output', seriesMode: 'latest' }),
    ]))
    vi.spyOn(hot, 'getSeriesLatest').mockResolvedValue(latest)
    await expect(coordinator.releaseTaskStorage(task.id, preconditions)).rejects.toThrow(/Series state is missing or exceeds/)
    await expectRetained(hot)
  })
})

describe('rehydration races and durable snapshot integrity', () => {
  it.each(['newer hot epoch', 'competing rehydrator', 'restored hot snapshot', 'lost CAS response'])(
    'preserves durable creation identity when recovering a %s', async recovery => {
      const creationToken = 'enrolled-creation'
      const { hot, durable, coordinator } = recovery === 'newer hot epoch'
        ? await fixture({}, creationToken) : await coldFixture({}, creationToken)
      if (recovery === 'newer hot epoch') {
        const lease = (await hot.acquireStorageLock(task.id, 'repair', 'repair', 30_000))!
        await hot.closeWriteFence(lease, 1)
        await hot.reopenWriteFence(lease, 1)
        await hot.releaseStorageLock(lease)
      } else if (recovery === 'lost CAS response') {
        const cas = durable.compareAndSetTaskStorageMetadata.bind(durable)
        vi.spyOn(durable, 'compareAndSetTaskStorageMetadata').mockImplementation(async update => {
          await cas(update)
          return false
        })
      } else {
        const cold = (await durable.getTaskStorageMetadata(task.id))!
        await coordinator.ensureTaskHotForWrite(task.id)
        if (recovery === 'competing rehydrator') {
          vi.spyOn(durable, 'getTaskStorageMetadata').mockResolvedValueOnce(cold)
        } else {
          await setMetadata(durable, cold)
        }
      }

      const token = await coordinator.ensureTaskHotForWrite(task.id)
      expect(token).toEqual({ taskId: task.id, storageEpoch: 2, creationToken: 'enrolled-creation' })
      await durable.saveEvent(event(2), { creationToken: token.creationToken! })
      await expect(durable.saveEvent(event(3), { creationToken: 'replaced-creation' }))
        .rejects.toMatchObject({ code: 'storage_fence_conflict' })
      expect(await durable.getEvents(task.id)).toEqual([event(0), event(1), event(2)])
    },
  )

  it('does not retry cold rehydration after a mutation has already started', async () => {
    const { hot, coordinator } = await coldFixture()
    await expect(coordinator.ensureTaskHotForWrite(task.id, false)).rejects.toThrow(/became cold after the write mutation started/)
    expect(await hot.getTask(task.id)).toBeNull()
  })

  it('bounds metadata repair retries while preserving the newer writable hot epoch', async () => {
    const { hot, durable, coordinator } = await fixture()
    const lease = (await hot.acquireStorageLock(task.id, 'repair', 'repair', 30_000))!
    await hot.closeWriteFence(lease, 1)
    await hot.reopenWriteFence(lease, 1)
    await hot.releaseStorageLock(lease)
    const cas = vi.spyOn(durable, 'compareAndSetTaskStorageMetadata').mockResolvedValue(false)
    await expect(coordinator.ensureTaskHotForWrite(task.id)).rejects.toThrow(/repair lost its metadata race/)
    expect(cas).toHaveBeenCalledTimes(3)
    expect(await hot.getWriteFence(task.id)).toMatchObject({ acceptingWrites: true, storageEpoch: 2 })
  })

  it('rejects a hot metadata record without a matching write fence', async () => {
    const { hot, coordinator } = await fixture()
    vi.spyOn(hot, 'getWriteFence').mockResolvedValue(null)
    await expect(coordinator.ensureTaskHotForWrite(task.id)).rejects.toThrow(/write fence does not match/)
  })

  it.each(['throws', 'lost'])('does not unlock after rehydration renewal is %s', async (failure) => {
    const { hot, coordinator } = await coldFixture()
    const renew = vi.spyOn(hot, 'renewStorageLock')
    if (failure === 'throws') renew.mockRejectedValue(new Error('redis unavailable'))
    else renew.mockResolvedValue(false)
    const unlock = vi.spyOn(hot, 'releaseStorageLock')
    await expect(coordinator.ensureTaskHotForWrite(task.id)).rejects.toThrow(/rehydration lease/)
    expect(unlock).not.toHaveBeenCalled()
    expect(await hot.getTask(task.id)).toBeNull()
  })

  it.each(['missing', 'releasing', 'new creation', 'new epoch', 'active generation'])('rechecks cold identity after acquiring the lease: %s', async (race) => {
    const { hot, durable, coordinator } = await coldFixture()
    const initial = (await durable.getTaskStorageMetadata(task.id))!
    const next = race === 'missing' ? null : { ...initial,
      ...(race === 'releasing' && { storageState: 'releasing' as const }),
      ...(race === 'new creation' && { creationToken: 'replacement' }),
      ...(race === 'new epoch' && { storageEpoch: 2 }),
      ...(race === 'active generation' && { activeReleaseGeneration: 'other' }),
    }
    vi.spyOn(durable, 'getTaskStorageMetadata').mockResolvedValueOnce(initial).mockResolvedValue(next)
    const restore = vi.spyOn(hot, 'restoreHotTaskFenced')
    await expect(coordinator.ensureTaskHotForWrite(task.id)).rejects.toThrow(
      race === 'missing' ? /metadata does not exist/ : race === 'releasing' ? /operation is in progress/ : /metadata changed/,
    )
    expect(restore).not.toHaveBeenCalled()
  })

  it.each([true, false])('handles a competing rehydrator that finishes before lease acquisition (valid fence: %s)', async (validFence) => {
    const { hot, durable, coordinator } = await coldFixture()
    const cold = (await durable.getTaskStorageMetadata(task.id))!
    const token = await coordinator.ensureTaskHotForWrite(task.id)
    vi.spyOn(durable, 'getTaskStorageMetadata').mockResolvedValueOnce(cold)
    if (!validFence) vi.spyOn(hot, 'getWriteFence').mockResolvedValue(null)
    const restore = vi.spyOn(hot, 'restoreHotTaskFenced')
    const result = coordinator.ensureTaskHotForWrite(task.id)
    if (validFence) await expect(result).resolves.toEqual(token)
    else await expect(result).rejects.toThrow(/write fence does not match/)
    expect(restore).not.toHaveBeenCalled()
  })

  it('does not adopt a restored epoch if its durable recovery CAS loses', async () => {
    const { hot, durable, coordinator } = await coldFixture()
    const cold = (await durable.getTaskStorageMetadata(task.id))!
    await coordinator.ensureTaskHotForWrite(task.id)
    await setMetadata(durable, cold)
    vi.spyOn(durable, 'compareAndSetTaskStorageMetadata').mockResolvedValue(false)
    await expect(coordinator.ensureTaskHotForWrite(task.id)).rejects.toThrow(/metadata recovery race/)
    await expectRetained(hot)
  })

  it('refuses to overwrite partial hot remnants for a cold task', async () => {
    const { hot, coordinator } = await coldFixture()
    await hot.appendEvent(task.id, event(1))
    await expect(coordinator.ensureTaskHotForWrite(task.id)).rejects.toThrow(/partial or stale hot storage/)
    expect(await hot.getEvents(task.id)).toEqual([event(1)])
  })

  it.each(['missing task', 'expired history', 'invalid maximum', 'watermark gap', 'foreign event', 'invalid index', 'event above maximum', 'epoch overflow'])('rejects an unsafe durable restore snapshot: %s', async (failure) => {
    const { hot, durable, coordinator, observations } = await coldFixture()
    if (failure === 'missing task') vi.spyOn(durable, 'getTask').mockResolvedValue(null)
    if (failure === 'expired history') vi.spyOn(durable, 'getTask').mockResolvedValue({ ...task, historyExpiredAt: 100 })
    if (failure === 'invalid maximum') vi.spyOn(durable, 'getLastEventIndex').mockResolvedValue(NaN)
    if (failure === 'watermark gap') vi.spyOn(durable, 'getLastEventIndex').mockResolvedValue(0)
    if (failure === 'foreign event') vi.spyOn(durable, 'getRecentEvents').mockResolvedValue([event(0, { taskId: 'foreign' })])
    if (failure === 'invalid index') vi.spyOn(durable, 'getRecentEvents').mockResolvedValue([event(0.5)])
    if (failure === 'event above maximum') vi.spyOn(durable, 'getRecentEvents').mockResolvedValue([event(2)])
    if (failure === 'epoch overflow') await setMetadata(durable, { storageEpoch: Number.MAX_SAFE_INTEGER })
    const restore = vi.spyOn(hot, 'restoreHotTaskFenced')
    await expect(coordinator.ensureTaskHotForWrite(task.id)).rejects.toThrow(
      failure === 'missing task' ? /Durable task does not exist/
        : failure === 'expired history' ? /history has expired/
          : failure === 'epoch overflow' ? /epoch exceeds safe bounds/ : /snapshot is inconsistent/,
    )
    expect(restore).not.toHaveBeenCalled()
    expect(observations).toContainEqual(expect.objectContaining({ event: 'storage_rehydrate', outcome: 'failed' }))
  })

  it.each([true, false])('resolves a restore CAS conflict only if the same epoch is already durable (same epoch: %s)', async (sameEpoch) => {
    const { hot, durable, coordinator } = await coldFixture()
    const cas = durable.compareAndSetTaskStorageMetadata.bind(durable)
    vi.spyOn(durable, 'compareAndSetTaskStorageMetadata').mockImplementation(async (update) => {
      if (sameEpoch) await cas(update)
      return false
    })
    const result = coordinator.ensureTaskHotForWrite(task.id)
    if (sameEpoch) {
      await expect(result).resolves.toMatchObject({ storageEpoch: 2 })
      await expectRetained(hot)
    } else {
      await expect(result).rejects.toThrow(/durable metadata race/)
      expect(await hot.getTaskStoragePresence(task.id)).toMatchObject({ task: false, writeFence: false, eventCount: 0 })
      expect(await durable.getTaskStorageMetadata(task.id)).toMatchObject({ storageState: 'cold', storageEpoch: 1 })
    }
  })

  it('returns the successful restore even if lock release fails', async () => {
    const { hot, coordinator } = await coldFixture()
    vi.spyOn(hot, 'releaseStorageLock').mockRejectedValue(new Error('connection lost'))
    await expect(coordinator.ensureTaskHotForWrite(task.id)).resolves.toMatchObject({ storageEpoch: 2 })
  })
})

describe('interrupted release recovery', () => {
  it('does not unlock a recovery lease that another owner has taken', async () => {
    const { hot, coordinator } = await interruptedFixture()
    vi.spyOn(hot, 'renewStorageLock').mockResolvedValue(false)
    const unlock = vi.spyOn(hot, 'releaseStorageLock')
    await expect(coordinator.recoverTaskStorage(task.id)).rejects.toThrow(/recovery lease was lost/)
    expect(unlock).not.toHaveBeenCalled()
    await expectRetained(hot)
  })

  it('does not adopt a reopened hot epoch after losing its metadata CAS', async () => {
    const { hot, durable, coordinator } = await interruptedFixture({ reopened: true })
    vi.spyOn(durable, 'compareAndSetTaskStorageMetadata').mockResolvedValue(false)
    await expect(coordinator.recoverTaskStorage(task.id)).rejects.toThrow(/Recovered hot epoch lost its metadata race/)
    expect(await hot.getWriteFence(task.id)).toMatchObject({ acceptingWrites: true, storageEpoch: 2 })
  })

  it('requires durable watermark coverage before recovering missing hot data as cold', async () => {
    const { durable, coordinator } = await interruptedFixture({ archived: true, deleted: true })
    vi.spyOn(durable, 'getArchiveWatermark').mockResolvedValue(0)
    await expect(coordinator.recoverTaskStorage(task.id)).rejects.toThrow(/not covered by the durable watermark/)
    expect(await durable.getTaskStorageMetadata(task.id)).toMatchObject({ storageState: 'releasing' })
  })

  it.each([true, false])('finishes an already-proven archive while respecting the cold CAS (CAS succeeds: %s)', async (succeeds) => {
    const { hot, durable, coordinator } = await interruptedFixture({ archived: true })
    if (!succeeds) {
      const cas = durable.compareAndSetTaskStorageMetadata.bind(durable)
      vi.spyOn(durable, 'compareAndSetTaskStorageMetadata').mockImplementation((update) =>
        update.next.storageState === 'cold' ? Promise.resolve(false) : cas(update))
    }
    const result = coordinator.recoverTaskStorage(task.id)
    if (succeeds) await expect(result).resolves.toMatchObject({ released: true, archiveWatermark: 1, storageState: 'cold' })
    else await expect(result).rejects.toThrow(/Recovered cold transition lost its generation/)
    expect(await hot.getTask(task.id)).toBeNull()
    expect(await durable.getEvents(task.id)).toEqual([event(0), event(1)])
  })

  it.each([true, false])('does not mutate hot data when recovery generation CAS loses (hot deleted: %s)', async (deleted) => {
    const { hot, durable, coordinator } = await interruptedFixture({ archived: deleted, deleted })
    vi.spyOn(durable, 'compareAndSetTaskStorageMetadata').mockResolvedValue(false)
    const remove = vi.spyOn(hot, 'deleteTaskStorageFenced')
    const reopen = vi.spyOn(hot, 'reopenWriteFence')
    await expect(coordinator.recoverTaskStorage(task.id)).rejects.toThrow(/generation was not installed/)
    expect(remove).not.toHaveBeenCalled()
    expect(reopen).not.toHaveBeenCalled()
  })

  it('keeps a missing-hot recovery pending when its cold transition CAS loses', async () => {
    const { durable, coordinator } = await interruptedFixture({ archived: true, deleted: true })
    const cas = durable.compareAndSetTaskStorageMetadata.bind(durable)
    vi.spyOn(durable, 'compareAndSetTaskStorageMetadata').mockImplementation((update) =>
      update.next.storageState === 'cold' ? Promise.resolve(false) : cas(update))
    await expect(coordinator.recoverTaskStorage(task.id)).rejects.toThrow(/Recovered cold transition lost its generation/)
    expect(await durable.getTaskStorageMetadata(task.id)).toMatchObject({ storageState: 'releasing', archiveWatermark: 1 })
  })

  it('leaves a reopened epoch repairable when its durable hot CAS loses', async () => {
    const { hot, durable, coordinator } = await interruptedFixture()
    const cas = durable.compareAndSetTaskStorageMetadata.bind(durable)
    vi.spyOn(durable, 'compareAndSetTaskStorageMetadata').mockImplementation((update) =>
      update.next.storageState === 'hot' ? Promise.resolve(false) : cas(update))
    await expect(coordinator.recoverTaskStorage(task.id)).rejects.toThrow(/Recovered hot transition lost its generation/)
    expect(await hot.getWriteFence(task.id)).toMatchObject({ acceptingWrites: true, storageEpoch: 2 })
    await expectRetained(hot)
  })

  it('does not lose a successful recovery result if releasing its lock fails', async () => {
    const { hot, coordinator } = await interruptedFixture()
    vi.spyOn(hot, 'releaseStorageLock').mockRejectedValue(new Error('connection lost'))
    await expect(coordinator.recoverTaskStorage(task.id)).resolves.toMatchObject({ storageState: 'hot' })
  })
})
