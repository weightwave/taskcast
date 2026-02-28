import { describe, it, expect, vi } from 'vitest'
import { TaskEngine } from '../../src/engine.js'
import { MemoryBroadcastProvider, MemoryShortTermStore } from '../../src/memory-adapters.js'

function makeEngine() {
  const store = new MemoryShortTermStore()
  const broadcast = new MemoryBroadcastProvider()
  const engine = new TaskEngine({ shortTerm: store, broadcast })
  return { engine, store, broadcast }
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
})

describe('TaskEngine.transitionTask', () => {
  it('transitions pending → running and saves task', async () => {
    const { engine, store } = makeEngine()
    const task = await engine.createTask({})
    await engine.transitionTask(task.id, 'running')
    const updated = await store.getTask(task.id)
    expect(updated?.status).toBe('running')
  })

  it('throws on invalid transition', async () => {
    const { engine } = makeEngine()
    const task = await engine.createTask({})
    await expect(engine.transitionTask(task.id, 'completed')).rejects.toThrow()
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
