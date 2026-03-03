import { describe, it, expect } from 'vitest'
import { canTransition, applyTransition, TERMINAL_STATUSES, isTerminal, isSuspended, SUSPENDED_STATUSES } from '../../src/state-machine.js'

describe('canTransition', () => {
  it('allows pending → running', () => {
    expect(canTransition('pending', 'running')).toBe(true)
  })
  it('allows running → completed', () => {
    expect(canTransition('running', 'completed')).toBe(true)
  })
  it('allows running → failed', () => {
    expect(canTransition('running', 'failed')).toBe(true)
  })
  it('allows running → timeout', () => {
    expect(canTransition('running', 'timeout')).toBe(true)
  })
  it('allows pending → cancelled', () => {
    expect(canTransition('pending', 'cancelled')).toBe(true)
  })
  it('allows running → cancelled', () => {
    expect(canTransition('running', 'cancelled')).toBe(true)
  })
  it('rejects completed → running (terminal state)', () => {
    expect(canTransition('completed', 'running')).toBe(false)
  })
  it('rejects failed → running (terminal state)', () => {
    expect(canTransition('failed', 'running')).toBe(false)
  })
  it('rejects pending → completed (must go through running)', () => {
    expect(canTransition('pending', 'completed')).toBe(false)
  })
  it('rejects same-state transition', () => {
    expect(canTransition('running', 'running')).toBe(false)
  })
})

describe('TERMINAL_STATUSES', () => {
  it('includes completed, failed, timeout, cancelled', () => {
    expect(TERMINAL_STATUSES).toContain('completed')
    expect(TERMINAL_STATUSES).toContain('failed')
    expect(TERMINAL_STATUSES).toContain('timeout')
    expect(TERMINAL_STATUSES).toContain('cancelled')
    expect(TERMINAL_STATUSES).not.toContain('pending')
    expect(TERMINAL_STATUSES).not.toContain('running')
  })
})

describe('applyTransition', () => {
  it('throws on invalid transition', () => {
    expect(() => applyTransition('completed', 'running')).toThrowError(/invalid transition/i)
  })
  it('returns new status on valid transition', () => {
    expect(applyTransition('pending', 'running')).toBe('running')
  })
})

describe('isTerminal', () => {
  it('returns true for terminal statuses', () => {
    expect(isTerminal('completed')).toBe(true)
    expect(isTerminal('failed')).toBe(true)
    expect(isTerminal('timeout')).toBe(true)
    expect(isTerminal('cancelled')).toBe(true)
  })
  it('returns false for non-terminal statuses', () => {
    expect(isTerminal('pending')).toBe(false)
    expect(isTerminal('running')).toBe(false)
  })
})

describe('SUSPENDED_STATUSES', () => {
  it('contains paused and blocked', () => {
    expect(SUSPENDED_STATUSES).toEqual(['paused', 'blocked'])
  })
})

describe('isSuspended', () => {
  it('returns true for paused', () => expect(isSuspended('paused')).toBe(true))
  it('returns true for blocked', () => expect(isSuspended('blocked')).toBe(true))
  it('returns false for running', () => expect(isSuspended('running')).toBe(false))
  it('returns false for terminal', () => expect(isSuspended('completed')).toBe(false))
})

describe('canTransition – suspended states', () => {
  // running → suspended
  it('allows running → paused', () => expect(canTransition('running', 'paused')).toBe(true))
  it('allows running → blocked', () => expect(canTransition('running', 'blocked')).toBe(true))

  // paused exits
  it('allows paused → running', () => expect(canTransition('paused', 'running')).toBe(true))
  it('allows paused → blocked', () => expect(canTransition('paused', 'blocked')).toBe(true))
  it('allows paused → cancelled', () => expect(canTransition('paused', 'cancelled')).toBe(true))
  it('rejects paused → completed', () => expect(canTransition('paused', 'completed')).toBe(false))
  it('rejects paused → failed', () => expect(canTransition('paused', 'failed')).toBe(false))

  // blocked exits
  it('allows blocked → running', () => expect(canTransition('blocked', 'running')).toBe(true))
  it('allows blocked → paused', () => expect(canTransition('blocked', 'paused')).toBe(true))
  it('allows blocked → cancelled', () => expect(canTransition('blocked', 'cancelled')).toBe(true))
  it('allows blocked → failed', () => expect(canTransition('blocked', 'failed')).toBe(true))
  it('rejects blocked → completed', () => expect(canTransition('blocked', 'completed')).toBe(false))

  // pending → suspended: not allowed
  it('rejects pending → paused', () => expect(canTransition('pending', 'paused')).toBe(false))
  it('rejects pending → blocked', () => expect(canTransition('pending', 'blocked')).toBe(false))

  // suspended are not terminal
  it('paused is not terminal', () => expect(isTerminal('paused')).toBe(false))
  it('blocked is not terminal', () => expect(isTerminal('blocked')).toBe(false))
})
