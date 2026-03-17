import { describe, it, expect } from 'vitest'
import { Hono } from 'hono'
import { TaskEngine, MemoryBroadcastProvider, MemoryShortTermStore } from '@taskcast/core'
import { createSSERouter, createSubscriberCounts } from '../src/routes/sse.js'
import type { AuthContext } from '../src/auth.js'

function makeApp() {
  const store = new MemoryShortTermStore()
  const broadcast = new MemoryBroadcastProvider()
  const engine = new TaskEngine({ shortTermStore: store, broadcast })
  const subscriberCounts = createSubscriberCounts()
  const app = new Hono()
  app.use('*', async (c, next) => {
    const auth: AuthContext = { taskIds: '*', scope: ['*'] }
    c.set('auth', auth)
    await next()
  })
  app.route('/tasks', createSSERouter(engine, subscriberCounts))
  return { app, engine }
}

async function collectSSEEvents(
  res: Response,
  count: number,
): Promise<Array<{ event: string; data: string }>> {
  const reader = res.body!.getReader()
  const decoder = new TextDecoder()
  const collected: Array<{ event: string; data: string }> = []
  let buffer = ''

  while (collected.length < count) {
    const { done, value } = await reader.read()
    if (done) break
    buffer += decoder.decode(value, { stream: true })
    const blocks = buffer.split('\n\n')
    buffer = blocks.pop() ?? ''
    for (const block of blocks) {
      if (!block.trim()) continue
      const lines = block.split('\n')
      const eventLine = lines.find((l) => l.startsWith('event:'))
      const dataLine = lines.find((l) => l.startsWith('data:'))
      if (eventLine && dataLine) {
        collected.push({
          event: eventLine.replace('event:', '').trim(),
          data: dataLine.replace('data:', '').trim(),
        })
      }
    }
  }

  reader.cancel()
  return collected
}

describe('GET /tasks/:taskId/events (SSE)', () => {
  it('returns 404 for unknown task', async () => {
    const { app } = makeApp()
    const res = await app.request('/tasks/nonexistent/events')
    expect(res.status).toBe(404)
  })

  it('replays history and delivers taskcast.done for completed task', async () => {
    const { app, engine } = makeApp()
    const task = await engine.createTask({})
    await engine.transitionTask(task.id, 'running')
    await engine.publishEvent(task.id, { type: 'llm.delta', level: 'info', data: { text: 'hi' } })
    await engine.transitionTask(task.id, 'completed', { result: { answer: 42 } })

    const res = await app.request(`/tasks/${task.id}/events`)
    expect(res.headers.get('content-type')).toContain('text/event-stream')

    // Collect: taskcast:status(running) + llm.delta + taskcast:status(completed) + taskcast.done
    const events = await collectSSEEvents(res, 4)
    const types = events.map((e) => e.event)
    expect(types).toContain('taskcast.event')
    expect(types).toContain('taskcast.done')
  })

  it('filters events by type wildcard', async () => {
    const { app, engine } = makeApp()
    const task = await engine.createTask({})
    await engine.transitionTask(task.id, 'running')
    await engine.publishEvent(task.id, { type: 'llm.delta', level: 'info', data: null })
    await engine.publishEvent(task.id, { type: 'tool.call', level: 'info', data: null })
    await engine.transitionTask(task.id, 'completed')

    const res = await app.request(`/tasks/${task.id}/events?types=llm.*&includeStatus=false`)
    const events = await collectSSEEvents(res, 2) // llm.delta + taskcast.done
    const eventTypes = events
      .filter((e) => e.event === 'taskcast.event')
      .map((e) => JSON.parse(e.data).type)
    expect(eventTypes).toEqual(['llm.delta'])
  })

  it('replays history with since.index filter', async () => {
    const { app, engine } = makeApp()
    const task = await engine.createTask({})
    await engine.transitionTask(task.id, 'running')
    await engine.publishEvent(task.id, { type: 'first', level: 'info', data: null })
    await engine.publishEvent(task.id, { type: 'second', level: 'info', data: null })
    await engine.transitionTask(task.id, 'completed')

    // since.index=0 skips filteredIndex <= 0, i.e. skips 'first' (filteredIndex=0), keeps 'second' (filteredIndex=1)
    const res = await app.request(`/tasks/${task.id}/events?since.index=0&includeStatus=false`)
    const events = await collectSSEEvents(res, 2) // second + done
    const dataEvents = events.filter((e) => e.event === 'taskcast.event')
    const types = dataEvents.map((e) => JSON.parse(e.data).type)
    expect(types).toContain('second')
    expect(types).not.toContain('first')
  })

  it('passes since.timestamp in filter to query params (coverage of parseFilter)', async () => {
    const { app, engine } = makeApp()
    const task = await engine.createTask({})
    await engine.transitionTask(task.id, 'running')
    await engine.publishEvent(task.id, { type: 'e1', level: 'info', data: null })
    await engine.transitionTask(task.id, 'completed')

    // since.timestamp is parsed by parseFilter; applyFilteredIndex ignores it, so all events come through
    const ts = Date.now() - 100000
    const res = await app.request(`/tasks/${task.id}/events?since.timestamp=${ts}&includeStatus=false`)
    const events = await collectSSEEvents(res, 2) // e1 + done
    expect(events.some((e) => e.event === 'taskcast.done')).toBe(true)
  })

  it('passes since.id in filter to query params (coverage of parseFilter)', async () => {
    const { app, engine } = makeApp()
    const task = await engine.createTask({})
    await engine.transitionTask(task.id, 'running')
    const evt = await engine.publishEvent(task.id, { type: 'first', level: 'info', data: null })
    await engine.publishEvent(task.id, { type: 'second', level: 'info', data: null })
    await engine.transitionTask(task.id, 'completed')

    // since.id is parsed by parseFilter; applyFilteredIndex ignores it, so events come through
    const res = await app.request(`/tasks/${task.id}/events?since.id=${evt.id}&includeStatus=false`)
    const events = await collectSSEEvents(res, 3) // first + second + done
    expect(events.some((e) => e.event === 'taskcast.done')).toBe(true)
  })

  it('delivers events without wrap when wrap=false', async () => {
    const { app, engine } = makeApp()
    const task = await engine.createTask({})
    await engine.transitionTask(task.id, 'running')
    await engine.publishEvent(task.id, { type: 'test.event', level: 'info', data: { x: 1 } })
    await engine.transitionTask(task.id, 'completed')

    const res = await app.request(`/tasks/${task.id}/events?wrap=false&includeStatus=false`)
    const events = await collectSSEEvents(res, 2)
    const dataEvent = events.find((e) => e.event === 'taskcast.event')
    if (dataEvent) {
      const parsed = JSON.parse(dataEvent.data)
      // raw event has id, taskId, index, etc but not filteredIndex
      expect(parsed).toHaveProperty('id')
      expect(parsed).toHaveProperty('taskId')
      expect(parsed).not.toHaveProperty('filteredIndex')
    }
  })

  it('delivers events with seriesId in envelope when wrap=true', async () => {
    const { app, engine } = makeApp()
    const task = await engine.createTask({})
    await engine.transitionTask(task.id, 'running')
    await engine.publishEvent(task.id, {
      type: 'chunk',
      level: 'info',
      data: null,
      seriesId: 's1',
      seriesMode: 'accumulate',
    })
    await engine.transitionTask(task.id, 'completed')

    const res = await app.request(`/tasks/${task.id}/events?includeStatus=false`)
    const events = await collectSSEEvents(res, 2)
    const dataEvent = events.find((e) => e.event === 'taskcast.event')
    if (dataEvent) {
      const parsed = JSON.parse(dataEvent.data)
      expect(parsed.seriesId).toBe('s1')
      expect(parsed.seriesMode).toBe('accumulate')
    }
  })

  it('returns 403 when auth scope is insufficient', async () => {
    const store = new (await import('@taskcast/core')).MemoryShortTermStore()
    const broadcast = new (await import('@taskcast/core')).MemoryBroadcastProvider()
    const engine = new (await import('@taskcast/core')).TaskEngine({ shortTermStore: store, broadcast })
    const app = new Hono()
    app.use('*', async (c, next) => {
      // No event:subscribe scope
      const auth = { taskIds: '*' as const, scope: [] as never[] }
      c.set('auth', auth)
      await next()
    })
    app.route('/tasks', createSSERouter(engine, createSubscriberCounts()))
    const task = await engine.createTask({})
    const res = await app.request(`/tasks/${task.id}/events`)
    expect(res.status).toBe(403)
  })

  it('delivers live events via subscription for in-progress task', async () => {
    const { app, engine } = makeApp()
    const task = await engine.createTask({})
    await engine.transitionTask(task.id, 'running')
    // Task is running - SSE will subscribe for live events
    // includeStatus=true (default) so the terminal taskcast:status event triggers taskcast.done

    // Schedule publishing to happen after SSE subscription is set up.
    // app.request() processes SSE asynchronously; setTimeout allows subscribe() to run first.
    setTimeout(async () => {
      await engine.publishEvent(task.id, { type: 'live.chunk', level: 'info', data: { t: 'hello' } })
      await engine.transitionTask(task.id, 'completed')
    }, 50)

    // This resolves when the SSE stream ends (terminal event sent)
    const res = await app.request(`/tasks/${task.id}/events`)
    expect(res.headers.get('content-type')).toContain('text/event-stream')

    // Collect: running status (history) + live.chunk + completed status + taskcast.done = 4
    const events = await collectSSEEvents(res, 4)
    const types = events.map((e) => e.event)
    expect(types).toContain('taskcast.event')
    expect(types).toContain('taskcast.done')
    const doneEvent = events.find((e) => e.event === 'taskcast.done')
    expect(JSON.parse(doneEvent!.data).reason).toBe('completed')
  }, 10000)

  it('limits history replay events with limit query parameter', async () => {
    const { app, engine } = makeApp()
    const task = await engine.createTask({})
    await engine.transitionTask(task.id, 'running')
    // Publish 10 events
    for (let i = 0; i < 10; i++) {
      await engine.publishEvent(task.id, { type: 'progress', level: 'info', data: { i } })
    }
    await engine.transitionTask(task.id, 'completed')

    // Connect with limit=3 — storage returns 3 events total (status:running + progress:0 + progress:1)
    const res = await app.request(`/tasks/${task.id}/events?limit=3`)
    expect(res.headers.get('content-type')).toContain('text/event-stream')

    // Collect: 3 history events + taskcast.done
    const events = await collectSSEEvents(res, 4)
    const dataEvents = events.filter((e) => e.event === 'taskcast.event')
    expect(dataEvents).toHaveLength(3)

    // First event should be taskcast:status (running transition)
    const firstType = JSON.parse(dataEvents[0]!.data).type
    expect(firstType).toBe('taskcast:status')

    // The remaining 2 should be progress events
    const progressEvents = dataEvents.filter((e) => JSON.parse(e.data).type === 'progress')
    expect(progressEvents).toHaveLength(2)
    expect(JSON.parse(progressEvents[0]!.data).data.i).toBe(0)
    expect(JSON.parse(progressEvents[1]!.data).data.i).toBe(1)

    // Should still get taskcast.done
    const doneEvent = events.find((e) => e.event === 'taskcast.done')
    expect(doneEvent).toBeDefined()
  })
})
