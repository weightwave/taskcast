import { afterAll, beforeAll, beforeEach, describe, expect, it, vi } from 'vitest'
import { Redis } from 'ioredis'
import postgres from 'postgres'
import { GenericContainer, Wait, type StartedTestContainer } from 'testcontainers'
import { join } from 'node:path'
import { TaskEngine, MemoryBroadcastProvider } from '../../src/index.js'
import { RedisShortTermStore } from '../../../redis/src/short-term.js'
import { PostgresLongTermStore } from '../../../postgres/src/long-term.js'
import { runMigrations } from '../../../postgres/src/migration-runner.js'

let pgContainer: StartedTestContainer | undefined
let redisContainer: StartedTestContainer | undefined
let sql: ReturnType<typeof postgres>
let redis: Redis
let hot: RedisShortTermStore
let durable: PostgresLongTermStore
let engine: TaskEngine

beforeAll(async () => {
  let pgUrl = process.env['TASKCAST_TEST_POSTGRES_URL']
  let redisUrl = process.env['TASKCAST_TEST_REDIS_URL']
  if (!pgUrl) {
    pgContainer = await new GenericContainer('postgres:16-alpine')
      .withEnvironment({ POSTGRES_USER: 'test', POSTGRES_PASSWORD: 'test', POSTGRES_DB: 'testdb' })
      .withExposedPorts(5432)
      .withWaitStrategy(Wait.forLogMessage(/ready to accept connections/, 2))
      .start()
    pgUrl = `postgres://test:test@localhost:${pgContainer.getMappedPort(5432)}/testdb`
  }
  if (!redisUrl) {
    redisContainer = await new GenericContainer('redis:7-alpine').withExposedPorts(6379).start()
    redisUrl = `redis://localhost:${redisContainer.getMappedPort(6379)}`
  }
  sql = postgres(pgUrl, { onnotice: () => {} })
  redis = new Redis(redisUrl)
  await runMigrations(sql, join(import.meta.dirname, '../../../../migrations/postgres'))
  hot = new RedisShortTermStore(redis)
  durable = new PostgresLongTermStore(sql)
}, 120_000)

afterAll(async () => {
  redis?.disconnect()
  await sql?.end()
  await redisContainer?.stop()
  await pgContainer?.stop()
})

beforeEach(async () => {
  await redis.flushdb()
  await sql`TRUNCATE taskcast_tasks CASCADE`
  engine = new TaskEngine({ shortTermStore: hot, longTermStore: durable, broadcast: new MemoryBroadcastProvider() })
})

async function overdue(taskId: string): Promise<void> {
  await sql`UPDATE taskcast_tasks SET execution_deadline_at = 0 WHERE id = ${taskId}`
}

describe('durable TTL with PostgreSQL and Redis', () => {
  it.each(['latest', 'events', 'empty'] as const)('terminalizes the %s tail once across concurrent sweepers', async (tail) => {
    const task = await engine.createTask({ id: `ttl-${tail}`, ttl: 60 })
    let lastIndex = -1
    if (tail !== 'empty') {
      await engine.transitionTask(task.id, 'running')
      for (let revision = 1; revision <= 3; revision++) {
        const event = await engine.publishEvent(task.id, {
          type: 'search.candidate_changed', level: 'info', data: { revision },
          ...(tail === 'latest' ? { seriesId: 'execution', seriesMode: 'latest' as const } : {}),
        })
        lastIndex = event.index
      }
    }
    await vi.waitFor(async () => expect(await durable.getLastEventIndex(task.id)).toBe(lastIndex))
    if (tail === 'latest') {
      const [row] = await sql`SELECT MAX(idx) AS max_index FROM taskcast_events WHERE task_id = ${task.id}`
      expect(Number(row!['max_index'])).toBeLessThan(lastIndex)
    }
    await overdue(task.id)
    const second = new TaskEngine({ shortTermStore: hot, longTermStore: durable, broadcast: new MemoryBroadcastProvider() })
    const results = await Promise.all([engine.sweepDurableTtl(1), second.sweepDurableTtl(1)])
    expect(results.reduce((sum, result) => sum + result.timedOut, 0)).toBe(1)
    expect(results.reduce((sum, result) => sum + result.failed, 0)).toBe(0)
    expect((await durable.getTask(task.id))?.status).toBe('timeout')
    expect((await hot.getTask(task.id))?.status).toBe('timeout')
    const terminal = (await durable.getEvents(task.id)).filter((event) =>
      event.type === 'taskcast:status' && (event.data as { status: string }).status === 'timeout')
    expect(terminal).toHaveLength(1)
    expect(terminal[0]?.index).toBe(lastIndex + 1)
    expect(await sql`SELECT * FROM taskcast_terminal_outbox WHERE projected_at IS NULL`).toHaveLength(0)
  })

  it('keeps the task running and reopens the fence when hot history is not durably caught up', async () => {
    const task = await engine.createTask({ id: 'ttl-lagging-history', ttl: 60 })
    await engine.transitionTask(task.id, 'running')
    for (let revision = 1; revision <= 2; revision++) {
      await engine.publishEvent(task.id, {
        type: 'search.candidate_changed', level: 'info', data: { revision },
        seriesId: 'execution', seriesMode: 'latest',
      })
    }
    await vi.waitFor(async () => expect(await durable.getLastEventIndex(task.id)).toBe(2))
    // Reproduce a missing durable tail while Redis has already accepted it.
    await sql`DELETE FROM taskcast_series_state WHERE task_id = ${task.id}`
    await overdue(task.id)
    expect(await engine.sweepDurableTtl(1)).toMatchObject({ claimed: 1, timedOut: 0, failed: 1 })
    expect((await durable.getTask(task.id))?.status).toBe('running')
    expect((await hot.getTask(task.id))?.status).toBe('running')
    expect(await sql`SELECT * FROM taskcast_terminal_outbox`).toHaveLength(0)
    expect(await hot.getWriteFence(task.id)).toMatchObject({ acceptingWrites: true })
  })
})
