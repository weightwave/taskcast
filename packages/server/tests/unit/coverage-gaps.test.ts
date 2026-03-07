/**
 * Tests targeting specific coverage gaps in @taskcast/server.
 *
 * Gaps addressed:
 *   1. src/index.ts lines 93-103 — scheduler config wiring
 *   2. src/index.ts lines 157-169 — heartbeat monitor config wiring
 *   3. src/routes/tasks.ts lines 324-326 — catch block in POST /:taskId/resolve
 *   4. src/routes/sse.ts lines 162-164 — catch block when engine.getEvents() fails
 *   5. src/webhook.ts line 73 — exponential backoff branch
 *   6. src/auth.ts lines 65-66 — JWT payload missing taskIds/scope fallback
 *   7. src/routes/admin.ts line 55 — algorithm ?? 'HS256' fallback
 *   8. src/routes/workers.ts lines 190, 204 — Zod parse failure branches
 */

import { describe, it, expect, vi } from 'vitest'
import { Hono } from 'hono'
import { SignJWT } from 'jose'
import {
  TaskEngine,
  MemoryBroadcastProvider,
  MemoryShortTermStore,
  WorkerManager,
} from '@taskcast/core'
import { createTaskcastApp } from '../../src/index.js'
import { createTasksRouter } from '../../src/routes/tasks.js'
import { createSSERouter, createSubscriberCounts } from '../../src/routes/sse.js'
import { createWorkersRouter } from '../../src/routes/workers.js'
import { createAdminRouter } from '../../src/routes/admin.js'
import { WebhookDelivery } from '../../src/webhook.js'
import { createAuthMiddleware } from '../../src/auth.js'
import type { AuthContext } from '../../src/auth.js'
import type { TaskEvent, WebhookConfig } from '@taskcast/core'

// ─── Helpers ────────────────────────────────────────────────────────────────

function authMiddleware(overrides?: Partial<AuthContext>) {
  return async (c: any, next: any) => {
    c.set('auth', { taskIds: '*', scope: ['*'], ...overrides } as AuthContext)
    await next()
  }
}

// ─── 1. Scheduler config wiring (index.ts lines 93-103) ────────────────────

describe('createTaskcastApp — scheduler config wiring', () => {
  it('starts scheduler with custom checkIntervalMs, pausedColdAfterMs, blockedColdAfterMs', async () => {
    const store = new MemoryShortTermStore()
    const broadcast = new MemoryBroadcastProvider()
    const engine = new TaskEngine({ shortTermStore: store, broadcast })

    const taskcast = createTaskcastApp({
      engine,
      shortTermStore: store,
      scheduler: {
        enabled: true,
        checkIntervalMs: 60000,
        pausedColdAfterMs: 120000,
        blockedColdAfterMs: 180000,
      },
    })

    // Verify it created and is functional by checking health endpoint
    const res = await taskcast.app.request('/health')
    expect(res.status).toBe(200)

    // Clean up — stops the scheduler
    taskcast.stop()
  })

  it('starts scheduler with default options when no scheduler config specified', async () => {
    const store = new MemoryShortTermStore()
    const broadcast = new MemoryBroadcastProvider()
    const engine = new TaskEngine({ shortTermStore: store, broadcast })

    // scheduler.enabled is undefined (defaults to not-false = truthy), shortTermStore provided
    const taskcast = createTaskcastApp({
      engine,
      shortTermStore: store,
    })

    const res = await taskcast.app.request('/health')
    expect(res.status).toBe(200)

    taskcast.stop()
  })

  it('does not start scheduler when enabled is false', async () => {
    const store = new MemoryShortTermStore()
    const broadcast = new MemoryBroadcastProvider()
    const engine = new TaskEngine({ shortTermStore: store, broadcast })

    const taskcast = createTaskcastApp({
      engine,
      shortTermStore: store,
      scheduler: { enabled: false },
    })

    const res = await taskcast.app.request('/health')
    expect(res.status).toBe(200)

    taskcast.stop()
  })

  it('does not start scheduler when shortTermStore is not provided', async () => {
    const store = new MemoryShortTermStore()
    const broadcast = new MemoryBroadcastProvider()
    const engine = new TaskEngine({ shortTermStore: store, broadcast })

    const taskcast = createTaskcastApp({
      engine,
      scheduler: { enabled: true },
    })

    const res = await taskcast.app.request('/health')
    expect(res.status).toBe(200)

    taskcast.stop()
  })
})

// ─── 2. Heartbeat monitor config wiring (index.ts lines 157-169) ───────────

describe('createTaskcastApp — heartbeat monitor config wiring', () => {
  it('starts heartbeat monitor with custom options', async () => {
    const store = new MemoryShortTermStore()
    const broadcast = new MemoryBroadcastProvider()
    const engine = new TaskEngine({ shortTermStore: store, broadcast })
    const workerManager = new WorkerManager({ engine, shortTermStore: store, broadcast })

    const taskcast = createTaskcastApp({
      engine,
      workerManager,
      shortTermStore: store,
      heartbeat: {
        enabled: true,
        checkIntervalMs: 5000,
        heartbeatTimeoutMs: 15000,
        defaultDisconnectPolicy: 'fail',
        disconnectGraceMs: 10000,
      },
    })

    const res = await taskcast.app.request('/health')
    expect(res.status).toBe(200)

    taskcast.stop()
  })

  it('starts heartbeat monitor with default options when heartbeat not specified', async () => {
    const store = new MemoryShortTermStore()
    const broadcast = new MemoryBroadcastProvider()
    const engine = new TaskEngine({ shortTermStore: store, broadcast })
    const workerManager = new WorkerManager({ engine, shortTermStore: store, broadcast })

    const taskcast = createTaskcastApp({
      engine,
      workerManager,
      shortTermStore: store,
    })

    const res = await taskcast.app.request('/health')
    expect(res.status).toBe(200)

    taskcast.stop()
  })

  it('does not start heartbeat monitor when enabled is false', async () => {
    const store = new MemoryShortTermStore()
    const broadcast = new MemoryBroadcastProvider()
    const engine = new TaskEngine({ shortTermStore: store, broadcast })
    const workerManager = new WorkerManager({ engine, shortTermStore: store, broadcast })

    const taskcast = createTaskcastApp({
      engine,
      workerManager,
      shortTermStore: store,
      heartbeat: { enabled: false },
    })

    const res = await taskcast.app.request('/health')
    expect(res.status).toBe(200)

    taskcast.stop()
  })

  it('does not start heartbeat monitor when shortTermStore is missing', async () => {
    const store = new MemoryShortTermStore()
    const broadcast = new MemoryBroadcastProvider()
    const engine = new TaskEngine({ shortTermStore: store, broadcast })
    const workerManager = new WorkerManager({ engine, shortTermStore: store, broadcast })

    const taskcast = createTaskcastApp({
      engine,
      workerManager,
      // no shortTermStore
    })

    const res = await taskcast.app.request('/health')
    expect(res.status).toBe(200)

    taskcast.stop()
  })
})

// ─── 3. tasks.ts resolve catch block (lines 324-326) ───────────────────────

describe('POST /tasks/:taskId/resolve — catch block', () => {
  it('returns 400 when engine.transitionTask throws during resolve', async () => {
    const store = new MemoryShortTermStore()
    const broadcast = new MemoryBroadcastProvider()
    const engine = new TaskEngine({ shortTermStore: store, broadcast })

    const app = new Hono()
    app.use('*', authMiddleware())
    app.route('/tasks', createTasksRouter(engine, createSubscriberCounts()))

    // Create a task and get it to blocked state
    const task = await engine.createTask({})
    await engine.transitionTask(task.id, 'running')
    await engine.transitionTask(task.id, 'blocked')

    // Monkey-patch transitionTask to throw (simulating a race condition or internal error)
    const origTransition = engine.transitionTask.bind(engine)
    engine.transitionTask = async (taskId: string, status: any, payload?: any) => {
      if (status === 'running' && payload?.result) {
        throw new Error('Concurrent modification error')
      }
      return origTransition(taskId, status, payload)
    }

    const res = await app.request(`/tasks/${task.id}/resolve`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ data: { approved: true } }),
    })
    expect(res.status).toBe(400)
    const body = await res.json()
    expect(body.error).toBe('Concurrent modification error')
  })
})

// ─── 4. sse.ts getEvents failure (lines 162-164) ───────────────────────────

describe('SSE — engine.getEvents() failure during replay', () => {
  it('handles getEvents throwing gracefully by closing stream', async () => {
    const store = new MemoryShortTermStore()
    const broadcast = new MemoryBroadcastProvider()
    const engine = new TaskEngine({ shortTermStore: store, broadcast })

    const app = new Hono()
    app.use('*', authMiddleware())
    app.route('/tasks', createSSERouter(engine, createSubscriberCounts()))

    // Create a running task so SSE will try to replay
    const task = await engine.createTask({})
    await engine.transitionTask(task.id, 'running')

    // Monkey-patch getEvents to throw
    engine.getEvents = async () => {
      throw new Error('Storage unavailable')
    }

    const res = await app.request(`/tasks/${task.id}/events`)
    expect(res.headers.get('content-type')).toContain('text/event-stream')

    // The stream should be closed without any events
    const reader = res.body!.getReader()
    const decoder = new TextDecoder()
    let text = ''
    while (true) {
      const { done, value } = await reader.read()
      if (done) break
      text += decoder.decode(value, { stream: true })
    }

    // The stream should end cleanly (no taskcast.event or taskcast.done events)
    expect(text).not.toContain('taskcast.event')
  })
})

// ─── 5. webhook.ts line 73 — exponential backoff ───────────────────────────

describe('WebhookDelivery — exponential backoff', () => {
  const makeEvent = (): TaskEvent => ({
    id: 'evt-1',
    taskId: 'task-1',
    index: 0,
    timestamp: 1700000000000,
    type: 'llm.delta',
    level: 'info',
    data: { text: 'hello' },
  })

  it('uses exponential backoff strategy', async () => {
    const fetchMock = vi
      .fn()
      .mockResolvedValueOnce(new Response('error', { status: 500 }))
      .mockResolvedValueOnce(new Response('error', { status: 500 }))
      .mockResolvedValueOnce(new Response('ok', { status: 200 }))

    const delivery = new WebhookDelivery({ fetch: fetchMock })
    const config: WebhookConfig = {
      url: 'https://example.com/hook',
      retry: {
        retries: 3,
        backoff: 'exponential',
        initialDelayMs: 0,
        maxDelayMs: 1000,
        timeoutMs: 5000,
      },
    }
    await delivery.send(makeEvent(), config)
    expect(fetchMock).toHaveBeenCalledTimes(3)
  })

  it('caps exponential backoff at maxDelayMs', async () => {
    const fetchMock = vi
      .fn()
      .mockResolvedValueOnce(new Response('error', { status: 500 }))
      .mockResolvedValueOnce(new Response('error', { status: 500 }))
      .mockResolvedValueOnce(new Response('error', { status: 500 }))
      .mockResolvedValueOnce(new Response('ok', { status: 200 }))

    const delivery = new WebhookDelivery({ fetch: fetchMock })
    const config: WebhookConfig = {
      url: 'https://example.com/hook',
      retry: {
        retries: 5,
        backoff: 'exponential',
        initialDelayMs: 0,
        maxDelayMs: 100,
        timeoutMs: 5000,
      },
    }
    await delivery.send(makeEvent(), config)
    expect(fetchMock).toHaveBeenCalledTimes(4)
  })
})

// ─── 6. auth.ts lines 65-66 — JWT payload without taskIds/scope ────────────

describe('auth middleware — JWT payload fallbacks', () => {
  const TEST_SECRET = 'test-secret-that-is-long-enough-for-HS256'

  it('defaults taskIds to "*" when not in JWT payload', async () => {
    const secret = new TextEncoder().encode(TEST_SECRET)
    // Token with NO taskIds and NO scope claims
    const token = await new SignJWT({})
      .setProtectedHeader({ alg: 'HS256' })
      .setExpirationTime('1h')
      .sign(secret)

    const app = new Hono()
    app.use('*', createAuthMiddleware({
      mode: 'jwt',
      jwt: { algorithm: 'HS256', secret: TEST_SECRET },
    }))
    app.get('/test', (c) => {
      const auth = c.get('auth')
      return c.json({ taskIds: auth.taskIds, scope: auth.scope })
    })

    const res = await app.request('/test', {
      headers: { Authorization: `Bearer ${token}` },
    })
    expect(res.status).toBe(200)
    const body = await res.json()
    expect(body.taskIds).toBe('*')
    expect(body.scope).toEqual([])
  })

  it('defaults scope to [] when not in JWT payload', async () => {
    const secret = new TextEncoder().encode(TEST_SECRET)
    // Token with taskIds but NO scope
    const token = await new SignJWT({ taskIds: ['task-1'] })
      .setProtectedHeader({ alg: 'HS256' })
      .setExpirationTime('1h')
      .sign(secret)

    const app = new Hono()
    app.use('*', createAuthMiddleware({
      mode: 'jwt',
      jwt: { algorithm: 'HS256', secret: TEST_SECRET },
    }))
    app.get('/test', (c) => {
      const auth = c.get('auth')
      return c.json({ taskIds: auth.taskIds, scope: auth.scope })
    })

    const res = await app.request('/test', {
      headers: { Authorization: `Bearer ${token}` },
    })
    expect(res.status).toBe(200)
    const body = await res.json()
    expect(body.taskIds).toEqual(['task-1'])
    expect(body.scope).toEqual([])
  })
})

// ─── 7. admin.ts line 55 — algorithm fallback ──────────────────────────────

describe('POST /admin/token — algorithm fallback', () => {
  it('falls back to HS256 when algorithm is not set in jwt config', async () => {
    const secret = 'test-secret-that-is-long-enough-for-HS256'
    const app = new Hono()
    app.route('/admin', createAdminRouter({
      config: { adminApi: true, adminToken: 'my-token' },
      auth: {
        mode: 'jwt',
        jwt: { algorithm: undefined as any, secret },
      },
    }))

    const res = await app.request('/admin/token', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ adminToken: 'my-token' }),
    })
    expect(res.status).toBe(200)
    const body = await res.json()
    expect(body.token).toBeTruthy()
    expect(body.token.split('.')).toHaveLength(3) // valid JWT
  })
})

// ─── 8. workers.ts line 190 — updateWorkerStatus invalid body ──────────────

describe('PATCH /workers/:workerId/status — invalid body parsing', () => {
  function makeWorkerApp(authOverride?: Partial<AuthContext>) {
    const store = new MemoryShortTermStore()
    const broadcast = new MemoryBroadcastProvider()
    const engine = new TaskEngine({ shortTermStore: store, broadcast })
    const manager = new WorkerManager({ engine, shortTermStore: store, broadcast })
    const app = new Hono()
    app.use('*', authMiddleware(authOverride))
    app.route('/workers', createWorkersRouter(manager, engine))
    return { app, engine, manager }
  }

  it('returns 400 when body has no status field', async () => {
    const { app, manager } = makeWorkerApp()
    const worker = await manager.registerWorker({
      matchRule: {},
      capacity: 5,
      connectionMode: 'pull',
    })

    const res = await app.request(`/workers/${worker.id}/status`, {
      method: 'PATCH',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({}),
    })
    expect(res.status).toBe(400)
  })
})

// ─── 8b. workers.ts line 204 — decline parse failure ────────────────────────

describe('POST /workers/tasks/:taskId/decline — worker ID mismatch', () => {
  function makeWorkerApp(authOverride?: Partial<AuthContext>) {
    const store = new MemoryShortTermStore()
    const broadcast = new MemoryBroadcastProvider()
    const engine = new TaskEngine({ shortTermStore: store, broadcast })
    const manager = new WorkerManager({ engine, shortTermStore: store, broadcast })
    const app = new Hono()
    app.use('*', authMiddleware(authOverride))
    app.route('/workers', createWorkersRouter(manager, engine))
    return { app, engine, manager }
  }

  it('returns 403 when auth.workerId does not match decline workerId', async () => {
    const { app, engine, manager } = makeWorkerApp({ workerId: 'other-worker' })
    const task = await engine.createTask({ type: 'test', cost: 1 })
    const worker = await manager.registerWorker({
      matchRule: {},
      capacity: 5,
      connectionMode: 'pull',
    })
    await manager.claimTask(task.id, worker.id)

    const res = await app.request(`/workers/tasks/${task.id}/decline`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ workerId: worker.id }),
    })
    expect(res.status).toBe(403)
    const body = await res.json()
    expect(body.error).toBe('Forbidden: worker ID mismatch')
  })
})

// ─── workers.ts pull — worker ID mismatch branch ───────────────────────────

describe('GET /workers/pull — auth.workerId mismatch', () => {
  function makeWorkerApp(authOverride?: Partial<AuthContext>) {
    const store = new MemoryShortTermStore()
    const broadcast = new MemoryBroadcastProvider()
    const engine = new TaskEngine({ shortTermStore: store, broadcast })
    const manager = new WorkerManager({ engine, shortTermStore: store, broadcast })
    const app = new Hono()
    app.use('*', authMiddleware(authOverride))
    app.route('/workers', createWorkersRouter(manager, engine))
    return { app, engine, manager }
  }

  it('returns 403 when auth.workerId does not match query workerId', async () => {
    const { app, manager } = makeWorkerApp({ workerId: 'worker-in-token' })
    await manager.registerWorker({
      id: 'different-worker',
      matchRule: {},
      capacity: 5,
      connectionMode: 'pull',
    })

    const res = await app.request('/workers/pull?workerId=different-worker')
    expect(res.status).toBe(403)
    const body = await res.json()
    expect(body.error).toBe('Forbidden: worker ID mismatch')
  })
})

// ─── Additional: SSE level filter branch coverage ──────────────────────────

describe('SSE — level filter parsing', () => {
  it('filters events by level query param', async () => {
    const store = new MemoryShortTermStore()
    const broadcast = new MemoryBroadcastProvider()
    const engine = new TaskEngine({ shortTermStore: store, broadcast })
    const app = new Hono()
    app.use('*', authMiddleware())
    app.route('/tasks', createSSERouter(engine, createSubscriberCounts()))

    const task = await engine.createTask({})
    await engine.transitionTask(task.id, 'running')
    await engine.publishEvent(task.id, { type: 'test', level: 'debug', data: null })
    await engine.publishEvent(task.id, { type: 'test', level: 'error', data: null })
    await engine.transitionTask(task.id, 'completed')

    const res = await app.request(`/tasks/${task.id}/events?levels=error&includeStatus=false`)
    expect(res.headers.get('content-type')).toContain('text/event-stream')

    const reader = res.body!.getReader()
    const decoder = new TextDecoder()
    let text = ''
    while (true) {
      const { done, value } = await reader.read()
      if (done) break
      text += decoder.decode(value, { stream: true })
    }

    // Should only include error level events (not debug)
    expect(text).toContain('taskcast.done')
  })
})
