import { afterEach, expect, it, vi } from 'vitest'
import { MemoryBroadcastProvider, MemoryLongTermStore, MemoryShortTermStore } from '../../src/memory-adapters.js'
import { TaskEngine } from '../../src/engine.js'
import { WorkerManager } from '../../src/worker-manager.js'

afterEach(() => vi.restoreAllMocks())
async function setup() {
  const hot = new MemoryShortTermStore()
  const durable = new MemoryLongTermStore()
  const broadcast = new MemoryBroadcastProvider()
  const engine = new TaskEngine({ shortTermStore: hot, longTermStore: durable, broadcast })
  const manager = new WorkerManager({ engine, shortTermStore: hot, longTermStore: durable, broadcast })
  const worker = await manager.registerWorker({ id: 'worker', matchRule: {}, capacity: 1, connectionMode: 'pull' })
  await engine.createTask({ id: 'task', assignMode: 'pull' })
  return { hot, durable, engine, manager, worker }
}

it('rolls back only the matching durable assignment when the hot assignment write fails', async () => {
  const { hot, durable, manager, worker } = await setup()
  vi.spyOn(hot, 'addAssignment').mockRejectedValue(new Error('hot store unavailable'))
  const remove = vi.spyOn(durable, 'deleteDurableAssignment')
  await expect(manager.claimTask('task', worker.id)).rejects.toThrow('hot store unavailable')
  expect(remove).toHaveBeenCalledWith('task', expect.stringMatching(/^task:worker:\d+$/))
})

it('clears durable ownership even when hot assignment state has already gone', async () => {
  const { durable, manager, worker } = await setup()
  await durable.saveDurableAssignment({ taskId: 'task', workerId: worker.id, cost: 1, assignedAt: 1, status: 'assigned' })
  const remove = vi.spyOn(durable, 'deleteDurableAssignment')
  await manager.releaseTask('task')
  expect(remove).toHaveBeenCalledWith('task')
})

it('releases durable and hot ownership together for an active worker assignment', async () => {
  const { hot, durable, manager, worker } = await setup()
  await manager.claimTask('task', worker.id)
  const remove = vi.spyOn(durable, 'deleteDurableAssignment')
  await manager.releaseTask('task')
  expect(remove).toHaveBeenCalledWith('task', expect.stringMatching(/^task:worker:\d+$/))
  expect(await hot.getTaskAssignment('task')).toBeNull()
  expect((await manager.getWorker(worker.id))?.usedSlots).toBe(0)
})
