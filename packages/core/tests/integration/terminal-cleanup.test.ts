import { describe, expect, it, vi } from 'vitest'
import { HistoryExpiredError, TaskEngine } from '../../src/engine.js'
import { MemoryBroadcastProvider, MemoryLongTermStore, MemoryShortTermStore } from '../../src/memory-adapters.js'
import type { CleanupRule } from '../../src/types.js'

function setup(enabled = true, target: 'events' | 'all' = 'events') {
  const hot = new MemoryShortTermStore()
  const durable = new MemoryLongTermStore()
  const rules: CleanupRule[] = [{ target, trigger: {} }]
  const engine = new TaskEngine({ shortTermStore: hot, longTermStore: durable, broadcast: new MemoryBroadcastProvider(), cleanup: { enabled, rules } })
  return { hot, durable, engine }
}
async function completed(engine: TaskEngine, id: string, events = 1) {
  await engine.createTask({ id })
  await engine.transitionTask(id, 'running')
  for (let n = 0; n < events; n++) await engine.publishEvent(id, { type: 'test', level: 'info', data: n })
  await engine.transitionTask(id, 'completed', { result: { kept: true } })
}

describe('terminal cleanup lifecycle', () => {
  it('does not scan while disabled and rejects unsupported enabled adapters', async () => {
    const { engine, durable } = setup(false)
    const scan = vi.spyOn(durable, 'claimCleanupTasks')
    expect(await engine.sweepCleanup()).toEqual({ claimed: 0, completed: 0, deferred: 0, failed: 0, deletedEvents: 0 })
    expect(scan).not.toHaveBeenCalled()
    expect(() => new TaskEngine({ shortTermStore: new MemoryShortTermStore(), broadcast: new MemoryBroadcastProvider(), cleanup: { enabled: true, rules: [] } })).toThrow(/cleanup/i)
    vi.spyOn(durable, 'claimCleanupTasks').mockRestore()
    // An adapter declaring support must also implement the full cleanup contract.
    Object.defineProperty(durable, 'beginTaskCleanup', { value: undefined })
    expect(() => new TaskEngine({ shortTermStore: new MemoryShortTermStore(), longTermStore: durable, broadcast: new MemoryBroadcastProvider(), cleanup: { enabled: true, rules: [] } })).toThrow(/capabilities/)
  })

  it('keeps durable expiry authoritative over a stale terminal cache after cleanup is disabled', async () => {
    const { engine, hot, durable } = setup()
    await completed(engine, 'stale')
    const cached = (await hot.getTask('stale'))!
    expect(await engine.sweepCleanup()).toMatchObject({ completed: 1 })
    await hot.saveTask(cached)
    const disabled = new TaskEngine({ shortTermStore: hot, longTermStore: durable, broadcast: new MemoryBroadcastProvider() })
    expect((await disabled.getTask('stale'))?.historyExpiredAt).toBeGreaterThan(0)
    expect(await disabled.getEvents('stale')).toEqual([])
    await expect(disabled.exportTaskArchive('stale')).rejects.toThrow(HistoryExpiredError)
  })

  it('releases hot storage before bounded durable cleanup and resumes cold tasks', async () => {
    const { engine, hot, durable } = setup()
    await completed(engine, 'large', 1499) // Includes two status events: 1501 total.
    const first = await engine.sweepCleanup(100, 1000)
    expect(first).toMatchObject({ claimed: 1, completed: 0, deferred: 1, failed: 0, deletedEvents: 1000 })
    expect(await hot.getTask('large')).toBeNull()
    expect((await durable.getTask('large'))?.historyExpiredAt).toBeGreaterThan(0)
    const afterRelease = new TaskEngine({ shortTermStore: hot, longTermStore: durable, broadcast: new MemoryBroadcastProvider(), cleanup: { enabled: true, rules: [] } })
    vi.spyOn(Date, 'now').mockReturnValue(Date.now() + 6000)
    try {
      expect(await afterRelease.sweepCleanup()).toMatchObject({ completed: 1, deletedEvents: 501, failed: 0 })
      expect((await durable.getTask('large'))?.result).toEqual({ kept: true })
      expect(await durable.getLastEventIndex('large')).toBe(1500)
    } finally { vi.restoreAllMocks() }
  })

  it('honors the task bound and excludes every nonterminal status', async () => {
    const { engine, durable } = setup(true, 'all')
    await completed(engine, 'a')
    await completed(engine, 'b')
    for (const status of ['pending', 'assigned', 'running', 'paused', 'blocked'] as const) {
      const task = await engine.createTask({ id: status })
      const context = { creationToken: (await durable.getTaskStorageMetadata(task.id))!.creationToken! }
      await durable.saveTask({ ...task, status }, context)
    }
    expect(await engine.sweepCleanup(1)).toMatchObject({ claimed: 1, completed: 1 })
    expect(await engine.sweepCleanup(1)).toMatchObject({ claimed: 1, completed: 1 })
    expect(await engine.sweepCleanup()).toMatchObject({ claimed: 0 })
    for (const status of ['pending', 'assigned', 'running', 'paused', 'blocked']) expect(await durable.getTask(status)).not.toBeNull()
  })

  it('requires v3 for cleanup while preserving v2 for ordinary hot release', async () => {
    const { engine, hot, durable } = setup()
    await completed(engine, 'mixed')
    await engine.registerStorageWriter({ instanceId: 'old', storageProtocolVersion: 2, build: 'old', expiresAt: 0 }, 30_000)
    expect(await engine.sweepCleanup()).toMatchObject({ completed: 0, deferred: 1 })
    expect(await hot.getTask('mixed')).not.toBeNull()
    expect((await durable.getTask('mixed'))?.historyExpiredAt).toBeUndefined()
    await expect(engine.releaseTaskStorageAtCurrentDurableIndex('mixed', Date.now())).resolves.toMatchObject({ storageState: 'cold' })
  })

  it('does not let unsettled ownership starve later tasks', async () => {
    const { engine, durable } = setup()
    await completed(engine, 'a-busy')
    await completed(engine, 'b-ready')
    await durable.saveDurableAssignment({ taskId: 'a-busy', workerId: 'w', assignedAt: Date.now(), cost: 1, status: 'running' })
    expect(await engine.sweepCleanup(1)).toMatchObject({ completed: 0, deferred: 1 })
    // A real lifecycle tick can arrive after the busy task's retry deadline.
    vi.spyOn(Date, 'now').mockReturnValue(Date.now() + 6000)
    try {
      expect(await engine.sweepCleanup(1)).toMatchObject({ completed: 1 })
    } finally { vi.restoreAllMocks() }
    expect((await durable.getTask('a-busy'))?.historyExpiredAt).toBeUndefined()
  })

  it('recovers after Redis release succeeds but PostgreSQL cleanup fails', async () => {
    const { engine, hot, durable } = setup()
    await completed(engine, 'retry')
    vi.spyOn(durable, 'beginTaskCleanup').mockRejectedValueOnce(new Error('database unavailable'))
    expect(await engine.sweepCleanup()).toMatchObject({ failed: 1, completed: 0 })
    expect(await hot.getTask('retry')).toBeNull()
    const release = vi.spyOn(engine, 'releaseTaskStorageAtCurrentDurableIndex')
    vi.spyOn(Date, 'now').mockReturnValue(Date.now() + 6000)
    try {
      expect(await engine.sweepCleanup()).toMatchObject({ completed: 1 })
      expect(release).not.toHaveBeenCalled()
    } finally { vi.restoreAllMocks() }
  })

  it('allows only one cleaner to advance a task', async () => {
    const { engine, hot, durable } = setup(true, 'all')
    await completed(engine, 'contended')
    const other = new TaskEngine({ shortTermStore: hot, longTermStore: durable, broadcast: new MemoryBroadcastProvider(), cleanup: { enabled: true, rules: [] } })
    const results = await Promise.all([engine.sweepCleanup(), other.sweepCleanup()])
    expect(results.reduce((count, result) => count + result.completed, 0)).toBe(1)
    expect(results.reduce((count, result) => count + result.claimed, 0)).toBe(1)
    expect(await durable.getTask('contended')).toBeNull()
  })

  it('renews the claim during a slow archive and stops after renewal is lost', async () => {
    const { engine, durable } = setup()
    await completed(engine, 'slow')
    const original = engine.releaseTaskStorageAtCurrentDurableIndex.bind(engine)
    let entered!: () => void
    const started = new Promise<void>(resolve => { entered = resolve })
    let finish!: () => void
    const gate = new Promise<void>(resolve => { finish = resolve })
    vi.spyOn(engine, 'releaseTaskStorageAtCurrentDurableIndex').mockImplementation(async (...args) => { entered(); await gate; return original(...args) })
    const renew = vi.spyOn(durable, 'renewCleanupClaim')
    const remove = vi.spyOn(durable, 'deleteTaskCleanupBatch')
    const sweep = engine.sweepCleanup(1, 1000, 90)
    await started
    await vi.waitFor(() => expect(renew.mock.calls.length).toBeGreaterThan(1), { interval: 10 })
    renew.mockResolvedValue(false)
    await new Promise(resolve => setTimeout(resolve, 40))
    finish()
    expect(await sweep).toMatchObject({ completed: 0, deferred: 1 })
    expect(remove).not.toHaveBeenCalled()
  })

  it('never starts physical deletion after losing the cleanup claim or storage lease', async () => {
    for (const lost of ['claim', 'lease']) {
      const { engine, hot, durable } = setup()
      await completed(engine, lost)
      const remove = vi.spyOn(durable, 'deleteTaskCleanupBatch')
      if (lost === 'claim') vi.spyOn(durable, 'renewCleanupClaim').mockResolvedValue(false)
      else vi.spyOn(hot, 'renewStorageLock').mockResolvedValue(false)
      expect(await engine.sweepCleanup()).toMatchObject({ completed: 0 })
      expect(remove).not.toHaveBeenCalled()
    }
  })
})


it('supports worker claim and decline with an enrolled task generation', async () => {
  const { WorkerManager } = await import('../../src/worker-manager.js')
  const { engine, hot, durable } = setup()
  const manager = new WorkerManager({ engine, shortTermStore: hot, longTermStore: durable, broadcast: new MemoryBroadcastProvider() })
  const worker = await manager.registerWorker({ matchRule: {}, capacity: 1, connectionMode: 'pull' })
  await engine.createTask({ id: 'worker-task', assignMode: 'pull' })
  expect(await manager.claimTask('worker-task', worker.id)).toMatchObject({ success: true })
  expect((await durable.getTask('worker-task'))?.status).toBe('assigned')
  await manager.declineTask('worker-task', worker.id, { blacklist: true })
  expect((await durable.getTask('worker-task'))?.status).toBe('pending')
  expect((await durable.getTask('worker-task'))?.assignedWorker).toBeUndefined()
})
