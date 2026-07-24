import { describe, expect, it } from 'vitest'
import postgres from 'postgres'
import {
  DependencyUnavailableError,
  type DependencyObservation,
  type DependencyObserver,
} from '@taskcast/core'
import {
  classifyPostgresConnectivity,
  postgresCheck,
} from '../src/health.js'
import { PostgresLongTermStore } from '../src/long-term.js'

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
