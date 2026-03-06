# Integration Tests Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Add comprehensive integration tests across all Taskcast packages and fix 7 existing test failures.

**Architecture:** Each package gets its own `tests/integration/` directory. Server integration tests use real TaskEngine + Hono app with memory adapters (no Docker). Client/SDK tests connect to a real in-process server. CLI tests use temp files and spawn the startup logic directly. Testcontainer tests (Redis/Postgres) are separate files that can be skipped without Docker.

**Tech Stack:** vitest, @hono/node-server (for real HTTP in client/SDK tests), testcontainers (Redis/Postgres), jose (JWT for auth tests)

---

### Task 1: Fix worker-manager-remaining-gaps.test.ts (3 failures)

**Files:**
- Modify: `packages/core/tests/unit/worker-manager-remaining-gaps.test.ts:133-165,520-530`

The tests call `manager.releaseTask()` which exists at `packages/core/src/worker-manager.ts:317`. The issue is the import or setup. Verify the method exists on the manager instance. If the tests are importing a stale type, fix the test setup.

**Step 1: Read the test setup to understand why releaseTask is not found**

Run: `cd packages/core && pnpm test -- tests/unit/worker-manager-remaining-gaps.test.ts 2>&1 | head -30`

**Step 2: Fix the test calls**

The `WorkerManager` class at `packages/core/src/worker-manager.ts:317` has `async releaseTask(taskId: string): Promise<void>`. Check if `makeSetup()` returns the correct type. If the method exists on the class but not on the instance, the issue is likely the test's `makeSetup()` returning a different type or a stale build.

Run: `cd packages/core && pnpm build && pnpm test -- tests/unit/worker-manager-remaining-gaps.test.ts 2>&1 | tail -20`

If still failing, check the `makeSetup` function — it may be constructing `WorkerManager` with wrong args or the `releaseTask` method may have been renamed. Fix accordingly.

**Step 3: Run test to verify it passes**

Run: `cd packages/core && pnpm test -- tests/unit/worker-manager-remaining-gaps.test.ts -v`
Expected: All 30 tests pass

**Step 4: Commit**

```bash
git add packages/core/tests/unit/worker-manager-remaining-gaps.test.ts
git commit -m "fix: repair worker-manager-remaining-gaps tests for releaseTask"
```

---

### Task 2: Fix worker-release.test.ts (4 failures)

**Files:**
- Modify: `packages/server/tests/worker-release.test.ts`

The tests assert that terminal transitions release worker capacity. The server wires `releaseTask` via `addTransitionListener` at `packages/server/src/index.ts:97-101`. The `releaseTask` calls are fire-and-forget (`catch(() => {})`), and tests use `await Promise.resolve()` x2 which may not be enough microtick flushing.

**Step 1: Diagnose the timing issue**

The `releaseTask` in `WorkerManager` (line 317-342) does multiple awaits: `getTaskAssignment`, `removeAssignment`, `getWorker`, `saveWorker`, `getTask`, `saveTask`. Two `Promise.resolve()` calls aren't enough. Replace with `vi.waitFor()` or a small delay.

**Step 2: Update the test assertions to use vi.waitFor**

In each failing test, replace:
```ts
await Promise.resolve()
await Promise.resolve()

const workerAfter = await store.getWorker(worker.id)
expect(workerAfter!.usedSlots).toBe(0)
```

With:
```ts
await vi.waitFor(async () => {
  const workerAfter = await store.getWorker(worker.id)
  expect(workerAfter!.usedSlots).toBe(0)
})
```

Also fix the full-flow test (line 232) where `claimResult.success` should be `claimResult` (claimTask returns a boolean or an object — check the `WorkerManager.claimTask` return type).

**Step 3: Run test to verify it passes**

Run: `cd packages/server && pnpm test -- tests/worker-release.test.ts -v`
Expected: All 6 tests pass

**Step 4: Run full test suite to confirm no regressions**

Run: `pnpm test 2>&1 | tail -5`
Expected: 0 failures (excluding Docker-dependent skips)

**Step 5: Commit**

```bash
git add packages/server/tests/worker-release.test.ts
git commit -m "fix: use vi.waitFor for async capacity release in worker-release tests"
```

---

### Task 3: Shared test infrastructure — test-server helper

**Files:**
- Create: `packages/server/tests/helpers/test-server.ts`

This helper creates a real TaskEngine + Hono app with memory adapters for integration tests.

**Step 1: Write the helper**

```ts
import {
  TaskEngine,
  MemoryBroadcastProvider,
  MemoryShortTermStore,
  WorkerManager,
} from '@taskcast/core'
import type { LongTermStore } from '@taskcast/core'
import { createTaskcastApp } from '../../src/index.js'
import type { TaskcastServerOptions } from '../../src/index.js'
import type { AuthConfig } from '../../src/auth.js'

export interface TestServerOptions {
  auth?: AuthConfig
  withWorkerManager?: boolean
  longTermStore?: LongTermStore
}

export interface TestServer {
  app: ReturnType<typeof createTaskcastApp>['app']
  engine: TaskEngine
  store: MemoryShortTermStore
  broadcast: MemoryBroadcastProvider
  workerManager?: WorkerManager
  stop: () => void
}

export function createTestServer(opts?: TestServerOptions): TestServer {
  const store = new MemoryShortTermStore()
  const broadcast = new MemoryBroadcastProvider()
  const engineOpts: ConstructorParameters<typeof TaskEngine>[0] = {
    shortTermStore: store,
    broadcast,
  }
  if (opts?.longTermStore) engineOpts.longTermStore = opts.longTermStore
  const engine = new TaskEngine(engineOpts)

  let workerManager: WorkerManager | undefined
  if (opts?.withWorkerManager) {
    workerManager = new WorkerManager({ engine, shortTermStore: store, broadcast })
  }

  const serverOpts: TaskcastServerOptions = {
    engine,
    shortTermStore: store,
    auth: opts?.auth ?? { mode: 'none' },
    ...(workerManager && { workerManager }),
  }
  const { app, stop } = createTaskcastApp(serverOpts)

  return { app, engine, store, broadcast, workerManager, stop }
}
```

**Step 2: Verify it compiles**

Run: `cd packages/server && npx tsc --noEmit tests/helpers/test-server.ts 2>&1 || echo 'check imports'`

If tsc doesn't work on test files directly, just proceed — vitest will validate.

**Step 3: Commit**

```bash
git add packages/server/tests/helpers/test-server.ts
git commit -m "test: add shared test server helper for integration tests"
```

---

### Task 4: Shared test infrastructure — SSE collector utility

**Files:**
- Create: `packages/server/tests/helpers/sse-collector.ts`

Reusable SSE event collector extracted from existing `sse.test.ts` pattern.

**Step 1: Write the helper**

```ts
export interface SSEEvent {
  event: string
  data: string
  id?: string
}

/**
 * Collects SSE events from a Response stream.
 * Resolves when `count` events are collected or the stream ends.
 */
export async function collectSSEEvents(
  res: Response,
  count: number,
): Promise<SSEEvent[]> {
  const reader = res.body!.getReader()
  const decoder = new TextDecoder()
  const collected: SSEEvent[] = []
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
      const idLine = lines.find((l) => l.startsWith('id:'))
      if (eventLine && dataLine) {
        collected.push({
          event: eventLine.replace('event:', '').trim(),
          data: dataLine.replace('data:', '').trim(),
          ...(idLine && { id: idLine.replace('id:', '').trim() }),
        })
      }
    }
  }

  reader.cancel()
  return collected
}

/**
 * Collects ALL SSE events until the stream closes.
 * Use for terminal tasks where the server will close the connection.
 */
export async function collectAllSSEEvents(res: Response): Promise<SSEEvent[]> {
  return collectSSEEvents(res, Infinity)
}
```

**Step 2: Commit**

```bash
git add packages/server/tests/helpers/sse-collector.ts
git commit -m "test: add SSE event collector utility for integration tests"
```

---

### Task 5: Server integration — task-lifecycle.test.ts

**Files:**
- Create: `packages/server/tests/integration/task-lifecycle.test.ts`

**Step 1: Write the test**

```ts
import { describe, it, expect, afterEach } from 'vitest'
import { createTestServer } from '../helpers/test-server.js'
import type { TestServer } from '../helpers/test-server.js'

describe('Server integration — task lifecycle', () => {
  let server: TestServer

  afterEach(() => server?.stop())

  it('POST create -> POST events -> PATCH complete -> GET query', async () => {
    server = createTestServer()
    const { app } = server

    // Create task
    const createRes = await app.request('/tasks', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ type: 'llm.chat', params: { model: 'gpt-4' } }),
    })
    expect(createRes.status).toBe(201)
    const task = await createRes.json()
    expect(task.id).toBeTruthy()
    expect(task.status).toBe('pending')

    // Transition to running
    const runRes = await app.request(`/tasks/${task.id}/status`, {
      method: 'PATCH',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ status: 'running' }),
    })
    expect(runRes.status).toBe(200)

    // Publish events
    const evtRes = await app.request(`/tasks/${task.id}/events`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ type: 'llm.delta', level: 'info', data: { text: 'hello' } }),
    })
    expect(evtRes.status).toBe(201)

    // Complete with result
    const completeRes = await app.request(`/tasks/${task.id}/status`, {
      method: 'PATCH',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ status: 'completed', result: { answer: 42 } }),
    })
    expect(completeRes.status).toBe(200)
    const completed = await completeRes.json()
    expect(completed.status).toBe('completed')
    expect(completed.completedAt).toBeTruthy()
    expect(completed.result).toEqual({ answer: 42 })

    // GET final state
    const getRes = await app.request(`/tasks/${task.id}`)
    expect(getRes.status).toBe(200)
    const fetched = await getRes.json()
    expect(fetched.status).toBe('completed')
    expect(fetched.result).toEqual({ answer: 42 })
  })

  it('batch events -> GET history preserves order', async () => {
    server = createTestServer()
    const { app } = server

    const task = (await (await app.request('/tasks', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ type: 'test' }),
    })).json())

    await app.request(`/tasks/${task.id}/status`, {
      method: 'PATCH',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ status: 'running' }),
    })

    // Batch publish
    const events = Array.from({ length: 5 }, (_, i) => ({
      type: 'chunk', level: 'info', data: { index: i },
    }))
    const batchRes = await app.request(`/tasks/${task.id}/events`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify(events),
    })
    expect(batchRes.status).toBe(201)

    // GET history
    const historyRes = await app.request(`/tasks/${task.id}/events/history`)
    expect(historyRes.status).toBe(200)
    const history = await historyRes.json()
    // History includes taskcast:status(running) + 5 chunks
    const chunks = history.filter((e: { type: string }) => e.type === 'chunk')
    expect(chunks).toHaveLength(5)
    for (let i = 0; i < 5; i++) {
      expect(chunks[i].data.index).toBe(i)
    }
  })

  it('publish to terminal task returns error', async () => {
    server = createTestServer()
    const { app } = server

    const task = (await (await app.request('/tasks', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({}),
    })).json())

    await app.request(`/tasks/${task.id}/status`, {
      method: 'PATCH',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ status: 'running' }),
    })
    await app.request(`/tasks/${task.id}/status`, {
      method: 'PATCH',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ status: 'completed' }),
    })

    const evtRes = await app.request(`/tasks/${task.id}/events`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ type: 'late', level: 'info', data: null }),
    })
    expect(evtRes.status).toBeGreaterThanOrEqual(400)
  })

  it('JSON round-trip has camelCase fields', async () => {
    server = createTestServer()
    const { app } = server

    const createRes = await app.request('/tasks', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ type: 'test', metadata: { userId: 'u1' } }),
    })
    const task = await createRes.json()
    expect(task).toHaveProperty('createdAt')
    expect(task).toHaveProperty('updatedAt')
    expect(task).not.toHaveProperty('created_at')
  })
})
```

**Step 2: Run test**

Run: `cd packages/server && pnpm test -- tests/integration/task-lifecycle.test.ts -v`
Expected: All 4 tests pass

**Step 3: Commit**

```bash
git add packages/server/tests/integration/task-lifecycle.test.ts
git commit -m "test: add server task lifecycle integration tests"
```

---

### Task 6: Server integration — sse-streaming.test.ts

**Files:**
- Create: `packages/server/tests/integration/sse-streaming.test.ts`

**Step 1: Write the test**

```ts
import { describe, it, expect, afterEach } from 'vitest'
import { createTestServer } from '../helpers/test-server.js'
import { collectSSEEvents, collectAllSSEEvents } from '../helpers/sse-collector.js'
import type { TestServer } from '../helpers/test-server.js'

describe('Server integration — SSE streaming', () => {
  let server: TestServer

  afterEach(() => server?.stop())

  it('replays history + streams live events for running task', async () => {
    server = createTestServer()
    const { app, engine } = server

    const task = await engine.createTask({ type: 'test' })
    await engine.transitionTask(task.id, 'running')
    await engine.publishEvent(task.id, { type: 'history.event', level: 'info', data: { n: 1 } })

    // Schedule live events after SSE connects
    setTimeout(async () => {
      await engine.publishEvent(task.id, { type: 'live.event', level: 'info', data: { n: 2 } })
      await engine.transitionTask(task.id, 'completed')
    }, 50)

    const res = await app.request(`/tasks/${task.id}/events`)
    expect(res.headers.get('content-type')).toContain('text/event-stream')

    // running status + history.event + live.event + completed status + done
    const events = await collectSSEEvents(res, 5)
    const dataEvents = events.filter(e => e.event === 'taskcast.event')
    const types = dataEvents.map(e => JSON.parse(e.data).type)
    expect(types).toContain('history.event')
    expect(types).toContain('live.event')
    expect(events.some(e => e.event === 'taskcast.done')).toBe(true)
  }, 10000)

  it('terminal task replays then closes immediately', async () => {
    server = createTestServer()
    const { app, engine } = server

    const task = await engine.createTask({})
    await engine.transitionTask(task.id, 'running')
    await engine.publishEvent(task.id, { type: 'evt', level: 'info', data: null })
    await engine.transitionTask(task.id, 'completed')

    const res = await app.request(`/tasks/${task.id}/events`)
    const events = await collectAllSSEEvents(res)
    const done = events.find(e => e.event === 'taskcast.done')
    expect(done).toBeTruthy()
    expect(JSON.parse(done!.data).reason).toBe('completed')
  })

  it('10 concurrent SSE clients all receive same events', async () => {
    server = createTestServer()
    const { app, engine } = server

    const task = await engine.createTask({})
    await engine.transitionTask(task.id, 'running')

    setTimeout(async () => {
      for (let i = 0; i < 5; i++) {
        await engine.publishEvent(task.id, { type: 'chunk', level: 'info', data: { i } })
      }
      await engine.transitionTask(task.id, 'completed')
    }, 50)

    // 10 concurrent SSE connections
    const promises = Array.from({ length: 10 }, () =>
      app.request(`/tasks/${task.id}/events?includeStatus=false`).then(r => collectAllSSEEvents(r))
    )
    const results = await Promise.all(promises)

    for (const events of results) {
      const dataEvents = events.filter(e => e.event === 'taskcast.event')
      expect(dataEvents).toHaveLength(5)
      expect(events.some(e => e.event === 'taskcast.done')).toBe(true)
    }
  }, 15000)

  it('filter by type only returns matching events', async () => {
    server = createTestServer()
    const { app, engine } = server

    const task = await engine.createTask({})
    await engine.transitionTask(task.id, 'running')
    await engine.publishEvent(task.id, { type: 'llm.delta', level: 'info', data: null })
    await engine.publishEvent(task.id, { type: 'tool.call', level: 'info', data: null })
    await engine.transitionTask(task.id, 'completed')

    const res = await app.request(`/tasks/${task.id}/events?types=llm.*&includeStatus=false`)
    const events = await collectAllSSEEvents(res)
    const dataEvents = events.filter(e => e.event === 'taskcast.event')
    expect(dataEvents).toHaveLength(1)
    expect(JSON.parse(dataEvents[0]!.data).type).toBe('llm.delta')
  })

  it('wrap=false returns raw event without filteredIndex', async () => {
    server = createTestServer()
    const { app, engine } = server

    const task = await engine.createTask({})
    await engine.transitionTask(task.id, 'running')
    await engine.publishEvent(task.id, { type: 'test', level: 'info', data: { x: 1 } })
    await engine.transitionTask(task.id, 'completed')

    const res = await app.request(`/tasks/${task.id}/events?wrap=false&includeStatus=false`)
    const events = await collectAllSSEEvents(res)
    const dataEvent = events.find(e => e.event === 'taskcast.event')
    const parsed = JSON.parse(dataEvent!.data)
    expect(parsed).toHaveProperty('id')
    expect(parsed).toHaveProperty('taskId')
    expect(parsed).not.toHaveProperty('filteredIndex')
  })
})
```

**Step 2: Run test**

Run: `cd packages/server && pnpm test -- tests/integration/sse-streaming.test.ts -v`

**Step 3: Commit**

```bash
git add packages/server/tests/integration/sse-streaming.test.ts
git commit -m "test: add server SSE streaming integration tests"
```

---

### Task 7: Server integration — concurrent-transitions.test.ts

**Files:**
- Create: `packages/server/tests/integration/concurrent-transitions.test.ts`

**Step 1: Write the test**

```ts
import { describe, it, expect, afterEach } from 'vitest'
import { createTestServer } from '../helpers/test-server.js'
import type { TestServer } from '../helpers/test-server.js'

describe('Server integration — concurrent transitions', () => {
  let server: TestServer

  afterEach(() => server?.stop())

  it('10 concurrent PATCH complete — exactly one succeeds', async () => {
    server = createTestServer()
    const { app, engine } = server

    const task = await engine.createTask({})
    await engine.transitionTask(task.id, 'running')

    const results = await Promise.all(
      Array.from({ length: 10 }, () =>
        app.request(`/tasks/${task.id}/status`, {
          method: 'PATCH',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify({ status: 'completed' }),
        })
      )
    )

    const statuses = results.map(r => r.status)
    const successes = statuses.filter(s => s === 200)
    const failures = statuses.filter(s => s >= 400)

    expect(successes).toHaveLength(1)
    expect(failures).toHaveLength(9)
  })

  it('task state is consistent after concurrent race', async () => {
    server = createTestServer()
    const { app, engine } = server

    const task = await engine.createTask({})
    await engine.transitionTask(task.id, 'running')

    // Race: some try to complete, some try to fail
    await Promise.all([
      ...Array.from({ length: 5 }, () =>
        app.request(`/tasks/${task.id}/status`, {
          method: 'PATCH',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify({ status: 'completed' }),
        })
      ),
      ...Array.from({ length: 5 }, () =>
        app.request(`/tasks/${task.id}/status`, {
          method: 'PATCH',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify({ status: 'failed', error: { message: 'oops' } }),
        })
      ),
    ])

    // Final state should be one of the terminal states
    const getRes = await app.request(`/tasks/${task.id}`)
    const final = await getRes.json()
    expect(['completed', 'failed']).toContain(final.status)
  })
})
```

**Step 2: Run test**

Run: `cd packages/server && pnpm test -- tests/integration/concurrent-transitions.test.ts -v`

**Step 3: Commit**

```bash
git add packages/server/tests/integration/concurrent-transitions.test.ts
git commit -m "test: add concurrent transition integration tests"
```

---

### Task 8: Server integration — auth-scope.test.ts

**Files:**
- Create: `packages/server/tests/integration/auth-scope.test.ts`

This test needs JWT tokens. Check existing `packages/server/tests/auth.test.ts` for the JWT signing pattern.

**Step 1: Read existing auth test for JWT signing pattern**

Read `packages/server/tests/auth.test.ts` to extract the token creation approach.

**Step 2: Write the test**

Use `jose` library (already used in auth.test.ts) to sign JWTs with specific scopes and taskIds. Create the server with `auth: { mode: 'jwt', secret: 'test-secret' }`.

Test scenarios:
- Token with `taskIds: ['task-1']` trying to access task-2 → 403
- Token with only `event:subscribe` scope trying POST /tasks → 403
- No token → 401
- Token with `task:create` + `event:publish` → create task and publish event succeeds

**Step 3: Run test**

Run: `cd packages/server && pnpm test -- tests/integration/auth-scope.test.ts -v`

**Step 4: Commit**

```bash
git add packages/server/tests/integration/auth-scope.test.ts
git commit -m "test: add auth scope enforcement integration tests"
```

---

### Task 9: Server integration — webhook-delivery.test.ts

**Files:**
- Create: `packages/server/tests/integration/webhook-delivery.test.ts`

Uses a local HTTP handler (Hono app or plain http server) to receive webhooks.

**Step 1: Write the test**

Create a local webhook receiver using Hono. Create a task with a webhook config pointing at the receiver. Publish events and verify the receiver gets them.

Key scenarios:
- Engine event → webhook delivered to local receiver with correct payload
- HMAC signature on received webhook is valid
- Webhook with filter only receives matching events
- Receiver returns 500 first → retry → success

Use `WebhookDelivery` from `packages/server/src/webhook.ts` wired through the engine hooks.

**Step 2: Run test**

Run: `cd packages/server && pnpm test -- tests/integration/webhook-delivery.test.ts -v`

**Step 3: Commit**

```bash
git add packages/server/tests/integration/webhook-delivery.test.ts
git commit -m "test: add webhook delivery integration tests"
```

---

### Task 10: Server integration — worker-flow.test.ts

**Files:**
- Create: `packages/server/tests/integration/worker-flow.test.ts`

**Step 1: Write the test**

```ts
import { describe, it, expect, afterEach } from 'vitest'
import { vi } from 'vitest'
import { createTestServer } from '../helpers/test-server.js'
import type { TestServer } from '../helpers/test-server.js'

describe('Server integration — worker flow', () => {
  let server: TestServer

  afterEach(() => server?.stop())

  it('register -> claim -> run -> complete -> capacity released', async () => {
    server = createTestServer({ withWorkerManager: true })
    const { app, store, workerManager } = server

    // Register worker
    const worker = await workerManager!.registerWorker({
      matchRule: {}, capacity: 5, connectionMode: 'pull',
    })

    // Create and claim task
    const createRes = await app.request('/tasks', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ type: 'test', cost: 2 }),
    })
    const task = await createRes.json()
    await workerManager!.claimTask(task.id, worker.id)

    const busyWorker = await store.getWorker(worker.id)
    expect(busyWorker!.usedSlots).toBe(2)

    // Run and complete via HTTP
    await app.request(`/tasks/${task.id}/status`, {
      method: 'PATCH',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ status: 'running' }),
    })
    await app.request(`/tasks/${task.id}/status`, {
      method: 'PATCH',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ status: 'completed' }),
    })

    // Wait for async releaseTask
    await vi.waitFor(async () => {
      const w = await store.getWorker(worker.id)
      expect(w!.usedSlots).toBe(0)
      expect(w!.status).toBe('idle')
    })
  })

  it('concurrent claim race — only one worker succeeds', async () => {
    server = createTestServer({ withWorkerManager: true })
    const { workerManager, engine } = server

    const workers = await Promise.all(
      Array.from({ length: 5 }, (_, i) =>
        workerManager!.registerWorker({
          matchRule: {}, capacity: 1, connectionMode: 'pull',
        })
      )
    )

    const task = await engine.createTask({ type: 'test', cost: 1 })

    const claims = await Promise.all(
      workers.map(w => workerManager!.claimTask(task.id, w.id).catch(() => null))
    )
    const successes = claims.filter(c => c && (typeof c === 'object' ? c.success : c))
    expect(successes.length).toBeLessThanOrEqual(1)
  })
})
```

**Step 2: Run test**

Run: `cd packages/server && pnpm test -- tests/integration/worker-flow.test.ts -v`

**Step 3: Commit**

```bash
git add packages/server/tests/integration/worker-flow.test.ts
git commit -m "test: add worker flow integration tests"
```

---

### Task 11: Core integration — lifecycle.test.ts

**Files:**
- Create: `packages/core/tests/integration/lifecycle.test.ts`

**Step 1: Write the test**

```ts
import { describe, it, expect, vi } from 'vitest'
import { TaskEngine } from '../../src/engine.js'
import { MemoryBroadcastProvider, MemoryShortTermStore } from '../../src/memory-adapters.js'
import type { LongTermStore, TaskEvent, TaskcastHooks } from '../../src/types.js'

function makeEngine(opts?: { hooks?: TaskcastHooks; longTermStore?: LongTermStore }) {
  const store = new MemoryShortTermStore()
  const broadcast = new MemoryBroadcastProvider()
  const engine = new TaskEngine({
    shortTermStore: store,
    broadcast,
    ...opts,
  })
  return { engine, store, broadcast }
}

describe('Core integration — full lifecycle', () => {
  it('pending -> running -> completed with hooks in order', async () => {
    const hookOrder: string[] = []
    const hooks: TaskcastHooks = {
      onTaskCreated: () => hookOrder.push('created'),
      onTaskTransitioned: (_task, _from, to) => hookOrder.push(`transitioned:${to}`),
    }
    const { engine } = makeEngine({ hooks })

    const task = await engine.createTask({ type: 'test' })
    expect(task.status).toBe('pending')

    const running = await engine.transitionTask(task.id, 'running')
    expect(running.status).toBe('running')

    await engine.publishEvent(task.id, { type: 'chunk', level: 'info', data: { text: 'hi' } })

    const completed = await engine.transitionTask(task.id, 'completed', {
      result: { answer: 42 },
    })
    expect(completed.status).toBe('completed')
    expect(completed.completedAt).toBeTruthy()
    expect(completed.result).toEqual({ answer: 42 })

    expect(hookOrder).toEqual(['created', 'transitioned:running', 'transitioned:completed'])
  })

  it('series accumulate across lifecycle', async () => {
    const { engine } = makeEngine()
    const task = await engine.createTask({})
    await engine.transitionTask(task.id, 'running')

    for (let i = 0; i < 10; i++) {
      await engine.publishEvent(task.id, {
        type: 'token',
        level: 'info',
        data: { text: `word${i} ` },
        seriesId: 'output',
        seriesMode: 'accumulate',
        seriesAccField: 'text',
      })
    }

    const events = await engine.getEvents(task.id)
    const seriesEvents = events.filter(e => e.seriesId === 'output')
    // accumulate mode replaces in-place — should have 1 accumulated event
    expect(seriesEvents).toHaveLength(1)
    const text = (seriesEvents[0]!.data as { text: string }).text
    for (let i = 0; i < 10; i++) {
      expect(text).toContain(`word${i}`)
    }
  })

  it('result/error persistence through transitions', async () => {
    const { engine } = makeEngine()
    const task = await engine.createTask({})
    await engine.transitionTask(task.id, 'running')
    const failed = await engine.transitionTask(task.id, 'failed', {
      error: { code: 'E001', message: 'boom', details: { stack: 'trace' } },
    })

    expect(failed.error).toEqual({ code: 'E001', message: 'boom', details: { stack: 'trace' } })

    const fetched = await engine.getTask(task.id)
    expect(fetched!.error).toEqual(failed.error)
  })

  it('LongTermStore receives async writes', async () => {
    const longTermEvents: TaskEvent[] = []
    const longTermStore: LongTermStore = {
      saveTask: vi.fn().mockResolvedValue(undefined),
      getTask: vi.fn().mockResolvedValue(null),
      saveEvent: vi.fn().mockImplementation(async (e: TaskEvent) => { longTermEvents.push(e) }),
      getEvents: vi.fn().mockResolvedValue([]),
      saveWorkerEvent: vi.fn().mockResolvedValue(undefined),
      getWorkerEvents: vi.fn().mockResolvedValue([]),
    }

    const { engine } = makeEngine({ longTermStore })
    const task = await engine.createTask({})
    await engine.transitionTask(task.id, 'running')
    await engine.publishEvent(task.id, { type: 'evt', level: 'info', data: null })

    // Allow async longTermStore.saveEvent to complete
    await vi.waitFor(() => {
      expect(longTermEvents.length).toBeGreaterThan(0)
    })
  })
})
```

**Step 2: Run test**

Run: `cd packages/core && pnpm test -- tests/integration/lifecycle.test.ts -v`

**Step 3: Commit**

```bash
git add packages/core/tests/integration/lifecycle.test.ts
git commit -m "test: add core lifecycle integration tests"
```

---

### Task 12: Core integration — multi-subscriber.test.ts

**Files:**
- Create: `packages/core/tests/integration/multi-subscriber.test.ts`

**Step 1: Write the test**

```ts
import { describe, it, expect } from 'vitest'
import { TaskEngine } from '../../src/engine.js'
import { MemoryBroadcastProvider, MemoryShortTermStore } from '../../src/memory-adapters.js'
import type { TaskEvent } from '../../src/types.js'

function makeEngine() {
  const store = new MemoryShortTermStore()
  const broadcast = new MemoryBroadcastProvider()
  const engine = new TaskEngine({ shortTermStore: store, broadcast })
  return { engine, store, broadcast }
}

describe('Core integration — multi-subscriber', () => {
  it('5 subscribers receive events independently', async () => {
    const { engine } = makeEngine()
    const task = await engine.createTask({})
    await engine.transitionTask(task.id, 'running')

    const received: TaskEvent[][] = Array.from({ length: 5 }, () => [])
    const unsubs = received.map((arr, _i) =>
      engine.subscribe(task.id, (evt) => arr.push(evt))
    )

    await engine.publishEvent(task.id, { type: 'llm.delta', level: 'info', data: { n: 1 } })
    await engine.publishEvent(task.id, { type: 'tool.call', level: 'info', data: { n: 2 } })
    await engine.publishEvent(task.id, { type: 'llm.delta', level: 'info', data: { n: 3 } })

    // All 5 subscribers receive all 3 events
    for (const arr of received) {
      expect(arr).toHaveLength(3)
    }

    unsubs.forEach(fn => fn())
  })

  it('unsubscribed client stops receiving', async () => {
    const { engine } = makeEngine()
    const task = await engine.createTask({})
    await engine.transitionTask(task.id, 'running')

    const received1: TaskEvent[] = []
    const received2: TaskEvent[] = []

    const unsub1 = engine.subscribe(task.id, (evt) => received1.push(evt))
    engine.subscribe(task.id, (evt) => received2.push(evt))

    await engine.publishEvent(task.id, { type: 'first', level: 'info', data: null })

    // Unsubscribe client 1
    unsub1()

    await engine.publishEvent(task.id, { type: 'second', level: 'info', data: null })

    expect(received1).toHaveLength(1)
    expect(received2).toHaveLength(2)
  })

  it('late subscriber only gets events after subscription', async () => {
    const { engine } = makeEngine()
    const task = await engine.createTask({})
    await engine.transitionTask(task.id, 'running')

    // Publish before subscribing
    await engine.publishEvent(task.id, { type: 'before', level: 'info', data: null })

    const received: TaskEvent[] = []
    const unsub = engine.subscribe(task.id, (evt) => received.push(evt))

    await engine.publishEvent(task.id, { type: 'after', level: 'info', data: null })

    expect(received).toHaveLength(1)
    expect(received[0]!.type).toBe('after')

    // But history has both
    const history = await engine.getEvents(task.id)
    const userEvents = history.filter(e => !e.type.startsWith('taskcast:'))
    expect(userEvents).toHaveLength(2)

    unsub()
  })
})
```

**Step 2: Run test**

Run: `cd packages/core && pnpm test -- tests/integration/multi-subscriber.test.ts -v`

**Step 3: Commit**

```bash
git add packages/core/tests/integration/multi-subscriber.test.ts
git commit -m "test: add core multi-subscriber integration tests"
```

---

### Task 13: Client integration — sse-client.test.ts

**Files:**
- Create: `packages/client/tests/integration/sse-client.test.ts`

This test needs a real HTTP server. Use `@hono/node-server` to `serve()` the Hono app, then connect the TaskcastClient to it.

**Step 1: Write the test**

```ts
import { describe, it, expect, afterEach } from 'vitest'
import { serve } from '@hono/node-server'
import {
  TaskEngine,
  MemoryBroadcastProvider,
  MemoryShortTermStore,
} from '@taskcast/core'
import { createTaskcastApp } from '@taskcast/server'
import { TaskcastClient } from '../../src/client.js'
import type { SSEEnvelope } from '@taskcast/core'

function startRealServer() {
  const store = new MemoryShortTermStore()
  const broadcast = new MemoryBroadcastProvider()
  const engine = new TaskEngine({ shortTermStore: store, broadcast })
  const { app, stop } = createTaskcastApp({ engine, auth: { mode: 'none' } })

  return new Promise<{
    engine: typeof engine
    baseUrl: string
    close: () => void
  }>((resolve) => {
    const server = serve({ fetch: app.fetch, port: 0 }, (info) => {
      const port = (info as { port: number }).port
      resolve({
        engine,
        baseUrl: `http://localhost:${port}`,
        close: () => { stop(); server.close() },
      })
    })
  })
}

describe('Client integration — real SSE endpoint', () => {
  let close: (() => void) | undefined

  afterEach(() => close?.())

  it('receives events and done via real SSE', async () => {
    const { engine, baseUrl, close: closeFn } = await startRealServer()
    close = closeFn

    const task = await engine.createTask({})
    await engine.transitionTask(task.id, 'running')

    // Schedule events
    setTimeout(async () => {
      await engine.publishEvent(task.id, { type: 'chunk', level: 'info', data: { text: 'hi' } })
      await engine.transitionTask(task.id, 'completed')
    }, 100)

    const events: SSEEnvelope[] = []
    let doneReason = ''

    const client = new TaskcastClient({ baseUrl })
    await client.subscribe(task.id, {
      onEvent: (env) => events.push(env),
      onDone: (reason) => { doneReason = reason },
    })

    expect(events.length).toBeGreaterThan(0)
    expect(doneReason).toBe('completed')
  }, 15000)

  it('filter types only returns matching events', async () => {
    const { engine, baseUrl, close: closeFn } = await startRealServer()
    close = closeFn

    const task = await engine.createTask({})
    await engine.transitionTask(task.id, 'running')
    await engine.publishEvent(task.id, { type: 'llm.delta', level: 'info', data: null })
    await engine.publishEvent(task.id, { type: 'tool.call', level: 'info', data: null })
    await engine.transitionTask(task.id, 'completed')

    const events: SSEEnvelope[] = []
    const client = new TaskcastClient({ baseUrl })
    await client.subscribe(task.id, {
      filter: { types: ['llm.*'], includeStatus: false },
      onEvent: (env) => events.push(env),
      onDone: () => {},
    })

    const types = events.map(e => e.type)
    expect(types).toContain('llm.delta')
    expect(types).not.toContain('tool.call')
  }, 15000)
})
```

**Step 2: Add @hono/node-server and @taskcast/server as dev dependencies for client package**

Run: `cd packages/client && pnpm add -D @hono/node-server @taskcast/server @taskcast/core`

Note: @taskcast/core may already be a dependency. Check `package.json` first.

**Step 3: Run test**

Run: `cd packages/client && pnpm test -- tests/integration/sse-client.test.ts -v`

**Step 4: Commit**

```bash
git add packages/client/
git commit -m "test: add client SSE integration tests against real server"
```

---

### Task 14: Server-SDK integration — sdk-client.test.ts

**Files:**
- Create: `packages/server-sdk/tests/integration/sdk-client.test.ts`

Same pattern as client: start real HTTP server with `@hono/node-server`, use `TaskcastServerClient`.

**Step 1: Write the test**

```ts
import { describe, it, expect, afterEach } from 'vitest'
import { serve } from '@hono/node-server'
import {
  TaskEngine,
  MemoryBroadcastProvider,
  MemoryShortTermStore,
} from '@taskcast/core'
import { createTaskcastApp } from '@taskcast/server'
import { TaskcastServerClient } from '../../src/client.js'

function startRealServer() {
  const store = new MemoryShortTermStore()
  const broadcast = new MemoryBroadcastProvider()
  const engine = new TaskEngine({ shortTermStore: store, broadcast })
  const { app, stop } = createTaskcastApp({ engine, auth: { mode: 'none' } })

  return new Promise<{ baseUrl: string; close: () => void }>((resolve) => {
    const server = serve({ fetch: app.fetch, port: 0 }, (info) => {
      resolve({
        baseUrl: `http://localhost:${(info as { port: number }).port}`,
        close: () => { stop(); server.close() },
      })
    })
  })
}

describe('Server-SDK integration — real HTTP server', () => {
  let close: (() => void) | undefined

  afterEach(() => close?.())

  it('createTask -> getTask returns consistent data', async () => {
    const { baseUrl, close: closeFn } = await startRealServer()
    close = closeFn

    const sdk = new TaskcastServerClient({ baseUrl })
    const created = await sdk.createTask({ type: 'llm.chat' })
    expect(created.id).toBeTruthy()
    expect(created.status).toBe('pending')

    const fetched = await sdk.getTask(created.id)
    expect(fetched.id).toBe(created.id)
    expect(fetched.type).toBe('llm.chat')
  })

  it('full transition flow: pending -> running -> completed', async () => {
    const { baseUrl, close: closeFn } = await startRealServer()
    close = closeFn

    const sdk = new TaskcastServerClient({ baseUrl })
    const task = await sdk.createTask({})

    const running = await sdk.transitionTask(task.id, 'running')
    expect(running.status).toBe('running')

    const completed = await sdk.transitionTask(task.id, 'completed', {
      result: { answer: 42 },
    })
    expect(completed.status).toBe('completed')
    expect(completed.result).toEqual({ answer: 42 })
  })

  it('publishEvent + getHistory returns events in order', async () => {
    const { baseUrl, close: closeFn } = await startRealServer()
    close = closeFn

    const sdk = new TaskcastServerClient({ baseUrl })
    const task = await sdk.createTask({})
    await sdk.transitionTask(task.id, 'running')

    await sdk.publishEvent(task.id, { type: 'e1', level: 'info', data: { n: 1 } })
    await sdk.publishEvent(task.id, { type: 'e2', level: 'info', data: { n: 2 } })
    await sdk.publishEvent(task.id, { type: 'e3', level: 'info', data: { n: 3 } })

    const history = await sdk.getHistory(task.id)
    const userEvents = history.filter(e => !e.type.startsWith('taskcast:'))
    expect(userEvents).toHaveLength(3)
    expect(userEvents.map(e => e.type)).toEqual(['e1', 'e2', 'e3'])
  })

  it('getTask for nonexistent task throws', async () => {
    const { baseUrl, close: closeFn } = await startRealServer()
    close = closeFn

    const sdk = new TaskcastServerClient({ baseUrl })
    await expect(sdk.getTask('nonexistent')).rejects.toThrow()
  })

  it('double complete throws conflict error', async () => {
    const { baseUrl, close: closeFn } = await startRealServer()
    close = closeFn

    const sdk = new TaskcastServerClient({ baseUrl })
    const task = await sdk.createTask({})
    await sdk.transitionTask(task.id, 'running')
    await sdk.transitionTask(task.id, 'completed')

    await expect(sdk.transitionTask(task.id, 'completed')).rejects.toThrow()
  })

  it('batch publish events', async () => {
    const { baseUrl, close: closeFn } = await startRealServer()
    close = closeFn

    const sdk = new TaskcastServerClient({ baseUrl })
    const task = await sdk.createTask({})
    await sdk.transitionTask(task.id, 'running')

    const inputs = Array.from({ length: 10 }, (_, i) => ({
      type: 'chunk', level: 'info' as const, data: { i },
    }))
    const results = await sdk.publishEvents(task.id, inputs)
    expect(results).toHaveLength(10)

    const history = await sdk.getHistory(task.id)
    const chunks = history.filter(e => e.type === 'chunk')
    expect(chunks).toHaveLength(10)
  })

  it('since pagination returns incremental results', async () => {
    const { baseUrl, close: closeFn } = await startRealServer()
    close = closeFn

    const sdk = new TaskcastServerClient({ baseUrl })
    const task = await sdk.createTask({})
    await sdk.transitionTask(task.id, 'running')

    await sdk.publishEvent(task.id, { type: 'e1', level: 'info', data: null })
    await sdk.publishEvent(task.id, { type: 'e2', level: 'info', data: null })
    await sdk.publishEvent(task.id, { type: 'e3', level: 'info', data: null })

    const all = await sdk.getHistory(task.id)
    // Get events after the second event (index-based)
    const partial = await sdk.getHistory(task.id, { since: { index: all[1]!.index } })
    expect(partial.length).toBeLessThan(all.length)
  })
})
```

**Step 2: Add dev dependencies**

Run: `cd packages/server-sdk && pnpm add -D @hono/node-server @taskcast/server @taskcast/core`

**Step 3: Run test**

Run: `cd packages/server-sdk && pnpm test -- tests/integration/sdk-client.test.ts -v`

**Step 4: Commit**

```bash
git add packages/server-sdk/
git commit -m "test: add server-sdk integration tests against real HTTP server"
```

---

### Task 15: CLI — config.test.ts

**Files:**
- Create: `packages/cli/tests/config.test.ts`
- Create: `packages/cli/vitest.config.ts` (if needed)

**Step 1: Set up vitest for CLI package**

Check if `packages/cli/package.json` has vitest. If not, add it:

Run: `cd packages/cli && pnpm add -D vitest`

Create `packages/cli/vitest.config.ts`:
```ts
import { defineConfig } from 'vitest/config'
export default defineConfig({ test: { include: ['tests/**/*.test.ts'] } })
```

Add `"test": "vitest run"` to `packages/cli/package.json` scripts.

**Step 2: Write config tests**

```ts
import { describe, it, expect } from 'vitest'
import { writeFileSync, mkdirSync, rmSync } from 'fs'
import { join } from 'path'
import { tmpdir } from 'os'
import { loadConfigFile } from '@taskcast/core'

describe('CLI — config loading', () => {
  const tmpDir = join(tmpdir(), `taskcast-test-${Date.now()}`)

  it('loads valid YAML config', async () => {
    mkdirSync(tmpDir, { recursive: true })
    const configPath = join(tmpDir, 'config.yaml')
    writeFileSync(configPath, `
port: 4000
auth:
  mode: jwt
adapters:
  broadcast:
    provider: redis
    url: redis://localhost:6379
`)
    const { config } = await loadConfigFile(configPath)
    expect(config.port).toBe(4000)
    expect(config.auth?.mode).toBe('jwt')
    expect(config.adapters?.broadcast?.provider).toBe('redis')
    rmSync(tmpDir, { recursive: true })
  })

  it('returns empty config for nonexistent file', async () => {
    const { config, source } = await loadConfigFile('/tmp/nonexistent-taskcast-config.yaml')
    expect(source).toBe('none')
    expect(config).toBeTruthy()
  })

  it('throws on invalid YAML', async () => {
    mkdirSync(tmpDir, { recursive: true })
    const configPath = join(tmpDir, 'bad.yaml')
    writeFileSync(configPath, '{{{{invalid yaml')
    await expect(loadConfigFile(configPath)).rejects.toThrow()
    rmSync(tmpDir, { recursive: true })
  })
})
```

**Step 3: Run test**

Run: `cd packages/cli && pnpm test -- tests/config.test.ts -v`

**Step 4: Commit**

```bash
git add packages/cli/
git commit -m "test: add CLI config loading tests"
```

---

### Task 16: CLI — startup.test.ts

**Files:**
- Create: `packages/cli/tests/startup.test.ts`

**Step 1: Write the test**

Test the server startup logic by directly importing and calling the adapter initialization code. Since the CLI uses `commander` with `.action()`, extract the testable parts or test through the public API.

```ts
import { describe, it, expect, afterEach } from 'vitest'
import {
  TaskEngine,
  MemoryBroadcastProvider,
  MemoryShortTermStore,
} from '@taskcast/core'
import { createTaskcastApp } from '@taskcast/server'

describe('CLI — startup scenarios', () => {
  it('memory mode: /health responds ok', async () => {
    const store = new MemoryShortTermStore()
    const broadcast = new MemoryBroadcastProvider()
    const engine = new TaskEngine({ shortTermStore: store, broadcast })
    const { app, stop } = createTaskcastApp({ engine, auth: { mode: 'none' } })

    const res = await app.request('/health')
    expect(res.status).toBe(200)
    const body = await res.json()
    expect(body).toEqual({ ok: true })

    stop()
  })

  it('auth jwt mode rejects unauthenticated requests', async () => {
    const store = new MemoryShortTermStore()
    const broadcast = new MemoryBroadcastProvider()
    const engine = new TaskEngine({ shortTermStore: store, broadcast })
    const { app, stop } = createTaskcastApp({
      engine,
      auth: { mode: 'jwt', secret: 'test-secret-key-for-hmac-256' },
    })

    const res = await app.request('/tasks', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ type: 'test' }),
    })
    expect(res.status).toBe(401)

    stop()
  })
})
```

**Step 2: Run test**

Run: `cd packages/cli && pnpm test -- tests/startup.test.ts -v`

**Step 3: Commit**

```bash
git add packages/cli/tests/startup.test.ts
git commit -m "test: add CLI startup integration tests"
```

---

### Task 17: Core integration — cleanup.test.ts

**Files:**
- Create: `packages/core/tests/integration/cleanup.test.ts`

**Step 1: Read the cleanup module to understand the API**

Read: `packages/core/src/cleanup.ts`

**Step 2: Write the test**

Test cleanup rule matching and event filtering with real engine + memory adapters. Create tasks with cleanup rules, publish events, then run cleanup and verify the correct events/tasks are removed.

**Step 3: Run test**

Run: `cd packages/core && pnpm test -- tests/integration/cleanup.test.ts -v`

**Step 4: Commit**

```bash
git add packages/core/tests/integration/cleanup.test.ts
git commit -m "test: add core cleanup rule integration tests"
```

---

### Task 18: Server integration — redis-adapters.test.ts (testcontainer)

**Files:**
- Create: `packages/server/tests/integration/redis-adapters.test.ts`

**Step 1: Write the test**

Pattern: use `GenericContainer` from testcontainers to start Redis. Create two separate TaskEngine+Server instances sharing the same Redis. Verify that events published through one instance's HTTP API are received by SSE clients connected to the other.

Skip with `describe.skipIf(!process.env.CI && !process.env.DOCKER_HOST)` for local development without Docker.

**Step 2: Run test (requires Docker)**

Run: `cd packages/server && pnpm test -- tests/integration/redis-adapters.test.ts -v`

**Step 3: Commit**

```bash
git add packages/server/tests/integration/redis-adapters.test.ts
git commit -m "test: add Redis adapter integration tests with testcontainer"
```

---

### Task 19: Final verification

**Step 1: Run full test suite**

Run: `pnpm test 2>&1 | tail -10`
Expected: 0 failures (Docker-dependent tests may skip)

**Step 2: Run type check**

Run: `pnpm lint`
Expected: No errors

**Step 3: Commit any fixups**

If any adjustments were needed, commit them.

**Step 4: Create changeset (if merging to main)**

Run: `pnpm changeset` and select patch for test-only changes.
