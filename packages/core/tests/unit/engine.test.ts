import { describe, it, expect, vi } from 'vitest'
import { TaskEngine, TaskConflictError, InvalidTransitionError } from '../../src/engine.js'
import { MemoryBroadcastProvider, MemoryShortTermStore } from '../../src/memory-adapters.js'
import type { LongTermStore } from '../../src/types.js'

function makeEngine() {
  const store = new MemoryShortTermStore()
  const broadcast = new MemoryBroadcastProvider()
  const engine = new TaskEngine({ shortTermStore: store, broadcast })
  return { engine, store, broadcast }
}

function makeLongTermStore(overrides: Partial<LongTermStore> = {}): LongTermStore {
  return {
    saveTask: vi.fn().mockResolvedValue(undefined),
    getTask: vi.fn().mockResolvedValue(null),
    saveEvent: vi.fn().mockResolvedValue(undefined),
    getEvents: vi.fn().mockResolvedValue([]),
    ...overrides,
  }
}

describe('TaskEngine.createTask', () => {
  it('creates a task with pending status', async () => {
    const { engine } = makeEngine()
    const task = await engine.createTask({ params: { prompt: 'hi' } })
    expect(task.status).toBe('pending')
    expect(task.params).toEqual({ prompt: 'hi' })
    expect(task.id).toBeTruthy()
    expect(task.createdAt).toBeGreaterThan(0)
  })

  it('creates a task with user-supplied id', async () => {
    const { engine } = makeEngine()
    const task = await engine.createTask({ id: 'my-task-id' })
    expect(task.id).toBe('my-task-id')
  })

  it('creates a task with all optional fields including authConfig', async () => {
    const { engine } = makeEngine()
    const authConfig = { token: 'secret' }
    const task = await engine.createTask({
      type: 'test',
      params: { key: 'val' },
      metadata: { source: 'unit' },
      webhooks: [],
      cleanup: { rules: [] },
      authConfig,
    })
    expect(task.authConfig).toEqual(authConfig)
    expect(task.type).toBe('test')
  })

  it('saves to longTermStore when configured', async () => {
    const store = new MemoryShortTermStore()
    const broadcast = new MemoryBroadcastProvider()
    const longTermStore = makeLongTermStore()
    const engine = new TaskEngine({ shortTermStore: store, broadcast, longTermStore: longTermStore })
    const task = await engine.createTask({ type: 'test' })
    expect(longTermStore.saveTask).toHaveBeenCalledWith(expect.objectContaining({ id: task.id }))
  })

  it('calls setTTL on shortTermStore when ttl is provided', async () => {
    const store = new MemoryShortTermStore()
    const broadcast = new MemoryBroadcastProvider()
    const setTTLSpy = vi.spyOn(store, 'setTTL')
    const engine = new TaskEngine({ shortTermStore: store, broadcast })
    const task = await engine.createTask({ ttl: 300 })
    expect(setTTLSpy).toHaveBeenCalledWith(task.id, 300)
  })

  it('persists tags, assignMode, cost, disconnectPolicy', async () => {
    const { engine, store } = makeEngine()
    const task = await engine.createTask({
      tags: ['gpu', 'high-priority'],
      assignMode: 'pull',
      cost: 3,
      disconnectPolicy: 'reassign',
    })
    expect(task.tags).toEqual(['gpu', 'high-priority'])
    expect(task.assignMode).toBe('pull')
    expect(task.cost).toBe(3)
    expect(task.disconnectPolicy).toBe('reassign')

    const stored = await store.getTask(task.id)
    expect(stored?.tags).toEqual(['gpu', 'high-priority'])
    expect(stored?.assignMode).toBe('pull')
    expect(stored?.cost).toBe(3)
    expect(stored?.disconnectPolicy).toBe('reassign')
  })

  it('omits undefined worker fields', async () => {
    const { engine } = makeEngine()
    const task = await engine.createTask({ type: 'test' })
    expect(task).not.toHaveProperty('tags')
    expect(task).not.toHaveProperty('assignMode')
    expect(task).not.toHaveProperty('cost')
    expect(task).not.toHaveProperty('disconnectPolicy')
  })

  it('calls onTaskCreated hook after task creation', async () => {
    const onTaskCreated = vi.fn()
    const store = new MemoryShortTermStore()
    const broadcast = new MemoryBroadcastProvider()
    const engine = new TaskEngine({ shortTermStore: store, broadcast, hooks: { onTaskCreated } })
    const task = await engine.createTask({ type: 'test' })
    expect(onTaskCreated).toHaveBeenCalledOnce()
    expect(onTaskCreated).toHaveBeenCalledWith(expect.objectContaining({ id: task.id, status: 'pending' }))
  })

  it('rejects duplicate explicit ID with TaskConflictError', async () => {
    const { engine } = makeEngine()
    await engine.createTask({ id: 'dup-test' })
    await expect(engine.createTask({ id: 'dup-test' })).rejects.toThrow(TaskConflictError)
    await expect(engine.createTask({ id: 'dup-test' })).rejects.toThrow('already exists')
  })

  it('allows auto-generated IDs without conflict', async () => {
    const { engine } = makeEngine()
    const t1 = await engine.createTask({})
    const t2 = await engine.createTask({})
    expect(t1.id).not.toBe(t2.id)
  })
})

describe('TaskEngine.transitionTask', () => {
  it('transitions pending → running and saves task', async () => {
    const { engine, store } = makeEngine()
    const task = await engine.createTask({})
    await engine.transitionTask(task.id, 'running')
    const updated = await store.getTask(task.id)
    expect(updated?.status).toBe('running')
  })

  it('throws InvalidTransitionError on invalid transition', async () => {
    const { engine } = makeEngine()
    const task = await engine.createTask({})
    await expect(engine.transitionTask(task.id, 'completed')).rejects.toThrow(InvalidTransitionError)
    await expect(engine.transitionTask(task.id, 'completed')).rejects.toThrow('Invalid transition')
  })

  it('throws when task not found', async () => {
    const { engine } = makeEngine()
    await expect(engine.transitionTask('missing', 'running')).rejects.toThrow(/not found/i)
  })

  it('emits taskcast:status event on transition', async () => {
    const { engine, broadcast } = makeEngine()
    const received: unknown[] = []
    const task = await engine.createTask({})
    broadcast.subscribe(task.id, (e) => received.push(e))
    await engine.transitionTask(task.id, 'running')
    expect(received).toHaveLength(1)
    expect((received[0] as { type: string }).type).toBe('taskcast:status')
  })

  it('sets completedAt on terminal transition', async () => {
    const { engine, store } = makeEngine()
    const task = await engine.createTask({})
    await engine.transitionTask(task.id, 'running')
    await engine.transitionTask(task.id, 'completed')
    const updated = await store.getTask(task.id)
    expect(updated?.completedAt).toBeGreaterThan(0)
  })

  it('saves to longTermStore on transition', async () => {
    const store = new MemoryShortTermStore()
    const broadcast = new MemoryBroadcastProvider()
    const longTermStore = makeLongTermStore()
    const engine = new TaskEngine({ shortTermStore: store, broadcast, longTermStore: longTermStore })
    const task = await engine.createTask({})
    vi.mocked(longTermStore.saveTask).mockClear()
    await engine.transitionTask(task.id, 'running')
    expect(longTermStore.saveTask).toHaveBeenCalledWith(expect.objectContaining({ status: 'running' }))
  })

  it('calls onTaskTimeout hook when transitioning to timeout', async () => {
    const onTaskTimeout = vi.fn()
    const store = new MemoryShortTermStore()
    const broadcast = new MemoryBroadcastProvider()
    const engine = new TaskEngine({ shortTermStore: store, broadcast, hooks: { onTaskTimeout } })
    const task = await engine.createTask({})
    await engine.transitionTask(task.id, 'running')
    await engine.transitionTask(task.id, 'timeout')
    expect(onTaskTimeout).toHaveBeenCalledWith(expect.objectContaining({ id: task.id, status: 'timeout' }))
  })

  it('calls onTaskFailed hook with error when transitioning to failed WITH error', async () => {
    const onTaskFailed = vi.fn()
    const store = new MemoryShortTermStore()
    const broadcast = new MemoryBroadcastProvider()
    const engine = new TaskEngine({ shortTermStore: store, broadcast, hooks: { onTaskFailed } })
    const task = await engine.createTask({})
    await engine.transitionTask(task.id, 'running')
    const error = { message: 'something went wrong', code: 'ERR_TEST' }
    await engine.transitionTask(task.id, 'failed', { error })
    expect(onTaskFailed).toHaveBeenCalledWith(
      expect.objectContaining({ id: task.id, status: 'failed' }),
      error,
    )
  })

  it('does NOT call onTaskFailed when transitioning to failed WITHOUT error', async () => {
    const onTaskFailed = vi.fn()
    const store = new MemoryShortTermStore()
    const broadcast = new MemoryBroadcastProvider()
    const engine = new TaskEngine({ shortTermStore: store, broadcast, hooks: { onTaskFailed } })
    const task = await engine.createTask({})
    await engine.transitionTask(task.id, 'running')
    await engine.transitionTask(task.id, 'failed')
    expect(onTaskFailed).not.toHaveBeenCalled()
  })

  it('stores result in task when payload.result is provided', async () => {
    const { engine, store } = makeEngine()
    const task = await engine.createTask({})
    await engine.transitionTask(task.id, 'running')
    await engine.transitionTask(task.id, 'completed', { result: { answer: 42 } })
    const updated = await store.getTask(task.id)
    expect(updated?.result).toEqual({ answer: 42 })
  })

  it('calls onTaskTransitioned hook with correct old and new status', async () => {
    const onTaskTransitioned = vi.fn()
    const store = new MemoryShortTermStore()
    const broadcast = new MemoryBroadcastProvider()
    const engine = new TaskEngine({ shortTermStore: store, broadcast, hooks: { onTaskTransitioned } })
    const task = await engine.createTask({})

    await engine.transitionTask(task.id, 'running')
    expect(onTaskTransitioned).toHaveBeenCalledOnce()
    expect(onTaskTransitioned).toHaveBeenCalledWith(
      expect.objectContaining({ id: task.id, status: 'running' }),
      'pending',
      'running',
    )

    onTaskTransitioned.mockClear()
    await engine.transitionTask(task.id, 'completed')
    expect(onTaskTransitioned).toHaveBeenCalledOnce()
    expect(onTaskTransitioned).toHaveBeenCalledWith(
      expect.objectContaining({ id: task.id, status: 'completed' }),
      'running',
      'completed',
    )
  })
})

describe('TaskEngine.publishEvent', () => {
  it('appends event and broadcasts it', async () => {
    const { engine, store, broadcast } = makeEngine()
    const task = await engine.createTask({})
    await engine.transitionTask(task.id, 'running')

    const received: unknown[] = []
    broadcast.subscribe(task.id, (e) => received.push(e))

    await engine.publishEvent(task.id, {
      type: 'llm.delta',
      level: 'info',
      data: { text: 'hello' },
    })

    const events = await store.getEvents(task.id)
    const userEvents = events.filter((e) => e.type !== 'taskcast:status')
    expect(userEvents).toHaveLength(1)
    expect(userEvents[0]?.type).toBe('llm.delta')
    expect(received).toHaveLength(1)
  })

  it('throws when task not found in publishEvent', async () => {
    const { engine } = makeEngine()
    await expect(
      engine.publishEvent('missing-task', { type: 'x', level: 'info', data: null })
    ).rejects.toThrow(/not found/i)
  })

  it('publishes event with seriesId and seriesMode', async () => {
    const { engine, store } = makeEngine()
    const task = await engine.createTask({})
    await engine.transitionTask(task.id, 'running')
    await engine.publishEvent(task.id, {
      type: 'llm.delta',
      level: 'info',
      data: { delta: 'hello' },
      seriesId: 's1',
      seriesMode: 'accumulate',
    })
    const events = await store.getEvents(task.id)
    const userEvents = events.filter((e) => e.type !== 'taskcast:status')
    expect(userEvents[0]?.seriesId).toBe('s1')
    expect(userEvents[0]?.seriesMode).toBe('accumulate')
  })

  it('assigns monotonically increasing index', async () => {
    const { engine, store } = makeEngine()
    const task = await engine.createTask({})
    await engine.transitionTask(task.id, 'running')
    await engine.publishEvent(task.id, { type: 'a', level: 'info', data: null })
    await engine.publishEvent(task.id, { type: 'b', level: 'info', data: null })
    const events = await store.getEvents(task.id)
    const indices = events.map((e) => e.index)
    expect(indices).toEqual([...indices].sort((a, b) => a - b))
  })

  it('rejects publish on completed task', async () => {
    const { engine } = makeEngine()
    const task = await engine.createTask({})
    await engine.transitionTask(task.id, 'running')
    await engine.transitionTask(task.id, 'completed')
    await expect(
      engine.publishEvent(task.id, { type: 'x', level: 'info', data: null })
    ).rejects.toThrow(/terminal/i)
  })

  it('saves event to longTermStore when configured', async () => {
    const store = new MemoryShortTermStore()
    const broadcast = new MemoryBroadcastProvider()
    const longTermStore = makeLongTermStore()
    const engine = new TaskEngine({ shortTermStore: store, broadcast, longTermStore: longTermStore })
    const task = await engine.createTask({})
    await engine.transitionTask(task.id, 'running')
    await engine.publishEvent(task.id, { type: 'llm.delta', level: 'info', data: { delta: 'hi' } })
    // saveEvent is fire-and-forget so flush microtasks
    await Promise.resolve()
    await Promise.resolve()
    expect(longTermStore.saveEvent).toHaveBeenCalled()
  })

  it('calls onEventDropped hook when longTermStore saveEvent rejects', async () => {
    const onEventDropped = vi.fn()
    const store = new MemoryShortTermStore()
    const broadcast = new MemoryBroadcastProvider()
    const longTermStore = makeLongTermStore({
      saveEvent: vi.fn().mockRejectedValue(new Error('storage unavailable')),
    })
    const engine = new TaskEngine({ shortTermStore: store, broadcast, longTermStore: longTermStore, hooks: { onEventDropped } })
    const task = await engine.createTask({})
    await engine.transitionTask(task.id, 'running')
    await engine.publishEvent(task.id, { type: 'llm.delta', level: 'info', data: null })
    // flush microtasks for the fire-and-forget .catch
    await Promise.resolve()
    await Promise.resolve()
    expect(onEventDropped).toHaveBeenCalled()
  })
})

describe('TaskEngine.getTask', () => {
  it('returns null for unknown task', async () => {
    const { engine } = makeEngine()
    expect(await engine.getTask('nope')).toBeNull()
  })

  it('returns existing task', async () => {
    const { engine } = makeEngine()
    const task = await engine.createTask({ type: 'test' })
    const found = await engine.getTask(task.id)
    expect(found?.id).toBe(task.id)
  })

  it('falls back to longTermStore when shortTermStore returns null', async () => {
    const store = new MemoryShortTermStore()
    const broadcast = new MemoryBroadcastProvider()
    const fallbackTask = {
      id: 'archived-task',
      status: 'completed' as const,
      createdAt: 0,
      updatedAt: 1000,
      completedAt: 1000,
    }
    const longTermStore = makeLongTermStore({
      getTask: vi.fn().mockResolvedValue(fallbackTask),
    })
    const engine = new TaskEngine({ shortTermStore: store, broadcast, longTermStore: longTermStore })
    const found = await engine.getTask('archived-task')
    expect(found).toEqual(fallbackTask)
    expect(longTermStore.getTask).toHaveBeenCalledWith('archived-task')
  })
})

describe('TaskEngine.getEvents', () => {
  it('returns events for a task', async () => {
    const { engine } = makeEngine()
    const task = await engine.createTask({})
    await engine.transitionTask(task.id, 'running')
    await engine.publishEvent(task.id, { type: 'test', level: 'info', data: null })
    const events = await engine.getEvents(task.id)
    expect(events.length).toBeGreaterThan(0)
  })
})

describe('TaskEngine.subscribe', () => {
  it('receives live events', async () => {
    const { engine } = makeEngine()
    const task = await engine.createTask({})
    await engine.transitionTask(task.id, 'running')

    const received: string[] = []
    const unsub = engine.subscribe(task.id, (e) => received.push(e.type))

    await engine.publishEvent(task.id, { type: 'live.event', level: 'info', data: null })
    expect(received).toContain('live.event')
    unsub()
  })
})

describe('TaskEngine constructor validation', () => {
  it('throws when both shortTerm and shortTermStore are provided', () => {
    const store = new MemoryShortTermStore()
    const broadcast = new MemoryBroadcastProvider()
    expect(() => new TaskEngine({
      shortTerm: store,
      shortTermStore: store,
      broadcast,
    } as any)).toThrow('Cannot specify both shortTerm and shortTermStore')
  })

  it('throws when both longTerm and longTermStore are provided', () => {
    const store = new MemoryShortTermStore()
    const broadcast = new MemoryBroadcastProvider()
    const longTermStore = makeLongTermStore()
    expect(() => new TaskEngine({
      shortTermStore: store,
      longTerm: longTermStore,
      longTermStore: longTermStore,
      broadcast,
    } as any)).toThrow('Cannot specify both longTerm and longTermStore')
  })

  it('accepts legacy shortTerm/longTerm option names', () => {
    const store = new MemoryShortTermStore()
    const broadcast = new MemoryBroadcastProvider()
    const engine = new TaskEngine({ shortTerm: store, broadcast } as any)
    expect(engine).toBeInstanceOf(TaskEngine)
  })
})
