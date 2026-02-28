import { describe, it, expect } from 'vitest'
import { matchesCleanupRule, filterEventsForCleanup } from '../../src/cleanup.js'
import type { Task, TaskEvent, CleanupRule } from '../../src/types.js'

const makeTask = (overrides: Partial<Task> = {}): Task => ({
  id: 'task-1',
  status: 'completed',
  createdAt: 0,
  updatedAt: 1000,
  completedAt: 1000,
  ...overrides,
})

const makeEvent = (overrides: Partial<TaskEvent> = {}): TaskEvent => ({
  id: 'evt-1',
  taskId: 'task-1',
  index: 0,
  timestamp: 500,
  type: 'llm.delta',
  level: 'info',
  data: null,
  ...overrides,
})

describe('matchesCleanupRule', () => {
  const now = 2000

  it('matches when no taskType filter', () => {
    const rule: CleanupRule = { trigger: {}, target: 'all' }
    expect(matchesCleanupRule(makeTask(), rule, now)).toBe(true)
  })

  it('does not match non-terminal task', () => {
    const rule: CleanupRule = { trigger: {}, target: 'all' }
    expect(matchesCleanupRule(makeTask({ status: 'running' }), rule, now)).toBe(false)
  })

  it('matches task type with wildcard', () => {
    const rule: CleanupRule = {
      match: { taskTypes: ['llm.*'] },
      trigger: {},
      target: 'all',
    }
    expect(matchesCleanupRule(makeTask({ type: 'llm.chat' }), rule, now)).toBe(true)
    expect(matchesCleanupRule(makeTask({ type: 'export.pdf' }), rule, now)).toBe(false)
  })

  it('matches specific terminal status', () => {
    const rule: CleanupRule = {
      match: { status: ['completed'] },
      trigger: {},
      target: 'all',
    }
    expect(matchesCleanupRule(makeTask({ status: 'completed' }), rule, now)).toBe(true)
    expect(matchesCleanupRule(makeTask({ status: 'failed' }), rule, now)).toBe(false)
  })

  it('respects afterMs trigger delay', () => {
    const rule: CleanupRule = { trigger: { afterMs: 1500 }, target: 'all' }
    expect(matchesCleanupRule(makeTask({ completedAt: 1000 }), rule, 2000)).toBe(false)
    expect(matchesCleanupRule(makeTask({ completedAt: 1000 }), rule, 2600)).toBe(true)
  })
})

describe('filterEventsForCleanup', () => {
  it('returns all events when no eventFilter', () => {
    const rule: CleanupRule = { trigger: {}, target: 'events' }
    const events = [makeEvent(), makeEvent({ type: 'tool.call' })]
    expect(filterEventsForCleanup(events, rule, 2000)).toHaveLength(2)
  })

  it('filters by type wildcard', () => {
    const rule: CleanupRule = {
      trigger: {},
      target: 'events',
      eventFilter: { types: ['llm.*'] },
    }
    const events = [
      makeEvent({ type: 'llm.delta' }),
      makeEvent({ type: 'tool.call' }),
    ]
    const result = filterEventsForCleanup(events, rule, 2000)
    expect(result).toHaveLength(1)
    expect(result[0]?.type).toBe('llm.delta')
  })

  it('filters by level', () => {
    const rule: CleanupRule = {
      trigger: {},
      target: 'events',
      eventFilter: { levels: ['debug'] },
    }
    const events = [
      makeEvent({ level: 'debug' }),
      makeEvent({ level: 'info' }),
    ]
    const result = filterEventsForCleanup(events, rule, 2000)
    expect(result).toHaveLength(1)
    expect(result[0]?.level).toBe('debug')
  })

  it('filters by olderThanMs relative to task completedAt', () => {
    const rule: CleanupRule = {
      trigger: {},
      target: 'events',
      eventFilter: { olderThanMs: 600 },
    }
    const completedAt = 1000
    const events = [
      makeEvent({ timestamp: 300 }),
      makeEvent({ timestamp: 500 }),
    ]
    // cutoff = completedAt - olderThanMs = 1000 - 600 = 400
    // events with timestamp < 400 get deleted → timestamp 300 qualifies
    const result = filterEventsForCleanup(events, rule, 2000, completedAt)
    expect(result).toHaveLength(1)
    expect(result[0]?.timestamp).toBe(300)
  })
})
