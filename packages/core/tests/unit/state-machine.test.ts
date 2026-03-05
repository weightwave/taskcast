import { describe, it, expect } from 'vitest'
import { canTransition, applyTransition, TERMINAL_STATUSES, isTerminal } from '../../src/state-machine.js'

describe('canTransition', () => {
  it('allows pending → running', () => {
    expect(canTransition('pending', 'running')).toBe(true)
  })
  it('allows pending → assigned', () => {
    expect(canTransition('pending', 'assigned')).toBe(true)
  })
  it('allows assigned → running', () => {
    expect(canTransition('assigned', 'running')).toBe(true)
  })
  it('allows assigned → pending (decline)', () => {
    expect(canTransition('assigned', 'pending')).toBe(true)
  })
  it('allows assigned → cancelled', () => {
    expect(canTransition('assigned', 'cancelled')).toBe(true)
  })
  it('rejects assigned → completed', () => {
    expect(canTransition('assigned', 'completed')).toBe(false)
  })
  it('rejects assigned → failed', () => {
    expect(canTransition('assigned', 'failed')).toBe(false)
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
    expect(TERMINAL_STATUSES).not.toContain('assigned')
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
  it('returns assigned on pending → assigned', () => {
    expect(applyTransition('pending', 'assigned')).toBe('assigned')
  })
  it('returns running on assigned → running', () => {
    expect(applyTransition('assigned', 'running')).toBe('running')
  })
  it('returns pending on assigned → pending (decline)', () => {
    expect(applyTransition('assigned', 'pending')).toBe('pending')
  })
  it('throws on assigned → completed (invalid)', () => {
    expect(() => applyTransition('assigned', 'completed')).toThrowError(/invalid transition/i)
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
    expect(isTerminal('assigned')).toBe(false)
    expect(isTerminal('running')).toBe(false)
  })
})
