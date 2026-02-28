import { describe, it, expect } from 'vitest'
import { matchesType, matchesFilter, applyFilteredIndex } from '../../src/filter.js'
import type { TaskEvent, SubscribeFilter } from '../../src/types.js'

const makeEvent = (overrides: Partial<TaskEvent> = {}): TaskEvent => ({
  id: 'test-id',
  taskId: 'task-1',
  index: 0,
  timestamp: 1000,
  type: 'llm.delta',
  level: 'info',
  data: null,
  ...overrides,
})

describe('matchesType', () => {
  it('matches exact type', () => {
    expect(matchesType('llm.delta', ['llm.delta'])).toBe(true)
  })
  it('matches wildcard prefix', () => {
    expect(matchesType('llm.delta', ['llm.*'])).toBe(true)
  })
  it('matches global wildcard', () => {
    expect(matchesType('anything', ['*'])).toBe(true)
  })
  it('does not match unrelated type', () => {
    expect(matchesType('tool.call', ['llm.*'])).toBe(false)
  })
  it('matches any pattern in array', () => {
    expect(matchesType('tool.call', ['llm.*', 'tool.*'])).toBe(true)
  })
  it('empty patterns array matches nothing', () => {
    expect(matchesType('llm.delta', [])).toBe(false)
  })
  it('undefined patterns matches everything', () => {
    expect(matchesType('llm.delta', undefined)).toBe(true)
  })
  it('wildcard does not match prefix alone without dot', () => {
    expect(matchesType('llm', ['llm.*'])).toBe(false)
  })
  it('wildcard matches exact prefix with dot', () => {
    expect(matchesType('llm.delta.chunk', ['llm.*'])).toBe(true)
  })
})

describe('matchesFilter', () => {
  it('passes event with no filter', () => {
    expect(matchesFilter(makeEvent(), {})).toBe(true)
  })
  it('filters by level', () => {
    expect(matchesFilter(makeEvent({ level: 'debug' }), { levels: ['info', 'warn'] })).toBe(false)
    expect(matchesFilter(makeEvent({ level: 'info' }), { levels: ['info', 'warn'] })).toBe(true)
  })
  it('filters taskcast:status when includeStatus=false', () => {
    const statusEvent = makeEvent({ type: 'taskcast:status' })
    expect(matchesFilter(statusEvent, { includeStatus: false })).toBe(false)
    expect(matchesFilter(statusEvent, { includeStatus: true })).toBe(true)
    expect(matchesFilter(statusEvent, {})).toBe(true)
  })
  it('filters by type with wildcard', () => {
    expect(matchesFilter(makeEvent({ type: 'llm.delta' }), { types: ['tool.*'] })).toBe(false)
    expect(matchesFilter(makeEvent({ type: 'tool.call' }), { types: ['tool.*'] })).toBe(true)
  })
})

describe('applyFilteredIndex', () => {
  it('assigns sequential filteredIndex to matching events', () => {
    const events = [
      makeEvent({ type: 'llm.delta', index: 0 }),
      makeEvent({ type: 'tool.call', index: 1 }),
      makeEvent({ type: 'llm.delta', index: 2 }),
      makeEvent({ type: 'llm.delta', index: 3 }),
    ]
    const result = applyFilteredIndex(events, { types: ['llm.*'] })
    expect(result).toHaveLength(3)
    expect(result[0]?.filteredIndex).toBe(0)
    expect(result[1]?.filteredIndex).toBe(1)
    expect(result[2]?.filteredIndex).toBe(2)
  })

  it('respects since.index (skips events where filteredIndex <= since.index)', () => {
    const events = [
      makeEvent({ type: 'llm.delta', index: 0 }),
      makeEvent({ type: 'llm.delta', index: 1 }),
      makeEvent({ type: 'llm.delta', index: 2 }),
    ]
    const result = applyFilteredIndex(events, { types: ['llm.*'], since: { index: 1 } })
    // since.index=1 → skip filteredIndex 0 and 1, return filteredIndex 2
    expect(result).toHaveLength(1)
    expect(result[0]?.filteredIndex).toBe(2)
  })

  it('preserves rawIndex', () => {
    const events = [
      makeEvent({ type: 'tool.call', index: 5 }),
      makeEvent({ type: 'llm.delta', index: 6 }),
    ]
    const result = applyFilteredIndex(events, { types: ['llm.*'] })
    expect(result[0]?.rawIndex).toBe(6)
    expect(result[0]?.filteredIndex).toBe(0)
  })

  it('returns empty array when no events match filter', () => {
    const events = [makeEvent({ type: 'tool.call', index: 0 })]
    const result = applyFilteredIndex(events, { types: ['llm.*'] })
    expect(result).toHaveLength(0)
  })

  it('since.index=0 skips only filteredIndex 0', () => {
    const events = [
      makeEvent({ type: 'llm.delta', index: 0 }),
      makeEvent({ type: 'llm.delta', index: 1 }),
    ]
    const result = applyFilteredIndex(events, { since: { index: 0 } })
    expect(result).toHaveLength(1)
    expect(result[0]?.filteredIndex).toBe(1)
  })
})
