import { describe, it, expect } from 'vitest'
import { canTransition, applyTransition, TERMINAL_STATUSES, isTerminal } from '../../src/state-machine.js'

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
