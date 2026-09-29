import { afterEach, describe, expect, it, vi } from 'vitest'
import { TaskEngine } from '../../src/engine.js'
import { MemoryBroadcastProvider, MemoryLongTermStore, MemoryShortTermStore } from '../../src/memory-adapters.js'
import { StorageCoordinator } from '../../src/storage-coordinator.js'
import { TtlCoordinator } from '../../src/ttl-coordinator.js'
import type { Task } from '../../src/types.js'

afterEach(() => vi.restoreAllMocks())
async function setup() {
  const hot = new MemoryShortTermStore()
  const durable = new MemoryLongTermStore()
  const broadcast = new MemoryBroadcastProvider()
  const task: Task = { id: 'ttl', status: 'running', ttl: 1, createdAt: 1, updatedAt: 1 }
  await hot.saveTask(task)
  await durable.saveTask(task)
  const metadata = (await durable.getTaskStorageMetadata(task.id))!
  await durable.compareAndSetTaskStorageMetadata({ taskId: task.id, expectedStorageState: 'hot', expectedStorageEpoch: 1, expectedReleaseGeneration: null, next: { ...metadata, executionDeadlineAt: Date.now() - 1 } })
  const storage = new StorageCoordinator({ shortTermStore: hot, longTermStore: durable })
  const options = { shortTermStore: hot, longTermStore: durable, broadcast, storageCoordinator: storage }
  return { hot, durable, broadcast, storage, task, options, coordinator: new TtlCoordinator(options) }
}

describe('TTL dependency and recovery boundaries', () => {
  it('rejects unsafe durations and incomplete adapter contracts before starting', async () => {
    const { hot, durable, options } = await setup()
    expect(() => new TtlCoordinator({ ...options, storageLockTtlMs: 0 })).toThrow(/duration/)
    Object.defineProperty(hot, 'projectTerminalFenced', { value: undefined, configurable: true })
    expect(() => new TtlCoordinator(options)).toThrow(/Short-term/)
    Object.defineProperty(hot, 'projectTerminalFenced', { value: MemoryShortTermStore.prototype.projectTerminalFenced })
    Object.defineProperty(durable, 'claimOverdueTasks', { value: undefined })
    expect(() => new TtlCoordinator(options)).toThrow(/Long-term/)
  })

  it('delivers timeout hooks and protects a sweep from a failing transition listener', async () => {
    const { hot, durable, broadcast } = await setup()
    const timedOut = vi.fn()
    const transitioned = vi.fn()
    const engine = new TaskEngine({ shortTermStore: hot, longTermStore: durable, broadcast, storageLockTtlMs: 1000, hooks: { onTaskTimeout: timedOut, onTaskTransitioned: transitioned } })
    engine.addTransitionListener(() => { throw new Error('listener failure') })
    expect(await engine.sweepDurableTtl(1)).toMatchObject({ timedOut: 1, failed: 0 })
    expect(timedOut).toHaveBeenCalledWith(expect.objectContaining({ status: 'timeout' }))
    expect(transitioned).toHaveBeenCalledWith(expect.objectContaining({ status: 'timeout' }), 'running', 'timeout')
  })

  it.each(['missing-task', 'changed-task', 'uncaught-history'] as const)('refuses to time out %s and reopens the live fence', async (fault) => {
    const { hot, durable, task, coordinator } = await setup()
    if (fault === 'missing-task') vi.spyOn(hot, 'getTaskMutationSnapshot').mockResolvedValueOnce(null)
    if (fault === 'changed-task') vi.spyOn(durable, 'getTask').mockResolvedValueOnce({ ...task, updatedAt: 2 })
    if (fault === 'uncaught-history') vi.spyOn(durable, 'getLastEventIndex').mockResolvedValueOnce(10)
    expect(await coordinator.sweepOverdue(1)).toMatchObject({ timedOut: 0, failed: 1 })
    expect((await hot.getWriteFence(task.id))?.acceptingWrites).toBe(true)
    expect((await durable.getTask(task.id))?.status).toBe('running')
  })

  it('retains durable timeout for replay if metadata disappears after hot projection', async () => {
    const { hot, durable, coordinator } = await setup()
    const project = hot.projectTerminalFenced.bind(hot)
    const readMetadata = durable.getTaskStorageMetadata.bind(durable)
    let metadataLost = false
    vi.spyOn(durable, 'getTaskStorageMetadata').mockImplementation(id => metadataLost ? Promise.resolve(null) : readMetadata(id))
    vi.spyOn(hot, 'projectTerminalFenced').mockImplementation(async (...args) => {
      const result = await project(...args)
      metadataLost = true
      return result
    })
    expect(await coordinator.sweepOverdue(1)).toMatchObject({ timedOut: 0, failed: 1 })
    expect((await durable.getTask('ttl'))?.status).toBe('timeout')
    vi.spyOn(Date, 'now').mockReturnValue(Date.now() + 31_000)
    expect(await durable.claimTerminalProjections(1, 'retry', 1000)).toHaveLength(1)
  })

  it.each([true, false])('accepts a lost metadata CAS only when the expected epoch was installed: %s', async (installed) => {
    const { durable, coordinator } = await setup()
    const cas = durable.compareAndSetTaskStorageMetadata.bind(durable)
    vi.spyOn(durable, 'compareAndSetTaskStorageMetadata').mockImplementation(async input => {
      if (installed) await cas(input)
      return false
    })
    expect(await coordinator.sweepOverdue(1)).toMatchObject(installed ? { timedOut: 1, failed: 0 } : { timedOut: 0, failed: 1 })
    expect((await durable.getTask('ttl'))?.status).toBe('timeout')
  })

  it.each(['no-claim', 'no-metadata', 'releasing', 'busy', 'epoch-overflow'] as const)('preserves an outbox projection when replay sees %s', async (fault) => {
    const { hot, durable, coordinator } = await setup()
    const [claim] = await durable.claimOverdueTasks(1, 1000)
    const task: Task = { id: 'ttl', status: 'timeout', ttl: 1, createdAt: 1, updatedAt: 2, completedAt: 2 }
    const projection = (await durable.terminalizeTtlClaim(claim!, task, { id: 'timeout', taskId: 'ttl', index: 0, timestamp: 2, type: 'taskcast:status', level: 'info', data: { status: 'timeout' } }, null))!
    vi.spyOn(Date, 'now').mockReturnValue(Date.now() + 2000)
    if (fault === 'no-claim') vi.spyOn(durable, 'claimTerminalProjections').mockResolvedValue([{ ...projection, claimToken: null, claimUntil: null }])
    if (fault === 'no-metadata') vi.spyOn(durable, 'getTaskStorageMetadata').mockResolvedValue(null)
    if (fault === 'releasing') {
      const metadata = (await durable.getTaskStorageMetadata('ttl'))!
      vi.spyOn(durable, 'getTaskStorageMetadata').mockResolvedValue({ ...metadata, storageState: 'releasing' })
    }
    if (fault === 'busy') vi.spyOn(hot, 'acquireStorageLock').mockResolvedValue(null)
    if (fault === 'epoch-overflow') {
      const metadata = (await durable.getTaskStorageMetadata('ttl'))!
      vi.spyOn(durable, 'getTaskStorageMetadata').mockResolvedValue({ ...metadata, storageEpoch: Number.MAX_SAFE_INTEGER })
      const closed = await hot.getWriteFence('ttl')
      vi.spyOn(hot, 'closeWriteFence').mockResolvedValue({ ...closed!, acceptingWrites: false, highWatermark: -1 })
    }
    const complete = vi.spyOn(durable, 'completeTerminalProjection')
    expect(await coordinator.sweepTerminalProjections(1)).toMatchObject({ projected: 0, failed: 1 })
    expect(complete).not.toHaveBeenCalled()
  })

  it('rehydrates a cold terminal outbox task before retrying its projection', async () => {
    const { hot, durable, coordinator, storage } = await setup()
    const [claim] = await durable.claimOverdueTasks(1, 1000)
    await durable.terminalizeTtlClaim(claim!, { id: 'ttl', status: 'timeout', createdAt: 1, updatedAt: 2, completedAt: 2 }, { id: 'timeout', taskId: 'ttl', index: 0, timestamp: 2, type: 'taskcast:status', level: 'info', data: { status: 'timeout' } }, null)
    vi.spyOn(Date, 'now').mockReturnValue(Date.now() + 2000)
    // Failure injection: a cold durable terminal snapshot survived while its outbox needs replay.
    const metadata = (await durable.getTaskStorageMetadata('ttl'))!
    vi.spyOn(durable, 'getTaskStorageMetadata').mockResolvedValueOnce({ ...metadata, storageState: 'cold' })
    const restore = vi.spyOn(storage, 'ensureTaskHotForWrite')
    expect(await coordinator.sweepTerminalProjections(1)).toMatchObject({ projected: 1, failed: 0 })
    expect(restore).toHaveBeenCalledWith('ttl')
    expect((await hot.getTask('ttl'))?.status).toBe('timeout')
  })
})
