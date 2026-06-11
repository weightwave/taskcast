import { describe, it, expect } from 'vitest'
import { createTestServer } from '../helpers/test-server.js'
import { collectSSEEvents, collectAllSSEEvents } from '../helpers/sse-collector.js'

describe('Server integration — SSE streaming', () => {
  it('replays history + streams live events for running task', async () => {
    const { app, engine } = createTestServer()

    const task = await engine.createTask({ type: 'test' })
    await engine.transitionTask(task.id, 'running')
    await engine.publishEvent(task.id, { type: 'history.event', level: 'info', data: { n: 1 } })

    // Schedule live events after SSE connects
    setTimeout(async () => {
      await engine.publishEvent(task.id, { type: 'live.event', level: 'info', data: { n: 2 } })
      await engine.transitionTask(task.id, 'completed')
    }, 50)

    const res = await app.request(`/tasks/${task.id}/events`)
    expect(res.headers.get('content-type')).toContain('text/event-stream')

    // running status + history.event + live.event + completed status + done
    const events = await collectSSEEvents(res, 5)
    const dataEvents = events.filter(e => e.event === 'taskcast.event')
    const types = dataEvents.map(e => JSON.parse(e.data).type)
    expect(types).toContain('history.event')
    expect(types).toContain('live.event')
    expect(events.some(e => e.event === 'taskcast.done')).toBe(true)
  }, 10000)

  it('terminal task replays then closes immediately', async () => {
    const { app, engine } = createTestServer()

    const task = await engine.createTask({})
    await engine.transitionTask(task.id, 'running')
    await engine.publishEvent(task.id, { type: 'evt', level: 'info', data: null })
    await engine.transitionTask(task.id, 'completed')

    const res = await app.request(`/tasks/${task.id}/events`)
    const events = await collectAllSSEEvents(res)
    const done = events.find(e => e.event === 'taskcast.done')
    expect(done).toBeTruthy()
    expect(JSON.parse(done!.data).reason).toBe('completed')
  })

  it('10 concurrent SSE clients all receive same events', async () => {
    const { app, engine } = createTestServer()

    const task = await engine.createTask({})
    await engine.transitionTask(task.id, 'running')

    setTimeout(async () => {
      for (let i = 0; i < 5; i++) {
        await engine.publishEvent(task.id, { type: 'chunk', level: 'info', data: { i } })
      }
      await engine.transitionTask(task.id, 'completed')
    }, 50)

    // 10 concurrent SSE connections (include status so stream closes on terminal)
    const promises = Array.from({ length: 10 }, () =>
      app.request(`/tasks/${task.id}/events`).then(r => collectAllSSEEvents(r))
    )
    const results = await Promise.all(promises)

    for (const events of results) {
      const dataEvents = events.filter(e => e.event === 'taskcast.event')
      const chunkEvents = dataEvents.filter(e => JSON.parse(e.data).type === 'chunk')
      expect(chunkEvents).toHaveLength(5)
      expect(events.some(e => e.event === 'taskcast.done')).toBe(true)
    }
  }, 15000)

  it('filter by type only returns matching events', async () => {
    const { app, engine } = createTestServer()

    const task = await engine.createTask({})
    await engine.transitionTask(task.id, 'running')
    await engine.publishEvent(task.id, { type: 'llm.delta', level: 'info', data: null })
    await engine.publishEvent(task.id, { type: 'tool.call', level: 'info', data: null })
    await engine.transitionTask(task.id, 'completed')

    const res = await app.request(`/tasks/${task.id}/events?types=llm.*&includeStatus=false`)
    const events = await collectAllSSEEvents(res)
    const dataEvents = events.filter(e => e.event === 'taskcast.event')
    expect(dataEvents).toHaveLength(1)
    expect(JSON.parse(dataEvents[0]!.data).type).toBe('llm.delta')
  })

  it('wrap=false returns raw event without filteredIndex', async () => {
    const { app, engine } = createTestServer()

    const task = await engine.createTask({})
    await engine.transitionTask(task.id, 'running')
    await engine.publishEvent(task.id, { type: 'test', level: 'info', data: { x: 1 } })
    await engine.transitionTask(task.id, 'completed')

    const res = await app.request(`/tasks/${task.id}/events?wrap=false&includeStatus=false`)
    const events = await collectAllSSEEvents(res)
    const dataEvent = events.find(e => e.event === 'taskcast.event')
    const parsed = JSON.parse(dataEvent!.data)
    expect(parsed).toHaveProperty('id')
    expect(parsed).toHaveProperty('taskId')
    expect(parsed).not.toHaveProperty('filteredIndex')
  })

  it('envelope preserves clientId/clientSeq when publisher uses seq ordering', async () => {
    const { app, engine } = createTestServer()

    const task = await engine.createTask({})
    await engine.transitionTask(task.id, 'running')
    await engine.publishEvent(task.id, {
      type: 'llm.delta',
      level: 'info',
      data: { delta: 'a' },
      clientId: 'worker-1',
      clientSeq: 0,
    })
    await engine.publishEvent(task.id, {
      type: 'llm.delta',
      level: 'info',
      data: { delta: 'b' },
      clientId: 'worker-1',
      clientSeq: 1,
    })
    // Event without seq — fields must be absent in envelope
    await engine.publishEvent(task.id, {
      type: 'plain',
      level: 'info',
      data: null,
    })
    await engine.transitionTask(task.id, 'completed')

    // wrap=true (default)
    const wrapped = await app.request(`/tasks/${task.id}/events?includeStatus=false`)
    const wrappedEvents = (await collectAllSSEEvents(wrapped))
      .filter(e => e.event === 'taskcast.event')
      .map(e => JSON.parse(e.data))

    const deltaWithSeq = wrappedEvents.filter(e => e.type === 'llm.delta')
    expect(deltaWithSeq).toHaveLength(2)
    expect(deltaWithSeq[0]).toMatchObject({ clientId: 'worker-1', clientSeq: 0 })
    expect(deltaWithSeq[1]).toMatchObject({ clientId: 'worker-1', clientSeq: 1 })

    const plain = wrappedEvents.find(e => e.type === 'plain')
    expect(plain).toBeTruthy()
    expect(plain).not.toHaveProperty('clientId')
    expect(plain).not.toHaveProperty('clientSeq')

    // wrap=false — raw events also carry the fields
    const raw = await app.request(`/tasks/${task.id}/events?wrap=false&includeStatus=false`)
    const rawEvents = (await collectAllSSEEvents(raw))
      .filter(e => e.event === 'taskcast.event')
      .map(e => JSON.parse(e.data))

    const rawDelta = rawEvents.filter(e => e.type === 'llm.delta')
    expect(rawDelta[0]).toMatchObject({ clientId: 'worker-1', clientSeq: 0 })
    expect(rawDelta[1]).toMatchObject({ clientId: 'worker-1', clientSeq: 1 })
  })
})
