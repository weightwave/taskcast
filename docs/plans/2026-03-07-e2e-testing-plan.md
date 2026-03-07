# E2E Testing Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Add three layers of testing — API E2E, dashboard component tests, and browser E2E via Playwright — with a dedicated CI workflow.

**Architecture:** New `packages/e2e/` package for API + browser E2E tests. Dashboard component tests live in `packages/dashboard-web/tests/`. CI runs in a separate `e2e.yml` workflow on PRs and main only.

**Tech Stack:** vitest, @playwright/test, @testing-library/react, msw, @hono/node-server

---

### Task 1: Scaffold `packages/e2e/` package

**Files:**
- Create: `packages/e2e/package.json`
- Create: `packages/e2e/tsconfig.json`
- Create: `packages/e2e/vitest.config.ts`

**Step 1: Create package.json**

```json
{
  "name": "@taskcast/e2e",
  "version": "0.3.0",
  "private": true,
  "type": "module",
  "scripts": {
    "test": "vitest run tests/",
    "test:browser": "playwright test"
  },
  "devDependencies": {
    "@hono/node-server": "^1.13.0",
    "@playwright/test": "^1.50.0",
    "@taskcast/core": "workspace:*",
    "@taskcast/server": "workspace:*",
    "typescript": "^5.7.0",
    "vitest": "^2.1.0"
  }
}
```

**Step 2: Create tsconfig.json**

```json
{
  "compilerOptions": {
    "target": "ESNext",
    "module": "ESNext",
    "moduleResolution": "bundler",
    "esModuleInterop": true,
    "strict": true,
    "skipLibCheck": true,
    "noEmit": true
  },
  "include": ["tests/**/*.ts", "browser/**/*.ts"]
}
```

**Step 3: Create vitest.config.ts**

```ts
import { defineConfig } from 'vitest/config'

export default defineConfig({
  test: {
    include: ['tests/**/*.test.ts'],
    testTimeout: 30_000,
  },
})
```

**Step 4: Install deps**

Run: `cd /path/to/worktree && pnpm install`

**Step 5: Commit**

```
feat(e2e): scaffold packages/e2e with vitest + playwright
```

---

### Task 2: Server test helpers

**Files:**
- Create: `packages/e2e/tests/helpers/server.ts`

**Step 1: Write the server helper**

This helper starts a real TS server using `createTaskcastApp` + `@hono/node-server` on a random port. All tests will use this.

```ts
import { serve } from '@hono/node-server'
import {
  TaskEngine,
  MemoryBroadcastProvider,
  MemoryShortTermStore,
  WorkerManager,
  resolveAdminToken,
} from '@taskcast/core'
import type { ShortTermStore, BroadcastProvider } from '@taskcast/core'
import { createTaskcastApp } from '@taskcast/server'
import type { TaskcastServerOptions } from '@taskcast/server'

export interface TestServer {
  baseUrl: string
  engine: TaskEngine
  workerManager?: WorkerManager
  close: () => void
}

export interface StartServerOptions {
  auth?: 'none' | 'jwt'
  jwtSecret?: string
  adminApi?: boolean
  adminToken?: string
  workers?: boolean
}

export async function startServer(opts: StartServerOptions = {}): Promise<TestServer> {
  const shortTermStore: ShortTermStore = new MemoryShortTermStore()
  const broadcast: BroadcastProvider = new MemoryBroadcastProvider()
  const engine = new TaskEngine({ shortTermStore, broadcast })

  const jwtSecret = opts.jwtSecret ?? 'e2e-test-secret-that-is-long-enough-for-HS256'

  const config = {
    adminApi: opts.adminApi ?? false,
    adminToken: opts.adminToken,
  }
  if (config.adminApi) resolveAdminToken(config)

  const serverOpts: TaskcastServerOptions = {
    engine,
    shortTermStore,
    auth: opts.auth === 'jwt'
      ? { mode: 'jwt', jwt: { algorithm: 'HS256', secret: jwtSecret } }
      : { mode: 'none' },
    config,
  }

  let workerManager: WorkerManager | undefined
  if (opts.workers) {
    workerManager = new WorkerManager({ engine, shortTermStore, broadcast })
    serverOpts.workerManager = workerManager
  }

  const { app, stop } = createTaskcastApp(serverOpts)

  return new Promise((resolve) => {
    // Port 0 = random available port
    const server = serve({ fetch: app.fetch, port: 0 }, (info) => {
      const port = (info as { port: number }).port
      resolve({
        baseUrl: `http://localhost:${port}`,
        engine,
        workerManager,
        close: () => {
          stop()
          server.close()
        },
      })
    })
  })
}
```

**Step 2: Verify it compiles**

Run: `cd packages/e2e && npx tsc --noEmit`

**Step 3: Commit**

```
feat(e2e): add test server helper with random port
```

---

### Task 3: API E2E — task lifecycle

**Files:**
- Create: `packages/e2e/tests/api/task-lifecycle.test.ts`

**Step 1: Write tests**

Test the complete task lifecycle: create → publish events → GET events → SSE subscribe → transition to running → transition to completed.

```ts
import { describe, it, expect, beforeAll, afterAll } from 'vitest'
import { startServer, type TestServer } from '../helpers/server.js'

let server: TestServer

beforeAll(async () => {
  server = await startServer()
})

afterAll(() => server?.close())

async function api(path: string, init?: RequestInit) {
  const res = await fetch(`${server.baseUrl}${path}`, {
    headers: { 'Content-Type': 'application/json', ...init?.headers },
    ...init,
  })
  return { status: res.status, body: await res.json().catch(() => null), res }
}

describe('task lifecycle', () => {
  it('creates a task', async () => {
    const { status, body } = await api('/tasks', {
      method: 'POST',
      body: JSON.stringify({ type: 'e2e-test' }),
    })
    expect(status).toBe(201)
    expect(body.id).toBeTruthy()
    expect(body.status).toBe('pending')
    expect(body.type).toBe('e2e-test')
  })

  it('creates task with explicit ID', async () => {
    const { status, body } = await api('/tasks', {
      method: 'POST',
      body: JSON.stringify({ id: 'explicit-id', type: 'test' }),
    })
    expect(status).toBe(201)
    expect(body.id).toBe('explicit-id')
  })

  it('rejects duplicate task ID', async () => {
    const { status } = await api('/tasks', {
      method: 'POST',
      body: JSON.stringify({ id: 'explicit-id' }),
    })
    expect(status).toBe(409)
  })

  it('gets a task by ID', async () => {
    const { body: created } = await api('/tasks', {
      method: 'POST',
      body: JSON.stringify({ type: 'get-test' }),
    })
    const { status, body } = await api(`/tasks/${created.id}`)
    expect(status).toBe(200)
    expect(body.id).toBe(created.id)
    expect(body.hot).toBe(true)
  })

  it('returns 404 for unknown task', async () => {
    const { status } = await api('/tasks/nonexistent')
    expect(status).toBe(404)
  })

  it('lists tasks', async () => {
    const { status, body } = await api('/tasks')
    expect(status).toBe(200)
    expect(body.tasks.length).toBeGreaterThanOrEqual(2)
  })

  it('lists tasks with status filter', async () => {
    const { body } = await api('/tasks?status=pending')
    for (const task of body.tasks) {
      expect(task.status).toBe('pending')
    }
  })

  it('publishes events to a task', async () => {
    const { body: task } = await api('/tasks', {
      method: 'POST',
      body: JSON.stringify({ id: 'event-task' }),
    })

    // Transition to running first
    await api(`/tasks/${task.id}/status`, {
      method: 'PATCH',
      body: JSON.stringify({ status: 'running' }),
    })

    const { status } = await api(`/tasks/${task.id}/events`, {
      method: 'POST',
      body: JSON.stringify({
        events: [
          { type: 'log', data: { message: 'hello' } },
          { type: 'progress', data: { percent: 50 } },
        ],
      }),
    })
    expect(status).toBe(200)
  })

  it('gets event history', async () => {
    const { status, body } = await api('/tasks/event-task/events/history')
    expect(status).toBe(200)
    expect(body.events.length).toBeGreaterThanOrEqual(2)
  })

  it('transitions task through full lifecycle', async () => {
    const { body: task } = await api('/tasks', {
      method: 'POST',
      body: JSON.stringify({ id: 'lifecycle-task' }),
    })
    expect(task.status).toBe('pending')

    // pending → running
    const { body: running } = await api(`/tasks/${task.id}/status`, {
      method: 'PATCH',
      body: JSON.stringify({ status: 'running' }),
    })
    expect(running.status).toBe('running')

    // running → completed
    const { body: completed } = await api(`/tasks/${task.id}/status`, {
      method: 'PATCH',
      body: JSON.stringify({ status: 'completed', result: { output: 'done' } }),
    })
    expect(completed.status).toBe('completed')
    expect(completed.result).toEqual({ output: 'done' })
  })

  it('rejects invalid transitions', async () => {
    // lifecycle-task is already completed — can't transition again
    const { status } = await api('/tasks/lifecycle-task/status', {
      method: 'PATCH',
      body: JSON.stringify({ status: 'running' }),
    })
    expect(status).toBe(409)
  })

  it('transitions to failed with error', async () => {
    const { body: task } = await api('/tasks', {
      method: 'POST',
      body: JSON.stringify({ id: 'fail-task' }),
    })
    await api(`/tasks/${task.id}/status`, {
      method: 'PATCH',
      body: JSON.stringify({ status: 'running' }),
    })
    const { body: failed } = await api(`/tasks/${task.id}/status`, {
      method: 'PATCH',
      body: JSON.stringify({ status: 'failed', error: { code: 'ERR', message: 'boom' } }),
    })
    expect(failed.status).toBe('failed')
    expect(failed.error).toEqual({ code: 'ERR', message: 'boom' })
  })
})
```

**Step 2: Run tests**

Run: `cd packages/e2e && npx vitest run tests/api/task-lifecycle.test.ts`
Expected: All tests pass.

**Step 3: Commit**

```
test(e2e): add task lifecycle API tests
```

---

### Task 4: API E2E — SSE streaming

**Files:**
- Create: `packages/e2e/tests/api/sse-streaming.test.ts`

**Step 1: Write tests**

Tests for SSE: replay history on terminal task, live streaming with done event, subscriber count enrichment.

Key implementation detail: Use `fetch` to read the SSE stream as text, parse `event:` / `data:` lines. The server emits `taskcast.event` and `taskcast.done` events.

```ts
import { describe, it, expect, beforeAll, afterAll } from 'vitest'
import { startServer, type TestServer } from '../helpers/server.js'

let server: TestServer

beforeAll(async () => {
  server = await startServer()
})
afterAll(() => server?.close())

function api(path: string, init?: RequestInit) {
  return fetch(`${server.baseUrl}${path}`, {
    headers: { 'Content-Type': 'application/json', ...init?.headers },
    ...init,
  })
}

async function collectSSE(
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

describe('SSE streaming', () => {
  it('replays history then closes for terminal task', async () => {
    // Create + run + complete a task
    await api('/tasks', { method: 'POST', body: JSON.stringify({ id: 'sse-done' }) })
    await api('/tasks/sse-done/status', { method: 'PATCH', body: JSON.stringify({ status: 'running' }) })
    await api('/tasks/sse-done/events', {
      method: 'POST',
      body: JSON.stringify({ events: [{ type: 'log', data: { msg: 'hi' } }] }),
    })
    await api('/tasks/sse-done/status', { method: 'PATCH', body: JSON.stringify({ status: 'completed' }) })

    // SSE should replay history + send done
    const res = await api('/tasks/sse-done/events')
    const events = await collectSSE(res, 10, 3000)

    const taskEvents = events.filter((e) => e.event === 'taskcast.event')
    const doneEvents = events.filter((e) => e.event === 'taskcast.done')

    expect(taskEvents.length).toBeGreaterThanOrEqual(1)
    expect(doneEvents.length).toBe(1)
    expect(JSON.parse(doneEvents[0].data).reason).toBe('completed')
  })

  it('streams live events and closes on terminal', async () => {
    await api('/tasks', { method: 'POST', body: JSON.stringify({ id: 'sse-live' }) })
    await api('/tasks/sse-live/status', { method: 'PATCH', body: JSON.stringify({ status: 'running' }) })

    // Start SSE subscription
    const ssePromise = api('/tasks/sse-live/events')

    // Give SSE time to connect, then publish events
    await new Promise((r) => setTimeout(r, 100))
    await api('/tasks/sse-live/events', {
      method: 'POST',
      body: JSON.stringify({ events: [{ type: 'progress', data: { pct: 50 } }] }),
    })
    await api('/tasks/sse-live/status', { method: 'PATCH', body: JSON.stringify({ status: 'completed' }) })

    const res = await ssePromise
    const events = await collectSSE(res, 20, 5000)

    const doneEvents = events.filter((e) => e.event === 'taskcast.done')
    expect(doneEvents.length).toBe(1)
  })

  it('shows subscriberCount on task during active subscription', async () => {
    await api('/tasks', { method: 'POST', body: JSON.stringify({ id: 'sse-subs' }) })
    await api('/tasks/sse-subs/status', { method: 'PATCH', body: JSON.stringify({ status: 'running' }) })

    // Start SSE subscription
    const sseRes = api('/tasks/sse-subs/events')
    await new Promise((r) => setTimeout(r, 200))

    // Check subscriber count
    const { status, body: task } = await api('/tasks/sse-subs').then(async (r) => ({
      status: r.status,
      body: await r.json(),
    }))
    expect(status).toBe(200)
    expect(task.subscriberCount).toBeGreaterThanOrEqual(1)

    // Complete to close SSE
    await api('/tasks/sse-subs/status', { method: 'PATCH', body: JSON.stringify({ status: 'completed' }) })
    const res = await sseRes
    await collectSSE(res, 20, 3000) // drain
  })
})
```

**Step 2: Run tests**

Run: `cd packages/e2e && npx vitest run tests/api/sse-streaming.test.ts`

**Step 3: Commit**

```
test(e2e): add SSE streaming API tests
```

---

### Task 5: API E2E — admin auth flow

**Files:**
- Create: `packages/e2e/tests/api/admin-auth.test.ts`

**Step 1: Write tests**

Test admin token exchange for JWT, then use JWT on protected endpoints. Also test auth boundaries (no token, invalid token, wrong scope).

Start a server with `auth: 'jwt'` and `adminApi: true`.

```ts
import { describe, it, expect, beforeAll, afterAll } from 'vitest'
import { startServer, type TestServer } from '../helpers/server.js'

let server: TestServer
const ADMIN_TOKEN = 'e2e-admin-token'

beforeAll(async () => {
  server = await startServer({
    auth: 'jwt',
    adminApi: true,
    adminToken: ADMIN_TOKEN,
  })
})
afterAll(() => server?.close())

function api(path: string, init?: RequestInit) {
  return fetch(`${server.baseUrl}${path}`, {
    headers: { 'Content-Type': 'application/json', ...init?.headers },
    ...init,
  })
}

describe('admin auth flow', () => {
  let jwt: string

  it('exchanges admin token for JWT', async () => {
    const res = await api('/admin/token', {
      method: 'POST',
      body: JSON.stringify({ adminToken: ADMIN_TOKEN }),
    })
    expect(res.status).toBe(200)
    const body = await res.json()
    expect(body.token).toBeTruthy()
    expect(body.expiresAt).toBeGreaterThan(Date.now() / 1000)
    jwt = body.token
  })

  it('rejects invalid admin token', async () => {
    const res = await api('/admin/token', {
      method: 'POST',
      body: JSON.stringify({ adminToken: 'wrong' }),
    })
    expect(res.status).toBe(401)
  })

  it('JWT grants access to protected endpoints', async () => {
    const res = await api('/tasks', {
      method: 'POST',
      headers: {
        'Content-Type': 'application/json',
        Authorization: `Bearer ${jwt}`,
      },
      body: JSON.stringify({ type: 'auth-test' }),
    })
    expect(res.status).toBe(201)
  })

  it('rejects requests without token', async () => {
    const res = await api('/tasks', {
      method: 'POST',
      body: JSON.stringify({ type: 'no-auth' }),
    })
    expect(res.status).toBe(401)
  })

  it('admin endpoint bypasses JWT auth', async () => {
    // Admin endpoint should work without Bearer token
    const res = await api('/admin/token', {
      method: 'POST',
      body: JSON.stringify({ adminToken: ADMIN_TOKEN }),
    })
    expect(res.status).toBe(200)
  })

  it('exchanges token with custom scopes', async () => {
    const res = await api('/admin/token', {
      method: 'POST',
      body: JSON.stringify({
        adminToken: ADMIN_TOKEN,
        scopes: ['task:create'],
        expiresIn: 60,
      }),
    })
    const body = await res.json()
    expect(body.token).toBeTruthy()
    // Token with limited scope should still work for task:create
    const createRes = await api('/tasks', {
      method: 'POST',
      headers: {
        'Content-Type': 'application/json',
        Authorization: `Bearer ${body.token}`,
      },
      body: JSON.stringify({ type: 'scoped-test' }),
    })
    expect(createRes.status).toBe(201)
  })
})
```

**Step 2: Run tests**

Run: `cd packages/e2e && npx vitest run tests/api/admin-auth.test.ts`

**Step 3: Commit**

```
test(e2e): add admin auth flow API tests
```

---

### Task 6: API E2E — concurrency

**Files:**
- Create: `packages/e2e/tests/api/concurrency.test.ts`

**Step 1: Write tests**

Test concurrent status transitions (only one succeeds) and multiple simultaneous SSE subscribers.

```ts
import { describe, it, expect, beforeAll, afterAll } from 'vitest'
import { startServer, type TestServer } from '../helpers/server.js'

let server: TestServer

beforeAll(async () => {
  server = await startServer()
})
afterAll(() => server?.close())

function api(path: string, init?: RequestInit) {
  return fetch(`${server.baseUrl}${path}`, {
    headers: { 'Content-Type': 'application/json', ...init?.headers },
    ...init,
  })
}

describe('concurrency', () => {
  it('only one concurrent terminal transition succeeds', async () => {
    await api('/tasks', { method: 'POST', body: JSON.stringify({ id: 'race-task' }) })
    await api('/tasks/race-task/status', { method: 'PATCH', body: JSON.stringify({ status: 'running' }) })

    // Fire 10 concurrent terminal transitions
    const results = await Promise.all(
      Array.from({ length: 10 }, () =>
        api('/tasks/race-task/status', {
          method: 'PATCH',
          body: JSON.stringify({ status: 'completed' }),
        }).then((r) => r.status),
      ),
    )

    const successes = results.filter((s) => s === 200)
    const conflicts = results.filter((s) => s === 409)
    expect(successes.length).toBe(1)
    expect(conflicts.length).toBe(9)
  })

  it('multiple SSE subscribers receive same events', async () => {
    await api('/tasks', { method: 'POST', body: JSON.stringify({ id: 'multi-sub' }) })
    await api('/tasks/multi-sub/status', { method: 'PATCH', body: JSON.stringify({ status: 'running' }) })

    // Start 3 SSE subscribers
    const subscribers = Array.from({ length: 3 }, () => api('/tasks/multi-sub/events'))

    await new Promise((r) => setTimeout(r, 200))

    // Publish an event then complete
    await api('/tasks/multi-sub/events', {
      method: 'POST',
      body: JSON.stringify({ events: [{ type: 'test', data: {} }] }),
    })
    await api('/tasks/multi-sub/status', { method: 'PATCH', body: JSON.stringify({ status: 'completed' }) })

    // All 3 should receive done events
    for (const subPromise of subscribers) {
      const res = await subPromise
      const reader = res.body!.getReader()
      const decoder = new TextDecoder()
      let text = ''
      const deadline = Date.now() + 3000
      while (Date.now() < deadline) {
        const { done, value } = await reader.read()
        if (done) break
        text += decoder.decode(value, { stream: true })
        if (text.includes('taskcast.done')) break
      }
      reader.cancel()
      expect(text).toContain('taskcast.done')
    }
  })
})
```

**Step 2: Run tests**

Run: `cd packages/e2e && npx vitest run tests/api/concurrency.test.ts`

**Step 3: Commit**

```
test(e2e): add concurrency API tests
```

---

### Task 7: Dashboard component tests — setup + connection store

**Files:**
- Modify: `packages/dashboard-web/package.json` (add devDependencies)
- Create: `packages/dashboard-web/vitest.config.ts`
- Create: `packages/dashboard-web/tests/setup.ts`
- Create: `packages/dashboard-web/tests/stores/connection.test.ts`

**Step 1: Add dependencies to dashboard-web package.json**

Add to `devDependencies`:
```json
{
  "vitest": "^2.1.0",
  "@testing-library/react": "^16.0.0",
  "@testing-library/dom": "^10.0.0",
  "jsdom": "^25.0.0",
  "msw": "^2.0.0"
}
```

Add to `scripts`:
```json
{
  "test": "vitest run"
}
```

**Step 2: Create vitest.config.ts**

```ts
import { defineConfig } from 'vitest/config'
import react from '@vitejs/plugin-react'
import path from 'path'

export default defineConfig({
  plugins: [react()],
  resolve: {
    alias: {
      '@': path.resolve(__dirname, 'src'),
    },
  },
  test: {
    environment: 'jsdom',
    include: ['tests/**/*.test.{ts,tsx}'],
    setupFiles: ['tests/setup.ts'],
  },
})
```

**Step 3: Create test setup**

`tests/setup.ts`:
```ts
import { afterEach } from 'vitest'
import { cleanup } from '@testing-library/react'

afterEach(() => {
  cleanup()
})
```

**Step 4: Write connection store tests**

`tests/stores/connection.test.ts`:
```ts
import { describe, it, expect, beforeEach, vi, afterEach } from 'vitest'
import { useConnectionStore } from '../../src/stores/connection'

// Reset store between tests
beforeEach(() => {
  useConnectionStore.setState({
    baseUrl: '',
    jwt: null,
    connected: false,
    error: null,
  })
})

afterEach(() => {
  vi.restoreAllMocks()
})

describe('connection store', () => {
  it('starts disconnected', () => {
    const state = useConnectionStore.getState()
    expect(state.connected).toBe(false)
    expect(state.baseUrl).toBe('')
    expect(state.jwt).toBeNull()
  })

  it('connects with admin token (jwt mode)', async () => {
    vi.spyOn(globalThis, 'fetch')
      .mockResolvedValueOnce(new Response(JSON.stringify({ ok: true }), { status: 200 })) // health
      .mockResolvedValueOnce(new Response(JSON.stringify({ token: 'jwt-123' }), { status: 200 })) // admin/token

    await useConnectionStore.getState().connect('http://localhost:3721', 'my-token')

    const state = useConnectionStore.getState()
    expect(state.connected).toBe(true)
    expect(state.baseUrl).toBe('http://localhost:3721')
    expect(state.jwt).toBe('jwt-123')
  })

  it('connects without JWT when admin API returns 404', async () => {
    vi.spyOn(globalThis, 'fetch')
      .mockResolvedValueOnce(new Response(JSON.stringify({ ok: true }), { status: 200 }))
      .mockResolvedValueOnce(new Response(null, { status: 404 }))

    await useConnectionStore.getState().connect('http://localhost:3721', 'token')

    const state = useConnectionStore.getState()
    expect(state.connected).toBe(true)
    expect(state.jwt).toBeNull()
  })

  it('sets error on invalid admin token', async () => {
    vi.spyOn(globalThis, 'fetch')
      .mockResolvedValueOnce(new Response(JSON.stringify({ ok: true }), { status: 200 }))
      .mockResolvedValueOnce(new Response(null, { status: 401 }))

    await expect(
      useConnectionStore.getState().connect('http://localhost:3721', 'wrong'),
    ).rejects.toThrow('Invalid admin token')

    expect(useConnectionStore.getState().connected).toBe(false)
    expect(useConnectionStore.getState().error).toBe('Invalid admin token')
  })

  it('sets error on unreachable server', async () => {
    vi.spyOn(globalThis, 'fetch').mockRejectedValueOnce(new Error('Connection refused'))

    await expect(
      useConnectionStore.getState().connect('http://bad:1234', 'token'),
    ).rejects.toThrow()

    expect(useConnectionStore.getState().connected).toBe(false)
    expect(useConnectionStore.getState().error).toBeTruthy()
  })

  it('disconnects', () => {
    useConnectionStore.setState({ connected: true, jwt: 'abc', baseUrl: 'http://x' })
    useConnectionStore.getState().disconnect()

    const state = useConnectionStore.getState()
    expect(state.connected).toBe(false)
    expect(state.jwt).toBeNull()
  })

  it('setAutoConnect sets connected state', () => {
    useConnectionStore.getState().setAutoConnect('http://auto', 'jwt-auto')

    const state = useConnectionStore.getState()
    expect(state.connected).toBe(true)
    expect(state.baseUrl).toBe('http://auto')
    expect(state.jwt).toBe('jwt-auto')
  })
})
```

**Step 5: Install deps and run**

Run: `pnpm install && cd packages/dashboard-web && npx vitest run tests/stores/connection.test.ts`

**Step 6: Commit**

```
test(dashboard-web): add vitest setup + connection store tests
```

---

### Task 8: Dashboard component tests — hooks (use-stats)

**Files:**
- Create: `packages/dashboard-web/tests/hooks/use-stats.test.ts`

**Step 1: Write tests**

Test `useStats` hook: status counts, sorting by createdAt, capacity calculation, isPending propagation. Use `@testing-library/react` `renderHook` with a `QueryClientProvider` wrapper.

The hook internally uses `useTasksQuery` and `useWorkersQuery` which call `apiFetch`. Mock `fetch` globally to return test data.

```ts
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest'
import { renderHook, waitFor } from '@testing-library/react'
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { createElement } from 'react'
import type { ReactNode } from 'react'
import { useStats } from '../../src/hooks/use-stats'
import { useConnectionStore } from '../../src/stores/connection'

function createWrapper() {
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  })
  return ({ children }: { children: ReactNode }) =>
    createElement(QueryClientProvider, { client: queryClient }, children)
}

const MOCK_TASKS = [
  { id: '1', status: 'running', type: 'a', createdAt: 100, hot: true },
  { id: '2', status: 'completed', type: 'b', createdAt: 300, hot: false },
  { id: '3', status: 'running', type: 'c', createdAt: 200, hot: true },
]

const MOCK_WORKERS = [
  { id: 'w1', status: 'idle', capacity: 10, usedSlots: 3 },
  { id: 'w2', status: 'offline', capacity: 5, usedSlots: 0 },
]

beforeEach(() => {
  useConnectionStore.setState({ baseUrl: 'http://test', connected: true, jwt: null })
  vi.spyOn(globalThis, 'fetch').mockImplementation(async (input) => {
    const url = typeof input === 'string' ? input : input.toString()
    if (url.includes('/tasks')) return new Response(JSON.stringify({ tasks: MOCK_TASKS }))
    if (url.includes('/workers')) return new Response(JSON.stringify({ workers: MOCK_WORKERS }))
    return new Response(null, { status: 404 })
  })
})

afterEach(() => { vi.restoreAllMocks() })

describe('useStats', () => {
  it('computes status counts', async () => {
    const { result } = renderHook(() => useStats(), { wrapper: createWrapper() })
    await waitFor(() => expect(result.current.totalTasks).toBeGreaterThan(0))
    expect(result.current.statusCounts).toEqual({ running: 2, completed: 1 })
  })

  it('sorts recent tasks by createdAt descending', async () => {
    const { result } = renderHook(() => useStats(), { wrapper: createWrapper() })
    await waitFor(() => expect(result.current.recentTasks.length).toBe(3))
    expect(result.current.recentTasks[0].id).toBe('2') // createdAt 300
    expect(result.current.recentTasks[1].id).toBe('3') // createdAt 200
    expect(result.current.recentTasks[2].id).toBe('1') // createdAt 100
  })

  it('computes worker capacity', async () => {
    const { result } = renderHook(() => useStats(), { wrapper: createWrapper() })
    await waitFor(() => expect(result.current.onlineWorkers).toBe(1))
    expect(result.current.totalCapacity).toBe(15)
    expect(result.current.usedCapacity).toBe(3)
  })
})
```

**Step 2: Run tests**

Run: `cd packages/dashboard-web && npx vitest run tests/hooks/use-stats.test.ts`

**Step 3: Commit**

```
test(dashboard-web): add use-stats hook tests
```

---

### Task 9: Dashboard component tests — error boundary + task table

**Files:**
- Create: `packages/dashboard-web/tests/components/error-boundary.test.tsx`
- Create: `packages/dashboard-web/tests/components/task-table.test.tsx`

**Step 1: Write error boundary test**

```tsx
import { describe, it, expect, vi } from 'vitest'
import { render, screen, fireEvent } from '@testing-library/react'
import { ErrorBoundary } from '../../src/components/error-boundary'

function ThrowingComponent({ shouldThrow }: { shouldThrow: boolean }) {
  if (shouldThrow) throw new Error('Test error')
  return <div>No error</div>
}

describe('ErrorBoundary', () => {
  it('renders children when no error', () => {
    render(
      <ErrorBoundary>
        <div>hello</div>
      </ErrorBoundary>,
    )
    expect(screen.getByText('hello')).toBeTruthy()
  })

  it('shows error message on render error', () => {
    vi.spyOn(console, 'error').mockImplementation(() => {})
    render(
      <ErrorBoundary>
        <ThrowingComponent shouldThrow={true} />
      </ErrorBoundary>,
    )
    expect(screen.getByText('Something went wrong')).toBeTruthy()
    expect(screen.getByText('Test error')).toBeTruthy()
  })

  it('recovers when Try Again is clicked', () => {
    vi.spyOn(console, 'error').mockImplementation(() => {})
    const { rerender } = render(
      <ErrorBoundary>
        <ThrowingComponent shouldThrow={true} />
      </ErrorBoundary>,
    )
    expect(screen.getByText('Something went wrong')).toBeTruthy()

    fireEvent.click(screen.getByText('Try Again'))

    rerender(
      <ErrorBoundary>
        <ThrowingComponent shouldThrow={false} />
      </ErrorBoundary>,
    )
    expect(screen.getByText('No error')).toBeTruthy()
  })
})
```

**Step 2: Write task table test**

```tsx
import { describe, it, expect, vi } from 'vitest'
import { render, screen, fireEvent } from '@testing-library/react'
import { TaskTable } from '../../src/components/tasks/task-table'
import type { DashboardTask } from '../../src/types'

const TASKS: DashboardTask[] = [
  { id: 'task-001', status: 'running', type: 'llm', hot: true, subscriberCount: 2, createdAt: 1000 } as DashboardTask,
  { id: 'task-002', status: 'completed', type: 'agent', hot: false, subscriberCount: 0, createdAt: 2000 } as DashboardTask,
]

describe('TaskTable', () => {
  it('renders empty state', () => {
    render(<TaskTable tasks={[]} selectedTaskId={null} onSelect={() => {}} />)
    expect(screen.getByText('No tasks found.')).toBeTruthy()
  })

  it('renders task rows', () => {
    render(<TaskTable tasks={TASKS} selectedTaskId={null} onSelect={() => {}} />)
    expect(screen.getByText('running')).toBeTruthy()
    expect(screen.getByText('completed')).toBeTruthy()
    expect(screen.getByText('llm')).toBeTruthy()
    expect(screen.getByText('agent')).toBeTruthy()
  })

  it('shows hot/cold badges', () => {
    render(<TaskTable tasks={TASKS} selectedTaskId={null} onSelect={() => {}} />)
    expect(screen.getByText('Hot')).toBeTruthy()
    expect(screen.getByText('Cold')).toBeTruthy()
  })

  it('calls onSelect when row is clicked', () => {
    const onSelect = vi.fn()
    render(<TaskTable tasks={TASKS} selectedTaskId={null} onSelect={onSelect} />)
    fireEvent.click(screen.getByText('running'))
    expect(onSelect).toHaveBeenCalledWith('task-001')
  })
})
```

**Step 3: Run tests**

Run: `cd packages/dashboard-web && npx vitest run tests/components/`

**Step 4: Commit**

```
test(dashboard-web): add error boundary + task table component tests
```

---

### Task 10: Playwright setup + browser E2E config

**Files:**
- Create: `packages/e2e/playwright.config.ts`
- Create: `packages/e2e/browser/helpers.ts`

**Step 1: Create playwright.config.ts**

```ts
import { defineConfig } from '@playwright/test'

export default defineConfig({
  testDir: './browser',
  timeout: 30_000,
  retries: 0,
  use: {
    baseURL: 'http://localhost:3722',
    headless: true,
  },
  projects: [
    { name: 'chromium', use: { browserName: 'chromium' } },
  ],
  webServer: [
    {
      command: 'node browser/start-servers.js',
      port: 3722,
      timeout: 15_000,
      reuseExistingServer: !process.env.CI,
    },
  ],
})
```

**Step 2: Create browser/helpers.ts**

This is a helper for the `webServer` command. It starts both the taskcast server and the dashboard UI.

Create `packages/e2e/browser/start-servers.js`:

```js
import { serve } from '@hono/node-server'
import { spawn } from 'child_process'
import {
  TaskEngine,
  MemoryBroadcastProvider,
  MemoryShortTermStore,
  WorkerManager,
  resolveAdminToken,
} from '@taskcast/core'
import { createTaskcastApp } from '@taskcast/server'

// Start API server on port 3799
const store = new MemoryShortTermStore()
const broadcast = new MemoryBroadcastProvider()
const engine = new TaskEngine({ shortTermStore: store, broadcast })
const workerManager = new WorkerManager({ engine, shortTermStore: store, broadcast })

const config = { adminApi: true, adminToken: 'e2e-admin-token' }
resolveAdminToken(config)

const { app, stop } = createTaskcastApp({
  engine,
  shortTermStore: store,
  workerManager,
  auth: { mode: 'none' },
  config,
})

const apiServer = serve({ fetch: app.fetch, port: 3799 }, () => {
  console.log('[e2e] API server on http://localhost:3799')
})

// Start dashboard UI on port 3722 via CLI
const dashProc = spawn('node', [
  '../../packages/cli/dist/index.js',
  'ui',
  '--port', '3722',
  '--server', 'http://localhost:3799',
  '--admin-token', 'e2e-admin-token',
], {
  stdio: 'inherit',
  cwd: import.meta.dirname,
})

process.on('SIGTERM', cleanup)
process.on('SIGINT', cleanup)

function cleanup() {
  dashProc.kill()
  stop()
  apiServer.close()
  process.exit(0)
}
```

**Step 3: Install Playwright**

Run: `cd packages/e2e && npx playwright install chromium`

**Step 4: Commit**

```
feat(e2e): add Playwright config + browser server helpers
```

---

### Task 11: Browser E2E — login + navigation

**Files:**
- Create: `packages/e2e/browser/login.spec.ts`
- Create: `packages/e2e/browser/navigation.spec.ts`

**Step 1: Write login test**

```ts
import { test, expect } from '@playwright/test'

test.describe('login', () => {
  test('shows login page when not connected', async ({ page }) => {
    // Clear any stored state
    await page.context().clearCookies()
    await page.goto('/')
    await expect(page.getByText('Connect to Taskcast')).toBeVisible()
  })

  test('connects with server URL and admin token', async ({ page }) => {
    await page.goto('/')
    await page.getByPlaceholder(/server url/i).fill('http://localhost:3799')
    await page.getByPlaceholder(/admin token/i).fill('e2e-admin-token')
    await page.getByRole('button', { name: /connect/i }).click()

    // Should redirect to overview
    await expect(page.getByText(/overview/i)).toBeVisible({ timeout: 5000 })
  })

  test('shows error for invalid admin token', async ({ page }) => {
    await page.goto('/')
    await page.getByPlaceholder(/server url/i).fill('http://localhost:3799')
    await page.getByPlaceholder(/admin token/i).fill('wrong-token')
    await page.getByRole('button', { name: /connect/i }).click()

    await expect(page.getByText(/invalid/i)).toBeVisible({ timeout: 3000 })
  })
})
```

**Step 2: Write navigation test**

```ts
import { test, expect } from '@playwright/test'

test.describe('navigation', () => {
  test.beforeEach(async ({ page }) => {
    // Auto-connect via /api/config
    await page.goto('/')
    // Dashboard should auto-connect if started with --admin-token
    await expect(page.getByText(/overview/i)).toBeVisible({ timeout: 5000 })
  })

  test('sidebar navigates between pages', async ({ page }) => {
    await page.getByRole('link', { name: /tasks/i }).click()
    await expect(page).toHaveURL(/\/tasks/)

    await page.getByRole('link', { name: /events/i }).click()
    await expect(page).toHaveURL(/\/events/)

    await page.getByRole('link', { name: /workers/i }).click()
    await expect(page).toHaveURL(/\/workers/)

    await page.getByRole('link', { name: /overview/i }).click()
    await expect(page).toHaveURL(/\/$/)
  })

  test('direct URL access works (SPA routing)', async ({ page }) => {
    await page.goto('/tasks')
    await expect(page.getByText(/tasks/i)).toBeVisible()
  })
})
```

**Step 3: Run browser tests**

Run: `cd packages/e2e && npx playwright test browser/login.spec.ts browser/navigation.spec.ts`

Note: These will fail initially until the dashboard is built. The subagent should ensure `pnpm --filter @taskcast/dashboard-web build` runs before Playwright tests.

**Step 4: Commit**

```
test(e2e): add login + navigation browser tests
```

---

### Task 12: Browser E2E — tasks page

**Files:**
- Create: `packages/e2e/browser/tasks.spec.ts`

**Step 1: Write tasks page test**

```ts
import { test, expect } from '@playwright/test'

test.describe('tasks page', () => {
  test.beforeEach(async ({ page }) => {
    await page.goto('/')
    await expect(page.getByText(/overview/i)).toBeVisible({ timeout: 5000 })
    await page.getByRole('link', { name: /tasks/i }).click()
  })

  test('shows empty state initially', async ({ page }) => {
    await expect(page.getByText(/no tasks/i)).toBeVisible()
  })

  test('creates a task via dialog', async ({ page }) => {
    await page.getByRole('button', { name: /create/i }).click()
    await page.getByLabel(/type/i).fill('e2e-browser-test')
    await page.getByRole('button', { name: /create/i }).last().click()

    // Task should appear in the list
    await expect(page.getByText('e2e-browser-test')).toBeVisible({ timeout: 5000 })
    await expect(page.getByText('pending')).toBeVisible()
  })

  test('clicks task row to show detail', async ({ page }) => {
    // Create a task first via API
    await fetch('http://localhost:3799/tasks', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ id: 'detail-test', type: 'detail-type' }),
    })

    await page.reload()
    await page.getByText('detail-type').click()

    // Detail panel should show
    await expect(page.getByText('detail-test')).toBeVisible()
  })
})
```

**Step 2: Run**

Run: `cd packages/e2e && npx playwright test browser/tasks.spec.ts`

**Step 3: Commit**

```
test(e2e): add tasks page browser tests
```

---

### Task 13: CI workflow

**Files:**
- Create: `.github/workflows/e2e.yml`

**Step 1: Write the workflow**

```yaml
name: E2E Tests
on:
  pull_request:
  push:
    branches: [main]

jobs:
  api-e2e:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: pnpm/action-setup@v4
      - uses: actions/setup-node@v4
        with:
          node-version: 22
          cache: 'pnpm'
      - run: pnpm install --frozen-lockfile
      - run: pnpm build
      - run: pnpm --filter @taskcast/e2e test

  dashboard-unit:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: pnpm/action-setup@v4
      - uses: actions/setup-node@v4
        with:
          node-version: 22
          cache: 'pnpm'
      - run: pnpm install --frozen-lockfile
      - run: pnpm build
      - run: pnpm --filter @taskcast/dashboard-web test

  browser-e2e:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: pnpm/action-setup@v4
      - uses: actions/setup-node@v4
        with:
          node-version: 22
          cache: 'pnpm'
      - run: pnpm install --frozen-lockfile
      - run: pnpm build
      - run: npx playwright install --with-deps chromium
      - run: npx playwright test
        working-directory: packages/e2e
      - uses: actions/upload-artifact@v4
        if: failure()
        with:
          name: playwright-report
          path: packages/e2e/playwright-report/
          retention-days: 7
```

**Step 2: Commit**

```
ci: add e2e.yml workflow for API, dashboard, and browser tests
```

---

### Task 14: Final verification

**Step 1: Run all API E2E tests**

Run: `cd packages/e2e && npx vitest run`

**Step 2: Run all dashboard component tests**

Run: `cd packages/dashboard-web && npx vitest run`

**Step 3: Run browser E2E (requires built dashboard)**

Run: `pnpm --filter @taskcast/dashboard-web build && cd packages/e2e && npx playwright test`

**Step 4: Commit any fixes**

**Step 5: Invoke finishing-a-development-branch skill**
