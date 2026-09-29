import { afterEach, describe, expect, it, vi } from 'vitest'
import { TaskEngine } from '../../src/engine.js'
import { MemoryBroadcastProvider, MemoryLongTermStore, MemoryShortTermStore } from '../../src/memory-adapters.js'
import { StorageCoordinator } from '../../src/storage-coordinator.js'
import { StorageFenceConflictError, StoragePreconditionError, type LongTermStore, type StorageReleaseRequest, type TaskArchive, type TaskArchiveRestoreData, type TaskEvent } from '../../src/types.js'

afterEach(() => vi.restoreAllMocks())
function setup() {
  const hot = new MemoryShortTermStore()
  const durable = new MemoryLongTermStore()
  const broadcast = new MemoryBroadcastProvider()
  const engine = new TaskEngine({ shortTermStore: hot, longTermStore: durable, broadcast, storageLockTtlMs: 1000, rehydrateReplayEvents: 10 })
  return { hot, durable, broadcast, engine }
}
function without(object: object, ...keys: string[]) {
  for (const key of keys) Object.defineProperty(object, key, { value: undefined, configurable: true })
}
const archive = (): TaskArchive => ({ schema: 'taskcast.taskArchive', version: 1, exportedAt: 10, task: { id: 'restored', status: 'pending', createdAt: 1, updatedAt: 1 }, events: [] })

describe('engine recovery contracts', () => {
  it('reports supported capabilities and rejects unsupported lifecycle operations', async () => {
    const { engine, hot } = setup()
    expect(engine.supportsStorageRelease()).toBe(true)
    expect(engine.supportsDurableTtl()).toBe(true)
    expect(engine.supportsCleanup()).toBe(false)
    const registration = { instanceId: 'writer', storageProtocolVersion: 3, build: 'test', expiresAt: 0 }
    await engine.registerStorageWriter(registration, 1000)
    expect(await engine.listStorageWriters()).toMatchObject([{ instanceId: 'writer' }])
    const unsupported = new TaskEngine({ shortTermStore: new MemoryShortTermStore(), broadcast: new MemoryBroadcastProvider() })
    expect(unsupported.supportsStorageRelease()).toBe(false)
    expect(unsupported.supportsDurableTtl()).toBe(false)
    for (const operation of [() => unsupported.recoverTaskStorage('missing'), () => unsupported.sweepDurableTtl(1), () => unsupported.sweepTerminalProjections(1), () => unsupported.releaseTaskStorageAtCurrentDurableIndex('missing', 0), () => unsupported.retryStorageReleaseRequests(1)]) {
      await expect(operation()).rejects.toMatchObject({ code: 'storage_release_unsupported' })
    }
    without(hot, 'registerStorageWriter', 'listStorageWriters')
    await expect(engine.registerStorageWriter(registration, 1000)).rejects.toThrow(/register/)
    await expect(engine.listStorageWriters()).rejects.toThrow(/list/)
  })

  it('keeps creation callbacks removable and retries transient durable claim completion', async () => {
    const { engine, durable } = setup()
    const listener = vi.fn()
    engine.addCreationListener(listener)
    engine.removeCreationListener(listener)
    engine.removeCreationListener(listener)
    vi.spyOn(durable, 'completeTaskCreation').mockRejectedValueOnce(new Error('temporary'))
    await engine.createTask({ id: 'retry' })
    expect(listener).not.toHaveBeenCalled()
    expect(durable.completeTaskCreation).toHaveBeenCalledTimes(2)
    vi.spyOn(durable, 'completeTaskCreation').mockResolvedValue(false)
    await expect(engine.createTask({ id: 'lost' })).rejects.toThrow(/claim was lost/)
  })

  it('uses legacy atomic identity creation and rejects stores that claim fencing without creation support', async () => {
    const { hot, durable, broadcast } = setup()
    without(durable, 'claimTaskCreation', 'completeTaskCreation', 'abortTaskCreation')
    const engine = new TaskEngine({ shortTermStore: hot, longTermStore: durable, broadcast })
    await expect(engine.createTask({ id: 'unsupported' })).rejects.toThrow(/token-fenced/)
    Object.defineProperty(durable, 'supportsHotColdRelease', { value: false })
    const legacy = new TaskEngine({ shortTermStore: hot, longTermStore: durable, broadcast })
    expect((await legacy.createTask({ id: 'legacy' })).id).toBe('legacy')
    await expect(legacy.createTask({ id: 'legacy' })).rejects.toThrow(/exists/)
  })

  it.each([NaN, -1, 1.5])('rejects invalid release and retry bounds: %s', async (value) => {
    const { engine } = setup()
    await expect(engine.releaseTaskStorage('x', { expectedLastEventIndex: value, inactiveSince: value })).rejects.toThrow(/preconditions/)
    await expect(engine.retryStorageReleaseRequests(value)).rejects.toThrow(/bounds/)
    await expect(engine.retryStorageReleaseRequests(1, value)).rejects.toThrow(/bounds/)
  })

  it('fails explicitly when durable release request methods are unavailable', async () => {
    const { engine, durable } = setup()
    without(durable, 'persistStorageReleaseRequest')
    await expect(engine.releaseTaskStorage('x', { expectedLastEventIndex: -1, inactiveSince: 0 })).rejects.toThrow(/persist/)
    without(durable, 'clearStorageReleaseRequest')
    await expect(engine.retryStorageReleaseRequests(1)).rejects.toThrow(/clear/)
    without(durable, 'listStorageReleaseRequests')
    await expect(engine.retryStorageReleaseRequests(1)).rejects.toThrow(/list/)
  })

  it('counts released, recovered, stale, failed and deferred requests independently', async () => {
    const { engine, durable } = setup()
    const ids = ['release', 'recover', 'stale', 'fail', 'recent']
    const requests: StorageReleaseRequest[] = ids.map(taskId => ({ taskId, requestedAt: 1, expectedLastEventIndex: -1, inactiveSince: taskId === 'recent' ? 100 : 1 }))
    vi.spyOn(durable, 'listStorageReleaseRequests').mockResolvedValue(requests)
    const clear = vi.spyOn(durable, 'clearStorageReleaseRequest').mockResolvedValue(true)
    vi.spyOn(StorageCoordinator.prototype, 'recoverTaskStorage').mockImplementation(async taskId => ({ taskId, storageState: taskId === 'recover' ? 'cold' : 'hot', archiveWatermark: -1, released: taskId === 'recover' }))
    vi.spyOn(StorageCoordinator.prototype, 'releaseTaskStorage').mockImplementation(async taskId => {
      if (taskId === 'stale') throw new StoragePreconditionError('task changed')
      if (taskId === 'fail') throw new Error('database unavailable')
      return { taskId, storageState: 'cold', archiveWatermark: -1, released: true }
    })
    expect(await engine.retryStorageReleaseRequests(5, 50)).toEqual({ claimed: 5, released: 1, recovered: 1, stale: 1, failed: 1, deferred: 1 })
    expect(clear.mock.calls.map(([request]) => request.taskId)).toEqual(['release', 'recover', 'stale'])
    expect((await engine.recoverTaskStorage('recover')).storageState).toBe('cold')
  })

  it('does not touch durable history if a legacy archive adapter cannot restore', async () => {
    const { hot, broadcast } = setup()
    without(hot, 'restoreTaskArchive')
    const engine = new TaskEngine({ shortTermStore: hot, broadcast })
    await expect(engine.importTaskArchive(archive())).rejects.toThrow(/shortTermStore/)
    const shared = { supportsHotColdRelease: false, sharesTaskArchiveRestoreStorage: true, getTask: async () => null } as unknown as LongTermStore
    const target = new TaskEngine({ shortTermStore: new MemoryShortTermStore(), longTermStore: shared, broadcast })
    expect(await target.importTaskArchive(archive())).toMatchObject({ taskId: 'restored', overwritten: false })
  })

  it('restores separate legacy durable storage before making an import visible', async () => {
    const { hot, durable, broadcast } = setup()
    Object.defineProperty(durable, 'supportsHotColdRelease', { value: false })
    const persisted = vi.fn(async (data: TaskArchiveRestoreData) => {
      expect(await hot.getTask(data.task.id)).toBeNull()
      await durable.saveTask(data.task)
      for (const event of data.events) await durable.saveEvent(event)
    })
    Object.defineProperty(durable, 'restoreTaskArchive', { value: persisted })
    const engine = new TaskEngine({ shortTermStore: hot, longTermStore: durable, broadcast })
    expect(await engine.importTaskArchive(archive())).toMatchObject({ taskId: 'restored', overwritten: false })
    expect(persisted).toHaveBeenCalledTimes(1)
    expect(await hot.getTask('restored')).toEqual(await durable.getTask('restored'))
  })

  it('exposes storage read failures to observers with a safe fallback error code', async () => {
    const { engine, durable } = setup()
    await engine.createTask({ id: 'read-fail' })
    const observe = vi.fn()
    engine.addStorageLifecycleListener(() => { throw new Error('observer failure') })
    engine.addStorageLifecycleListener(observe)
    vi.spyOn(durable, 'getEvents').mockRejectedValue(new Error('database unavailable'))
    await expect(engine.getEvents('read-fail')).rejects.toThrow('database unavailable')
    expect(observe).toHaveBeenCalledWith(expect.objectContaining({ errorCode: 'storage_unavailable' }))
  })

  it('rejects a non-advancing durable page rather than looping forever', async () => {
    const { engine, durable } = setup()
    await engine.createTask({ id: 'paged' })
    const event: TaskEvent = { id: 'repeat', taskId: 'paged', index: 0, timestamp: 1, type: 'test', level: 'info', data: null }
    vi.spyOn(durable, 'getEvents').mockResolvedValue([event, event])
    await expect(engine.getEvents('paged', { limit: 2 })).rejects.toThrow(/did not advance/)
  })

  it('rejects a publish whose task disappears between preflight and serialized commit', async () => {
    const { engine, hot } = setup()
    await engine.createTask({ id: 'removed' })
    const get = engine.getTask.bind(engine)
    vi.spyOn(engine, 'getTask').mockImplementationOnce(get).mockResolvedValue(null)
    const append = vi.spyOn(hot, 'commitEventFenced')
    await expect(engine.publishEvent('removed', { type: 'test', level: 'info', data: null })).rejects.toThrow(/not found/)
    expect(append).not.toHaveBeenCalled()
  })

  it.each(['publish', 'transition'] as const)('stops %s when a retry observes a new task generation', async (action) => {
    const { engine, hot } = setup()
    await engine.createTask({ id: 'replaced' })
    vi.spyOn(StorageCoordinator.prototype, 'ensureTaskHotForWrite')
      .mockResolvedValueOnce({ taskId: 'replaced', storageEpoch: 1, creationToken: 'old' })
      .mockResolvedValue({ taskId: 'replaced', storageEpoch: 1, creationToken: 'new' })
    const operation = action === 'publish' ? 'commitEventFenced' : 'commitTaskEventsFenced'
    vi.spyOn(hot, operation).mockRejectedValue(new StorageFenceConflictError('retry'))
    await expect(action === 'publish' ? engine.publishEvent('replaced', { type: 'test', level: 'info', data: null }) : engine.transitionTask('replaced', 'running')).rejects.toThrow(/epoch changed/)
  })

  it.each(['publish', 'transition'] as const)('bounds %s retries while the same generation remains unavailable', async (action) => {
    const { engine, hot } = setup()
    await engine.createTask({ id: 'retry-write' })
    const operation = action === 'publish' ? 'commitEventFenced' : 'commitTaskEventsFenced'
    const commit = vi.spyOn(hot, operation).mockRejectedValue(new StorageFenceConflictError('still busy'))
    await expect(action === 'publish' ? engine.publishEvent('retry-write', { type: 'test', level: 'info', data: null }) : engine.transitionTask('retry-write', 'running')).rejects.toThrow('still busy')
    expect(commit).toHaveBeenCalledTimes(3)
  })

  it('can normalize a hot cursor before reading the durable history page', async () => {
    const { engine, durable } = setup()
    await engine.createTask({ id: 'cursor' })
    const first = await engine.publishEvent('cursor', { type: 'test', level: 'info', data: 1 })
    const second = await engine.publishEvent('cursor', { type: 'test', level: 'info', data: 2 })
    const read = vi.spyOn(durable, 'getEvents')
    expect((await engine.getEvents('cursor', { since: { id: first.id }, limit: 1 })).map(e => e.id)).toEqual([second.id])
    expect(read).toHaveBeenCalledWith('cursor', { since: { index: first.index }, limit: 1 })
  })
  it('normalizes a durable series cursor after the hot event buffer is lost', async () => {
    const { engine, hot, durable } = setup()
    await engine.createTask({ id: 'series-cursor' })
    const first = await engine.publishEvent('series-cursor', { type: 'progress', level: 'info', data: 1, seriesId: 'progress', seriesMode: 'latest' })
    const second = await engine.publishEvent('series-cursor', { type: 'result', level: 'info', data: 2 })
    vi.spyOn(hot, 'getEvents').mockResolvedValue([])
    const read = vi.spyOn(durable, 'getEvents')
    expect((await engine.getEvents('series-cursor', { since: { id: first.id }, limit: 1 })).map(e => e.id)).toEqual([second.id])
    expect(read).toHaveBeenCalledWith('series-cursor', { since: { index: first.index }, limit: 1 })
  })

})
