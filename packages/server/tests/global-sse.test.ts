import { describe, it, expect, vi } from 'vitest'
import { Hono } from 'hono'
import { TaskEngine, MemoryBroadcastProvider, MemoryShortTermStore } from '@taskcast/core'
import { createGlobalSSERoute, createSubscriberCounts } from '../src/routes/sse.js'
import type { AuthContext } from '../src/auth.js'

function makeApp() {
  const store = new MemoryShortTermStore()
  const broadcast = new MemoryBroadcastProvider()
  const engine = new TaskEngine({ shortTermStore: store, broadcast })
  const app = new Hono()
  app.use('*', async (c, next) => {
    const auth: AuthContext = { taskIds: '*', scope: ['*'] }
    c.set('auth', auth)
    await next()
  })
  app.route('/events', createGlobalSSERoute(engine))
  return { app, engine }
}

async function collectSSEEvents(
  res: Response,
  count: number,
  timeoutMs = 5000,
): Promise<Array<{ event: string; data: string }>> {
  const reader = res.body!.getReader()
  const decoder = new TextDecoder()
  const collected: Array<{ event: string; data: string }> = []
  let buffer = ''

  const deadline = Date.now() + timeoutMs

  while (collected.length < count && Date.now() < deadline) {
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

describe('GET /events (global SSE)', () => {
  it('returns 200 with text/event-stream content-type', async () => {
    const { app, engine } = makeApp()

    // Create and complete a task so the stream has something,
    // but we just need to check the content-type header
    setTimeout(async () => {
      const task = await engine.createTask({})
      await engine.transitionTask(task.id, 'running')
      await engine.publishEvent(task.id, { type: 'test', level: 'info', data: null })
    }, 50)

    const res = await app.request('/events')
    expect(res.status).toBe(200)
    expect(res.headers.get('content-type')).toContain('text/event-stream')
    // Cancel the stream
    res.body!.getReader().cancel()
  }, 10000)

  it('streams events from newly created tasks', async () => {
    const { app, engine } = makeApp()

    setTimeout(async () => {
      const task = await engine.createTask({})
      await engine.transitionTask(task.id, 'running')
      await engine.publishEvent(task.id, { type: 'llm.delta', level: 'info', data: { text: 'hello' } })
    }, 50)

    const res = await app.request('/events')
    // We expect: taskcast:status(running) + llm.delta = at least 2 taskcast.event
    const events = await collectSSEEvents(res, 2)
    expect(events.length).toBeGreaterThanOrEqual(2)
    expect(events.every((e) => e.event === 'taskcast.event')).toBe(true)
  }, 10000)

  it('includes taskId in the event envelope', async () => {
    const { app, engine } = makeApp()
    let createdTaskId: string

    setTimeout(async () => {
      const task = await engine.createTask({})
      createdTaskId = task.id
      await engine.transitionTask(task.id, 'running')
      await engine.publishEvent(task.id, { type: 'test.event', level: 'info', data: { x: 1 } })
    }, 50)

    const res = await app.request('/events')
    const events = await collectSSEEvents(res, 2)
    const dataEvents = events.filter((e) => e.event === 'taskcast.event')
    expect(dataEvents.length).toBeGreaterThan(0)

    for (const evt of dataEvents) {
      const parsed = JSON.parse(evt.data)
      expect(parsed).toHaveProperty('taskId')
      expect(parsed.taskId).toBe(createdTaskId!)
    }
  }, 10000)

  it('filters events by type (wildcard)', async () => {
    const { app, engine } = makeApp()

    setTimeout(async () => {
      const task = await engine.createTask({})
      await engine.transitionTask(task.id, 'running')
      await engine.publishEvent(task.id, { type: 'llm.delta', level: 'info', data: null })
      await engine.publishEvent(task.id, { type: 'tool.call', level: 'info', data: null })
      await engine.publishEvent(task.id, { type: 'llm.done', level: 'info', data: null })
    }, 50)

    const res = await app.request('/events?types=llm.*')
    const events = await collectSSEEvents(res, 2)
    const types = events
      .filter((e) => e.event === 'taskcast.event')
      .map((e) => JSON.parse(e.data).type)

    expect(types).toContain('llm.delta')
    expect(types).toContain('llm.done')
    expect(types).not.toContain('tool.call')
    expect(types).not.toContain('taskcast:status')
  }, 10000)

  it('filters events by level', async () => {
    const { app, engine } = makeApp()

    setTimeout(async () => {
      const task = await engine.createTask({})
      await engine.transitionTask(task.id, 'running')
      await engine.publishEvent(task.id, { type: 'a', level: 'info', data: null })
      await engine.publishEvent(task.id, { type: 'b', level: 'warn', data: null })
      await engine.publishEvent(task.id, { type: 'c', level: 'error', data: null })
    }, 50)

    const res = await app.request('/events?levels=warn,error')
    const events = await collectSSEEvents(res, 2)
    const collected = events
      .filter((e) => e.event === 'taskcast.event')
      .map((e) => JSON.parse(e.data))

    expect(collected.length).toBe(2)
    expect(collected.map((e: { type: string }) => e.type)).toContain('b')
    expect(collected.map((e: { type: string }) => e.type)).toContain('c')
    expect(collected.map((e: { level: string }) => e.level)).not.toContain('info')
  }, 10000)

  it('streams events from multiple tasks', async () => {
    const { app, engine } = makeApp()

    setTimeout(async () => {
      const task1 = await engine.createTask({})
      await engine.transitionTask(task1.id, 'running')
      await engine.publishEvent(task1.id, { type: 'from.task1', level: 'info', data: null })

      const task2 = await engine.createTask({})
      await engine.transitionTask(task2.id, 'running')
      await engine.publishEvent(task2.id, { type: 'from.task2', level: 'info', data: null })
    }, 50)

    const res = await app.request('/events')
    // taskcast:status(running) for task1 + from.task1 + taskcast:status(running) for task2 + from.task2 = 4
    const events = await collectSSEEvents(res, 4)
    const taskIds = events
      .filter((e) => e.event === 'taskcast.event')
      .map((e) => JSON.parse(e.data).taskId)

    const uniqueTaskIds = [...new Set(taskIds)]
    expect(uniqueTaskIds.length).toBe(2)
  }, 10000)

  it('does not replay historical events from pre-existing tasks', async () => {
    const { app, engine } = makeApp()

    // Create a task with events BEFORE SSE connection
    const existingTask = await engine.createTask({})
    await engine.transitionTask(existingTask.id, 'running')
    await engine.publishEvent(existingTask.id, { type: 'old.event', level: 'info', data: null })

    // Now connect SSE and create a new task
    setTimeout(async () => {
      const newTask = await engine.createTask({})
      await engine.transitionTask(newTask.id, 'running')
      await engine.publishEvent(newTask.id, { type: 'new.event', level: 'info', data: null })
    }, 50)

    const res = await app.request('/events')
    const events = await collectSSEEvents(res, 2)
    const types = events
      .filter((e) => e.event === 'taskcast.event')
      .map((e) => JSON.parse(e.data).type)

    // Should NOT contain old.event from the pre-existing task
    expect(types).not.toContain('old.event')
    // Should contain new.event from the newly created task
    expect(types).toContain('new.event')
  }, 10000)

  it('returns 403 when auth scope is insufficient', async () => {
    const store = new MemoryShortTermStore()
    const broadcast = new MemoryBroadcastProvider()
    const engine = new TaskEngine({ shortTermStore: store, broadcast })
    const app = new Hono()
    app.use('*', async (c, next) => {
      const auth: AuthContext = { taskIds: '*', scope: [] as never[] }
      c.set('auth', auth)
      await next()
    })
    app.route('/events', createGlobalSSERoute(engine))

    const res = await app.request('/events')
    expect(res.status).toBe(403)
  })

  it('does not send taskcast.done (runs indefinitely)', async () => {
    const { app, engine } = makeApp()

    setTimeout(async () => {
      const task = await engine.createTask({})
      await engine.transitionTask(task.id, 'running')
      await engine.publishEvent(task.id, { type: 'test', level: 'info', data: null })
      await engine.transitionTask(task.id, 'completed')
    }, 50)

    const res = await app.request('/events')
    // Collect events - should get taskcast:status(running) + test + taskcast:status(completed) = 3
    // but no taskcast.done
    const events = await collectSSEEvents(res, 3)
    const eventTypes = events.map((e) => e.event)
    expect(eventTypes).not.toContain('taskcast.done')
    expect(eventTypes.every((t) => t === 'taskcast.event')).toBe(true)
  }, 10000)

  it('includes seriesId and seriesMode in envelope when present', async () => {
    const { app, engine } = makeApp()

    setTimeout(async () => {
      const task = await engine.createTask({})
      await engine.transitionTask(task.id, 'running')
      await engine.publishEvent(task.id, {
        type: 'chunk',
        level: 'info',
        data: { text: 'hi' },
        seriesId: 's1',
        seriesMode: 'accumulate',
      })
    }, 50)

    const res = await app.request('/events')
    // taskcast:status(running) + chunk
    const events = await collectSSEEvents(res, 2)
    const chunkEvent = events.find((e) => {
      if (e.event !== 'taskcast.event') return false
      const parsed = JSON.parse(e.data)
      return parsed.type === 'chunk'
    })
    expect(chunkEvent).toBeDefined()
    const parsed = JSON.parse(chunkEvent!.data)
    expect(parsed.seriesId).toBe('s1')
    expect(parsed.seriesMode).toBe('accumulate')
  }, 10000)

  it('exits keepalive loop when stream is aborted', async () => {
    vi.useFakeTimers()
    try {
      const { app } = makeApp()

      const resPromise = app.request('/events')

      // Advance past the first keepalive write + sleep
      await vi.advanceTimersByTimeAsync(30000)

      const res = await resPromise
      expect(res.status).toBe(200)

      // Cancel the reader to trigger stream abort (sets closed = true)
      const reader = res.body!.getReader()
      reader.cancel()

      // Advance timer so the pending setTimeout resolves and while(!closed) exits
      await vi.advanceTimersByTimeAsync(30000)
    } finally {
      vi.useRealTimers()
    }
  }, 10000)
})
