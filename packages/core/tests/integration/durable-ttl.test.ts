import { describe, expect, it, vi } from 'vitest'
import {
  MemoryBroadcastProvider,
  MemoryLongTermStore,
  MemoryShortTermStore,
  StorageCoordinator,
  TaskEngine,
  WorkerManager,
  type StorageLease,
  type Task,
  type TaskEvent,
  type TerminalProjection,
  type TerminalProjectionResult,
  type TtlClaim,
  type WorkerAssignment,
} from '../../src/index.js'

const nonTerminalStatuses = [
  'pending',
  'assigned',
  'running',
  'paused',
  'blocked',
] as const

const makeTask = (
  id: string,
  status: Task['status'] = 'running',
  overrides: Partial<Task> = {},
): Task => ({
  id,
  status,
  createdAt: Date.now(),
  updatedAt: Date.now(),
  ttl: 60,
  ...overrides,
})

async function seedTask(
  hot: MemoryShortTermStore,
  durable: MemoryLongTermStore,
  task: Task,
): Promise<void> {
  await hot.saveTask(task)
  await durable.saveTask(task)
}

async function markOverdue(
  durable: MemoryLongTermStore,
  taskId: string,
): Promise<void> {
  const metadata = (await durable.getTaskStorageMetadata(taskId))!
  await durable.compareAndSetTaskStorageMetadata({
    taskId,
    expectedStorageState: metadata.storageState,
    expectedStorageEpoch: metadata.storageEpoch,
    expectedReleaseGeneration: metadata.activeReleaseGeneration,
    next: {
      ...metadata,
      executionDeadlineAt: Date.now() - 1,
    },
  })
}

function makeEngine(
  hot: MemoryShortTermStore,
  durable: MemoryLongTermStore,
  broadcast = new MemoryBroadcastProvider(),
  hooks?: ConstructorParameters<typeof TaskEngine>[0]['hooks'],
): TaskEngine {
  return new TaskEngine({
    shortTermStore: hot,
    longTermStore: durable,
    broadcast,
    ...(hooks && { hooks }),
  })
}

describe('durable execution TTL', () => {
  it('does not expire hot storage when PostgreSQL owns the execution deadline', async () => {
    class TrackingHotStore extends MemoryShortTermStore {
      setTtlCalls = 0
      clearTtlCalls = 0

      override async setTTL(taskId: string, ttlSeconds: number): Promise<void> {
        this.setTtlCalls += 1
        await super.setTTL(taskId, ttlSeconds)
      }

      override async clearTTL(taskId: string): Promise<void> {
        this.clearTtlCalls += 1
        await super.clearTTL(taskId)
      }
    }
    const hot = new TrackingHotStore()
    const durable = new MemoryLongTermStore()
    const engine = makeEngine(hot, durable)

    await engine.createTask({ id: 'durable-deadline', ttl: 60 })
    await engine.transitionTask('durable-deadline', 'running')
    await engine.transitionTask('durable-deadline', 'paused')
    await engine.transitionTask('durable-deadline', 'running', { ttl: 30 })

    expect(hot.setTtlCalls).toBe(0)
    expect(hot.clearTtlCalls).toBeGreaterThan(0)
    await expect(
      durable.getTaskStorageMetadata('durable-deadline'),
    ).resolves.toMatchObject({
      executionDeadlineAt: expect.any(Number),
    })
  })

  it('times out all five non-terminal states without relying on Redis expiry', async () => {
    for (const status of nonTerminalStatuses) {
      const hot = new MemoryShortTermStore()
      const durable = new MemoryLongTermStore()
      const broadcast = new MemoryBroadcastProvider()
      const task = makeTask(`task-${status}`, status)
      await seedTask(hot, durable, task)
      await markOverdue(durable, task.id)
      const observed: TaskEvent[] = []
      broadcast.subscribe(task.id, (event) => observed.push(event))
      const onTaskTimeout = vi.fn()
      const engine = makeEngine(hot, durable, broadcast, { onTaskTimeout })

      await expect(engine.sweepDurableTtl(10)).resolves.toMatchObject({
        claimed: 1,
        timedOut: 1,
        failed: 0,
      })
      await expect(hot.getTask(task.id)).resolves.toMatchObject({
        status: 'timeout',
      })
      await expect(durable.getTask(task.id)).resolves.toMatchObject({
        status: 'timeout',
      })
      await expect(durable.getEvents(task.id)).resolves.toMatchObject([
        {
          index: 0,
          type: 'taskcast:status',
          data: { status: 'timeout' },
        },
      ])
      expect(observed).toHaveLength(1)
      expect(onTaskTimeout).toHaveBeenCalledOnce()
    }
  })

  it('allows only one of two replicas to claim and terminalize an overdue task', async () => {
    const hot = new MemoryShortTermStore()
    const durable = new MemoryLongTermStore()
    await seedTask(hot, durable, makeTask('two-replicas'))
    await markOverdue(durable, 'two-replicas')
    const first = makeEngine(hot, durable)
    const second = makeEngine(hot, durable)

    const results = await Promise.all([
      first.sweepDurableTtl(10),
      second.sweepDurableTtl(10),
    ])

    expect(results.reduce((sum, result) => sum + result.timedOut, 0)).toBe(1)
    await expect(durable.getEvents('two-replicas')).resolves.toHaveLength(1)
  })

  it('recovers an overdue task after restart and rehydrates a cold task', async () => {
    const hot = new MemoryShortTermStore()
    const durable = new MemoryLongTermStore()
    const task = makeTask('cold-overdue')
    await seedTask(hot, durable, task)
    const storage = new StorageCoordinator({
      shortTermStore: hot,
      longTermStore: durable,
    })
    await storage.releaseTaskStorage(task.id, {
      expectedLastEventIndex: -1,
      inactiveSince: Date.now(),
    })
    await markOverdue(durable, task.id)

    const restarted = makeEngine(hot, durable)
    await expect(restarted.sweepDurableTtl(10)).resolves.toMatchObject({
      timedOut: 1,
      failed: 0,
    })
    await expect(hot.getTask(task.id)).resolves.toMatchObject({
      status: 'timeout',
    })
    await expect(durable.getTaskStorageMetadata(task.id)).resolves.toMatchObject({
      storageState: 'hot',
      storageEpoch: 3,
    })
  })

  it('keeps durable state non-terminal when PostgreSQL terminalization fails', async () => {
    class FailingTerminalStore extends MemoryLongTermStore {
      override async terminalizeTtlClaim(
        _claim: TtlClaim,
        _task: Task,
        _event: TaskEvent,
        _assignment: WorkerAssignment | null,
      ): Promise<TerminalProjection | null> {
        throw new Error('postgres unavailable')
      }
    }
    const hot = new MemoryShortTermStore()
    const durable = new FailingTerminalStore()
    await seedTask(hot, durable, makeTask('postgres-failure'))
    await markOverdue(durable, 'postgres-failure')
    const engine = makeEngine(hot, durable)

    await expect(engine.sweepDurableTtl(10)).resolves.toMatchObject({
      claimed: 1,
      timedOut: 0,
      failed: 1,
    })
    await expect(durable.getTask('postgres-failure')).resolves.toMatchObject({
      status: 'running',
    })
    await expect(hot.getWriteFence('postgres-failure')).resolves.toMatchObject({
      acceptingWrites: true,
      storageEpoch: 2,
    })
  })

  it('loses safely to non-terminal and terminal task-version races', async () => {
    class RacingStore extends MemoryLongTermStore {
      race: 'non-terminal' | 'terminal' = 'non-terminal'

      override async terminalizeTtlClaim(
        claim: TtlClaim,
        task: Task,
        event: TaskEvent,
        assignment: WorkerAssignment | null,
      ): Promise<TerminalProjection | null> {
        const current = (await this.getTask(claim.taskId))!
        await this.saveTask({
          ...current,
          status: this.race === 'terminal' ? 'completed' : 'blocked',
          updatedAt: current.updatedAt + 1,
          ...(this.race === 'terminal' && { completedAt: current.updatedAt + 1 }),
        })
        return super.terminalizeTtlClaim(claim, task, event, assignment)
      }
    }

    for (const race of ['non-terminal', 'terminal'] as const) {
      const hot = new MemoryShortTermStore()
      const durable = new RacingStore()
      durable.race = race
      await seedTask(hot, durable, makeTask(`race-${race}`))
      await markOverdue(durable, `race-${race}`)
      const engine = makeEngine(hot, durable)

      await expect(engine.sweepDurableTtl(10)).resolves.toMatchObject({
        timedOut: 0,
        raceLost: 1,
        failed: 0,
      })
      await expect(durable.getTask(`race-${race}`)).resolves.toMatchObject({
        status: race === 'terminal' ? 'completed' : 'blocked',
      })
    }
  })

  it('repairs a crash after the durable commit and settles worker capacity once', async () => {
    class FailOnceProjectionStore extends MemoryShortTermStore {
      fail = true

      override async projectTerminalFenced(
        projection: TerminalProjection,
        lease: StorageLease,
        expectedEpoch: number,
        nextEpoch: number,
      ): Promise<TerminalProjectionResult> {
        if (this.fail) {
          this.fail = false
          throw new Error('process crashed after postgres commit')
        }
        return super.projectTerminalFenced(
          projection,
          lease,
          expectedEpoch,
          nextEpoch,
        )
      }
    }
    const hot = new FailOnceProjectionStore()
    const durable = new MemoryLongTermStore()
    const engine = makeEngine(hot, durable)
    const manager = new WorkerManager({
      engine,
      shortTermStore: hot,
      longTermStore: durable,
      broadcast: new MemoryBroadcastProvider(),
    })
    await manager.registerWorker({
      id: 'worker-1',
      matchRule: {},
      capacity: 5,
      connectionMode: 'pull',
    })
    await engine.createTask({
      id: 'assigned-timeout',
      ttl: 60,
      cost: 3,
    })
    await expect(manager.claimTask('assigned-timeout', 'worker-1')).resolves.toEqual({
      success: true,
    })
    await new Promise((resolve) => setTimeout(resolve, 0))
    await markOverdue(durable, 'assigned-timeout')

    await expect(engine.sweepDurableTtl(10, 20)).resolves.toMatchObject({
      timedOut: 0,
      failed: 1,
    })
    await expect(durable.getTask('assigned-timeout')).resolves.toMatchObject({
      status: 'timeout',
    })
    await expect(hot.getTask('assigned-timeout')).resolves.toMatchObject({
      status: 'assigned',
    })

    await new Promise((resolve) => setTimeout(resolve, 25))
    await expect(engine.sweepTerminalProjections(10, 1_000)).resolves.toMatchObject({
      projected: 1,
      failed: 0,
    })
    await expect(hot.getTask('assigned-timeout')).resolves.toMatchObject({
      status: 'timeout',
    })
    await expect(hot.getTaskAssignment('assigned-timeout')).resolves.toBeNull()
    await expect(manager.getWorker('worker-1')).resolves.toMatchObject({
      usedSlots: 0,
      status: 'idle',
    })

    await expect(engine.sweepTerminalProjections(10, 1_000)).resolves.toMatchObject({
      claimed: 0,
      projected: 0,
    })
    await expect(manager.getWorker('worker-1')).resolves.toMatchObject({
      usedSlots: 0,
    })
  })
})
