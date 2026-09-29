import { describe, expect, it } from 'vitest'
import type { Task } from '@taskcast/core'
import { nextCleanupDeadline } from '../src/cleanup-store.js'

const terminal = (rules: NonNullable<Task['cleanup']>['rules']): Task => ({
  id: 'deadline', status: 'completed', createdAt: 0, updatedAt: 1_000,
  completedAt: 1_000, cleanupPolicyVersion: 1, cleanupResolvedAt: 0,
  cleanup: { rules },
})

describe('durable cleanup deadline', () => {
  it('uses the earliest eligible target and advances to all after history expires', () => {
    const task = terminal([
      { target: 'events', trigger: { afterMs: 200 } },
      { target: 'all', trigger: { afterMs: 500 } },
    ])
    expect(nextCleanupDeadline(task)).toBe(1_200)
    expect(nextCleanupDeadline({ ...task, historyExpiredAt: 1_200 })).toBe(1_500)
    expect(nextCleanupDeadline(terminal([{ target: 'events', trigger: { afterMs: 200 } }]))).toBe(1_200)
    expect(nextCleanupDeadline(terminal([{ target: 'all', trigger: { afterMs: 500 } }]))).toBe(1_500)
    expect(nextCleanupDeadline(terminal([]))).toBeNull()
  })
})
