import { describe, it, expect, beforeAll, afterAll } from 'vitest'
import { startServer, type TestServer } from '../helpers/server.js'

describe('Task Lifecycle API', () => {
  let server: TestServer

  beforeAll(async () => {
    server = await startServer()
  })

  afterAll(() => {
    server.close()
  })

  // ─── Create ───────────────────────────────────────────────────────────────

  it('creates a task and returns 201 with id, status, type', async () => {
    const res = await fetch(`${server.baseUrl}/tasks`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ type: 'test' }),
    })

    expect(res.status).toBe(201)
    const task = await res.json()
    expect(task.id).toBeDefined()
    expect(task.status).toBe('pending')
    expect(task.type).toBe('test')
    expect(task.createdAt).toBeDefined()
    expect(task.updatedAt).toBeDefined()
  })

  it('creates a task with explicit ID', async () => {
    const id = 'explicit-id-' + Date.now()
    const res = await fetch(`${server.baseUrl}/tasks`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ id, type: 'custom' }),
    })

    expect(res.status).toBe(201)
    const task = await res.json()
    expect(task.id).toBe(id)
    expect(task.type).toBe('custom')
  })

  it('rejects duplicate user-supplied task ID with 409', async () => {
    const id = 'dup-id-' + Date.now()

    const res1 = await fetch(`${server.baseUrl}/tasks`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ id, type: 'first' }),
    })
    expect(res1.status).toBe(201)

    // Second create with same ID is rejected
    const res2 = await fetch(`${server.baseUrl}/tasks`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ id, type: 'second' }),
    })
    expect(res2.status).toBe(409)
    const body = await res2.json()
    expect(body.error).toContain('already exists')

    // Original task is untouched
    const getRes = await fetch(`${server.baseUrl}/tasks/${id}`)
    expect(getRes.status).toBe(200)
    const task = await getRes.json()
    expect(task.type).toBe('first')
  })

  // ─── Get ──────────────────────────────────────────────────────────────────

  it('gets a task by ID with hot field', async () => {
    const createRes = await fetch(`${server.baseUrl}/tasks`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ type: 'get-test' }),
    })
    const { id } = await createRes.json()

    const res = await fetch(`${server.baseUrl}/tasks/${id}`)
    expect(res.status).toBe(200)
    const task = await res.json()
    expect(task.id).toBe(id)
    expect(task.hot).toBe(false)
    expect(task.subscriberCount).toBe(0)
  })

  it('returns 404 for unknown task', async () => {
    const res = await fetch(`${server.baseUrl}/tasks/nonexistent-task-id`)
    expect(res.status).toBe(404)
    const body = await res.json()
    expect(body.error).toBeDefined()
  })

  // ─── List ─────────────────────────────────────────────────────────────────

  it('lists tasks and returns an array', async () => {
    // Ensure at least one task exists
    await fetch(`${server.baseUrl}/tasks`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ type: 'list-test' }),
    })

    const res = await fetch(`${server.baseUrl}/tasks`)
    expect(res.status).toBe(200)
    const body = await res.json()
    expect(body.tasks).toBeInstanceOf(Array)
    expect(body.tasks.length).toBeGreaterThan(0)
  })

  it('lists tasks with status filter', async () => {
    const createRes = await fetch(`${server.baseUrl}/tasks`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ type: 'filter-test' }),
    })
    const { id } = await createRes.json()

    // Transition to running
    await fetch(`${server.baseUrl}/tasks/${id}/status`, {
      method: 'PATCH',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ status: 'running' }),
    })

    const runningRes = await fetch(`${server.baseUrl}/tasks?status=running`)
    expect(runningRes.status).toBe(200)
    const runningBody = await runningRes.json()
    expect(runningBody.tasks.some((t: { id: string }) => t.id === id)).toBe(true)

    const completedRes = await fetch(`${server.baseUrl}/tasks?status=completed`)
    expect(completedRes.status).toBe(200)
    const completedBody = await completedRes.json()
    expect(completedBody.tasks.some((t: { id: string }) => t.id === id)).toBe(false)
  })

  // ─── Events ───────────────────────────────────────────────────────────────

  it('publishes events to a running task', async () => {
    const createRes = await fetch(`${server.baseUrl}/tasks`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ type: 'event-test' }),
    })
    const { id } = await createRes.json()

    // Transition to running first
    await fetch(`${server.baseUrl}/tasks/${id}/status`, {
      method: 'PATCH',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ status: 'running' }),
    })

    const res = await fetch(`${server.baseUrl}/tasks/${id}/events`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ type: 'log', level: 'info', data: { message: 'hello' } }),
    })

    expect(res.status).toBe(201)
    const event = await res.json()
    expect(event.type).toBe('log')
    expect(event.level).toBe('info')
    expect(event.data).toEqual({ message: 'hello' })
    expect(event.taskId).toBe(id)
  })

  it('gets event history', async () => {
    const createRes = await fetch(`${server.baseUrl}/tasks`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ type: 'history-test' }),
    })
    const { id } = await createRes.json()

    // Transition to running
    await fetch(`${server.baseUrl}/tasks/${id}/status`, {
      method: 'PATCH',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ status: 'running' }),
    })

    // Publish two events
    await fetch(`${server.baseUrl}/tasks/${id}/events`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ type: 'log', level: 'info', data: { step: 1 } }),
    })
    await fetch(`${server.baseUrl}/tasks/${id}/events`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ type: 'log', level: 'info', data: { step: 2 } }),
    })

    const res = await fetch(`${server.baseUrl}/tasks/${id}/events/history`)
    expect(res.status).toBe(200)
    const events = await res.json()
    // History includes the taskcast:status event from transition + our 2 log events
    expect(events.length).toBeGreaterThanOrEqual(2)
    const logEvents = events.filter((e: { type: string }) => e.type === 'log')
    expect(logEvents.length).toBe(2)
    expect(logEvents[0].data).toEqual({ step: 1 })
    expect(logEvents[1].data).toEqual({ step: 2 })
  })

  // ─── Transitions ──────────────────────────────────────────────────────────

  it('transitions through full lifecycle: pending -> running -> completed with result', async () => {
    const createRes = await fetch(`${server.baseUrl}/tasks`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ type: 'lifecycle-test' }),
    })
    const { id } = await createRes.json()

    // pending -> running
    const runRes = await fetch(`${server.baseUrl}/tasks/${id}/status`, {
      method: 'PATCH',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ status: 'running' }),
    })
    expect(runRes.status).toBe(200)
    const runTask = await runRes.json()
    expect(runTask.status).toBe('running')

    // running -> completed with result
    const completeRes = await fetch(`${server.baseUrl}/tasks/${id}/status`, {
      method: 'PATCH',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ status: 'completed', result: { output: 'done' } }),
    })
    expect(completeRes.status).toBe(200)
    const completedTask = await completeRes.json()
    expect(completedTask.status).toBe('completed')
    expect(completedTask.result).toEqual({ output: 'done' })
  })

  it('rejects invalid transition (completed -> running) with 400', async () => {
    const createRes = await fetch(`${server.baseUrl}/tasks`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ type: 'invalid-transition' }),
    })
    const { id } = await createRes.json()

    // Move to terminal state
    await fetch(`${server.baseUrl}/tasks/${id}/status`, {
      method: 'PATCH',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ status: 'running' }),
    })
    await fetch(`${server.baseUrl}/tasks/${id}/status`, {
      method: 'PATCH',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ status: 'completed' }),
    })

    // Try invalid transition
    const res = await fetch(`${server.baseUrl}/tasks/${id}/status`, {
      method: 'PATCH',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ status: 'running' }),
    })
    expect(res.status).toBe(400)
    const body = await res.json()
    expect(body.error).toBeDefined()
  })

  it('transitions to failed with error payload', async () => {
    const createRes = await fetch(`${server.baseUrl}/tasks`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ type: 'fail-test' }),
    })
    const { id } = await createRes.json()

    await fetch(`${server.baseUrl}/tasks/${id}/status`, {
      method: 'PATCH',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ status: 'running' }),
    })

    const res = await fetch(`${server.baseUrl}/tasks/${id}/status`, {
      method: 'PATCH',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({
        status: 'failed',
        error: { message: 'Something broke', code: 'ERR_TEST', details: { reason: 'unit test' } },
      }),
    })
    expect(res.status).toBe(200)
    const task = await res.json()
    expect(task.status).toBe('failed')
    expect(task.error).toEqual({
      message: 'Something broke',
      code: 'ERR_TEST',
      details: { reason: 'unit test' },
    })
  })
})