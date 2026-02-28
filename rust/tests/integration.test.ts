import { describe, it, expect } from 'vitest'

const BASE_URL = process.env['TASKCAST_TEST_URL'] ?? 'http://localhost:3799'

async function api(path: string, opts?: RequestInit) {
  const res = await fetch(`${BASE_URL}${path}`, {
    headers: { 'Content-Type': 'application/json', ...opts?.headers },
    ...opts,
  })
  return { status: res.status, data: await res.json().catch(() => null), headers: res.headers }
}

describe('Rust server API compatibility', () => {
  it('POST /tasks creates a task', async () => {
    const { status, data } = await api('/tasks', {
      method: 'POST',
      body: JSON.stringify({ type: 'test', params: { key: 'value' } }),
    })
    expect(status).toBe(201)
    expect(data.id).toBeDefined()
    expect(data.status).toBe('pending')
    expect(data.type).toBe('test')
    expect(data.params).toEqual({ key: 'value' })
    expect(data.createdAt).toBeTypeOf('number')
    expect(data.updatedAt).toBeTypeOf('number')
  })

  it('GET /tasks/:id returns the task', async () => {
    const { data: task } = await api('/tasks', {
      method: 'POST',
      body: JSON.stringify({ type: 'get-test' }),
    })
    const { status, data } = await api(`/tasks/${task.id}`)
    expect(status).toBe(200)
    expect(data.id).toBe(task.id)
    expect(data.status).toBe('pending')
  })

  it('GET /tasks/:id returns 404 for nonexistent', async () => {
    const { status } = await api('/tasks/nonexistent-id')
    expect(status).toBe(404)
  })

  it('PATCH /tasks/:id/status transitions status', async () => {
    const { data: task } = await api('/tasks', {
      method: 'POST',
      body: JSON.stringify({}),
    })
    const { status, data } = await api(`/tasks/${task.id}/status`, {
      method: 'PATCH',
      body: JSON.stringify({ status: 'running' }),
    })
    expect(status).toBe(200)
    expect(data.status).toBe('running')
  })

  it('PATCH /tasks/:id/status rejects invalid transition', async () => {
    const { data: task } = await api('/tasks', {
      method: 'POST',
      body: JSON.stringify({}),
    })
    const { status } = await api(`/tasks/${task.id}/status`, {
      method: 'PATCH',
      body: JSON.stringify({ status: 'completed' }),
    })
    expect(status).toBe(400)
  })

  it('PATCH /tasks/:id/status sets completedAt for terminal', async () => {
    const { data: task } = await api('/tasks', {
      method: 'POST',
      body: JSON.stringify({}),
    })
    await api(`/tasks/${task.id}/status`, {
      method: 'PATCH',
      body: JSON.stringify({ status: 'running' }),
    })
    const { data } = await api(`/tasks/${task.id}/status`, {
      method: 'PATCH',
      body: JSON.stringify({ status: 'completed', result: { output: 'done' } }),
    })
    expect(data.status).toBe('completed')
    expect(data.completedAt).toBeTypeOf('number')
    expect(data.result).toEqual({ output: 'done' })
  })

  it('POST /tasks/:id/events publishes single event', async () => {
    const { data: task } = await api('/tasks', { method: 'POST', body: JSON.stringify({}) })
    await api(`/tasks/${task.id}/status`, { method: 'PATCH', body: JSON.stringify({ status: 'running' }) })

    const { status, data } = await api(`/tasks/${task.id}/events`, {
      method: 'POST',
      body: JSON.stringify({ type: 'llm.delta', level: 'info', data: { text: 'hello' } }),
    })
    expect(status).toBe(201)
    expect(data.type).toBe('llm.delta')
    expect(data.level).toBe('info')
    expect(data.data).toEqual({ text: 'hello' })
    expect(data.taskId).toBe(task.id)
    expect(data.index).toBe(1) // index 0 is the taskcast:status event from transition
  })

  it('POST /tasks/:id/events publishes batch', async () => {
    const { data: task } = await api('/tasks', { method: 'POST', body: JSON.stringify({}) })
    await api(`/tasks/${task.id}/status`, { method: 'PATCH', body: JSON.stringify({ status: 'running' }) })

    const { status, data } = await api(`/tasks/${task.id}/events`, {
      method: 'POST',
      body: JSON.stringify([
        { type: 'a', level: 'info', data: null },
        { type: 'b', level: 'debug', data: { x: 1 } },
      ]),
    })
    expect(status).toBe(201)
    expect(Array.isArray(data)).toBe(true)
    expect(data).toHaveLength(2)
    expect(data[0].type).toBe('a')
    expect(data[1].type).toBe('b')
  })

  it('POST /tasks/:id/events rejects on terminal task', async () => {
    const { data: task } = await api('/tasks', { method: 'POST', body: JSON.stringify({}) })
    await api(`/tasks/${task.id}/status`, { method: 'PATCH', body: JSON.stringify({ status: 'cancelled' }) })

    const { status } = await api(`/tasks/${task.id}/events`, {
      method: 'POST',
      body: JSON.stringify({ type: 'x', level: 'info', data: null }),
    })
    expect(status).toBe(400)
  })

  it('GET /tasks/:id/events/history returns events', async () => {
    const { data: task } = await api('/tasks', { method: 'POST', body: JSON.stringify({}) })
    await api(`/tasks/${task.id}/status`, { method: 'PATCH', body: JSON.stringify({ status: 'running' }) })
    await api(`/tasks/${task.id}/events`, {
      method: 'POST',
      body: JSON.stringify({ type: 'evt1', level: 'info', data: null }),
    })

    const { status, data } = await api(`/tasks/${task.id}/events/history`)
    expect(status).toBe(200)
    expect(Array.isArray(data)).toBe(true)
    expect(data.length).toBeGreaterThanOrEqual(2) // status event + evt1
  })

  it('GET /tasks/:id/events/history with since.index', async () => {
    const { data: task } = await api('/tasks', { method: 'POST', body: JSON.stringify({}) })
    await api(`/tasks/${task.id}/status`, { method: 'PATCH', body: JSON.stringify({ status: 'running' }) })
    await api(`/tasks/${task.id}/events`, {
      method: 'POST',
      body: JSON.stringify({ type: 'evt1', level: 'info', data: null }),
    })
    await api(`/tasks/${task.id}/events`, {
      method: 'POST',
      body: JSON.stringify({ type: 'evt2', level: 'info', data: null }),
    })

    // Events: taskcast:status (index 0), evt1 (index 1), evt2 (index 2)
    // since.index=1 returns events with filteredIndex > 1, so evt2 only
    const { data } = await api(`/tasks/${task.id}/events/history?since.index=1`)
    expect(data.length).toBe(1)
    expect(data[0].type).toBe('evt2')
  })

  it('full lifecycle: create -> run -> events -> complete', async () => {
    // Create
    const { data: task } = await api('/tasks', {
      method: 'POST',
      body: JSON.stringify({ type: 'lifecycle', metadata: { test: true } }),
    })
    expect(task.status).toBe('pending')

    // Transition to running
    const { data: running } = await api(`/tasks/${task.id}/status`, {
      method: 'PATCH',
      body: JSON.stringify({ status: 'running' }),
    })
    expect(running.status).toBe('running')

    // Publish events
    await api(`/tasks/${task.id}/events`, {
      method: 'POST',
      body: JSON.stringify({ type: 'progress', level: 'info', data: { percent: 50 } }),
    })
    await api(`/tasks/${task.id}/events`, {
      method: 'POST',
      body: JSON.stringify({ type: 'progress', level: 'info', data: { percent: 100 } }),
    })

    // Complete
    const { data: completed } = await api(`/tasks/${task.id}/status`, {
      method: 'PATCH',
      body: JSON.stringify({ status: 'completed', result: { items: 42 } }),
    })
    expect(completed.status).toBe('completed')
    expect(completed.result).toEqual({ items: 42 })
    expect(completed.completedAt).toBeTypeOf('number')

    // Verify history
    const { data: events } = await api(`/tasks/${task.id}/events/history`)
    expect(events.length).toBeGreaterThanOrEqual(4) // 2 status + 2 progress
  })
})
