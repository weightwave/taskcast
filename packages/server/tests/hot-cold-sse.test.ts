import { describe, expect, it, vi } from 'vitest'
import { Hono } from 'hono'
import {
  MemoryBroadcastProvider,
  MemoryLongTermStore,
  MemoryShortTermStore,
  TaskEngine,
  type TaskEvent,
} from '@taskcast/core'
import { createSSERouter, createSubscriberCounts } from '../src/routes/sse.js'
import type { AuthContext } from '../src/auth.js'

function makeApp() {
  const hot = new MemoryShortTermStore()
  const durable = new MemoryLongTermStore()
  const engine = new TaskEngine({
    shortTermStore: hot,
    longTermStore: durable,
    broadcast: new MemoryBroadcastProvider(),
  })
  const app = new Hono()
  app.use('*', async (c, next) => {
    const auth: AuthContext = { taskIds: '*', scope: ['*'] }
    c.set('auth', auth)
    await next()
  })
  app.route('/tasks', createSSERouter(engine, createSubscriberCounts()))
  return { app, engine }
}

function blockNextHistory(engine: TaskEngine): {
  started: Promise<void>
  release: () => void
} {
  const original = engine.getEvents.bind(engine)
  let reportStarted!: () => void
  let release!: () => void
  const started = new Promise<void>((resolve) => { reportStarted = resolve })
  const gate = new Promise<void>((resolve) => { release = resolve })
  let blocked = false
  engine.getEvents = async (...args) => {
    if (!blocked) {
      blocked = true
      reportStarted()
      await gate
    }
    return original(...args)
  }
  return { started, release }
}

function freezeNextHistory(engine: TaskEngine, alter: (events: TaskEvent[]) => TaskEvent[] = (events) => events): {
  started: Promise<void>
  release: () => void
} {
  const original = engine.getEvents.bind(engine)
  let reportStarted!: () => void
  let release!: () => void
  const started = new Promise<void>((resolve) => { reportStarted = resolve })
  const gate = new Promise<void>((resolve) => { release = resolve })
  let blocked = false
  engine.getEvents = async (...args) => {
    const snapshot = alter(await original(...args))
    if (!blocked) {
      blocked = true
      reportStarted()
      await gate
    }
    return snapshot
  }
  return { started, release }
}

function parseSSE(text: string): Array<{
  event: string
  id?: string
  data: unknown
}> {
  return text.split('\n\n').flatMap((block) => {
    if (!block.trim()) return []
    const lines = block.split('\n')
    const event = lines.find((line) => line.startsWith('event:'))
      ?.slice('event:'.length).trim()
    const data = lines.find((line) => line.startsWith('data:'))
      ?.slice('data:'.length).trim()
    if (!event || data === undefined) return []
    const id = lines.find((line) => line.startsWith('id:'))
      ?.slice('id:'.length).trim()
    return [{ event, ...(id && { id }), data: JSON.parse(data) as unknown }]
  })
}

describe('hot/cold SSE snapshot boundary', () => {
  it('drains new buffered events in order and closes on a terminal transition', async () => {
    const { app, engine } = makeApp()
    const task = await engine.createTask({ id: 'buffered-after-snapshot' })
    await engine.transitionTask(task.id, 'running')
    const history = freezeNextHistory(engine)
    const responsePromise = app.request(`/tasks/${task.id}/events`)
    await history.started
    const raced = await engine.publishEvent(task.id, {
      type: 'race.event', level: 'info', data: { value: 1 },
    })
    await engine.transitionTask(task.id, 'completed')
    history.release()

    const frames = parseSSE(await (await responsePromise).text())
    expect(frames.filter(({ id }) => id === raced.id)).toHaveLength(1)
    expect(frames.at(-1)).toMatchObject({ event: 'taskcast.done', data: { reason: 'completed' } })
  })

  it('closes a replay when a buffered event reuses an index with different content', async () => {
    const { app, engine } = makeApp()
    const task = await engine.createTask({ id: 'buffered-conflict' })
    const original = await engine.publishEvent(task.id, {
      type: 'original', level: 'info', data: { value: 1 },
    })
    let deliver!: (event: TaskEvent) => void
    const subscribe = engine.subscribe.bind(engine)
    vi.spyOn(engine, 'subscribe').mockImplementation((...args) => {
      deliver = args[1]
      return subscribe(...args)
    })
    const history = freezeNextHistory(engine)
    const responsePromise = app.request(`/tasks/${task.id}/events`)
    await history.started
    deliver({ ...original, data: { value: 2 } })
    history.release()

    const frames = parseSSE(await (await responsePromise).text())
    expect(frames.filter(({ id }) => id === original.id)).toHaveLength(1)
    expect(frames.some(({ event }) => event === 'taskcast.done')).toBe(false)
  })

  it.each(['broadcast', 'history'] as const)(
    'recognizes transient accumulated data in a duplicate %s event',
    async (side) => {
      const { app, engine } = makeApp()
      const task = await engine.createTask({ id: `transient-${side}` })
      await engine.transitionTask(task.id, 'running')
      const raw = await engine.publishEvent(task.id, {
        type: 'chunk', level: 'info', data: { delta: 'A' },
      })
      const accumulated = { delta: 'AB' }
      let deliver!: (event: TaskEvent) => void
      const subscribe = engine.subscribe.bind(engine)
      vi.spyOn(engine, 'subscribe').mockImplementation((...args) => {
        deliver = args[1]
        return subscribe(...args)
      })
      const history = freezeNextHistory(engine, (events) => side === 'history'
        ? events.map((event) => event.id === raw.id
          ? { ...event, _accumulatedData: accumulated }
          : event)
        : events.map((event) => event.id === raw.id
          ? { ...event, data: accumulated }
          : event))
      const responsePromise = app.request(`/tasks/${task.id}/events`)
      await history.started
      deliver(side === 'broadcast'
        ? { ...raw, data: { delta: 'B' }, _accumulatedData: accumulated }
        : { ...raw, data: accumulated })
      await engine.transitionTask(task.id, 'completed')
      history.release()

      const frames = parseSSE(await (await responsePromise).text())
      expect(frames.filter(({ id }) => id === raw.id)).toHaveLength(1)
      expect(frames.at(-1)?.event).toBe('taskcast.done')
    },
  )

  it('closes replay when a series snapshot conflicts with its stored event', async () => {
    const { app, engine } = makeApp()
    const task = await engine.createTask({ id: 'series-snapshot-conflict' })
    const raw = await engine.publishEvent(task.id, {
      type: 'chunk', level: 'info', data: { delta: 'A' },
      seriesId: 'output', seriesMode: 'accumulate', seriesAccField: 'delta',
    })
    vi.spyOn(engine, 'getSeriesLatest').mockResolvedValueOnce({
      ...raw, seriesMode: 'latest', data: { delta: 'different' },
    })

    const frames = parseSSE(await (await app.request(`/tasks/${task.id}/events`)).text())
    expect(frames.filter(({ event }) => event === 'taskcast.event')).toHaveLength(1)
    expect(frames.some(({ event }) => event === 'taskcast.done')).toBe(false)
  })
  it('delivers a publish during cold history fetch exactly once', async () => {
    const { app, engine } = makeApp()
    const task = await engine.createTask({ id: 'cold-race' })
    await engine.transitionTask(task.id, 'running')
    const [running] = await engine.getEvents(task.id)
    await engine.releaseTaskStorage(task.id, {
      expectedLastEventIndex: running!.index,
      inactiveSince: running!.timestamp,
    })
    const history = blockNextHistory(engine)

    const responsePromise = app.request(`/tasks/${task.id}/events`)
    await history.started
    const raced = await engine.publishEvent(task.id, {
      type: 'race.event',
      level: 'info',
      data: { value: 1 },
    })
    await engine.transitionTask(task.id, 'completed')
    history.release()

    const response = await responsePromise
    const frames = parseSSE(await response.text())
    expect(frames.filter(({ id }) => id === raced.id)).toHaveLength(1)
    expect(
      frames.filter(
        ({ event, data }) =>
          event === 'taskcast.event' &&
          (data as { type?: string }).type === 'race.event',
      ),
    ).toHaveLength(1)
    expect(frames.filter(({ event }) => event === 'taskcast.done')).toHaveLength(1)
  })

  it('deduplicates an accumulated snapshot against its buffered raw delta', async () => {
    const { app, engine } = makeApp()
    const task = await engine.createTask({ id: 'cold-acc-race' })
    await engine.transitionTask(task.id, 'running')
    await engine.publishEvent(task.id, {
      type: 'chunk',
      level: 'info',
      data: { delta: 'A' },
      seriesId: 'output',
      seriesMode: 'accumulate',
      seriesAccField: 'delta',
    })
    const beforeRelease = await engine.getEvents(task.id)
    await engine.releaseTaskStorage(task.id, {
      expectedLastEventIndex: beforeRelease.at(-1)!.index,
      inactiveSince: beforeRelease.at(-1)!.timestamp,
    })
    const history = blockNextHistory(engine)

    const responsePromise = app.request(
      `/tasks/${task.id}/events?includeStatus=false&seriesFormat=accumulated`,
    )
    await history.started
    const raced = await engine.publishEvent(task.id, {
      type: 'chunk',
      level: 'info',
      data: { delta: 'B' },
      seriesId: 'output',
      seriesMode: 'accumulate',
      seriesAccField: 'delta',
    })
    await engine.transitionTask(task.id, 'completed')
    history.release()

    const response = await responsePromise
    const frames = parseSSE(await response.text())
    const chunks = frames.filter(
      ({ event, data }) =>
        event === 'taskcast.event' &&
        (data as { type?: string }).type === 'chunk',
    )
    expect(chunks).toHaveLength(1)
    expect(chunks[0]!.id).toBe(raced.id)
    expect((chunks[0]!.data as { data: unknown }).data).toEqual({ delta: 'AB' })
    expect(frames.filter(({ event }) => event === 'taskcast.done')).toHaveLength(1)
  })
})
