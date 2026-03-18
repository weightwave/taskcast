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

  it('logs GET /tasks/:id/events as SSE subscriber connected', async () => {
    const { app, engine, logs } = makeVerboseApp()
    const task = await engine.createTask({ type: 'test' })
    await engine.transitionTask(task.id, 'running')
    await engine.transitionTask(task.id, 'completed')
    logs.length = 0

    const res = await app.request(`/tasks/${task.id}/events`)
    expect(res.status).toBe(200)
    // Drain the SSE stream so the response completes
    const reader = res.body!.getReader()
    while (true) {
      const { done } = await reader.read()
      if (done) break
    }
    expect(logs).toHaveLength(1)
    expect(logs[0]).toContain('SSE')
    expect(logs[0]).toContain('subscriber connected')
  })

  it('logs GET /events as global subscriber connected', async () => {
    const { app, logs } = makeVerboseApp()
    logs.length = 0

    const res = await app.request('/events')
    expect(res.status).toBe(200)
    // Cancel the stream to let the response finish
    res.body!.getReader().cancel()
    // Wait a tick for the log to be flushed
    await new Promise((r) => setTimeout(r, 50))
    expect(logs).toHaveLength(1)
    expect(logs[0]).toContain('SSE')
    expect(logs[0]).toContain('global subscriber connected')
  })

  it('logs PATCH status transition without target status in body', async () => {
    const { app, engine, logs } = makeVerboseApp()
    const task = await engine.createTask({ type: 'test' })
    logs.length = 0

    // Send PATCH with non-JSON body to trigger the catch block (line 29)
    // and the 'status transition' fallback (lines 71-72)
    await app.request(`/tasks/${task.id}/status`, {
      method: 'PATCH',
      headers: { 'Content-Type': 'text/plain' },
      body: 'not json',
    })
    expect(logs).toHaveLength(1)
    expect(logs[0]).toContain('PATCH')
    expect(logs[0]).toContain('status transition')
  })

  it('logs POST /tasks/:id/events with batch events context', async () => {
    const { app, engine, logs } = makeVerboseApp()
    const task = await engine.createTask({ type: 'test' })
    await engine.transitionTask(task.id, 'running')
    logs.length = 0

    const res = await app.request(`/tasks/${task.id}/events`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify([
        { type: 'a', level: 'info', data: null },
        { type: 'b', level: 'info', data: null },
      ]),
    })
    expect(res.status).toBe(201)
    expect(logs).toHaveLength(1)
    expect(logs[0]).toContain('2 events')
  })

  it('logs POST /tasks/:id/events without type as event published', async () => {
    const { app, engine, logs } = makeVerboseApp()
    const task = await engine.createTask({ type: 'test' })
    await engine.transitionTask(task.id, 'running')
    logs.length = 0

    // Send a non-JSON body to POST /tasks/:id/events so requestBody is undefined
    // This triggers the 'event published' fallback (line 84)
    await app.request(`/tasks/${task.id}/events`, {
      method: 'POST',
      headers: { 'Content-Type': 'text/plain' },
      body: 'not json',
    })
    expect(logs).toHaveLength(1)
    expect(logs[0]).toContain('event published')
  })

  it('logs POST /tasks/:id/resolve as resolve', async () => {
    const { app, engine, logs } = makeVerboseApp()
    const task = await engine.createTask({ type: 'test' })
    await engine.transitionTask(task.id, 'running')
    await engine.transitionTask(task.id, 'blocked')
    logs.length = 0

    const res = await app.request(`/tasks/${task.id}/resolve`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ data: { approved: true } }),
    })
    expect(res.status).toBe(200)
    expect(logs).toHaveLength(1)
    expect(logs[0]).toContain('POST')
    expect(logs[0]).toContain('/resolve')
    expect(logs[0]).toContain('resolve')
  })

  it('logs "body too large to log" when Content-Length exceeds 64KB and handler still receives body', async () => {
    const { app, engine, logs } = makeVerboseApp()
    const task = await engine.createTask({ type: 'test' })
    await engine.transitionTask(task.id, 'running')
    logs.length = 0

    // Create a body larger than 64KB
    const largeData = 'x'.repeat(65537)
    const body = JSON.stringify({ type: 'llm.delta', level: 'info', data: { text: largeData } })

    const res = await app.request(`/tasks/${task.id}/events`, {
      method: 'POST',
      headers: {
        'Content-Type': 'application/json',
        'Content-Length': String(Buffer.byteLength(body)),
      },
      body,
    })
    // Handler should still receive the full body and process the request normally
    expect(res.status).toBe(201)
    expect(logs).toHaveLength(1)
    expect(logs[0]).toContain('body too large to log')
  })
})
