import { describe, it, expect, beforeEach } from 'vitest'
import { TaskEngine, MemoryBroadcastProvider, MemoryShortTermStore } from '@taskcast/core'
import { createTaskcastApp } from '@taskcast/server'
import type { TaskcastApp } from '@taskcast/server'
import { pingServer } from '../../src/commands/ping.js'
import { runDoctor, formatDoctorResult } from '../../src/commands/doctor.js'
import { formatTaskList, formatTaskInspect } from '../../src/commands/tasks.js'
import { formatEvent } from '../../src/commands/logs.js'

// ─── Shared test infrastructure ──────────────────────────────────────────────

let taskcastApp: TaskcastApp
let engine: TaskEngine

/**
 * Build a fetch-like function that delegates to Hono's app.request().
 * This lets us call pingServer / runDoctor against the in-memory server
 * without binding to a real TCP port.
 */
function appFetch(url: string, init?: RequestInit): Promise<Response> {
  const parsed = new URL(url)
  const path = parsed.pathname + parsed.search
  return taskcastApp.app.request(path, init)
}

// ─── Setup ───────────────────────────────────────────────────────────────────

beforeEach(() => {
  const store = new MemoryShortTermStore()
  const broadcast = new MemoryBroadcastProvider()
  engine = new TaskEngine({ shortTermStore: store, broadcast })
  taskcastApp = createTaskcastApp({ engine, auth: { mode: 'none' } })
})

// ─── 1. Ping via HTTP ────────────────────────────────────────────────────────

describe('ping via HTTP', () => {
  it('returns OK when server is healthy', async () => {
    const result = await pingServer('http://localhost', appFetch as typeof fetch)
    expect(result.ok).toBe(true)
    expect(result.latencyMs).toBeTypeOf('number')
    expect(result.latencyMs).toBeGreaterThanOrEqual(0)
  })

  it('verifies /health endpoint returns { ok: true }', async () => {
    const res = await taskcastApp.app.request('/health')
    expect(res.status).toBe(200)
    const body = await res.json()
    expect(body).toEqual({ ok: true })
  })
})

// ─── 2. Doctor via HTTP ──────────────────────────────────────────────────────

describe('doctor via HTTP', () => {
  it('returns full health detail', async () => {
    const res = await taskcastApp.app.request('/health/detail')
    expect(res.status).toBe(200)
    const body = await res.json()
    expect(body.ok).toBe(true)
    expect(body.uptime).toBeTypeOf('number')
    expect(body.auth.mode).toBe('none')
    expect(body.adapters.broadcast).toEqual({ provider: 'memory', status: 'ok' })
    expect(body.adapters.shortTermStore).toEqual({ provider: 'memory', status: 'ok' })
  })

  it('runDoctor reports all OK against live server', async () => {
    const node = { url: 'http://localhost' }
    const result = await runDoctor(node, appFetch as typeof fetch)

    expect(result.server.ok).toBe(true)
    expect(result.server.uptime).toBeTypeOf('number')
    expect(result.auth.status).toBe('ok')
    expect(result.auth.mode).toBe('none')
    expect(result.adapters.broadcast.provider).toBe('memory')
    expect(result.adapters.shortTermStore.provider).toBe('memory')
  })

  it('formatDoctorResult produces expected output from live data', async () => {
    const node = { url: 'http://localhost' }
    const result = await runDoctor(node, appFetch as typeof fetch)
    const output = formatDoctorResult(result)

    expect(output).toContain('Server:    OK  taskcast at http://localhost')
    expect(output).toContain('Auth:      OK  none')
    expect(output).toContain('Broadcast: OK  memory')
    expect(output).toContain('ShortTerm: OK  memory')
    expect(output).toContain('LongTerm:  SKIP  not configured')
  })
})

// ─── 3. Tasks list flow ──────────────────────────────────────────────────────

describe('tasks list flow', () => {
  it('returns empty list when no tasks exist', async () => {
    const res = await taskcastApp.app.request('/tasks')
    expect(res.status).toBe(200)
    const body = await res.json()
    expect(body.tasks).toEqual([])
  })

  it('lists tasks created via engine', async () => {
    await engine.createTask({ type: 'llm.chat' })
    await engine.createTask({ type: 'agent.step' })

    const res = await taskcastApp.app.request('/tasks')
    expect(res.status).toBe(200)
    const body = await res.json()
    expect(body.tasks).toHaveLength(2)

    const types = body.tasks.map((t: any) => t.type)
    expect(types).toContain('llm.chat')
    expect(types).toContain('agent.step')
  })

  it('formatTaskList renders rows from HTTP response', async () => {
    const t1 = await engine.createTask({ type: 'llm.chat' })
    const t2 = await engine.createTask({ type: 'batch.job' })

    const res = await taskcastApp.app.request('/tasks')
    const body = await res.json()
    const output = formatTaskList(body.tasks)

    expect(output).toContain('ID')
    expect(output).toContain('TYPE')
    expect(output).toContain('STATUS')
    expect(output).toContain(t1.id)
    expect(output).toContain(t2.id)
    expect(output).toContain('llm.chat')
    expect(output).toContain('batch.job')
    expect(output).toContain('pending')
  })

  it('formatTaskList returns "No tasks found." for empty response', async () => {
    const res = await taskcastApp.app.request('/tasks')
    const body = await res.json()
    const output = formatTaskList(body.tasks)
    expect(output).toBe('No tasks found.')
  })
})

// ─── 4. Tasks inspect flow ───────────────────────────────────────────────────

describe('tasks inspect flow', () => {
  it('retrieves task by ID', async () => {
    const task = await engine.createTask({ type: 'llm.chat', params: { prompt: 'hi' } })

    const res = await taskcastApp.app.request(`/tasks/${task.id}`)
    expect(res.status).toBe(200)
    const body = await res.json()
    expect(body.id).toBe(task.id)
    expect(body.type).toBe('llm.chat')
    expect(body.status).toBe('pending')
    expect(body.params).toEqual({ prompt: 'hi' })
  })

  it('returns 404 for non-existent task', async () => {
    const res = await taskcastApp.app.request('/tasks/nonexistent')
    expect(res.status).toBe(404)
  })

  it('retrieves event history for a task', async () => {
    const task = await engine.createTask({ type: 'llm.chat' })
    await engine.transitionTask(task.id, 'running')
    await engine.publishEvent(task.id, { type: 'llm.delta', level: 'info', data: { delta: 'Hello' } })
    await engine.publishEvent(task.id, { type: 'llm.delta', level: 'info', data: { delta: ' world' } })

    const res = await taskcastApp.app.request(`/tasks/${task.id}/events/history`)
    expect(res.status).toBe(200)
    const events = await res.json()
    // transitionTask emits a taskcast:status event, so we get 3 events total:
    // [0] taskcast:status (from transition), [1] llm.delta, [2] llm.delta
    expect(events).toHaveLength(3)
    expect(events[0].type).toBe('taskcast:status')
    expect(events[1].type).toBe('llm.delta')
    expect(events[1].data).toEqual({ delta: 'Hello' })
    expect(events[2].type).toBe('llm.delta')
    expect(events[2].data).toEqual({ delta: ' world' })
  })

  it('formatTaskInspect produces correct output from real data', async () => {
    const task = await engine.createTask({ type: 'llm.chat', params: { model: 'gpt-4' } })
    await engine.transitionTask(task.id, 'running')
    await engine.publishEvent(task.id, { type: 'llm.delta', level: 'info', data: { delta: 'hi' } })

    const taskRes = await taskcastApp.app.request(`/tasks/${task.id}`)
    const taskBody = await taskRes.json()

    const histRes = await taskcastApp.app.request(`/tasks/${task.id}/events/history`)
    const events = await histRes.json()

    const output = formatTaskInspect(taskBody, events)

    expect(output).toContain(`Task: ${task.id}`)
    expect(output).toContain('Type:    llm.chat')
    expect(output).toContain('Status:  running')
    expect(output).toContain('Params:  {"model":"gpt-4"}')
    // 2 events: taskcast:status (from transition) + llm.delta
    expect(output).toContain('Recent Events (last 2):')
    expect(output).toContain('llm.delta')
    expect(output).toContain('info')
  })

  it('formatTaskInspect shows "No events." when task has no events', async () => {
    const task = await engine.createTask({ type: 'llm.chat' })

    const taskRes = await taskcastApp.app.request(`/tasks/${task.id}`)
    const taskBody = await taskRes.json()

    const histRes = await taskcastApp.app.request(`/tasks/${task.id}/events/history`)
    const events = await histRes.json()

    const output = formatTaskInspect(taskBody, events)
    expect(output).toContain('No events.')
    expect(output).not.toContain('Recent Events')
  })
})

// ─── 5. Full lifecycle ───────────────────────────────────────────────────────

describe('full lifecycle', () => {
  it('create -> running -> publish events -> completed', async () => {
    // 1. Create task via HTTP
    const createRes = await taskcastApp.app.request('/tasks', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ type: 'llm.chat', params: { prompt: 'hello' } }),
    })
    expect(createRes.status).toBe(201)
    const created = await createRes.json()
    expect(created.status).toBe('pending')
    expect(created.type).toBe('llm.chat')
    const taskId = created.id

    // 2. Transition to running
    const runRes = await taskcastApp.app.request(`/tasks/${taskId}/status`, {
      method: 'PATCH',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ status: 'running' }),
    })
    expect(runRes.status).toBe(200)
    const running = await runRes.json()
    expect(running.status).toBe('running')

    // 3. Publish events
    const evtRes = await taskcastApp.app.request(`/tasks/${taskId}/events`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ type: 'llm.delta', level: 'info', data: { delta: 'response text' } }),
    })
    expect(evtRes.status).toBe(201)
    const evt = await evtRes.json()
    expect(evt.type).toBe('llm.delta')
    expect(evt.level).toBe('info')
    expect(evt.data).toEqual({ delta: 'response text' })

    // 4. Complete the task
    const completeRes = await taskcastApp.app.request(`/tasks/${taskId}/status`, {
      method: 'PATCH',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ status: 'completed', result: { tokens: 42 } }),
    })
    expect(completeRes.status).toBe(200)
    const completed = await completeRes.json()
    expect(completed.status).toBe('completed')
    expect(completed.result).toEqual({ tokens: 42 })

    // 5. Verify final state via GET
    const getRes = await taskcastApp.app.request(`/tasks/${taskId}`)
    expect(getRes.status).toBe(200)
    const final = await getRes.json()
    expect(final.status).toBe('completed')
    expect(final.result).toEqual({ tokens: 42 })

    // 6. Verify event history
    // Each transitionTask call emits a taskcast:status event automatically.
    // So: taskcast:status (running) + llm.delta + taskcast:status (completed) = 3 events
    const histRes = await taskcastApp.app.request(`/tasks/${taskId}/events/history`)
    expect(histRes.status).toBe(200)
    const events = await histRes.json()
    expect(events).toHaveLength(3)
    expect(events[0].type).toBe('taskcast:status')
    expect(events[1].type).toBe('llm.delta')
    expect(events[2].type).toBe('taskcast:status')
  })

  it('cannot transition backward from terminal state', async () => {
    const task = await engine.createTask({ type: 'test' })
    await engine.transitionTask(task.id, 'running')
    await engine.transitionTask(task.id, 'completed')

    const res = await taskcastApp.app.request(`/tasks/${task.id}/status`, {
      method: 'PATCH',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ status: 'running' }),
    })
    expect(res.status).toBe(400)
  })

  it('create -> running -> fail', async () => {
    const task = await engine.createTask({ type: 'flaky-task' })
    await engine.transitionTask(task.id, 'running')

    const failRes = await taskcastApp.app.request(`/tasks/${task.id}/status`, {
      method: 'PATCH',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({
        status: 'failed',
        error: { message: 'out of memory', code: 'OOM' },
      }),
    })
    expect(failRes.status).toBe(200)
    const failed = await failRes.json()
    expect(failed.status).toBe('failed')
    expect(failed.error.message).toBe('out of memory')
    expect(failed.error.code).toBe('OOM')
  })

  it('batch event publishing', async () => {
    const task = await engine.createTask({ type: 'batch' })
    await engine.transitionTask(task.id, 'running')

    const batchRes = await taskcastApp.app.request(`/tasks/${task.id}/events`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify([
        { type: 'step', level: 'info', data: { step: 1 } },
        { type: 'step', level: 'info', data: { step: 2 } },
        { type: 'step', level: 'info', data: { step: 3 } },
      ]),
    })
    expect(batchRes.status).toBe(201)
    const events = await batchRes.json()
    expect(events).toHaveLength(3)
    expect(events[0].data).toEqual({ step: 1 })
    expect(events[2].data).toEqual({ step: 3 })

    // Verify all events in history
    // 1 taskcast:status (from transition to running) + 3 step events = 4 total
    const histRes = await taskcastApp.app.request(`/tasks/${task.id}/events/history`)
    const history = await histRes.json()
    expect(history).toHaveLength(4)
    expect(history[0].type).toBe('taskcast:status')
    expect(history[1].data).toEqual({ step: 1 })
    expect(history[2].data).toEqual({ step: 2 })
    expect(history[3].data).toEqual({ step: 3 })
  })
})

// ─── 6. Filtering ────────────────────────────────────────────────────────────

describe('filtering', () => {
  it('filters tasks by type via query param', async () => {
    await engine.createTask({ type: 'llm.chat' })
    await engine.createTask({ type: 'llm.embed' })
    await engine.createTask({ type: 'agent.step' })

    // The server route uses filter.types = [type] for exact match
    const res = await taskcastApp.app.request('/tasks?type=llm.chat')
    expect(res.status).toBe(200)
    const body = await res.json()
    expect(body.tasks).toHaveLength(1)
    expect(body.tasks[0].type).toBe('llm.chat')
  })

  it('filters tasks by status via query param', async () => {
    const t1 = await engine.createTask({ type: 'a' })
    const t2 = await engine.createTask({ type: 'b' })
    await engine.transitionTask(t1.id, 'running')

    const res = await taskcastApp.app.request('/tasks?status=running')
    expect(res.status).toBe(200)
    const body = await res.json()
    expect(body.tasks).toHaveLength(1)
    expect(body.tasks[0].id).toBe(t1.id)
    expect(body.tasks[0].status).toBe('running')
  })

  it('returns all tasks when no filters applied', async () => {
    await engine.createTask({ type: 'a' })
    await engine.createTask({ type: 'b' })
    await engine.createTask({ type: 'c' })

    const res = await taskcastApp.app.request('/tasks')
    expect(res.status).toBe(200)
    const body = await res.json()
    expect(body.tasks).toHaveLength(3)
  })

  it('combined status + type filtering', async () => {
    const t1 = await engine.createTask({ type: 'llm.chat' })
    const t2 = await engine.createTask({ type: 'llm.chat' })
    const t3 = await engine.createTask({ type: 'agent.step' })
    await engine.transitionTask(t1.id, 'running')

    const res = await taskcastApp.app.request('/tasks?status=running&type=llm.chat')
    expect(res.status).toBe(200)
    const body = await res.json()
    expect(body.tasks).toHaveLength(1)
    expect(body.tasks[0].id).toBe(t1.id)
  })
})

// ─── 7. Verbose mode ─────────────────────────────────────────────────────────

describe('verbose mode', () => {
  it('captures request logs when verbose is enabled', async () => {
    const logs: string[] = []
    const verboseApp = createTaskcastApp({
      engine,
      auth: { mode: 'none' },
      verbose: true,
      verboseLogger: (line) => logs.push(line),
    })

    // Make a request that should be logged
    await verboseApp.app.request('/health')

    expect(logs.length).toBeGreaterThanOrEqual(1)
    expect(logs[0]).toContain('GET')
    expect(logs[0]).toContain('/health')
    expect(logs[0]).toContain('200')

    verboseApp.stop()
  })

  it('logs POST task creation', async () => {
    const logs: string[] = []
    const verboseApp = createTaskcastApp({
      engine,
      auth: { mode: 'none' },
      verbose: true,
      verboseLogger: (line) => logs.push(line),
    })

    await verboseApp.app.request('/tasks', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ type: 'test' }),
    })

    expect(logs.length).toBeGreaterThanOrEqual(1)
    const postLog = logs.find((l) => l.includes('POST'))
    expect(postLog).toBeDefined()
    expect(postLog).toContain('/tasks')
    expect(postLog).toContain('201')

    verboseApp.stop()
  })

  it('does not log when verbose is disabled', async () => {
    const logs: string[] = []
    const quietApp = createTaskcastApp({
      engine,
      auth: { mode: 'none' },
      verbose: false,
      verboseLogger: (line) => logs.push(line),
    })

    await quietApp.app.request('/health')
    expect(logs).toHaveLength(0)

    quietApp.stop()
  })
})

// ─── 8. formatEvent with real server data ────────────────────────────────────

describe('formatEvent with real server data', () => {
  it('formats events fetched from the server', async () => {
    const task = await engine.createTask({ type: 'llm.chat' })
    await engine.transitionTask(task.id, 'running')
    await engine.publishEvent(task.id, {
      type: 'llm.delta',
      level: 'info',
      data: { delta: 'Hello' },
    })

    const histRes = await taskcastApp.app.request(`/tasks/${task.id}/events/history`)
    const events = await histRes.json()

    // events[0] is taskcast:status from the transition, events[1] is our llm.delta
    const formatted = formatEvent(events[1])
    expect(formatted).toMatch(/\[\d{2}:\d{2}:\d{2}\]/)
    expect(formatted).toContain('llm.delta')
    expect(formatted).toContain('info')
    expect(formatted).toContain('{"delta":"Hello"}')
  })

  it('formats events with taskId prefix', async () => {
    const task = await engine.createTask({ type: 'agent.step' })
    await engine.transitionTask(task.id, 'running')
    await engine.publishEvent(task.id, {
      type: 'agent.step',
      level: 'info',
      data: { step: 1 },
    })

    const histRes = await taskcastApp.app.request(`/tasks/${task.id}/events/history`)
    const events = await histRes.json()

    // events[0] is taskcast:status from the transition, events[1] is our agent.step
    const formatted = formatEvent(events[1], task.id)
    expect(formatted).toContain(`${task.id.slice(0, 7)}..`)
    expect(formatted).toContain('agent.step')
  })

  it('formats the auto-emitted taskcast:status event', async () => {
    const task = await engine.createTask({ type: 'test' })
    await engine.transitionTask(task.id, 'running')

    const histRes = await taskcastApp.app.request(`/tasks/${task.id}/events/history`)
    const events = await histRes.json()

    const formatted = formatEvent(events[0])
    expect(formatted).toMatch(/\[\d{2}:\d{2}:\d{2}\]/)
    expect(formatted).toContain('taskcast:status')
    expect(formatted).toContain('info')
  })
})

// ─── 9. Error cases ──────────────────────────────────────────────────────────

describe('error cases', () => {
  it('rejects invalid task creation body', async () => {
    const res = await taskcastApp.app.request('/tasks', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ ttl: 'not-a-number' }),
    })
    expect(res.status).toBe(400)
  })

  it('rejects publishing events to non-existent task', async () => {
    const res = await taskcastApp.app.request('/tasks/nonexistent/events', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ type: 'test', level: 'info', data: {} }),
    })
    expect(res.status).toBe(404)
  })

  it('rejects invalid status transition', async () => {
    const task = await engine.createTask({ type: 'test' })

    // pending -> completed is invalid (must go through running first)
    const res = await taskcastApp.app.request(`/tasks/${task.id}/status`, {
      method: 'PATCH',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ status: 'completed' }),
    })
    expect(res.status).toBe(400)
  })

  it('rejects transition of non-existent task', async () => {
    const res = await taskcastApp.app.request('/tasks/nonexistent/status', {
      method: 'PATCH',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ status: 'running' }),
    })
    expect(res.status).toBe(404)
  })

  it('rejects event history for non-existent task', async () => {
    const res = await taskcastApp.app.request('/tasks/nonexistent/events/history')
    expect(res.status).toBe(404)
  })

  it('rejects malformed transition body', async () => {
    const task = await engine.createTask({ type: 'test' })
    const res = await taskcastApp.app.request(`/tasks/${task.id}/status`, {
      method: 'PATCH',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ status: 'invalid_status_value' }),
    })
    expect(res.status).toBe(400)
  })
})
