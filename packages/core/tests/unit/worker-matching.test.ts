import { describe, it, expect } from 'vitest'
import { matchesTag, matchesWorkerRule } from '../../src/worker-matching.js'
import type { Task, TagMatcher, WorkerMatchRule } from '../../src/types.js'

function makeTask(overrides: Partial<Task> = {}): Task {
  return {
    id: 'task-1',
    status: 'pending',
    createdAt: Date.now(),
    updatedAt: Date.now(),
    ...overrides,
  }
}

// ─── matchesTag ─────────────────────────────────────────────────────────────

describe('matchesTag', () => {
  describe('all', () => {
    it('matches when all required tags are present', () => {
      expect(matchesTag(['gpu', 'high-priority', 'us-east'], { all: ['gpu', 'high-priority'] })).toBe(true)
    })

    it('rejects when some required tags are missing', () => {
      expect(matchesTag(['gpu'], { all: ['gpu', 'high-priority'] })).toBe(false)
    })

    it('rejects when none of the required tags are present', () => {
      expect(matchesTag(['cpu'], { all: ['gpu', 'high-priority'] })).toBe(false)
    })

    it('treats empty all array as vacuous true', () => {
      expect(matchesTag(['gpu'], { all: [] })).toBe(true)
    })

    it('treats empty all array with no tags as vacuous true', () => {
      expect(matchesTag(undefined, { all: [] })).toBe(true)
    })
  })

  describe('any', () => {
    it('matches when at least one tag is present', () => {
      expect(matchesTag(['gpu', 'us-east'], { any: ['gpu', 'high-priority'] })).toBe(true)
    })

    it('rejects when no tags match', () => {
      expect(matchesTag(['cpu', 'us-west'], { any: ['gpu', 'high-priority'] })).toBe(false)
    })

    it('treats empty any array as vacuous true', () => {
      expect(matchesTag(['cpu'], { any: [] })).toBe(true)
    })

    it('treats empty any array with no tags as vacuous true', () => {
      expect(matchesTag(undefined, { any: [] })).toBe(true)
    })
  })

  describe('none', () => {
    it('matches when none of the excluded tags are present', () => {
      expect(matchesTag(['gpu', 'us-east'], { none: ['high-priority', 'slow'] })).toBe(true)
    })

    it('rejects when an excluded tag is present', () => {
      expect(matchesTag(['gpu', 'slow'], { none: ['slow'] })).toBe(false)
    })

    it('treats empty none array as vacuous true', () => {
      expect(matchesTag(['slow'], { none: [] })).toBe(true)
    })

    it('treats empty none array with no tags as vacuous true', () => {
      expect(matchesTag(undefined, { none: [] })).toBe(true)
    })
  })

  describe('combined', () => {
    it('matches when all conditions are satisfied (all + any + none)', () => {
      expect(matchesTag(
        ['gpu', 'high-priority', 'us-east'],
        { all: ['gpu'], any: ['us-east', 'us-west'], none: ['deprecated'] },
      )).toBe(true)
    })

    it('rejects when all fails but any and none pass', () => {
      expect(matchesTag(
        ['high-priority', 'us-east'],
        { all: ['gpu'], any: ['us-east'], none: ['deprecated'] },
      )).toBe(false)
    })

    it('rejects when any fails but all and none pass', () => {
      expect(matchesTag(
        ['gpu'],
        { all: ['gpu'], any: ['us-east', 'us-west'], none: ['deprecated'] },
      )).toBe(false)
    })

    it('rejects when none fails but all and any pass', () => {
      expect(matchesTag(
        ['gpu', 'us-east', 'deprecated'],
        { all: ['gpu'], any: ['us-east'], none: ['deprecated'] },
      )).toBe(false)
    })

    it('matches all + any combined', () => {
      expect(matchesTag(
        ['gpu', 'high-priority'],
        { all: ['gpu'], any: ['high-priority', 'low-priority'] },
      )).toBe(true)
    })
  })

  describe('edge cases', () => {
    it('empty matcher matches everything', () => {
      expect(matchesTag(['gpu'], {})).toBe(true)
    })

    it('empty matcher matches undefined tags', () => {
      expect(matchesTag(undefined, {})).toBe(true)
    })

    it('empty matcher matches empty tags', () => {
      expect(matchesTag([], {})).toBe(true)
    })

    it('undefined taskTags treated as empty array', () => {
      expect(matchesTag(undefined, { all: ['gpu'] })).toBe(false)
    })

    it('undefined taskTags with none matcher passes when none is non-empty', () => {
      expect(matchesTag(undefined, { none: ['gpu'] })).toBe(true)
    })
  })
})

// ─── matchesWorkerRule ──────────────────────────────────────────────────────

describe('matchesWorkerRule', () => {
  describe('taskTypes', () => {
    it('matches exact task type', () => {
      const task = makeTask({ type: 'llm.chat' })
      expect(matchesWorkerRule(task, { taskTypes: ['llm.chat'] })).toBe(true)
    })

    it('rejects non-matching task type', () => {
      const task = makeTask({ type: 'llm.chat' })
      expect(matchesWorkerRule(task, { taskTypes: ['image.generate'] })).toBe(false)
    })

    it('matches with wildcard pattern', () => {
      const task = makeTask({ type: 'llm.chat' })
      expect(matchesWorkerRule(task, { taskTypes: ['llm.*'] })).toBe(true)
    })

    it('matches with global wildcard', () => {
      const task = makeTask({ type: 'anything' })
      expect(matchesWorkerRule(task, { taskTypes: ['*'] })).toBe(true)
    })

    it('wildcard llm.* does not match bare llm', () => {
      const task = makeTask({ type: 'llm' })
      expect(matchesWorkerRule(task, { taskTypes: ['llm.*'] })).toBe(false)
    })

    it('matches when one of multiple taskTypes matches', () => {
      const task = makeTask({ type: 'image.generate' })
      expect(matchesWorkerRule(task, { taskTypes: ['llm.*', 'image.*'] })).toBe(true)
    })

    it('task with no type does not match if rule has taskTypes', () => {
      const task = makeTask({})
      expect(matchesWorkerRule(task, { taskTypes: ['llm.*'] })).toBe(false)
    })

    it('task with no type does not match even with global wildcard', () => {
      const task = makeTask({})
      expect(matchesWorkerRule(task, { taskTypes: ['*'] })).toBe(false)
    })
  })

  describe('tags', () => {
    it('matches when tags match', () => {
      const task = makeTask({ tags: ['gpu', 'high-priority'] })
      expect(matchesWorkerRule(task, { tags: { all: ['gpu'] } })).toBe(true)
    })

    it('rejects when tags do not match', () => {
      const task = makeTask({ tags: ['cpu'] })
      expect(matchesWorkerRule(task, { tags: { all: ['gpu'] } })).toBe(false)
    })

    it('matches with empty tag matcher', () => {
      const task = makeTask({ tags: ['cpu'] })
      expect(matchesWorkerRule(task, { tags: {} })).toBe(true)
    })
  })

  describe('combined taskTypes + tags', () => {
    it('matches when both conditions are satisfied', () => {
      const task = makeTask({ type: 'llm.chat', tags: ['gpu'] })
      expect(matchesWorkerRule(task, { taskTypes: ['llm.*'], tags: { all: ['gpu'] } })).toBe(true)
    })

    it('rejects when taskTypes matches but tags do not', () => {
      const task = makeTask({ type: 'llm.chat', tags: ['cpu'] })
      expect(matchesWorkerRule(task, { taskTypes: ['llm.*'], tags: { all: ['gpu'] } })).toBe(false)
    })

    it('rejects when tags match but taskTypes do not', () => {
      const task = makeTask({ type: 'image.generate', tags: ['gpu'] })
      expect(matchesWorkerRule(task, { taskTypes: ['llm.*'], tags: { all: ['gpu'] } })).toBe(false)
    })
  })

  describe('edge cases', () => {
    it('empty rule matches everything', () => {
      const task = makeTask({ type: 'llm.chat', tags: ['gpu'] })
      expect(matchesWorkerRule(task, {})).toBe(true)
    })

    it('empty rule matches task with no type or tags', () => {
      const task = makeTask({})
      expect(matchesWorkerRule(task, {})).toBe(true)
    })

    it('empty taskTypes array matches nothing (matchesType behavior)', () => {
      const task = makeTask({ type: 'llm.chat' })
      expect(matchesWorkerRule(task, { taskTypes: [] })).toBe(true)
    })
  })
})
