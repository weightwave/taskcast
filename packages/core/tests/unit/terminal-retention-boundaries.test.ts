import { afterEach, describe, expect, it, vi } from 'vitest'
import { TaskEngine } from '../../src/engine.js'
import { CleanupCoordinator } from '../../src/cleanup-coordinator.js'
import { cleanupDeadline, resolveCleanupConfig } from '../../src/cleanup-policy.js'
import { MemoryBroadcastProvider, MemoryLongTermStore, MemoryShortTermStore } from '../../src/memory-adapters.js'
import type { TaskArchive, TaskArchiveRestoreData } from '../../src/types.js'

afterEach(() => { vi.restoreAllMocks(); vi.useRealTimers() })
function setup() {
  const hot = new MemoryShortTermStore()
  const durable = new MemoryLongTermStore()
  const engine = new TaskEngine({ shortTermStore: hot, longTermStore: durable, broadcast: new MemoryBroadcastProvider(), cleanup: { enabled: true, rules: [{ target: 'events', trigger: {} }] } })
  return { hot, durable, engine }
}
async function terminal() {
  const state = setup()
  await state.engine.createTask({ id: 't' })
  await state.engine.transitionTask('t', 'running')
  await state.engine.transitionTask('t', 'completed')
  await state.engine.releaseTaskStorageAtCurrentDurableIndex('t', Date.now())
  return state
}
const archive: TaskArchive = { schema: 'taskcast.taskArchive', version: 1, exportedAt: 1, task: { id: 't', status: 'running', createdAt: 0, updatedAt: 0 }, events: [] }

async function importing() {
  const state = setup()
  await state.engine.createTask({ id: 'template', cleanup: { rules: [] } })
  const template = (await state.durable.getTaskStorageMetadata('template'))!
  const metadata = vi.spyOn(state.durable, 'getTaskStorageMetadata').mockResolvedValue(null)
  const restore = vi.fn(async (data: TaskArchiveRestoreData) => {
    metadata.mockResolvedValue({ ...template, taskId: data.task.id, storageState: 'cold', storageEpoch: data.storageEpoch!, creationToken: 'imported' })
  })
  Object.defineProperty(state.durable, 'restoreTaskArchive', { value: restore })
  const cas = vi.spyOn(state.durable, 'compareAndSetTaskStorageMetadata').mockResolvedValue(true)
  return { ...state, metadata, restore, cas }
}

describe('retention safety at concurrent boundaries', () => {
  it('preserves named rules with independently optional type and status selectors', () => {
    const rules = [{ name: 'named', match: {}, target: 'events' as const, trigger: {} }, { match: { status: ['completed' as const] }, target: 'all' as const, trigger: {} }, { match: { taskTypes: ['search.*'] }, target: 'events' as const, trigger: {} }]
    expect(resolveCleanupConfig({ cleanup: { enabled: true, rules } }, {}).rules).toEqual(rules)
    for (const type of [undefined, 'other']) expect(cleanupDeadline({ ...archive.task, status: 'completed', completedAt: 0, type, cleanupPolicyVersion: 1, cleanup: { rules: [rules[2]!] } }, 'events')).toBeNull()
  })

  it.each([0, 2_147_483_648])('rejects unsafe cleanup bounds %s before scanning', async value => {
    const { hot, durable } = setup()
    const scan = vi.spyOn(durable, 'claimCleanupTasks')
    const coordinator = new CleanupCoordinator(hot, durable, async () => {})
    await expect(coordinator.sweep(value, 1, 1000)).rejects.toMatchObject({ code: 'storage_precondition_failed' })
    expect(scan).not.toHaveBeenCalled()
  })

  it.each(['generation', 'lock', 'policy', 'begin'] as const)('defers cleanup on %s change without deleting history', async changed => {
    const { engine, hot, durable } = await terminal()
    const remove = vi.spyOn(durable, 'deleteTaskCleanupBatch')
    if (changed === 'generation') {
      const original = durable.getTaskStorageMetadata.bind(durable)
      vi.spyOn(durable, 'getTaskStorageMetadata').mockImplementation(async id => ({ ...(await original(id))!, creationToken: 'replacement' }))
    } else if (changed === 'lock') vi.spyOn(hot, 'acquireStorageLock').mockResolvedValue(null)
    else if (changed === 'policy') {
      const task = (await durable.getTask('t'))!
      vi.spyOn(durable, 'getTask').mockResolvedValue({ ...task, cleanupPolicyVersion: undefined })
    } else vi.spyOn(durable, 'beginTaskCleanup').mockResolvedValue(false)
    expect(await engine.sweepCleanup()).toMatchObject({ deferred: 1, completed: 0 })
    expect(remove).not.toHaveBeenCalled()
  })

  it('keeps at most one in-flight claim renewal', async () => {
    const { engine, durable } = await terminal()
    vi.useFakeTimers()
    const original = durable.renewCleanupClaim.bind(durable)
    let release!: () => void
    let entered!: () => void
    const blocked = new Promise<void>(resolve => { release = resolve })
    const started = new Promise<void>(resolve => { entered = resolve })
    const renew = vi.spyOn(durable, 'renewCleanupClaim').mockImplementationOnce(async (...args) => { entered(); await blocked; return original(...args) })
    const sweeping = engine.sweepCleanup(1, 1000, 3000)
    await started
    // The periodic renewal itself remains pending across multiple timer ticks.
    renew.mockImplementation(async (...args) => { await blocked; return original(...args) })
    await vi.advanceTimersByTimeAsync(2000)
    expect(renew).toHaveBeenCalledTimes(2)
    release()
    expect(await sweeping).toMatchObject({ completed: 1 })
  })

  it.each(['gone', 'expired'] as const)('rejects an archive if its task becomes %s while events are read', async state => {
    const { engine, hot } = setup()
    const task = await engine.createTask({ id: 't' })
    const current = vi.spyOn(engine, 'getTask').mockResolvedValueOnce(task).mockResolvedValue(state === 'gone' ? null : { ...task, historyExpiredAt: 1 })
    vi.spyOn(hot, 'getEvents').mockResolvedValue([])
    await expect(engine.exportTaskArchive('t')).rejects.toThrow(state === 'gone' ? /not found/ : /expired/)
    expect(current).toHaveBeenCalledTimes(2)
  })

  it.each(['getEvents', 'getSeriesLatest'] as const)('suppresses %s when a task disappears during the read', async method => {
    const { engine } = setup()
    const task = await engine.createTask({ id: 't' })
    vi.spyOn(engine, 'getTask').mockResolvedValueOnce(task).mockResolvedValue(null)
    expect(await (method === 'getEvents' ? engine.getEvents('t') : engine.getSeriesLatest('t', 's'))).toEqual(method === 'getEvents' ? [] : null)
  })

  it('returns no series for already expired or unknown tasks', async () => {
    const { engine } = setup()
    expect(await engine.getSeriesLatest('missing', 's')).toBeNull()
    vi.spyOn(engine, 'getTask').mockResolvedValue({ ...archive.task, historyExpiredAt: 1 })
    expect(await engine.getSeriesLatest('t', 's')).toBeNull()
  })

  it('imports a new task without an existing epoch or generation', async () => {
    const { engine, hot, restore, cas } = await importing()
    expect(await engine.importTaskArchive(archive)).toMatchObject({ overwritten: false })
    expect(restore).toHaveBeenCalledWith(expect.objectContaining({ expectedCreationToken: null, storageEpoch: 1 }), undefined)
    expect(cas).toHaveBeenCalledWith(expect.objectContaining({ next: expect.objectContaining({ storageState: 'hot', storageEpoch: 2 }) }))
    expect(await hot.getTask('t')).toMatchObject({ status: 'running' })
  })

  it.each(['busy', 'created', 'missing-metadata', 'hot-metadata', 'wrong-epoch', 'cas'] as const)('rejects fenced import when %s races with restore', async failure => {
    const { engine, hot, durable, metadata, restore, cas } = await importing()
    if (failure === 'busy') vi.spyOn(hot, 'acquireStorageLock').mockResolvedValue(null)
    else if (failure === 'created') vi.spyOn(engine, 'getTask').mockResolvedValueOnce(null).mockResolvedValue(archive.task)
    else if (failure === 'cas') cas.mockResolvedValue(false)
    else {
      const original = restore.getMockImplementation()!
      restore.mockImplementation(async data => {
        await original(data)
        const value = await durable.getTaskStorageMetadata('t')
        metadata.mockResolvedValue(failure === 'missing-metadata' ? null : { ...value!, storageState: failure === 'hot-metadata' ? 'hot' : 'cold', storageEpoch: failure === 'wrong-epoch' ? 99 : 1 })
      })
    }
    await expect(engine.importTaskArchive(archive)).rejects.toThrow()
    if (failure !== 'cas') expect(cas).not.toHaveBeenCalled()
    if (failure !== 'busy') expect(await hot.acquireStorageLock('t', 'after', 'probe', 1000)).not.toBeNull()
  })
})
