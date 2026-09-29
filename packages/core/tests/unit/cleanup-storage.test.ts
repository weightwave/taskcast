import { describe, expect, it, vi } from 'vitest'
import { TaskEngine } from '../../src/engine.js'
import { MemoryBroadcastProvider, MemoryLongTermStore, MemoryShortTermStore } from '../../src/memory-adapters.js'
import type { DurableWriteContext, Task, TaskEvent } from '../../src/types.js'

const event = (taskId: string, index: number): TaskEvent => ({ id: `${taskId}-${index}`, taskId, index, timestamp: 1000, type: 'test', level: 'info', data: 'payload' })
async function cold(store: MemoryLongTermStore, id: string, through: number) {
  const metadata = (await store.getTaskStorageMetadata(id))!
  expect(await store.compareAndSetTaskStorageMetadata({ taskId: id, expectedStorageState: metadata.storageState, expectedStorageEpoch: metadata.storageEpoch, expectedReleaseGeneration: null,
    next: { ...metadata, storageState: 'cold', archiveWatermark: through, coldAt: 1000 } })).toBe(true)
}
async function enroll(store: MemoryLongTermStore, id: string, target: 'events' | 'all', count: number) {
  const task: Task = { id, status: 'running', createdAt: 0, updatedAt: 0, cleanupPolicyVersion: 1, cleanupResolvedAt: 0, cleanup: { rules: [{ target, trigger: {} }] } }
  const ctx = { creationToken: `${id}-generation` }
  await store.claimTaskCreation(task, ctx.creationToken, 30_000)
  await store.completeTaskCreation(id, ctx.creationToken)
  for (let n = 0; n < count; n++) await store.saveEvent(event(id, n), ctx)
  const completed: Task = { ...task, status: 'completed', completedAt: 1000, result: { kept: true } }
  await store.saveTask(completed, ctx)
  await cold(store, id, count - 1)
  return { task: completed, ctx }
}

describe('memory cleanup contract and engine write context', () => {
  it('resumes bounded cleanup, preserves task results and rejects expired writes', async () => {
    const store = new MemoryLongTermStore()
    const { task, ctx } = await enroll(store, 'large', 'events', 1501)
    const [claim] = await store.claimCleanupTasks(1, 30_000)
    expect(await store.beginTaskCleanup(claim!, 2, 1500)).toBe(false)
    expect(await store.beginTaskCleanup(claim!, 1, 1500)).toBe(true)
    expect(await store.deleteTaskCleanupBatch(claim!, 1000)).toEqual({ deletedEvents: 1000, complete: false })
    await store.deferCleanupClaim(claim!, 0)
    const [retry] = await store.claimCleanupTasks(1, 30_000)
    expect(await store.deleteTaskCleanupBatch(retry!, 1000)).toEqual({ deletedEvents: 501, complete: true })
    expect((await store.getTask('large'))?.result).toEqual({ kept: true })
    expect(await store.getLastEventIndex('large')).toBe(1500)
    await expect(store.saveTask(task, ctx)).rejects.toThrow()
    await expect(store.saveEvent(event('large', 0), ctx)).rejects.toThrow()
    expect(await store.claimCleanupTasks(1, 30_000)).toEqual([])
  })

  it('rejects old task and series writes after whole-task deletion and same-ID recreation', async () => {
    const store = new MemoryLongTermStore()
    const { task, ctx } = await enroll(store, 'reuse', 'all', 1)
    const [claim] = await store.claimCleanupTasks(1, 30_000)
    await store.beginTaskCleanup(claim!, 1, 0)
    await store.deleteTaskCleanupBatch(claim!, 1000)
    expect(await store.getTask('reuse')).toBeNull()
    await expect(store.saveTask(task)).rejects.toThrow()
    await expect(store.saveTask(task, ctx)).rejects.toThrow()
    await store.claimTaskCreation({ ...task, status: 'running', completedAt: undefined }, 'new', 30_000)
    await store.completeTaskCreation('reuse', 'new')
    await expect(store.replaceLastSeriesEvent('reuse', 's', { ...event('reuse', 0), seriesId: 's', seriesMode: 'latest' }, ctx)).rejects.toThrow()
    await expect(store.accumulateSeries('reuse', 's', { ...event('reuse', 0), seriesId: 's', seriesMode: 'accumulate' }, 'delta', ctx)).rejects.toThrow()
    await store.saveEvent(event('reuse', 1), { creationToken: 'new' })
    expect((await store.getEvents('reuse')).map(e => e.index)).toEqual([1])
  })

  it('captures the generation for generated IDs before delayed durable writes', async () => {
    const store = new MemoryLongTermStore()
    const dropped = vi.fn()
    let resume!: () => void
    const gate = new Promise<void>(resolve => { resume = resolve })
    const original = store.saveEvent.bind(store)
    let captured: DurableWriteContext | undefined
    vi.spyOn(store, 'saveEvent').mockImplementation(async (e, context) => {
      if (e.type === 'delayed') { captured = context; await gate }
      await original(e, context)
    })
    const engine = new TaskEngine({ shortTermStore: new MemoryShortTermStore(), longTermStore: store, broadcast: new MemoryBroadcastProvider(), hooks: { onEventDropped: dropped }, cleanup: { enabled: true, rules: [{ target: 'all', trigger: {} }] } })
    const task = await engine.createTask({})
    await engine.transitionTask(task.id, 'running')
    await engine.publishEvent(task.id, { type: 'delayed', level: 'info', data: 'old generation' })
    expect(captured?.creationToken).toBe((await store.getTaskStorageMetadata(task.id))?.creationToken)
    expect(captured?.creationToken).toBeTruthy()
    await engine.transitionTask(task.id, 'completed', { result: { done: true } })
    // Adapter fixture: normal release/locking is covered by the lifecycle coordinator suite.
    await cold(store, task.id, 2)
    const [claim] = await store.claimCleanupTasks(1, 30_000)
    expect(await store.beginTaskCleanup(claim!, 1, 2)).toBe(true)
    await store.deleteTaskCleanupBatch(claim!, 1000)
    await engine.createTask({ id: task.id })
    resume()
    await vi.waitFor(() => expect(dropped).toHaveBeenCalled())
    expect((await store.getEvents(task.id)).some(e => e.type === 'delayed')).toBe(false)
  })
})
