import { describe, expect, it, vi } from 'vitest'
import {
  DependencyUnavailableError,
  findDependencyUnavailableError,
  type DependencyObservation,
  type DependencyObserver,
} from '../../src/index.js'

describe('dependency contract', () => {
  it('finds a typed dependency error through a cause chain', () => {
    const cause = new Error('redis://user:secret@redis:6379 socket closed')
    const unavailable = new DependencyUnavailableError(
      'redisCommand',
      'connection_reset',
      cause,
    )
    const outer = new Error('store failed', { cause: unavailable })

    expect(findDependencyUnavailableError(outer)).toBe(unavailable)
    expect(unavailable.message).toBe(
      'redisCommand unavailable (connection_reset)',
    )
    expect(unavailable.cause).toBe(cause)
  })

  it('returns undefined for cycles and ordinary errors', () => {
    const cyclic = new Error('ordinary') as Error & { cause?: unknown }
    cyclic.cause = cyclic
    expect(findDependencyUnavailableError(cyclic)).toBeUndefined()
    expect(findDependencyUnavailableError('not an error')).toBeUndefined()
  })

  it('keeps observations low-cardinality', () => {
    const observe = vi.fn()
    const observer: DependencyObserver = { observe }
    const observation: DependencyObservation = {
      dependency: 'redisPubSub',
      state: 'reconnecting',
      errorKind: 'connection_closed',
      attempt: 3,
      nextRetryMs: 1750,
    }

    observer.observe(observation)

    expect(observe).toHaveBeenCalledWith(observation)
    expect(JSON.stringify(observation)).not.toContain('redis://')
  })
})
