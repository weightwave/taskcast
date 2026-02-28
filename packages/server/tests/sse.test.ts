import { describe, it, expect } from 'vitest'
import { Hono } from 'hono'
import { TaskEngine, MemoryBroadcastProvider, MemoryShortTermStore } from '@taskcast/core'
import { createSSERouter } from '../src/routes/sse.js'
import type { AuthContext } from '../src/auth.js'

function makeApp() {
  const store = new MemoryShortTermStore()
  const broadcast = new MemoryBroadcastProvider()
  const engine = new TaskEngine({ shortTerm: store, broadcast })
  const app = new Hono()
  app.use('*', async (c, next) => {
    const auth: AuthContext = { taskIds: '*', scope: ['*'] }
    c.set('auth', auth)
    await next()
  })
  app.route('/tasks', createSSERouter(engine))
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
})
