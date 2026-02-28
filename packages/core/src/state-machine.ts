import type { TaskStatus } from './types.js'

export const TERMINAL_STATUSES: readonly TaskStatus[] = [
  'completed',
  'failed',
  'timeout',
  'cancelled',
] as const

const ALLOWED_TRANSITIONS: Record<TaskStatus, TaskStatus[]> = {
  pending: ['running', 'cancelled'],
  running: ['completed', 'failed', 'timeout', 'cancelled'],
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
