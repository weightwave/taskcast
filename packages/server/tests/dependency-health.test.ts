import { afterEach, describe, expect, it, vi } from 'vitest'
import { DependencyUnavailableError } from '@taskcast/core'
import { DependencyHealthRegistry } from '../src/dependency-health.js'

afterEach(() => {
  vi.useRealTimers()
})

describe('DependencyHealthRegistry', () => {
  it('checks only registered dependencies and sanitizes failures', async () => {
    const now = { value: 1_000 }
    const records: unknown[] = []
    let inactiveCalls = 0
    const health = new DependencyHealthRegistry({
      now: () => now.value,
      logger: (record) => records.push(record),
    })
    health.register('redisCommand', async () => {})
    health.register('redisPubSub', async () => {
      throw new DependencyUnavailableError(
        'redisPubSub',
        'connection_closed',
        new Error('must not leak'),
      )
    })
    const inactive = async () => {
      inactiveCalls += 1
    }
    void inactive

    const result = await health.checkReadiness(2_000)

    expect(result.ok).toBe(false)
    expect(result.dependencies.redisCommand).toEqual({ state: 'healthy' })
    expect(result.dependencies.redisPubSub).toEqual({
      state: 'unhealthy',
      errorKind: 'connection_closed',
    })
    expect(result.dependencies.postgres).toBeUndefined()
    expect(inactiveCalls).toBe(0)
    expect(JSON.stringify(result)).not.toContain('must not leak')
    expect(JSON.stringify(records)).not.toContain('must not leak')
  })

  it('starts checks concurrently in one group', async () => {
    const started: string[] = []
    let release!: () => void
    const gate = new Promise<void>((resolve) => {
      release = resolve
    })
    const health = new DependencyHealthRegistry({ logger: () => {} })
    health.register('redisCommand', async () => {
      started.push('redisCommand')
      await gate
    })
    health.register('postgres', async () => {
      started.push('postgres')
      await gate
    })

    const readiness = health.checkReadiness()
    await Promise.resolve()

    expect(started).toEqual(['redisCommand', 'postgres'])
    release()
    await expect(readiness).resolves.toMatchObject({ ok: true })
  })

  it('applies one overall deadline and marks every unfinished check timed out', async () => {
    vi.useFakeTimers()
    const health = new DependencyHealthRegistry({ logger: () => {} })
    health.register('redisCommand', () => new Promise<void>(() => {}))
    health.register('postgres', () => new Promise<void>(() => {}))

    const readiness = health.checkReadiness(2_000)
    let settled = false
    void readiness.then(() => {
      settled = true
    })

    await vi.advanceTimersByTimeAsync(1_999)
    expect(settled).toBe(false)
    await vi.advanceTimersByTimeAsync(1)

    await expect(readiness).resolves.toEqual({
      ok: false,
      dependencies: {
        redisCommand: { state: 'unhealthy', errorKind: 'timeout' },
        postgres: { state: 'unhealthy', errorKind: 'timeout' },
      },
    })
  })

  it('rejects duplicate dependency registrations', () => {
    const health = new DependencyHealthRegistry({ logger: () => {} })
    health.register('redisCommand', async () => {})

    expect(() => health.register('redisCommand', async () => {})).toThrow(
      'redisCommand',
    )
  })

  it('deduplicates transition logs, rate-limits summaries, and logs recovery downtime', () => {
    const now = { value: 1_000 }
    const records: Array<Record<string, unknown>> = []
    const health = new DependencyHealthRegistry({
      now: () => now.value,
      logger: (record) => records.push(record),
    })
    health.register('redisPubSub', async () => {})

    health.observe({
      dependency: 'redisPubSub',
      state: 'reconnecting',
      errorKind: 'connection_reset',
      attempt: 1,
      nextRetryMs: 500,
    })
    now.value = 60_999
    health.observe({
      dependency: 'redisPubSub',
      state: 'reconnecting',
      errorKind: 'connection_reset',
      attempt: 2,
      nextRetryMs: 1_000,
    })
    now.value = 61_000
    health.observe({
      dependency: 'redisPubSub',
      state: 'reconnecting',
      errorKind: 'connection_reset',
      attempt: 3,
      nextRetryMs: 2_000,
    })
    now.value = 120_999
    health.observe({
      dependency: 'redisPubSub',
      state: 'reconnecting',
      errorKind: 'connection_reset',
      attempt: 4,
    })
    now.value = 121_000
    health.observe({
      dependency: 'redisPubSub',
      state: 'reconnecting',
      errorKind: 'connection_reset',
      attempt: 5,
    })
    now.value = 126_000
    health.observe({ dependency: 'redisPubSub', state: 'healthy' })

    expect(records.map((record) => record.event)).toEqual([
      'dependency_state_change',
      'dependency_outage_summary',
      'dependency_outage_summary',
      'dependency_state_change',
    ])
    expect(records[0]).toMatchObject({
      level: 'warn',
      dependency: 'redisPubSub',
      from: 'starting',
      to: 'reconnecting',
      attempt: 1,
      nextRetryMs: 500,
      errorKind: 'connection_reset',
    })
    expect(records[3]).toMatchObject({
      level: 'info',
      dependency: 'redisPubSub',
      from: 'reconnecting',
      to: 'healthy',
      downtimeMs: 125_000,
    })
    expect(health.snapshot().redisPubSub).toMatchObject({
      state: 'healthy',
      consecutiveFailures: 0,
      reconnectAttempts: 0,
    })
    expect(JSON.stringify(records)).not.toMatch(
      /url|host|port|credential|password|raw|sql|argument|authorization|payload/i,
    )
  })

  it('exposes reconnect attempts only for Redis PubSub', () => {
    const health = new DependencyHealthRegistry({ logger: () => {} })
    health.register('redisCommand', async () => {})
    health.register('redisPubSub', async () => {})
    health.register('postgres', async () => {})
    health.observe({
      dependency: 'redisCommand',
      state: 'unhealthy',
      attempt: 7,
      errorKind: 'unavailable',
    })
    health.observe({
      dependency: 'redisPubSub',
      state: 'reconnecting',
      attempt: 3,
      errorKind: 'connection_closed',
    })

    expect(health.snapshot().redisCommand).not.toHaveProperty('reconnectAttempts')
    expect(health.snapshot().postgres).not.toHaveProperty('reconnectAttempts')
    expect(health.snapshot().redisPubSub).toHaveProperty('reconnectAttempts', 3)
  })
})
