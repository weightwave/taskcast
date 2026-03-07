import { describe, it, expect } from 'vitest'
import { Hono } from 'hono'
import { TaskEngine, MemoryBroadcastProvider, MemoryShortTermStore } from '@taskcast/core'
import { createSSERouter, createSubscriberCounts, getSubscriberCount } from '../src/routes/sse.js'
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
  return { app, engine, subscriberCounts }
}

function parseSSEText(text: string): Array<{ event: string; data: string }> {
  const events: Array<{ event: string; data: string }> = []
  const blocks = text.split('\n\n')
  for (const block of blocks) {
    if (!block.trim()) continue
    const lines = block.split('\n')
    const eventLine = lines.find((l) => l.startsWith('event:'))
    const dataLine = lines.find((l) => l.startsWith('data:'))
    if (eventLine && dataLine) {
      events.push({
        event: eventLine.replace('event:', '').trim(),
        data: dataLine.replace('data:', '').trim(),
      })
    }
  }
  return events
}

describe('Concurrent SSE subscribers', () => {
  it('10 SSE connections all receive replayed history + done', async () => {
    const { app, engine } = makeApp()
    const task = await engine.createTask({})
    await engine.transitionTask(task.id, 'running')

    const EVENT_COUNT = 5

    // Publish events first (they become history)
    for (let i = 0; i < EVENT_COUNT; i++) {
      await engine.publishEvent(task.id, {
        type: `event.${i}`,
        level: 'info',
        data: { index: i },
      })
    }

    // Transition to terminal so SSE replays history + sends done + closes
    await engine.transitionTask(task.id, 'completed')

    // Open 10 SSE connections — each will replay history, send done, and close immediately
    const SUBSCRIBER_COUNT = 10
    const responses = await Promise.all(
      Array.from({ length: SUBSCRIBER_COUNT }, () =>
        app.request(`/tasks/${task.id}/events?includeStatus=false`),
      ),
    )

    for (const res of responses) {
      expect(res.headers.get('content-type')).toContain('text/event-stream')
    }

    // Consume all response bodies
    const bodies = await Promise.all(responses.map((r) => r.text()))

    for (const body of bodies) {
      const events = parseSSEText(body)
      const dataEvents = events.filter((e) => e.event === 'taskcast.event')
      const doneEvents = events.filter((e) => e.event === 'taskcast.done')

      expect(dataEvents.length).toBe(EVENT_COUNT)
      expect(doneEvents.length).toBe(1)

      const types = dataEvents.map((e) => JSON.parse(e.data).type)
      for (let i = 0; i < EVENT_COUNT; i++) {
        expect(types).toContain(`event.${i}`)
      }
    }
  })

  it('subscriber count tracks active connections', async () => {
    const { app, engine, subscriberCounts } = makeApp()
    const task = await engine.createTask({})
    await engine.transitionTask(task.id, 'running')

    expect(getSubscriberCount(subscriberCounts, task.id)).toBe(0)

    // Start 5 SSE connections (non-terminal task, they'll block waiting)
    const ssePromises = Array.from({ length: 5 }, () =>
      app.request(`/tasks/${task.id}/events`),
    )

    await new Promise((r) => setTimeout(r, 50))
    expect(getSubscriberCount(subscriberCounts, task.id)).toBe(5)

    // Transition to terminal to close all streams
    await engine.transitionTask(task.id, 'completed')

    // Consume all responses
    const responses = await Promise.all(ssePromises)
    await Promise.all(responses.map((r) => r.text()))

    await new Promise((r) => setTimeout(r, 50))
    expect(getSubscriberCount(subscriberCounts, task.id)).toBe(0)
  }, 10000)
})
