import { describe, it, expect, vi } from 'vitest'
import { TaskEngine } from '../../src/engine.js'
import { MemoryBroadcastProvider, MemoryShortTermStore } from '../../src/memory-adapters.js'
import type { TaskEvent } from '../../src/types.js'

function makeEngine() {
  const store = new MemoryShortTermStore()
  const broadcast = new MemoryBroadcastProvider()
  const engine = new TaskEngine({ shortTermStore: store, broadcast })
  return { engine, store, broadcast }
}

/** Helper: create a task and move it to running */
async function createRunningTask(engine: TaskEngine) {
  const task = await engine.createTask({ ttl: 600 })
  await engine.transitionTask(task.id, 'running')
  return task
}

describe('Suspended-state field management', () => {
  it('sets reason when transitioning to paused', async () => {
    const { engine, store } = makeEngine()
    const task = await createRunningTask(engine)

    const updated = await engine.transitionTask(task.id, 'paused', { reason: 'user requested pause' })

    expect(updated.reason).toBe('user requested pause')
    expect(updated.status).toBe('paused')

    const stored = await store.getTask(task.id)
    expect(stored?.reason).toBe('user requested pause')
  })

  it('sets reason, blockedRequest, and resumeAt when transitioning to blocked', async () => {
    const { engine, store } = makeEngine()
    const task = await createRunningTask(engine)
    const blockedRequest = { type: 'approval', data: { approver: 'admin' } }

    const updated = await engine.transitionTask(task.id, 'blocked', {
      reason: 'needs approval',
      blockedRequest,
      resumeAfterMs: 30000,
    })

    expect(updated.reason).toBe('needs approval')
    expect(updated.blockedRequest).toEqual(blockedRequest)
    expect(updated.resumeAt).toBeGreaterThan(Date.now() - 5000)
    expect(updated.status).toBe('blocked')

    const stored = await store.getTask(task.id)
    expect(stored?.blockedRequest).toEqual(blockedRequest)
    expect(stored?.resumeAt).toBe(updated.resumeAt)
  })

  it('clears reason when transitioning from paused to running', async () => {
    const { engine, store } = makeEngine()
    const task = await createRunningTask(engine)

    await engine.transitionTask(task.id, 'paused', { reason: 'pausing' })
    const resumed = await engine.transitionTask(task.id, 'running')

    expect(resumed.reason).toBeUndefined()
    expect(resumed).not.toHaveProperty('reason')

    const stored = await store.getTask(task.id)
    expect(stored).not.toHaveProperty('reason')
  })

  it('clears reason, blockedRequest, resumeAt when transitioning from blocked to running', async () => {
    const { engine, store } = makeEngine()
    const task = await createRunningTask(engine)

    await engine.transitionTask(task.id, 'blocked', {
      reason: 'waiting for input',
      blockedRequest: { type: 'user-input', data: { prompt: 'Confirm?' } },
      resumeAfterMs: 60000,
    })

    const resumed = await engine.transitionTask(task.id, 'running')

    expect(resumed).not.toHaveProperty('reason')
    expect(resumed).not.toHaveProperty('blockedRequest')
    expect(resumed).not.toHaveProperty('resumeAt')

    const stored = await store.getTask(task.id)
    expect(stored).not.toHaveProperty('reason')
    expect(stored).not.toHaveProperty('blockedRequest')
    expect(stored).not.toHaveProperty('resumeAt')
  })

  it('resumeAfterMs computes correct resumeAt timestamp', async () => {
    const { engine } = makeEngine()
    const task = await createRunningTask(engine)

    const before = Date.now()
    const updated = await engine.transitionTask(task.id, 'blocked', {
      resumeAfterMs: 5000,
    })
    const after = Date.now()

    expect(updated.resumeAt).toBeGreaterThanOrEqual(before + 5000)
    expect(updated.resumeAt).toBeLessThanOrEqual(after + 5000)
  })

  it('non-suspended transitions do not affect reason/blockedRequest fields', async () => {
    const { engine } = makeEngine()
    const task = await engine.createTask({})

    // pending → running: no suspended fields
    const running = await engine.transitionTask(task.id, 'running')
    expect(running).not.toHaveProperty('reason')
    expect(running).not.toHaveProperty('blockedRequest')
    expect(running).not.toHaveProperty('resumeAt')

    // running → completed: no suspended fields
    const completed = await engine.transitionTask(task.id, 'completed', { result: { done: true } })
    expect(completed).not.toHaveProperty('reason')
    expect(completed).not.toHaveProperty('blockedRequest')
    expect(completed).not.toHaveProperty('resumeAt')
  })
})

describe('TTL manipulation for suspended states', () => {
  it('calls clearTTL when transitioning to paused', async () => {
    const { engine, store } = makeEngine()
    const clearTTLSpy = vi.spyOn(store, 'clearTTL')
    const task = await createRunningTask(engine)

    await engine.transitionTask(task.id, 'paused')

    expect(clearTTLSpy).toHaveBeenCalledWith(task.id)
  })

  it('calls setTTL when transitioning from paused to running (resets full TTL)', async () => {
    const { engine, store } = makeEngine()
    const setTTLSpy = vi.spyOn(store, 'setTTL')
    const task = await createRunningTask(engine)

    await engine.transitionTask(task.id, 'paused')
    setTTLSpy.mockClear()

    await engine.transitionTask(task.id, 'running')

    expect(setTTLSpy).toHaveBeenCalledWith(task.id, 600)
  })

  it('calls setTTL when transitioning from paused to blocked (clock resumes)', async () => {
    const { engine, store } = makeEngine()
    const setTTLSpy = vi.spyOn(store, 'setTTL')
    const task = await createRunningTask(engine)

    await engine.transitionTask(task.id, 'paused')
    setTTLSpy.mockClear()

    await engine.transitionTask(task.id, 'blocked')

    expect(setTTLSpy).toHaveBeenCalledWith(task.id, 600)
  })

  it('calls clearTTL when transitioning from blocked to paused', async () => {
    const { engine, store } = makeEngine()
    const clearTTLSpy = vi.spyOn(store, 'clearTTL')
    const task = await createRunningTask(engine)

    await engine.transitionTask(task.id, 'blocked')
    clearTTLSpy.mockClear()

    await engine.transitionTask(task.id, 'paused')

    expect(clearTTLSpy).toHaveBeenCalledWith(task.id)
  })

  it('TTL override via payload.ttl works', async () => {
    const { engine, store } = makeEngine()
    const setTTLSpy = vi.spyOn(store, 'setTTL')
    const task = await createRunningTask(engine)

    // Transition to blocked with a TTL override
    setTTLSpy.mockClear()
    const updated = await engine.transitionTask(task.id, 'blocked', { ttl: 1200 })

    expect(updated.ttl).toBe(1200)
    expect(setTTLSpy).toHaveBeenCalledWith(task.id, 1200)
  })

  it('TTL override to paused does not call setTTL (clock is stopped)', async () => {
    const { engine, store } = makeEngine()
    const setTTLSpy = vi.spyOn(store, 'setTTL')
    const clearTTLSpy = vi.spyOn(store, 'clearTTL')
    const task = await createRunningTask(engine)

    setTTLSpy.mockClear()
    clearTTLSpy.mockClear()

    const updated = await engine.transitionTask(task.id, 'paused', { ttl: 999 })

    expect(updated.ttl).toBe(999)
    // clearTTL should be called (to === 'paused')
    expect(clearTTLSpy).toHaveBeenCalledWith(task.id)
    // setTTL should NOT be called because to === 'paused' suppresses it
    expect(setTTLSpy).not.toHaveBeenCalled()
  })
})

describe('Suspended-state event emissions', () => {
  it('emits taskcast:blocked event when entering blocked with blockedRequest', async () => {
    const { engine, broadcast } = makeEngine()
    const task = await createRunningTask(engine)

    const events: TaskEvent[] = []
    broadcast.subscribe(task.id, (e) => events.push(e))

    const blockedRequest = { type: 'confirmation', data: { message: 'Are you sure?' } }
    await engine.transitionTask(task.id, 'blocked', {
      reason: 'needs confirmation',
      blockedRequest,
    })

    const blockedEvent = events.find((e) => e.type === 'taskcast:blocked')
    expect(blockedEvent).toBeDefined()
    expect(blockedEvent!.data).toEqual({
      reason: 'needs confirmation',
      request: blockedRequest,
    })
  })

  it('does NOT emit taskcast:blocked event when entering blocked without blockedRequest', async () => {
    const { engine, broadcast } = makeEngine()
    const task = await createRunningTask(engine)

    const events: TaskEvent[] = []
    broadcast.subscribe(task.id, (e) => events.push(e))

    await engine.transitionTask(task.id, 'blocked', { reason: 'generic block' })

    const blockedEvent = events.find((e) => e.type === 'taskcast:blocked')
    expect(blockedEvent).toBeUndefined()
  })

  it('emits taskcast:resolved event when transitioning from blocked to running', async () => {
    const { engine, broadcast } = makeEngine()
    const task = await createRunningTask(engine)

    await engine.transitionTask(task.id, 'blocked', {
      blockedRequest: { type: 'user-input', data: { field: 'name' } },
    })

    const events: TaskEvent[] = []
    broadcast.subscribe(task.id, (e) => events.push(e))

    await engine.transitionTask(task.id, 'running', { result: { name: 'Alice' } })

    const resolvedEvent = events.find((e) => e.type === 'taskcast:resolved')
    expect(resolvedEvent).toBeDefined()
    expect(resolvedEvent!.data).toEqual({ resolution: { name: 'Alice' } })
  })

  it('does NOT emit taskcast:resolved when transitioning from blocked to running if no blockedRequest was set', async () => {
    const { engine, broadcast } = makeEngine()
    const task = await createRunningTask(engine)

    // Transition to blocked without blockedRequest
    await engine.transitionTask(task.id, 'blocked')

    const events: TaskEvent[] = []
    broadcast.subscribe(task.id, (e) => events.push(e))

    await engine.transitionTask(task.id, 'running')

    const resolvedEvent = events.find((e) => e.type === 'taskcast:resolved')
    expect(resolvedEvent).toBeUndefined()
  })

  it('emits both taskcast:status and taskcast:blocked events in order', async () => {
    const { engine, broadcast } = makeEngine()
    const task = await createRunningTask(engine)

    const eventTypes: string[] = []
    broadcast.subscribe(task.id, (e) => eventTypes.push(e.type))

    await engine.transitionTask(task.id, 'blocked', {
      blockedRequest: { type: 'auth', data: {} },
    })

    expect(eventTypes).toEqual(['taskcast:status', 'taskcast:blocked'])
  })

  it('emits both taskcast:status and taskcast:resolved events in order when resolving', async () => {
    const { engine, broadcast } = makeEngine()
    const task = await createRunningTask(engine)

    await engine.transitionTask(task.id, 'blocked', {
      blockedRequest: { type: 'auth', data: {} },
    })

    const eventTypes: string[] = []
    broadcast.subscribe(task.id, (e) => eventTypes.push(e.type))

    await engine.transitionTask(task.id, 'running')

    expect(eventTypes).toEqual(['taskcast:status', 'taskcast:resolved'])
  })
})
