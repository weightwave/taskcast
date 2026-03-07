import { describe, it, expect } from 'vitest'
import { TaskEngine, MemoryBroadcastProvider, MemoryShortTermStore } from '@taskcast/core'
import { createTaskcastApp } from '../src/index.js'

function makeVerboseApp(verbose = true) {
  const logs: string[] = []
  const store = new MemoryShortTermStore()
  const broadcast = new MemoryBroadcastProvider()
  const engine = new TaskEngine({ shortTermStore: store, broadcast })
  const { app } = createTaskcastApp({
    engine,
    auth: { mode: 'none' },
    verbose,
    verboseLogger: (line) => logs.push(line),
  })
  return { app, engine, logs }
}

describe('verbose logger middleware', () => {
  it('logs POST /tasks with 201 status', async () => {
    const { app, logs } = makeVerboseApp()
    const res = await app.request('/tasks', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ type: 'test' }),
    })
    expect(res.status).toBe(201)
    expect(logs).toHaveLength(1)
    expect(logs[0]).toContain('POST')
    expect(logs[0]).toContain('/tasks')
    expect(logs[0]).toContain('201')
    expect(logs[0]).toContain('task created')
  })

  it('logs PATCH status transition with target status', async () => {
    const { app, engine, logs } = makeVerboseApp()
    const task = await engine.createTask({ type: 'test' })
    logs.length = 0 // clear creation log if any

    const res = await app.request(`/tasks/${task.id}/status`, {
      method: 'PATCH',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ status: 'running' }),
    })
    expect(res.status).toBe(200)
    expect(logs).toHaveLength(1)
    expect(logs[0]).toContain('PATCH')
    expect(logs[0]).toContain('/status')
    expect(logs[0]).toContain('200')
    expect(logs[0]).toMatch(/\u2192 running/)
  })

  it('logs POST /tasks/:id/events with event type', async () => {
    const { app, engine, logs } = makeVerboseApp()
    const task = await engine.createTask({ type: 'test' })
    await engine.transitionTask(task.id, 'running')
    logs.length = 0

    const res = await app.request(`/tasks/${task.id}/events`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ type: 'llm.delta', level: 'info', data: { text: 'hi' } }),
    })
    expect(res.status).toBe(201)
    expect(logs).toHaveLength(1)
    expect(logs[0]).toContain('POST')
    expect(logs[0]).toContain('/events')
    expect(logs[0]).toContain('type: llm.delta')
  })

  it('logs GET /tasks/:id with 200 status', async () => {
    const { app, engine, logs } = makeVerboseApp()
    const task = await engine.createTask({ type: 'test' })
    logs.length = 0

    const res = await app.request(`/tasks/${task.id}`)
    expect(res.status).toBe(200)
    expect(logs).toHaveLength(1)
    expect(logs[0]).toContain('GET')
    expect(logs[0]).toContain(`/tasks/${task.id}`)
    expect(logs[0]).toContain('200')
  })

  it('logs GET /health', async () => {
    const { app, logs } = makeVerboseApp()
    const res = await app.request('/health')
    expect(res.status).toBe(200)
    expect(logs).toHaveLength(1)
    expect(logs[0]).toContain('GET')
    expect(logs[0]).toContain('/health')
    expect(logs[0]).toContain('200')
  })

  it('does not log when verbose is false', async () => {
    const { app, logs } = makeVerboseApp(false)
    const res = await app.request('/health')
    expect(res.status).toBe(200)
    expect(logs).toHaveLength(0)
  })

  it('includes timestamp in ISO-like format', async () => {
    const { app, logs } = makeVerboseApp()
    await app.request('/health')
    expect(logs).toHaveLength(1)
    // Should match [YYYY-MM-DD HH:MM:SS]
    expect(logs[0]).toMatch(/^\[\d{4}-\d{2}-\d{2} \d{2}:\d{2}:\d{2}\]/)
  })

  it('includes duration in milliseconds', async () => {
    const { app, logs } = makeVerboseApp()
    await app.request('/health')
    expect(logs).toHaveLength(1)
    // Should contain a number followed by "ms"
    expect(logs[0]).toMatch(/\d+ms/)
  })

  it('logs 404 for unknown task', async () => {
    const { app, logs } = makeVerboseApp()
    const res = await app.request('/tasks/nonexistent')
    expect(res.status).toBe(404)
    expect(logs).toHaveLength(1)
    expect(logs[0]).toContain('404')
  })

  it('logs multiple requests independently', async () => {
    const { app, logs } = makeVerboseApp()
    await app.request('/health')
    await app.request('/health')
    await app.request('/health')
    expect(logs).toHaveLength(3)
  })
})
