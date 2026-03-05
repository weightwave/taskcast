import { describe, it, expect } from 'vitest'
import { TaskEngine } from '../../src/engine.js'
import { MemoryBroadcastProvider, MemoryShortTermStore } from '../../src/memory-adapters.js'

function makeEngine() {
  const shortTerm = new MemoryShortTermStore()
  const broadcast = new MemoryBroadcastProvider()
  const engine = new TaskEngine({ shortTerm, broadcast })
  return { engine, shortTerm, broadcast }
}

describe('Suspended state lifecycle – integration', () => {
  it('full lifecycle: pending → running → paused → running → blocked → running → completed', async () => {
    const { engine } = makeEngine()
    const task = await engine.createTask({})
    expect(task.status).toBe('pending')

    const r1 = await engine.transitionTask(task.id, 'running')
    expect(r1.status).toBe('running')

    const p = await engine.transitionTask(task.id, 'paused', { reason: 'User break' })
    expect(p.status).toBe('paused')
    expect(p.reason).toBe('User break')

    const r2 = await engine.transitionTask(task.id, 'running')
    expect(r2.status).toBe('running')
    expect(r2.reason).toBeUndefined()

    const b = await engine.transitionTask(task.id, 'blocked', {
      reason: 'Waiting approval',
      blockedRequest: { type: 'approval', data: { q: 'Deploy?' } },
      resumeAfterMs: 60000,
    })
    expect(b.status).toBe('blocked')
    expect(b.reason).toBe('Waiting approval')
    expect(b.blockedRequest).toBeDefined()
    expect(b.resumeAt).toBeDefined()

    const r3 = await engine.transitionTask(task.id, 'running')
    expect(r3.status).toBe('running')
    expect(r3.reason).toBeUndefined()
    expect(r3.blockedRequest).toBeUndefined()
    expect(r3.resumeAt).toBeUndefined()

    const c = await engine.transitionTask(task.id, 'completed', { result: { answer: 42 } })
    expect(c.status).toBe('completed')
    expect(c.result).toEqual({ answer: 42 })
  })

  it('blocked with resolve flow: running → blocked → running via scheduler', async () => {
    const { engine, shortTerm } = makeEngine()
    const task = await engine.createTask({})
    await engine.transitionTask(task.id, 'running')
    await engine.transitionTask(task.id, 'blocked', { resumeAfterMs: 1 })

    await new Promise((r) => setTimeout(r, 10))

    const { TaskScheduler } = await import('../../src/scheduler.js')
    const scheduler = new TaskScheduler({ engine, shortTerm, checkIntervalMs: 100 })
    await scheduler.tick()

    const updated = await engine.getTask(task.id)
    expect(updated!.status).toBe('running')
  })

  it('events are published during suspended states', async () => {
    const { engine } = makeEngine()
    const task = await engine.createTask({})
    await engine.transitionTask(task.id, 'running')
    await engine.transitionTask(task.id, 'paused')

    await engine.publishEvent(task.id, { type: 'log', level: 'info', data: { msg: 'paused log' } })

    await engine.transitionTask(task.id, 'running')
    await engine.transitionTask(task.id, 'blocked')

    await engine.publishEvent(task.id, { type: 'log', level: 'info', data: { msg: 'blocked log' } })

    const events = await engine.getEvents(task.id)
    const logEvents = events.filter((e) => e.type === 'log')
    expect(logEvents).toHaveLength(2)
  })

  it('concurrent pause/resume transitions – only valid ones succeed', async () => {
    const { engine } = makeEngine()
    const task = await engine.createTask({})
    await engine.transitionTask(task.id, 'running')

    // 10 concurrent attempts to pause
    const results = await Promise.allSettled(
      Array.from({ length: 10 }, () => engine.transitionTask(task.id, 'paused')),
    )
    const successes = results.filter((r) => r.status === 'fulfilled')
    // At least 1 should succeed (first one wins)
    expect(successes.length).toBeGreaterThanOrEqual(1)

    const final = await engine.getTask(task.id)
    expect(final!.status).toBe('paused')
  })

  it('paused → blocked → paused → cancelled lifecycle', async () => {
    const { engine } = makeEngine()
    const task = await engine.createTask({})
    await engine.transitionTask(task.id, 'running')
    await engine.transitionTask(task.id, 'paused')
    await engine.transitionTask(task.id, 'blocked', { reason: 'External dep' })
    await engine.transitionTask(task.id, 'paused', { reason: 'User pause' })
    const cancelled = await engine.transitionTask(task.id, 'cancelled')
    expect(cancelled.status).toBe('cancelled')
  })
})
