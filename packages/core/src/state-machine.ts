import type { TaskStatus } from './types.js'

export const TERMINAL_STATUSES: readonly TaskStatus[] = [
  'completed',
  'failed',
  'timeout',
  'cancelled',
] as const

export const SUSPENDED_STATUSES: readonly TaskStatus[] = [
  'paused',
  'blocked',
] as const

const ALLOWED_TRANSITIONS: Record<TaskStatus, TaskStatus[]> = {
  pending: ['running', 'cancelled'],
  running: ['paused', 'blocked', 'completed', 'failed', 'timeout', 'cancelled'],
  paused: ['running', 'blocked', 'cancelled'],
  blocked: ['running', 'paused', 'cancelled', 'failed'],
  completed: [],
  failed: [],
  timeout: [],
  cancelled: [],
}

export function canTransition(from: TaskStatus, to: TaskStatus): boolean {
  if (from === to) return false
  /* v8 ignore next -- ALLOWED_TRANSITIONS covers all TaskStatus values; ?. and ?? false are unreachable */
  return ALLOWED_TRANSITIONS[from]?.includes(to) ?? false
}

export function applyTransition(from: TaskStatus, to: TaskStatus): TaskStatus {
  if (!canTransition(from, to)) {
    throw new Error(`Invalid transition: ${from} → ${to}`)
  }
  return to
}

export function isTerminal(status: TaskStatus): boolean {
  return TERMINAL_STATUSES.includes(status)
}

export function isSuspended(status: TaskStatus): boolean {
  return SUSPENDED_STATUSES.includes(status)
}