import { afterAll, beforeAll, beforeEach, describe, expect, it } from 'vitest'
import postgres from 'postgres'
import { GenericContainer, Wait, type StartedTestContainer } from 'testcontainers'
import { join } from 'node:path'
import type {
  Task,
  TaskEvent,
  TtlClaim,
  WorkerAssignment,
} from '@taskcast/core'
import { PostgresLongTermStore } from '../../src/long-term.js'
import { runMigrations } from '../../src/migration-runner.js'

let container: StartedTestContainer
let sql: ReturnType<typeof postgres>
let store: PostgresLongTermStore

beforeAll(async () => {
  let connection = process.env['TASKCAST_TEST_POSTGRES_URL']
  if (!connection) {
    container = await new GenericContainer('postgres:16-alpine')
      .withEnvironment({
        POSTGRES_USER: 'test',
        POSTGRES_PASSWORD: 'test',
        POSTGRES_DB: 'testdb',
      })
      .withExposedPorts(5432)
      .withWaitStrategy(Wait.forLogMessage(/ready to accept connections/, 2))
      .start()
    connection = `postgres://test:test@localhost:${container.getMappedPort(5432)}/testdb`
  }
  sql = postgres(connection, { onnotice: () => {} })
  await runMigrations(sql, join(import.meta.dirname, '../../../../migrations/postgres'))
  store = new PostgresLongTermStore(sql)
}, 120_000)

afterAll(async () => {
  await sql?.end()
  await container?.stop()
})

beforeEach(async () => {
  await sql`TRUNCATE taskcast_tasks CASCADE`
})

const makeTask = (
  id = 'ttl-task',
  overrides: Partial<Task> = {},
): Task => ({
  id,
  status: 'running',
  createdAt: 1_000,
  updatedAt: 1_000,
  ttl: 60,
  ...overrides,
})

const makeAssignment = (taskId = 'ttl-task'): WorkerAssignment => ({
  taskId,
  workerId: 'worker-1',
  cost: 3,
  assignedAt: 2_000,
  status: 'running',
})

const makeTimeoutEvent = (taskId = 'ttl-task', index = 0): TaskEvent => ({
  id: `timeout-${taskId}`,
  taskId,
  index,
  timestamp: 3_000,
  type: 'taskcast:status',
  level: 'info',
  data: { status: 'timeout' },
})

async function makeOverdue(task = makeTask()): Promise<TtlClaim> {
  await store.saveTask(task)
  await sql`
    UPDATE taskcast_tasks
    SET execution_deadline_at =
      FLOOR(EXTRACT(EPOCH FROM clock_timestamp()) * 1000)::BIGINT - 1
    WHERE id = ${task.id}
  `
  const [claim] = await store.claimOverdueTasks(1, 30_000)
  expect(claim).toBeDefined()
  return claim!
}

describe('PostgresLongTermStore durable TTL', () => {
  it('uses database time for absolute deadlines and suspends paused tasks', async () => {
    const before = Date.now()
    await store.saveTask(makeTask())
    const [created] = await sql`
      SELECT execution_deadline_at, task_version
      FROM taskcast_tasks
      WHERE id = 'ttl-task'
    `
    expect(Number(created!['execution_deadline_at'])).toBeGreaterThanOrEqual(
      before + 59_000,
    )
    expect(Number(created!['task_version'])).toBe(0)

    await store.saveTask(makeTask('ttl-task', {
      status: 'paused',
      updatedAt: 2_000,
    }))
    const [paused] = await sql`
      SELECT execution_deadline_at, task_version
      FROM taskcast_tasks
      WHERE id = 'ttl-task'
    `
    expect(paused!['execution_deadline_at']).toBeNull()
    expect(Number(paused!['task_version'])).toBe(1)

    await store.saveTask(makeTask('ttl-task', {
      status: 'running',
      updatedAt: 3_000,
    }))
    const [resumed] = await sql`
      SELECT execution_deadline_at, task_version
      FROM taskcast_tasks
      WHERE id = 'ttl-task'
    `
    expect(Number(resumed!['execution_deadline_at'])).toBeGreaterThanOrEqual(
      Date.now() + 59_000,
    )
    expect(Number(resumed!['task_version'])).toBe(2)
  })

  it('claims an overdue row once and allows takeover only after claim expiry', async () => {
    await makeOverdue()
    await expect(store.claimOverdueTasks(1, 30_000)).resolves.toEqual([])

    await sql`
      UPDATE taskcast_tasks
      SET ttl_claim_until =
        FLOOR(EXTRACT(EPOCH FROM clock_timestamp()) * 1000)::BIGINT - 1
      WHERE id = 'ttl-task'
    `
    const [reclaimed] = await store.claimOverdueTasks(1, 30_000)
    expect(reclaimed).toMatchObject({
      taskId: 'ttl-task',
      taskVersion: 0,
    })
  })

  it('terminalizes task, timeout event, assignment and outbox atomically', async () => {
    const claim = await makeOverdue()
    const assignment = makeAssignment()
    await store.saveDurableAssignment(assignment)
    const timeoutTask = makeTask('ttl-task', {
      status: 'timeout',
      completedAt: 3_000,
      updatedAt: 3_000,
    })
    const event = makeTimeoutEvent()

    const projection = await store.terminalizeTtlClaim(
      claim,
      timeoutTask,
      event,
      assignment,
    )

    expect(projection).toMatchObject({
      task: timeoutTask,
      event,
      assignment,
      claimToken: claim.claimToken,
      claimUntil: claim.claimUntil,
    })
    await expect(store.getTask('ttl-task')).resolves.toMatchObject({
      status: 'timeout',
      completedAt: 3_000,
    })
    await expect(store.getEvents('ttl-task')).resolves.toEqual([event])
    const assignments = await sql`SELECT * FROM taskcast_durable_assignments`
    expect(assignments).toHaveLength(0)

    await sql`
      UPDATE taskcast_terminal_outbox
      SET claim_until =
        FLOOR(EXTRACT(EPOCH FROM clock_timestamp()) * 1000)::BIGINT - 1
    `
    const claimed = await store.claimTerminalProjections(10, 'projector-1', 30_000)
    expect(claimed).toHaveLength(1)
    expect(claimed[0]).toEqual({
      ...projection,
      claimToken: 'projector-1',
      claimUntil: expect.any(Number),
    })
    await store.completeTerminalProjection(claimed[0]!)
    await expect(
      store.claimTerminalProjections(10, 'projector-2', 30_000),
    ).resolves.toEqual([])
  })

  it('loses safely to a non-terminal version change and a terminal transition', async () => {
    const versionClaim = await makeOverdue(makeTask('version-race'))
    await store.saveTask(makeTask('version-race', {
      status: 'blocked',
      updatedAt: 2_000,
    }))
    await expect(store.terminalizeTtlClaim(
      versionClaim,
      makeTask('version-race', {
        status: 'timeout',
        completedAt: 3_000,
        updatedAt: 3_000,
      }),
      makeTimeoutEvent('version-race'),
      null,
    )).resolves.toBeNull()
    await expect(store.getTask('version-race')).resolves.toMatchObject({
      status: 'blocked',
    })
    await store.saveTask(makeTask('version-race', {
      status: 'paused',
      updatedAt: 2_500,
    }))

    const terminalClaim = await makeOverdue(makeTask('terminal-race'))
    await store.saveTask(makeTask('terminal-race', {
      status: 'completed',
      completedAt: 2_000,
      updatedAt: 2_000,
    }))
    await expect(store.terminalizeTtlClaim(
      terminalClaim,
      makeTask('terminal-race', {
        status: 'timeout',
        completedAt: 3_000,
        updatedAt: 3_000,
      }),
      makeTimeoutEvent('terminal-race'),
      null,
    )).resolves.toBeNull()
    await expect(store.getTask('terminal-race')).resolves.toMatchObject({
      status: 'completed',
    })
  })

  it('compare-deletes only the requested durable assignment identity', async () => {
    await store.saveTask(makeTask())
    const assignment = makeAssignment()
    await store.saveDurableAssignment(assignment)

    await store.deleteDurableAssignment('ttl-task', 'wrong-assignment')
    expect(await sql`SELECT task_id FROM taskcast_durable_assignments`).toHaveLength(1)

    await store.deleteDurableAssignment(
      'ttl-task',
      'ttl-task:worker-1:2000',
    )
    expect(await sql`SELECT task_id FROM taskcast_durable_assignments`).toHaveLength(0)
  })

  it('validates sweep and claim bounds', async () => {
    await expect(store.claimOverdueTasks(0, 30_000)).rejects.toThrow()
    await expect(store.claimOverdueTasks(1, 0)).rejects.toThrow()
    await expect(
      store.claimTerminalProjections(0, 'projector', 30_000),
    ).rejects.toThrow()
    await expect(
      store.claimTerminalProjections(1, '', 30_000),
    ).rejects.toThrow()
    await expect(
      store.claimTerminalProjections(1, 'projector', 0),
    ).rejects.toThrow()
  })
})
