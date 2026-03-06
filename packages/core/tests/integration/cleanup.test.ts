import { describe, it, expect } from 'vitest'
import { TaskEngine } from '../../src/engine.js'
import { MemoryBroadcastProvider, MemoryShortTermStore } from '../../src/memory-adapters.js'
import { matchesCleanupRule, filterEventsForCleanup } from '../../src/cleanup.js'
import type { CleanupRule } from '../../src/types.js'

function makeEngine() {
  const store = new MemoryShortTermStore()
  const broadcast = new MemoryBroadcastProvider()
  const engine = new TaskEngine({ shortTermStore: store, broadcast })
  return { engine, store, broadcast }
}

describe('Core integration — cleanup rules', () => {
  it('matchesCleanupRule returns true for terminal task after delay', async () => {
    const { engine } = makeEngine()
    const task = await engine.createTask({ type: 'llm.chat' })
    await engine.transitionTask(task.id, 'running')
    const completed = await engine.transitionTask(task.id, 'completed')

    const rule: CleanupRule = {
      trigger: { afterMs: 1000 },
    }

    // Not enough time passed
    expect(matchesCleanupRule(completed, rule, completed.completedAt! + 500)).toBe(false)

    // Enough time passed
    expect(matchesCleanupRule(completed, rule, completed.completedAt! + 1500)).toBe(true)
  })

  it('matchesCleanupRule respects status filter', async () => {
    const { engine } = makeEngine()
    const task = await engine.createTask({ type: 'test' })
    await engine.transitionTask(task.id, 'running')
    const failed = await engine.transitionTask(task.id, 'failed', {
      error: { message: 'boom' },
    })

    const completedOnly: CleanupRule = {
      trigger: { afterMs: 0 },
      match: { status: ['completed'] },
    }

    const failedOnly: CleanupRule = {
      trigger: { afterMs: 0 },
      match: { status: ['failed'] },
    }

    expect(matchesCleanupRule(failed, completedOnly, Date.now())).toBe(false)
    expect(matchesCleanupRule(failed, failedOnly, Date.now())).toBe(true)
  })

  it('matchesCleanupRule respects taskTypes filter', async () => {
    const { engine } = makeEngine()
    const task = await engine.createTask({ type: 'llm.chat' })
    await engine.transitionTask(task.id, 'running')
    const completed = await engine.transitionTask(task.id, 'completed')

    const llmOnly: CleanupRule = {
      trigger: { afterMs: 0 },
      match: { taskTypes: ['llm.*'] },
    }

    const toolOnly: CleanupRule = {
      trigger: { afterMs: 0 },
      match: { taskTypes: ['tool.*'] },
    }

    expect(matchesCleanupRule(completed, llmOnly, Date.now())).toBe(true)
    expect(matchesCleanupRule(completed, toolOnly, Date.now())).toBe(false)
  })

  it('non-terminal task never matches cleanup', async () => {
    const { engine } = makeEngine()
    const task = await engine.createTask({ type: 'test' })
    await engine.transitionTask(task.id, 'running')
    const running = (await engine.getTask(task.id))!

    const rule: CleanupRule = { trigger: { afterMs: 0 } }
    expect(matchesCleanupRule(running, rule, Date.now())).toBe(false)
  })

  it('filterEventsForCleanup filters by type pattern', async () => {
    const { engine } = makeEngine()
    const task = await engine.createTask({})
    await engine.transitionTask(task.id, 'running')
    await engine.publishEvent(task.id, { type: 'llm.delta', level: 'info', data: null })
    await engine.publishEvent(task.id, { type: 'tool.call', level: 'info', data: null })
    await engine.publishEvent(task.id, { type: 'llm.done', level: 'info', data: null })

    const events = await engine.getEvents(task.id)
    const rule: CleanupRule = {
      trigger: { afterMs: 0 },
      eventFilter: { types: ['llm.*'] },
    }

    const filtered = filterEventsForCleanup(events, rule, Date.now())
    expect(filtered.every(e => e.type.startsWith('llm.'))).toBe(true)
    expect(filtered).toHaveLength(2)
  })

  it('filterEventsForCleanup filters by level', async () => {
    const { engine } = makeEngine()
    const task = await engine.createTask({})
    await engine.transitionTask(task.id, 'running')
    await engine.publishEvent(task.id, { type: 'a', level: 'debug', data: null })
    await engine.publishEvent(task.id, { type: 'b', level: 'info', data: null })
    await engine.publishEvent(task.id, { type: 'c', level: 'error', data: null })

    const events = await engine.getEvents(task.id)
    const rule: CleanupRule = {
      trigger: { afterMs: 0 },
      eventFilter: { levels: ['debug'] },
    }

    const filtered = filterEventsForCleanup(events, rule, Date.now())
    expect(filtered).toHaveLength(1)
    expect(filtered[0]!.level).toBe('debug')
  })
})
