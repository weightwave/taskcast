import { describe, it, expect, beforeAll, afterAll, beforeEach, vi } from 'vitest'
import { Redis } from 'ioredis'
import postgres from 'postgres'
import { GenericContainer, Wait, type StartedTestContainer } from 'testcontainers'
import { runMigrations } from '../../../postgres/src/migration-runner.js'
import { join } from 'path'
import { TaskEngine } from '../../src/engine.js'
import { RedisBroadcastProvider } from '../../../redis/src/broadcast.js'
import { RedisShortTermStore } from '../../../redis/src/short-term.js'
import { PostgresLongTermStore } from '../../../postgres/src/long-term.js'

let redisContainer: StartedTestContainer
let pgContainer: StartedTestContainer
let engine: TaskEngine
let pubClient: Redis
let subClient: Redis
let storeClient: Redis
let sql: ReturnType<typeof postgres>
let shortTermStore: RedisShortTermStore
let longTermStore: PostgresLongTermStore
let broadcast: RedisBroadcastProvider

beforeAll(async () => {
  // Start Redis and Postgres in parallel
  ;[redisContainer, pgContainer] = await Promise.all([
    new GenericContainer('redis:7-alpine').withExposedPorts(6379).start(),
    new GenericContainer('postgres:16-alpine')
      .withExposedPorts(5432)
      .withEnvironment({
        POSTGRES_USER: 'test',
        POSTGRES_PASSWORD: 'test',
        POSTGRES_DB: 'testdb',
      })
      .withWaitStrategy(Wait.forLogMessage(/ready to accept connections/, 2))
      .start(),
  ])

  const redisUrl = `redis://localhost:${redisContainer.getMappedPort(6379)}`
  pubClient = new Redis(redisUrl)
  subClient = new Redis(redisUrl)
  storeClient = new Redis(redisUrl)

  const pgPort = pgContainer.getMappedPort(5432)
  sql = postgres(`postgres://test:test@localhost:${pgPort}/testdb`)

  await runMigrations(sql, join(import.meta.dirname, '../../../../migrations/postgres'))

  broadcast = new RedisBroadcastProvider(pubClient, subClient)
  shortTermStore = new RedisShortTermStore(storeClient)
  longTermStore = new PostgresLongTermStore(sql)

  engine = new TaskEngine({ broadcast, shortTermStore: shortTermStore, longTermStore: longTermStore })
}, 120000)

afterAll(async () => {
  pubClient.disconnect()
  subClient.disconnect()
  storeClient.disconnect()
  await sql.end()
  await Promise.all([redisContainer?.stop(), pgContainer?.stop()])
})

beforeEach(async () => {
  vi.restoreAllMocks()
  await storeClient.flushall()
  await sql`TRUNCATE taskcast_events, taskcast_tasks CASCADE`
})

describe('Full stack: task lifecycle', () => {
  it.each(['hot', 'expired'] as const)('restores %s history with a fresh generation and no automatic retention', async (state) => {
    const retention = new TaskEngine({ broadcast, shortTermStore, longTermStore, cleanup: { enabled: true, rules: [{ target: 'events', trigger: {} }] } })
    const task = await retention.createTask({ id: `restore-${state}` })
    await retention.transitionTask(task.id, 'running')
    await retention.publishEvent(task.id, { type: 'progress', level: 'info', data: 'latest', seriesId: 'progress', seriesMode: 'latest' })
    await retention.transitionTask(task.id, 'completed', { result: { kept: true } })
    await vi.waitFor(async () => expect(await longTermStore.getEvents(task.id)).toHaveLength(3))
    const original = (await longTermStore.getTaskStorageMetadata(task.id))!
    const archive = await retention.exportTaskArchive(task.id)
    if (state === 'expired') {
      expect(await retention.sweepCleanup()).toMatchObject({ completed: 1 })
      await expect(retention.exportTaskArchive(task.id)).rejects.toThrow(/expired/)
    }
    await expect(retention.importTaskArchive(archive)).rejects.toThrow(/exists/)
    archive.task.historyExpiredAt = 1
    expect(await retention.importTaskArchive(archive, { overwrite: true })).toMatchObject({ taskId: task.id, eventCount: 3, overwritten: true })
    const restored = (await retention.getTask(task.id))!
    expect(restored.result).toEqual({ kept: true })
    expect(restored.historyExpiredAt).toBeUndefined()
    expect(restored.cleanupPolicyVersion).toBeUndefined()
    expect(await retention.getEvents(task.id)).toHaveLength(3)
    expect((await retention.getSeriesLatest(task.id, 'progress'))?.data).toBe('latest')
    const current = (await longTermStore.getTaskStorageMetadata(task.id))!
    expect(current.creationToken).not.toBe(original.creationToken)
    expect(current.storageState).toBe('hot')
    await expect(longTermStore.saveTask(restored, { creationToken: original.creationToken! })).rejects.toThrow()
    await expect(longTermStore.saveTask(restored)).rejects.toThrow()
    expect(await retention.sweepCleanup()).toMatchObject({ claimed: 0 })
  })

  it('releases the import lease after ownership loss and allows a subsequent retry', async () => {
    const task = await engine.createTask({ id: 'import-lease' })
    const archive = await engine.exportTaskArchive(task.id)
    const restore = vi.spyOn(longTermStore, 'restoreTaskArchive')
    vi.spyOn(shortTermStore, 'renewStorageLock').mockResolvedValueOnce(false)
    await expect(engine.importTaskArchive(archive, { overwrite: true })).rejects.toThrow(/lease was lost/)
    expect(restore).not.toHaveBeenCalled()
    expect(await engine.importTaskArchive(archive, { overwrite: true })).toMatchObject({ overwritten: true, eventCount: 0 })
    expect((await engine.getTask(task.id))?.status).toBe('pending')
  })

  it('creates task, publishes events, completes, persists to Postgres', async () => {
    const task = await engine.createTask({ type: 'llm.chat', params: { prompt: 'hello' } })
    await engine.transitionTask(task.id, 'running')

    await engine.publishEvent(task.id, {
      type: 'llm.delta',
      level: 'info',
      data: { delta: 'Hello' },
      seriesId: 'msg-1',
      seriesMode: 'accumulate',
    })
    await engine.publishEvent(task.id, {
      type: 'llm.delta',
      level: 'info',
      data: { delta: ' World' },
      seriesId: 'msg-1',
      seriesMode: 'accumulate',
    })

    await engine.transitionTask(task.id, 'completed', { result: { answer: 'Hello World' } })

    // Verify Redis short-term store
    const redisEvents = await engine.getEvents(task.id)
    const userEvents = redisEvents.filter((e) => e.type !== 'taskcast:status')
    expect(userEvents).toHaveLength(1)
    expect(userEvents[0]?.data).toEqual({ delta: 'Hello World' })

    // Verify Postgres long-term store
    const pgTask = await sql`SELECT * FROM taskcast_tasks WHERE id = ${task.id}`
    expect(pgTask[0]?.status).toBe('completed')
    expect((pgTask[0]?.result as { answer: string })?.answer).toBe('Hello World')

    const pgEvents = await sql`SELECT * FROM taskcast_events WHERE task_id = ${task.id} ORDER BY idx`
    expect(pgEvents.length).toBeGreaterThan(0)
  })

  it('SSE broadcast reaches subscriber in real time', async () => {
    const task = await engine.createTask({})
    await engine.transitionTask(task.id, 'running')

    const received: string[] = []
    const unsub = engine.subscribe(task.id, (event) => {
      received.push(event.type)
    })

    await new Promise((r) => setTimeout(r, 50)) // wait for redis subscription

    await engine.publishEvent(task.id, { type: 'tool.call', level: 'info', data: null })
    await engine.publishEvent(task.id, { type: 'tool.result', level: 'info', data: null })

    await new Promise((r) => setTimeout(r, 200)) // wait for delivery

    expect(received).toContain('tool.call')
    expect(received).toContain('tool.result')
    unsub()
  })

  it('reconnect with since.index resumes filtered stream correctly', async () => {
    const task = await engine.createTask({})
    await engine.transitionTask(task.id, 'running')

    // Publish mixed events
    for (let i = 0; i < 6; i++) {
      await engine.publishEvent(task.id, {
        type: i % 2 === 0 ? 'llm.delta' : 'tool.call',
        level: 'info',
        data: { i },
      })
    }
    await engine.transitionTask(task.id, 'completed')

    // Get all events and apply filtered index
    const { applyFilteredIndex } = await import('../../src/filter.js')
    const allEvents = await engine.getEvents(task.id)
    const firstSession = applyFilteredIndex(allEvents, { types: ['llm.*'] })
    expect(firstSession).toHaveLength(3) // indices 0, 2, 4 → filteredIndex 0,1,2

    // Reconnect from filteredIndex=1 (last seen=1, so get from 2 onwards)
    const secondSession = applyFilteredIndex(allEvents, {
      types: ['llm.*'],
      since: { index: 1 },
    })
    expect(secondSession).toHaveLength(1) // only filteredIndex=2
    expect(secondSession[0]?.filteredIndex).toBe(2)
  })
})
