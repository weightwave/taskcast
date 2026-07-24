import { join } from 'node:path'
import { describe, expect, it } from 'vitest'
import postgres from 'postgres'
import {
  GenericContainer,
  Wait,
  type StartedTestContainer,
} from 'testcontainers'
import {
  DependencyUnavailableError,
  type DependencyObservation,
  type DependencyObserver,
  type Task,
} from '@taskcast/core'
import {
  classifyPostgresConnectivity,
  postgresCheck,
} from '../src/health.js'
import { PostgresLongTermStore } from '../src/long-term.js'
import { runMigrations } from '../src/migration-runner.js'
import { TcpFaultProxy } from '../../redis/tests/helpers/tcp-fault-proxy.js'

async function eventually(
  operation: () => Promise<void>,
  timeoutMs = 10_000,
): Promise<void> {
  const deadline = Date.now() + timeoutMs
  let lastError: unknown
  while (Date.now() < deadline) {
    try {
      await operation()
      return
    } catch (error) {
      lastError = error
      await new Promise((resolve) => setTimeout(resolve, 25))
    }
  }
  throw lastError ?? new Error('condition did not pass before the deadline')
}

async function withDeadline<T>(
  operation: Promise<T>,
  timeoutMs: number,
  message: string,
): Promise<T> {
  let timer: ReturnType<typeof setTimeout> | undefined
  try {
    return await Promise.race([
      operation,
      new Promise<never>((_, reject) => {
        timer = setTimeout(() => reject(new Error(message)), timeoutMs)
      }),
    ])
  } finally {
    if (timer !== undefined) clearTimeout(timer)
  }
}

async function settleBeforeDeadline<T>(
  operation: Promise<T>,
  timeoutMs: number,
  message: string,
): Promise<PromiseSettledResult<T>> {
  return withDeadline(
    operation.then(
      (value): PromiseFulfilledResult<T> => ({ status: 'fulfilled', value }),
      (reason): PromiseRejectedResult => ({ status: 'rejected', reason }),
    ),
    timeoutMs,
    message,
  )
}

interface FakeSql {
  sql: ReturnType<typeof postgres>
  queryCalls(): number
  queries(): string[]
}

function fakeSql(results: Array<unknown | Error>): FakeSql {
  let queryCalls = 0
  const queries: string[] = []
  const sql = ((first: TemplateStringsArray | string) => {
    if (typeof first === 'string') return first
    queries.push(first.join('?'))
    const result = results[queryCalls++]
    if (result instanceof Error) throw result
    return Promise.resolve(result ?? [])
  }) as unknown as ReturnType<typeof postgres>

  return {
    sql,
    queryCalls: () => queryCalls,
    queries: () => queries,
  }
}

class RecordingObserver implements DependencyObserver {
  readonly observations: DependencyObservation[] = []

  observe(observation: DependencyObservation): void {
    this.observations.push(observation)
  }
}

describe('classifyPostgresConnectivity', () => {
  it.each([
    ['ECONNREFUSED', 'connection_refused'],
    ['ECONNRESET', 'connection_reset'],
    ['EPIPE', 'connection_reset'],
    ['ETIMEDOUT', 'timeout'],
    ['ESOCKETTIMEDOUT', 'timeout'],
    ['ENOTFOUND', 'dns'],
    ['EAI_AGAIN', 'dns'],
    ['CONNECTION_CLOSED', 'connection_closed'],
    ['08006', 'unavailable'],
    ['57P01', 'unavailable'],
  ] as const)('classifies %s as %s', (code, expected) => {
    const error = Object.assign(new Error('database operation failed'), { code })
    expect(classifyPostgresConnectivity(error)).toBe(expected)
  })

  it('classifies postgres.js CONNECT_TIMEOUT exactly as timeout', () => {
    const error = Object.assign(new Error('database operation failed'), {
      code: 'CONNECT_TIMEOUT',
    })

    expect(classifyPostgresConnectivity(error)).toBe('timeout')
  })

  it('classifies connectivity errors through their cause chain', () => {
    const source = Object.assign(new Error('socket reset'), { code: 'ECONNRESET' })
    expect(classifyPostgresConnectivity(new Error('query failed', { cause: source })))
      .toBe('connection_reset')
  })

  it('does not classify a plain object with a connectivity code', () => {
    expect(classifyPostgresConnectivity({
      code: 'ECONNREFUSED',
    })).toBeUndefined()
  })

  it('does not classify an Error with an inherited connectivity code', () => {
    const prototype = Object.assign(Object.create(Error.prototype) as Error, {
      code: 'ECONNREFUSED',
    })
    const error = new Error('database operation failed')
    Object.setPrototypeOf(error, prototype)
    expect(error).toBeInstanceOf(Error)

    expect(classifyPostgresConnectivity(error)).toBeUndefined()
  })

  it.each([
    '23505',
    '23503',
    '23514',
    '42601',
    '08',
    '080000',
    'constructor',
    'toString',
    '__proto__',
  ])('does not classify ordinary SQLSTATE %s', (code) => {
    const error = Object.assign(new Error('ordinary database error'), { code })
    expect(classifyPostgresConnectivity(error)).toBeUndefined()
  })

  it.each([
    new Error('Task already exists: task-1'),
    new Error('Archive event id conflicts with another task: event-1'),
    new TypeError('validation failed'),
  ])('does not classify application error: $message', (error) => {
    expect(classifyPostgresConnectivity(error)).toBeUndefined()
  })
})

describe('postgresCheck', () => {
  it('executes SELECT 1 exactly once', async () => {
    const fake = fakeSql([[]])

    await postgresCheck(fake.sql)

    expect(fake.queryCalls()).toBe(1)
    expect(fake.queries()).toEqual(['SELECT 1'])
  })
})

describe('PostgresLongTermStore observations', () => {
  it('wraps one classified public operation without replay and reports recovery', async () => {
    const source = Object.assign(new Error('connection closed'), {
      code: 'CONNECTION_CLOSED',
    })
    const fake = fakeSql([source, []])
    const observer = new RecordingObserver()
    const store = new PostgresLongTermStore(fake.sql, observer)

    const first = store.getTask('task-1')
    await expect(first).rejects.toMatchObject({
      name: 'DependencyUnavailableError',
      dependency: 'postgres',
      kind: 'connection_closed',
    })
    await first.catch((error: unknown) => {
      expect(error).toBeInstanceOf(DependencyUnavailableError)
      expect((error as DependencyUnavailableError).cause).toBe(source)
    })
    expect(fake.queryCalls()).toBe(1)
    expect(observer.observations).toEqual([{
      dependency: 'postgres',
      state: 'unhealthy',
      errorKind: 'connection_closed',
    }])

    await expect(store.getTask('task-1')).resolves.toBeNull()
    expect(fake.queryCalls()).toBe(2)
    expect(observer.observations).toEqual([
      {
        dependency: 'postgres',
        state: 'unhealthy',
        errorKind: 'connection_closed',
      },
      {
        dependency: 'postgres',
        state: 'healthy',
      },
    ])
  })

  it('leaves ordinary public operation errors unchanged and unobserved', async () => {
    const syntaxError = Object.assign(new Error('syntax error'), { code: '42601' })
    const fake = fakeSql([syntaxError])
    const observer = new RecordingObserver()
    const store = new PostgresLongTermStore(fake.sql, observer)

    await expect(store.getTask('task-1')).rejects.toBe(syntaxError)
    expect(fake.queryCalls()).toBe(1)
    expect(observer.observations).toEqual([])
  })
})

describe('PostgreSQL pool disconnect recovery', () => {
  it('fails one in-flight statement and recovers readiness and store on the same pool', async () => {
    let container: StartedTestContainer | undefined
    let proxy: TcpFaultProxy | undefined
    let sql: ReturnType<typeof postgres> | undefined

    try {
      container = await new GenericContainer('postgres:16-alpine')
        .withEnvironment({
          POSTGRES_USER: 'test',
          POSTGRES_PASSWORD: 'test',
          POSTGRES_DB: 'testdb',
        })
        .withExposedPorts(5432)
        .withWaitStrategy(Wait.forLogMessage(/ready to accept connections/, 2))
        .start()
      proxy = new TcpFaultProxy(
        '127.0.0.1',
        container.getMappedPort(5432),
      )
      await proxy.open()
      sql = postgres(
        `postgres://test:test@127.0.0.1:${proxy.port}/testdb`,
        {
          max: 1,
          connect_timeout: 2,
        },
      )
      const store = new PostgresLongTermStore(sql)
      await runMigrations(
        sql,
        join(import.meta.dirname, '../../../migrations/postgres'),
      )
      await expect(postgresCheck(sql)).resolves.toBeUndefined()

      const marker = 'taskcast_pg_no_replay_ts'
      await sql.unsafe(`
        CREATE TABLE taskcast_test_no_replay (
          marker TEXT PRIMARY KEY,
          executions INTEGER NOT NULL
        )
      `)
      await sql.unsafe(`
        INSERT INTO taskcast_test_no_replay (marker, executions)
        VALUES ('${marker}', 0)
      `)
      const matchedBefore = proxy.matchedCommands
      proxy.dropNextResponse((request) =>
        request.includes(Buffer.from(marker)),
      )
      const statementOutcome = await settleBeforeDeadline(
        Promise.resolve(
          sql.unsafe(`
            UPDATE taskcast_test_no_replay
            SET executions = executions + 1
            WHERE marker = '${marker}'
            /* ${marker} */
            RETURNING executions
          `).simple(),
        ),
        5_000,
        'in-flight PostgreSQL statement did not fail before the deadline',
      )
      expect(statementOutcome.status).toBe('rejected')
      expect(proxy.matchedCommands - matchedBefore).toBe(1)

      await proxy.refuse()
      const readinessOutcome = await settleBeforeDeadline(
        postgresCheck(sql),
        5_000,
        'PostgreSQL readiness did not fail during refusal',
      )
      expect(readinessOutcome.status).toBe('rejected')

      await proxy.open()
      await eventually(() => postgresCheck(sql!))
      const [{ executions }] = await sql.unsafe<{ executions: number }[]>(
        `SELECT executions FROM taskcast_test_no_replay WHERE marker = '${marker}'`,
      )
      expect(executions).toBe(1)

      const recoveredTask: Task = {
        id: 'task-postgres-recovered',
        status: 'pending',
        params: { prompt: 'same pool' },
        createdAt: Date.now(),
        updatedAt: Date.now(),
      }
      await store.saveTask(recoveredTask)
      await expect(store.getTask(recoveredTask.id)).resolves.toEqual(
        recoveredTask,
      )
      expect(proxy.matchedCommands - matchedBefore).toBe(1)
    } finally {
      try {
        if (proxy !== undefined) await proxy.open()
      } finally {
        try {
          if (sql !== undefined) await sql.end({ timeout: 1 })
        } finally {
          try {
            await proxy?.stop()
          } finally {
            await container?.stop()
          }
        }
      }
    }
  }, 60_000)
})
