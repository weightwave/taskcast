import { afterAll, beforeAll, beforeEach, describe, expect, it } from 'vitest'
import { Redis } from 'ioredis'
import { GenericContainer, type StartedTestContainer } from 'testcontainers'
import type {
  DurableSeriesState,
  RehydrateSnapshot,
  ShortTermStore,
  StorageLease,
  Task,
  TaskEvent,
  Worker,
} from '@taskcast/core'
import { RedisShortTermStore } from '../src/short-term.js'

const prefix = `taskcast-lifecycle-${process.pid}`

let container: StartedTestContainer | undefined
let redis: Redis
let store: RedisShortTermStore
let lifecycle: ShortTermStore

beforeAll(async () => {
  let connection = process.env['TASKCAST_TEST_REDIS_URL']
  if (!connection) {
    container = await new GenericContainer('redis:7-alpine').withExposedPorts(6379).start()
    connection = `redis://localhost:${container.getMappedPort(6379)}`
  }
  redis = new Redis(connection)
  store = new RedisShortTermStore(redis, {
    prefix,
    legacySeriesWrites: false,
  })
  lifecycle = store
}, 60000)

afterAll(async () => {
  const keys = await redis.keys(`${prefix}:*`)
  if (keys.length > 0) await redis.del(...keys)
  redis.disconnect()
  await container?.stop()
})

beforeEach(async () => {
  const keys = await redis.keys(`${prefix}:*`)
  if (keys.length > 0) await redis.del(...keys)
})

const makeTask = (overrides: Partial<Task> = {}): Task => ({
  id: 'task-1',
  status: 'running',
  createdAt: 1000,
  updatedAt: 1000,
  ...overrides,
})

const makeEvent = (
  id: string,
  overrides: Partial<Omit<TaskEvent, 'index'>> = {},
): Omit<TaskEvent, 'index'> => ({
  id,
  taskId: 'task-1',
  timestamp: 1000,
  type: 'llm.delta',
  level: 'info',
  data: { text: id },
  ...overrides,
})

async function openTask(): Promise<void> {
  await store.saveTask(makeTask())
}

async function taskRevision(): Promise<string> {
  const snapshot = await lifecycle.getTaskMutationSnapshot!('task-1')
  if (!snapshot) throw new Error('task mutation snapshot is missing')
  return snapshot.revision
}

async function acquire(
  lockToken = 'lock-1',
  generation = 'generation-1',
  ttlMs = 5000,
): Promise<StorageLease> {
  const lease = await lifecycle.acquireStorageLock!(
    'task-1',
    lockToken,
    generation,
    ttlMs,
  )
  expect(lease).not.toBeNull()
  return lease!
}

describe('RedisShortTermStore storage lifecycle', () => {
  it('creates an open epoch-one fence with the task', async () => {
    await openTask()

    expect(lifecycle.supportsHotColdRelease).toBe(true)
    await expect(lifecycle.getWriteFence!('task-1')).resolves.toEqual({
      taskId: 'task-1',
      acceptingWrites: true,
      storageEpoch: 1,
      activeReleaseGeneration: null,
    })
  })

  it('serializes tokenized locks, rejects stale owners, and recovers after expiry', async () => {
    await openTask()
    const lease = await acquire('owner-a', 'generation-a', 1000)

    await expect(
      lifecycle.acquireStorageLock!('task-1', 'owner-b', 'generation-b', 1000),
    ).resolves.toBeNull()
    await expect(
      lifecycle.renewStorageLock!({ ...lease, lockToken: 'stale' }, 1000),
    ).resolves.toBe(false)
    await expect(
      lifecycle.releaseStorageLock!({ ...lease, generation: 'stale' }),
    ).resolves.toBe(false)
    await expect(lifecycle.renewStorageLock!(lease, 1000)).resolves.toBe(true)
    await expect(lifecycle.releaseStorageLock!(lease)).resolves.toBe(true)
    await expect(lifecycle.releaseStorageLock!(lease)).resolves.toBe(false)

    const expiring = await acquire('owner-expiring', 'generation-expiring', 30)
    await new Promise((resolve) => setTimeout(resolve, 50))
    const recovered = await acquire('owner-recovered', 'generation-recovered', 1000)
    expect(recovered.lockToken).toBe('owner-recovered')
    await expect(lifecycle.renewStorageLock!(expiring, 1000)).resolves.toBe(false)
  })

  it('closes a writer race without consuming an index and reopens on a new epoch', async () => {
    await openTask()
    const oldToken = { taskId: 'task-1', storageEpoch: 1 }
    await expect(
      lifecycle.commitEventFenced!('task-1', makeEvent('event-0'), oldToken),
    ).resolves.toMatchObject({ event: { index: 0 }, stored: true })

    const lease = await acquire()
    await expect(lifecycle.closeWriteFence!(lease, 1)).resolves.toMatchObject({
      acceptingWrites: false,
      storageEpoch: 1,
      activeReleaseGeneration: 'generation-1',
      highWatermark: 0,
    })

    await expect(
      lifecycle.commitEventFenced!('task-1', makeEvent('blocked'), oldToken),
    ).rejects.toMatchObject({ code: 'storage_fence_conflict', retryable: true })
    await expect(redis.get(`${prefix}:idx:task-1`)).resolves.toBe('1')

    const newToken = await lifecycle.reopenWriteFence!(lease, 1)
    expect(newToken).toEqual({ taskId: 'task-1', storageEpoch: 2 })
    await expect(
      lifecycle.commitEventFenced!('task-1', makeEvent('stale'), oldToken),
    ).rejects.toMatchObject({ code: 'storage_fence_conflict' })
    await expect(
      lifecycle.commitEventFenced!('task-1', makeEvent('event-1'), newToken),
    ).resolves.toMatchObject({ event: { index: 1 } })
  })

  it('lets a new lock generation adopt a closed fence for recovery', async () => {
    await openTask()
    await lifecycle.commitEventFenced!(
      'task-1',
      makeEvent('event-0'),
      { taskId: 'task-1', storageEpoch: 1 },
    )
    const stale = await acquire('stale-owner', 'stale-generation')
    await lifecycle.closeWriteFence!(stale, 1)
    await lifecycle.releaseStorageLock!(stale)

    const recovery = await acquire('recovery-owner', 'recovery-generation')
    await expect(lifecycle.closeWriteFence!(recovery, 1)).resolves.toMatchObject({
      acceptingWrites: false,
      storageEpoch: 1,
      activeReleaseGeneration: 'recovery-generation',
      highWatermark: 0,
    })
    await expect(lifecycle.renewStorageLock!(stale, 5_000)).resolves.toBe(false)
    await expect(lifecycle.reopenWriteFence!(recovery, 1)).resolves.toEqual({
      taskId: 'task-1',
      storageEpoch: 2,
    })
  })

  it('linearizes concurrent fenced commits against close without index gaps', async () => {
    await openTask()
    const lease = await acquire()
    const token = { taskId: 'task-1', storageEpoch: 1 }
    const writerRedis = redis.duplicate()
    const writer = new RedisShortTermStore(writerRedis, {
      prefix,
      legacySeriesWrites: false,
    })
    const commits = Array.from({ length: 50 }, (_, index) =>
      writer.commitEventFenced('task-1', makeEvent(`race-${index}`), token),
    )
    const close = lifecycle.closeWriteFence!(lease, 1)
    const [closed, results] = await Promise.all([close, Promise.allSettled(commits)])
    writerRedis.disconnect()

    const committed = results
      .filter((result): result is PromiseFulfilledResult<Awaited<(typeof commits)[number]>> =>
        result.status === 'fulfilled',
      )
      .map((result) => result.value.event.index)
      .sort((left, right) => left - right)
    expect(committed).toEqual(
      Array.from({ length: committed.length }, (_, index) => index),
    )
    expect(closed.highWatermark).toBe(committed.at(-1) ?? -1)
    await expect(redis.get(`${prefix}:idx:task-1`)).resolves.toBe(
      committed.length === 0 ? null : String(committed.length),
    )
  })

  it('commits keep-all, latest, and accumulate series atomically in index order', async () => {
    await openTask()
    const token = { taskId: 'task-1', storageEpoch: 1 }

    await lifecycle.commitEventFenced!('task-1', makeEvent('keep-0'), token)
    await lifecycle.commitEventFenced!(
      'task-1',
      makeEvent('latest-1', {
        seriesId: 'status',
        seriesMode: 'latest',
        data: { status: 'starting' },
      }),
      token,
    )
    await lifecycle.commitEventFenced!('task-1', makeEvent('keep-2'), token)
    await lifecycle.commitEventFenced!(
      'task-1',
      makeEvent('latest-3', {
        seriesId: 'status',
        seriesMode: 'latest',
        data: { status: 'ready' },
      }),
      token,
    )
    const firstDelta = await lifecycle.commitEventFenced!(
      'task-1',
      makeEvent('delta-4', {
        seriesId: 'output',
        seriesMode: 'accumulate',
        seriesAccField: 'delta',
        data: { delta: 'hello' },
      }),
      token,
    )
    await lifecycle.commitEventFenced!('task-1', makeEvent('keep-5'), token)
    const secondDelta = await lifecycle.commitEventFenced!(
      'task-1',
      makeEvent('delta-6', {
        seriesId: 'output',
        seriesMode: 'accumulate',
        seriesAccField: 'delta',
        data: { delta: ' world' },
      }),
      token,
    )

    expect(firstDelta.accumulatedEvent?.data).toEqual({ delta: 'hello' })
    expect(secondDelta.event.data).toEqual({ delta: ' world' })
    expect(secondDelta.accumulatedEvent?.data).toEqual({ delta: 'hello world' })
    const events = await store.getEvents('task-1')
    expect(events.map((event) => event.index)).toEqual([0, 2, 3, 4, 5, 6])
    expect(events.map((event) => event.id)).not.toContain('latest-1')
    await expect(store.getSeriesLatest('task-1', 'status')).resolves.toMatchObject({
      id: 'latest-3',
      index: 3,
    })
    await expect(store.getSeriesLatest('task-1', 'output')).resolves.toMatchObject({
      id: 'delta-6',
      index: 6,
      data: { delta: 'hello world' },
    })
  })

  it('does not lose concurrent accumulated deltas while retrying CAS', async () => {
    await openTask()
    const token = { taskId: 'task-1', storageEpoch: 1 }

    const committed = await Promise.all(
      Array.from({ length: 50 }, (_, index) =>
        lifecycle.commitEventFenced!(
          'task-1',
          makeEvent(`delta-${index}`, {
            seriesId: 'output',
            seriesMode: 'accumulate',
            data: { delta: 'x' },
          }),
          token,
        ),
      ),
    )

    expect(new Set(committed.map((result) => result.event.index)).size).toBe(50)
    await expect(store.getSeriesLatest('task-1', 'output')).resolves.toMatchObject({
      data: { delta: 'x'.repeat(50) },
    })
    await expect(store.getEvents('task-1')).resolves.toHaveLength(50)
  })

  it('preserves accumulate state across the rolling legacy-writer window', async () => {
    const compatibleStore = new RedisShortTermStore(redis, {
      prefix,
      legacySeriesWrites: true,
    })
    await compatibleStore.saveTask(makeTask())
    const token = { taskId: 'task-1', storageEpoch: 1 }

    await compatibleStore.commitEventFenced(
      'task-1',
      makeEvent('new-a', {
        seriesId: 'output',
        seriesMode: 'accumulate',
        data: { delta: 'A' },
      }),
      token,
    )

    const oldIndex = (await redis.incr(`${prefix}:idx:task-1`)) - 1
    const oldDelta = {
      ...makeEvent('old-b', {
        seriesId: 'output',
        seriesMode: 'accumulate',
        data: { delta: 'B' },
      }),
      index: oldIndex,
    }
    await redis.rpush(`${prefix}:events:task-1`, JSON.stringify(oldDelta))
    await redis.set(
      `${prefix}:series:task-1:output`,
      JSON.stringify({
        ...oldDelta,
        data: { delta: 'AB' },
      }),
    )
    await redis.sadd(`${prefix}:seriesIds:task-1`, 'output')

    const committed = await compatibleStore.commitEventFenced(
      'task-1',
      makeEvent('new-c', {
        seriesId: 'output',
        seriesMode: 'accumulate',
        data: { delta: 'C' },
      }),
      token,
    )
    expect(committed.accumulatedEvent?.data).toEqual({ delta: 'ABC' })
    await expect(
      compatibleStore.getSeriesLatest('task-1', 'output'),
    ).resolves.toMatchObject({ data: { delta: 'ABC' } })

    const fixed = await store.commitEventFenced(
      'task-1',
      makeEvent('fixed-d', {
        seriesId: 'output',
        seriesMode: 'accumulate',
        data: { delta: 'D' },
      }),
      token,
    )
    expect(fixed.accumulatedEvent?.data).toEqual({ delta: 'ABCD' })
    await expect(redis.exists(`${prefix}:series:task-1:output`)).resolves.toBe(0)
    await expect(store.getEvents('task-1')).resolves.toHaveLength(4)
  })

  it('replaces the latest event written by a legacy writer during rollout', async () => {
    const compatibleStore = new RedisShortTermStore(redis, {
      prefix,
      legacySeriesWrites: true,
    })
    await compatibleStore.saveTask(makeTask())
    const token = { taskId: 'task-1', storageEpoch: 1 }

    await compatibleStore.commitEventFenced(
      'task-1',
      makeEvent('new-a', {
        seriesId: 'status',
        seriesMode: 'latest',
      }),
      token,
    )

    const oldIndex = (await redis.incr(`${prefix}:idx:task-1`)) - 1
    const oldLatest = {
      ...makeEvent('old-b', {
        seriesId: 'status',
        seriesMode: 'latest',
      }),
      index: oldIndex,
    }
    const oldLatestJson = JSON.stringify(oldLatest)
    await redis.lset(`${prefix}:events:task-1`, 0, oldLatestJson)
    await redis.set(`${prefix}:series:task-1:status`, oldLatestJson)
    await redis.sadd(`${prefix}:seriesIds:task-1`, 'status')

    await compatibleStore.commitEventFenced(
      'task-1',
      makeEvent('new-c', {
        seriesId: 'status',
        seriesMode: 'latest',
      }),
      token,
    )

    await expect(compatibleStore.getEvents('task-1')).resolves.toMatchObject([
      { id: 'new-c' },
    ])
    await expect(
      compatibleStore.getSeriesLatest('task-1', 'status'),
    ).resolves.toMatchObject({ id: 'new-c' })
  })

  it('preserves opaque event JSON through fenced and accumulated commits', async () => {
    await openTask()
    const token = { taskId: 'task-1', storageEpoch: 1 }
    const fragile = {
      empty: [],
      nested: [[], { empty: [] }],
      precise: 1.2345678901234567,
      maxSafe: 9007199254740991,
    }

    const keep = await lifecycle.commitEventFenced!(
      'task-1',
      makeEvent('opaque-0', { data: fragile }),
      token,
    )
    expect(keep.event.data).toEqual(fragile)

    await lifecycle.commitEventFenced!(
      'task-1',
      makeEvent('opaque-delta-1', {
        seriesId: 'output',
        seriesMode: 'accumulate',
        data: { ...fragile, delta: 'hello' },
      }),
      token,
    )
    const accumulated = await lifecycle.commitEventFenced!(
      'task-1',
      makeEvent('opaque-delta-2', {
        seriesId: 'output',
        seriesMode: 'accumulate',
        data: { ...fragile, delta: ' world' },
      }),
      token,
    )
    expect(accumulated.event.data).toEqual({ ...fragile, delta: ' world' })
    expect(accumulated.accumulatedEvent?.data).toEqual({
      ...fragile,
      delta: 'hello world',
    })
    await expect(store.getEvents('task-1')).resolves.toEqual([
      keep.event,
      expect.objectContaining({ data: { ...fragile, delta: 'hello' } }),
      expect.objectContaining({ data: { ...fragile, delta: ' world' } }),
    ])
  })

  it('never decodes event JSON in Lua or leaves a partial commit', async () => {
    await openTask()
    const token = { taskId: 'task-1', storageEpoch: 1 }
    const unpairedSurrogate = '\ud800'

    const first = await lifecycle.commitEventFenced!(
      'task-1',
      makeEvent('surrogate-0', { data: { text: unpairedSurrogate } }),
      token,
    )
    expect(first.event).toMatchObject({
      index: 0,
      data: { text: unpairedSurrogate },
    })

    await lifecycle.commitEventFenced!(
      'task-1',
      makeEvent('surrogate-delta-1', {
        seriesId: 'output',
        seriesMode: 'accumulate',
        data: { delta: unpairedSurrogate },
      }),
      token,
    )
    const second = await lifecycle.commitEventFenced!(
      'task-1',
      makeEvent('surrogate-delta-2', {
        seriesId: 'output',
        seriesMode: 'accumulate',
        data: { delta: 'tail' },
      }),
      token,
    )

    expect(second.event.index).toBe(2)
    expect(second.accumulatedEvent?.data).toEqual({
      delta: `${unpairedSurrogate}tail`,
    })
    await expect(redis.get(`${prefix}:idx:task-1`)).resolves.toBe('3')
    await expect(redis.llen(`${prefix}:events:task-1`)).resolves.toBe(3)
    await expect(redis.get(`${prefix}:hotWindow:task-1`)).resolves.toBe(
      JSON.stringify({ firstIndex: 0, lastIndex: 2 }),
    )
  })

  it('preserves exact indexes at the maximum writable safe-integer boundary', async () => {
    await openTask()
    const token = { taskId: 'task-1', storageEpoch: 1 }
    const maximumWritableIndex = 9_007_199_254_740_990
    await redis.set(`${prefix}:idx:task-1`, String(maximumWritableIndex))

    const committed = await lifecycle.commitEventFenced!(
      'task-1',
      makeEvent('maximum-safe-index'),
      token,
    )
    expect(committed.event.index).toBe(maximumWritableIndex)
    await expect(redis.get(`${prefix}:idx:task-1`)).resolves.toBe(
      '9007199254740991',
    )
    const rawEvent = await redis.lindex(`${prefix}:events:task-1`, 0)
    expect(rawEvent).toContain('"index":9007199254740990')
    expect(JSON.parse(rawEvent!).index).toBe(maximumWritableIndex)
    await expect(redis.get(`${prefix}:hotWindow:task-1`)).resolves.toBe(
      '{"firstIndex":9007199254740990,"lastIndex":9007199254740990}',
    )

    await expect(
      lifecycle.commitEventFenced!('task-1', makeEvent('overflow'), token),
    ).rejects.toMatchObject({ code: 'storage_integrity_error' })
    await expect(redis.llen(`${prefix}:events:task-1`)).resolves.toBe(1)
    await expect(redis.get(`${prefix}:idx:task-1`)).resolves.toBe(
      '9007199254740991',
    )

    const lease = await acquire()
    await expect(lifecycle.closeWriteFence!(lease, 1)).resolves.toMatchObject({
      highWatermark: maximumWritableIndex,
    })
  })

  it('stores unbounded new series cardinality in fixed task keys', async () => {
    await openTask()
    const token = { taskId: 'task-1', storageEpoch: 1 }

    await Promise.all(
      Array.from({ length: 200 }, (_, index) =>
        lifecycle.commitEventFenced!(
          'task-1',
          makeEvent(`series-${index}`, {
            seriesId: `series-${index}`,
            seriesMode: 'latest',
          }),
          token,
        ),
      ),
    )

    await expect(redis.hlen(`${prefix}:seriesState:task-1`)).resolves.toBe(200)
    await expect(redis.keys(`${prefix}:series:task-1:*`)).resolves.toEqual([])
    await expect(redis.exists(`${prefix}:seriesIds:task-1`)).resolves.toBe(0)
  })

  it('checks the fence in the same operation as a task update', async () => {
    await openTask()
    const token = { taskId: 'task-1', storageEpoch: 1 }
    const lease = await acquire()
    await lifecycle.closeWriteFence!(lease, 1)

    await expect(
      lifecycle.saveTaskFenced!(makeTask({ status: 'completed' }), token),
    ).rejects.toMatchObject({ code: 'storage_fence_conflict' })
    await expect(store.getTask('task-1')).resolves.toMatchObject({ status: 'running' })
  })

  it('commits a task transition and all derived events in one fenced operation', async () => {
    await openTask()
    const token = { taskId: 'task-1', storageEpoch: 1 }
    const committed = await lifecycle.commitTaskEventsFenced!(
      makeTask({ status: 'blocked', reason: 'approval' }),
      await taskRevision(),
      [
        makeEvent('status', {
          type: 'taskcast:status',
          data: { status: 'blocked' },
        }),
        makeEvent('blocked', {
          type: 'taskcast:blocked',
          data: { reason: 'approval' },
        }),
      ],
      token,
    )
    if (!committed) throw new Error('transition unexpectedly lost its status CAS')

    expect(committed.map((event) => event.index)).toEqual([0, 1])
    await expect(store.getTask('task-1')).resolves.toMatchObject({
      status: 'blocked',
      reason: 'approval',
    })
    await expect(store.getEvents('task-1')).resolves.toEqual(committed)

    const lease = await acquire()
    await lifecycle.closeWriteFence!(lease, 1)
    await expect(
      lifecycle.commitTaskEventsFenced!(
        makeTask({ status: 'completed' }),
        await taskRevision(),
        [makeEvent('completed', { type: 'taskcast:status' })],
        token,
      ),
    ).rejects.toMatchObject({ code: 'storage_fence_conflict' })
    await expect(store.getTask('task-1')).resolves.toMatchObject({
      status: 'blocked',
    })
    await expect(store.getEvents('task-1')).resolves.toHaveLength(2)
  })

  it('rejects an overflowing task-event batch without partial mutation', async () => {
    await openTask()
    const token = { taskId: 'task-1', storageEpoch: 1 }
    await redis.set(`${prefix}:idx:task-1`, '9007199254740990')

    await expect(
      lifecycle.commitTaskEventsFenced!(
        makeTask({ status: 'blocked' }),
        await taskRevision(),
        [makeEvent('status'), makeEvent('blocked')],
        token,
      ),
    ).rejects.toMatchObject({ code: 'storage_integrity_error' })
    await expect(store.getTask('task-1')).resolves.toMatchObject({
      status: 'running',
    })
    await expect(store.getEvents('task-1')).resolves.toEqual([])
    await expect(redis.get(`${prefix}:idx:task-1`)).resolves.toBe(
      '9007199254740990',
    )
  })

  it('allows only one task transition from the same expected status', async () => {
    await openTask()
    const token = { taskId: 'task-1', storageEpoch: 1 }
    const revision = await taskRevision()

    const results = await Promise.all([
      lifecycle.commitTaskEventsFenced!(
        makeTask({ status: 'completed' }),
        revision,
        [makeEvent('completed', { type: 'taskcast:status' })],
        token,
      ),
      lifecycle.commitTaskEventsFenced!(
        makeTask({ status: 'failed' }),
        revision,
        [makeEvent('failed', { type: 'taskcast:status' })],
        token,
      ),
    ])

    expect(results.filter((result) => result !== null)).toHaveLength(1)
    expect(results.filter((result) => result === null)).toHaveLength(1)
    await expect(store.getEvents('task-1')).resolves.toHaveLength(1)
    await expect(store.getTask('task-1')).resolves.toMatchObject({
      status: expect.stringMatching(/^(completed|failed)$/),
    })
  })

  it('rejects a stale task revision after an assigned task is reclaimed', async () => {
    await store.saveTask(makeTask({ status: 'pending' }))
    const makeWorker = (id: string): Worker => ({
      id,
      status: 'idle',
      matchRule: {},
      capacity: 2,
      usedSlots: 0,
      weight: 1,
      connectionMode: 'pull',
      connectedAt: 1_000,
      lastHeartbeatAt: 1_000,
    })
    await store.saveWorker(makeWorker('worker-a'))
    await store.saveWorker(makeWorker('worker-b'))
    await expect(store.claimTask('task-1', 'worker-a', 1)).resolves.toBe(true)
    const snapshot = await lifecycle.getTaskMutationSnapshot!('task-1')
    if (!snapshot) throw new Error('task mutation snapshot is missing')
    await expect(store.claimTask('task-1', 'worker-b', 1)).resolves.toBe(true)

    await expect(lifecycle.commitTaskEventsFenced!(
      { ...snapshot.task, status: 'running' },
      snapshot.revision,
      [makeEvent('status', { type: 'taskcast:status' })],
      { taskId: 'task-1', storageEpoch: 1 },
    )).resolves.toBeNull()
    await expect(store.getTask('task-1')).resolves.toMatchObject({
      status: 'assigned',
      assignedWorker: 'worker-b',
    })
    await expect(store.getEvents('task-1')).resolves.toEqual([])
  })

  it('reads bounded sparse source pages using opaque list cursors', async () => {
    await openTask()
    const token = { taskId: 'task-1', storageEpoch: 1 }
    await lifecycle.commitEventFenced!('task-1', makeEvent('keep-0'), token)
    await lifecycle.commitEventFenced!(
      'task-1',
      makeEvent('latest-1', { seriesId: 'status', seriesMode: 'latest' }),
      token,
    )
    await lifecycle.commitEventFenced!('task-1', makeEvent('keep-2'), token)
    await lifecycle.commitEventFenced!(
      'task-1',
      makeEvent('latest-3', { seriesId: 'status', seriesMode: 'latest' }),
      token,
    )
    await lifecycle.commitEventFenced!('task-1', makeEvent('keep-4'), token)

    const first = await lifecycle.readArchiveSourcePage!('task-1', 4, null, 2)
    expect(first.events.map((event) => event.index)).toEqual([0, 2])
    expect(first.nextCursor).toEqual(expect.any(String))
    expect(first.nextCursor).not.toBe('2')
    expect(first.done).toBe(false)

    const second = await lifecycle.readArchiveSourcePage!(
      'task-1',
      4,
      first.nextCursor,
      2,
    )
    expect(second.events.map((event) => event.index)).toEqual([3, 4])
    expect(second.done).toBe(true)
    expect(second.nextCursor).toBeNull()
  })

  it('rejects a corrupted archive source whose indexes are out of order', async () => {
    await openTask()
    await redis.rpush(
      `${prefix}:events:task-1`,
      JSON.stringify({ ...makeEvent('event-2'), index: 2 }),
      JSON.stringify({ ...makeEvent('event-1'), index: 1 }),
    )

    await expect(
      lifecycle.readArchiveSourcePage!('task-1', 2, null, 10),
    ).rejects.toMatchObject({ code: 'storage_integrity_error', retryable: false })
  })

  it('rejects stale deletion and removes every task storage key atomically', async () => {
    await openTask()
    const token = { taskId: 'task-1', storageEpoch: 1 }
    await lifecycle.commitEventFenced!(
      'task-1',
      makeEvent('latest-0', { seriesId: 'status', seriesMode: 'latest' }),
      token,
    )
    await redis.set(
      `${prefix}:series:task-1:orphan`,
      JSON.stringify({ ...makeEvent('orphan'), index: 0 }),
    )
    const lease = await acquire()
    await lifecycle.closeWriteFence!(lease, 1)

    await expect(
      lifecycle.deleteTaskStorageFenced!({ ...lease, lockToken: 'stale' }, 1),
    ).rejects.toMatchObject({ code: 'storage_fence_conflict' })
    await expect(lifecycle.getTaskStoragePresence!('task-1')).resolves.toMatchObject({
      task: true,
      eventCount: 1,
      nextIndex: true,
      seriesStateCount: 2,
      writeFence: true,
    })

    await lifecycle.deleteTaskStorageFenced!(lease, 1)
    await expect(lifecycle.getTaskStoragePresence!('task-1')).resolves.toEqual({
      task: false,
      eventCount: 0,
      nextIndex: false,
      seriesStateCount: 0,
      writeFence: false,
    })
    await expect(redis.sismember(`${prefix}:tasks`, 'task-1')).resolves.toBe(0)
    await expect(redis.exists(`${prefix}:hotWindow:task-1`)).resolves.toBe(0)
    await expect(redis.exists(`${prefix}:seriesIds:task-1`)).resolves.toBe(0)
    await expect(redis.keys(`${prefix}:series:task-1:*`)).resolves.toEqual([])
    await expect(lifecycle.reopenWriteFence!(lease, 1)).rejects.toMatchObject({
      code: 'storage_fence_conflict',
    })
  })

  it('keeps wildcard and prefix-colliding task series isolated during release', async () => {
    for (const taskId of ['*', 'victim', 'parent', 'parent:child']) {
      await store.saveTask(makeTask({ id: taskId }))
      await lifecycle.commitEventFenced!(
        taskId,
        makeEvent(`${taskId}-latest`, {
          taskId,
          seriesId: 'status',
          seriesMode: 'latest',
        }),
        { taskId, storageEpoch: 1 },
      )
    }

    for (const taskId of ['*', 'parent']) {
      const lease = await lifecycle.acquireStorageLock!(
        taskId,
        `${taskId}-owner`,
        `${taskId}-generation`,
        5000,
      )
      expect(lease).not.toBeNull()
      await lifecycle.closeWriteFence!(lease!, 1)
      await lifecycle.deleteTaskStorageFenced!(lease!, 1)
    }

    await expect(store.getSeriesLatest('victim', 'status')).resolves.toMatchObject({
      id: 'victim-latest',
    })
    await expect(
      store.getSeriesLatest('parent:child', 'status'),
    ).resolves.toMatchObject({ id: 'parent:child-latest' })
    await expect(lifecycle.getTaskStoragePresence!('victim')).resolves.toMatchObject({
      task: true,
      seriesStateCount: 1,
    })
    await expect(
      lifecycle.getTaskStoragePresence!('parent:child'),
    ).resolves.toMatchObject({ task: true, seriesStateCount: 1 })
  })

  it('restores a bounded hot window atomically and never reuses a durable index', async () => {
    await openTask()
    const lease = await acquire()
    await lifecycle.closeWriteFence!(lease, 1)
    await lifecycle.deleteTaskStorageFenced!(lease, 1)

    const replayEvents: TaskEvent[] = [
      { ...makeEvent('event-7'), index: 7 },
      {
        ...makeEvent('event-9', {
          seriesId: 'status',
          seriesMode: 'latest',
          data: {
            status: 'ready',
            empty: [],
            nested: [[], { empty: [] }],
            precise: 1.2345678901234567,
            maxSafe: 9007199254740991,
          },
        }),
        index: 9,
      },
    ]
    const seriesLatest: DurableSeriesState[] = [
      {
        taskId: 'task-1',
        seriesId: 'status',
        mode: 'latest',
        event: replayEvents[1]!,
        throughIndex: 9,
      },
    ]
    const snapshot: RehydrateSnapshot = {
      task: makeTask({ updatedAt: 2000 }),
      archiveWatermark: 9,
      maxEventIndex: 9,
      replayEvents,
      seriesLatest,
      storageEpoch: 1,
    }

    const token = await lifecycle.restoreHotTaskFenced!(snapshot, lease, 2)
    expect(token).toEqual({ taskId: 'task-1', storageEpoch: 2 })
    await expect(store.getEvents('task-1')).resolves.toEqual(replayEvents)
    await expect(redis.get(`${prefix}:hotWindow:task-1`)).resolves.toBe(
      JSON.stringify({ firstIndex: 7, lastIndex: 9 }),
    )

    const committed = await lifecycle.commitEventFenced!(
      'task-1',
      makeEvent('event-10'),
      token,
    )
    expect(committed.event.index).toBe(10)

    await expect(
      lifecycle.restoreHotTaskFenced!(snapshot, { ...lease, lockToken: 'stale' }, 2),
    ).rejects.toMatchObject({ code: 'storage_fence_conflict' })
    await expect(lifecycle.restoreHotTaskFenced!(snapshot, lease, 2)).resolves.toEqual(
      token,
    )
    await expect(store.getEvents('task-1')).resolves.toHaveLength(3)
  })

  it('expires writer readiness registrations unless they are heartbeated', async () => {
    const startedAt = Date.now()
    await lifecycle.registerStorageWriter!(
      {
        instanceId: 'writer-a',
        storageProtocolVersion: 1,
        build: 'test-build',
        expiresAt: 0,
      },
      40,
    )

    const writers = await lifecycle.listStorageWriters!()
    expect(writers).toHaveLength(1)
    expect(writers[0]).toMatchObject({
      instanceId: 'writer-a',
      storageProtocolVersion: 1,
      build: 'test-build',
    })
    expect(writers[0]!.expiresAt).toBeGreaterThanOrEqual(startedAt + 35)

    await new Promise((resolve) => setTimeout(resolve, 60))
    await expect(lifecycle.listStorageWriters!()).resolves.toEqual([])
    await expect(redis.smembers(`${prefix}:storageWriters`)).resolves.toEqual([])
  })
})
