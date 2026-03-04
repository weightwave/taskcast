# Suspended States & Worker Protocol Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Add paused/blocked states with TTL management, blocked request/resolve flow, WebSocket worker bidirectional communication, and hot/cold task storage mechanism.

**Architecture:** Extend the core state machine with two suspended states (paused, blocked). Add a background scheduler for wake-up timers and cold/hot task demotion. Add WebSocket endpoint for worker bidirectional communication. Add REST endpoints for blocked resolution and external signals. All changes in both TypeScript and Rust.

**Tech Stack:** TypeScript (Hono, Vitest), Rust (Axum, Tokio, sqlx), WebSocket, SSE

**Design doc:** `docs/plans/2026-03-03-suspended-states-worker-protocol-design.md`

---

## Phase 1: Core State Machine (TypeScript)

### Task 1: Add suspended statuses and new fields to types

**Files:**
- Modify: `packages/core/src/types.ts`

**Step 1: Update TaskStatus union**

Add `'paused' | 'blocked'` to the `TaskStatus` type:

```typescript
export type TaskStatus =
  | 'pending'
  | 'running'
  | 'paused'
  | 'blocked'
  | 'completed'
  | 'failed'
  | 'timeout'
  | 'cancelled'
```

**Step 2: Add BlockedRequest interface and new Task fields**

After the `TaskAuthConfig` interface:

```typescript
export interface BlockedRequest {
  type: string
  data: unknown
}
```

Add to the `Task` interface:

```typescript
  reason?: string
  resumeAt?: number
  blockedRequest?: BlockedRequest
  workerId?: string
  workerOnly?: boolean
```

**Step 3: Add new permission scopes**

```typescript
export type PermissionScope =
  | 'task:create'
  | 'task:manage'
  | 'task:resolve'
  | 'task:signal'
  | 'event:publish'
  | 'event:subscribe'
  | 'event:history'
  | 'webhook:create'
  | 'worker:connect'
  | '*'
```

**Step 4: Run type check**

Run: `cd packages/core && pnpm lint`
Expected: Compilation errors in state-machine.ts (ALLOWED_TRANSITIONS doesn't cover new statuses). This is expected — we fix it in Task 2.

**Step 5: Commit**

```bash
git add packages/core/src/types.ts
git commit -m "feat(core): add paused/blocked statuses, BlockedRequest, worker fields, new permission scopes"
```

---

### Task 2: Update state machine transitions

**Files:**
- Modify: `packages/core/src/state-machine.ts`
- Test: `packages/core/tests/unit/state-machine.test.ts`

**Step 1: Write failing tests for new transitions and helpers**

Add to `state-machine.test.ts`:

```typescript
import { isSuspended, SUSPENDED_STATUSES } from '../../src/state-machine.js'

describe('SUSPENDED_STATUSES', () => {
  it('contains paused and blocked', () => {
    expect(SUSPENDED_STATUSES).toEqual(['paused', 'blocked'])
  })
})

describe('isSuspended', () => {
  it('returns true for paused', () => expect(isSuspended('paused')).toBe(true))
  it('returns true for blocked', () => expect(isSuspended('blocked')).toBe(true))
  it('returns false for running', () => expect(isSuspended('running')).toBe(false))
  it('returns false for terminal', () => expect(isSuspended('completed')).toBe(false))
})

describe('canTransition – suspended states', () => {
  // running → suspended
  it('allows running → paused', () => expect(canTransition('running', 'paused')).toBe(true))
  it('allows running → blocked', () => expect(canTransition('running', 'blocked')).toBe(true))

  // paused exits
  it('allows paused → running', () => expect(canTransition('paused', 'running')).toBe(true))
  it('allows paused → blocked', () => expect(canTransition('paused', 'blocked')).toBe(true))
  it('allows paused → cancelled', () => expect(canTransition('paused', 'cancelled')).toBe(true))
  it('rejects paused → completed', () => expect(canTransition('paused', 'completed')).toBe(false))
  it('rejects paused → failed', () => expect(canTransition('paused', 'failed')).toBe(false))

  // blocked exits
  it('allows blocked → running', () => expect(canTransition('blocked', 'running')).toBe(true))
  it('allows blocked → paused', () => expect(canTransition('blocked', 'paused')).toBe(true))
  it('allows blocked → cancelled', () => expect(canTransition('blocked', 'cancelled')).toBe(true))
  it('allows blocked → failed', () => expect(canTransition('blocked', 'failed')).toBe(true))
  it('rejects blocked → completed', () => expect(canTransition('blocked', 'completed')).toBe(false))

  // pending → suspended: not allowed
  it('rejects pending → paused', () => expect(canTransition('pending', 'paused')).toBe(false))
  it('rejects pending → blocked', () => expect(canTransition('pending', 'blocked')).toBe(false))

  // suspended are not terminal
  it('paused is not terminal', () => expect(isTerminal('paused')).toBe(false))
  it('blocked is not terminal', () => expect(isTerminal('blocked')).toBe(false))
})
```

**Step 2: Run tests to verify they fail**

Run: `cd packages/core && pnpm test -- --run state-machine`
Expected: FAIL — `isSuspended` and `SUSPENDED_STATUSES` not exported, transitions not defined.

**Step 3: Implement state machine changes**

In `state-machine.ts`:

```typescript
import type { TaskStatus } from './types.js'

export const TERMINAL_STATUSES: readonly TaskStatus[] = [
  'completed',
  'failed',
  'timeout',
  'cancelled',
] as const

export const SUSPENDED_STATUSES: readonly TaskStatus[] = [
  'paused',
  'blocked',
] as const

const ALLOWED_TRANSITIONS: Record<TaskStatus, TaskStatus[]> = {
  pending: ['running', 'cancelled'],
  running: ['paused', 'blocked', 'completed', 'failed', 'timeout', 'cancelled'],
  paused: ['running', 'blocked', 'cancelled'],
  blocked: ['running', 'paused', 'cancelled', 'failed'],
  completed: [],
  failed: [],
  timeout: [],
  cancelled: [],
}

export function canTransition(from: TaskStatus, to: TaskStatus): boolean {
  if (from === to) return false
  return ALLOWED_TRANSITIONS[from]?.includes(to) ?? false
}

export function applyTransition(from: TaskStatus, to: TaskStatus): TaskStatus {
  if (!canTransition(from, to)) {
    throw new Error(`Invalid transition: ${from} → ${to}`)
  }
  return to
}

export function isTerminal(status: TaskStatus): boolean {
  return TERMINAL_STATUSES.includes(status)
}

export function isSuspended(status: TaskStatus): boolean {
  return SUSPENDED_STATUSES.includes(status)
}
```

**Step 4: Run tests to verify they pass**

Run: `cd packages/core && pnpm test -- --run state-machine`
Expected: All PASS.

**Step 5: Commit**

```bash
git add packages/core/src/state-machine.ts packages/core/tests/unit/state-machine.test.ts
git commit -m "feat(core): add paused/blocked to state machine with isSuspended helper"
```

---

## Phase 2: Engine TTL & Reason Logic (TypeScript)

### Task 3: Extend TransitionPayload and engine transition logic

**Files:**
- Modify: `packages/core/src/engine.ts`
- Test: `packages/core/tests/unit/engine.test.ts`

**Step 1: Write failing tests for reason, ttl override, and resumeAt**

Add to `engine.test.ts`:

```typescript
describe('transitionTask – suspended states', () => {
  it('sets reason when transitioning to paused', async () => {
    const engine = createTestEngine()
    const task = await engine.createTask({})
    await engine.transitionTask(task.id, 'running')
    const paused = await engine.transitionTask(task.id, 'paused', { reason: 'User requested' })
    expect(paused.reason).toBe('User requested')
  })

  it('clears reason when leaving suspended state', async () => {
    const engine = createTestEngine()
    const task = await engine.createTask({})
    await engine.transitionTask(task.id, 'running')
    await engine.transitionTask(task.id, 'paused', { reason: 'User requested' })
    const resumed = await engine.transitionTask(task.id, 'running')
    expect(resumed.reason).toBeUndefined()
  })

  it('sets resumeAt when transitioning to blocked with resumeAfterMs', async () => {
    const engine = createTestEngine()
    const task = await engine.createTask({})
    await engine.transitionTask(task.id, 'running')
    const before = Date.now()
    const blocked = await engine.transitionTask(task.id, 'blocked', { resumeAfterMs: 30000 })
    expect(blocked.resumeAt).toBeGreaterThanOrEqual(before + 30000)
    expect(blocked.resumeAt).toBeLessThanOrEqual(Date.now() + 30000)
  })

  it('clears resumeAt when leaving blocked', async () => {
    const engine = createTestEngine()
    const task = await engine.createTask({})
    await engine.transitionTask(task.id, 'running')
    await engine.transitionTask(task.id, 'blocked', { resumeAfterMs: 30000 })
    const resumed = await engine.transitionTask(task.id, 'running')
    expect(resumed.resumeAt).toBeUndefined()
  })

  it('allows ttl override on any transition', async () => {
    const engine = createTestEngine()
    const task = await engine.createTask({ ttl: 3600 })
    await engine.transitionTask(task.id, 'running')
    const blocked = await engine.transitionTask(task.id, 'blocked', { ttl: 1800 })
    expect(blocked.ttl).toBe(1800)
  })

  it('allows publishing events in paused state', async () => {
    const engine = createTestEngine()
    const task = await engine.createTask({})
    await engine.transitionTask(task.id, 'running')
    await engine.transitionTask(task.id, 'paused')
    const event = await engine.publishEvent(task.id, { type: 'test', level: 'info', data: {} })
    expect(event.taskId).toBe(task.id)
  })

  it('allows publishing events in blocked state', async () => {
    const engine = createTestEngine()
    const task = await engine.createTask({})
    await engine.transitionTask(task.id, 'running')
    await engine.transitionTask(task.id, 'blocked')
    const event = await engine.publishEvent(task.id, { type: 'test', level: 'info', data: {} })
    expect(event.taskId).toBe(task.id)
  })
})
```

**Step 2: Run tests to verify they fail**

Run: `cd packages/core && pnpm test -- --run engine`
Expected: FAIL — reason/resumeAfterMs/ttl not accepted in payload.

**Step 3: Implement engine changes**

Update `TransitionPayload` type in `engine.ts` (the `transitionTask` method signature):

```typescript
async transitionTask(
  taskId: string,
  to: TaskStatus,
  payload?: {
    result?: Task['result']
    error?: Task['error']
    reason?: string
    ttl?: number
    resumeAfterMs?: number
  },
): Promise<Task> {
  const task = await this.getTask(taskId)
  if (!task) throw new Error(`Task not found: ${taskId}`)
  if (!canTransition(task.status, to)) {
    throw new Error(`Invalid transition: ${task.status} → ${to}`)
  }

  const now = Date.now()
  const newResult = payload?.result ?? task.result
  const newError = payload?.error ?? task.error
  const newCompletedAt = isTerminal(to) ? now : task.completedAt

  // TTL override
  const newTtl = payload?.ttl ?? task.ttl

  // Reason: set when entering suspended, clear when leaving
  const newReason = isSuspended(to) ? (payload?.reason ?? task.reason) : undefined

  // ResumeAt: set when entering blocked with resumeAfterMs, clear otherwise
  const newResumeAt = (to === 'blocked' && payload?.resumeAfterMs)
    ? now + payload.resumeAfterMs
    : (to === 'blocked' ? task.resumeAt : undefined)

  const updated: Task = {
    ...task,
    status: to,
    updatedAt: now,
    ttl: newTtl,
    ...(newCompletedAt !== undefined && { completedAt: newCompletedAt }),
    ...(newResult !== undefined && { result: newResult }),
    ...(newError !== undefined && { error: newError }),
    ...(newReason !== undefined ? { reason: newReason } : {}),
    ...(newResumeAt !== undefined ? { resumeAt: newResumeAt } : {}),
  }

  // Remove cleared optional fields
  if (!isSuspended(to)) {
    delete updated.reason
    delete updated.resumeAt
  }

  await this.opts.shortTerm.saveTask(updated)
  if (this.opts.longTerm) await this.opts.longTerm.saveTask(updated)

  // TTL management
  if (isSuspended(to) && to === 'paused') {
    // paused = stop clock
    await this.opts.shortTerm.clearTTL?.(taskId)
  } else if (to === 'running' && isSuspended(task.status) && task.status === 'paused' && updated.ttl) {
    // resuming from paused = reset full TTL
    await this.opts.shortTerm.setTTL(taskId, updated.ttl)
  } else if (to === 'blocked' && isSuspended(task.status) && task.status === 'paused' && updated.ttl) {
    // paused → blocked = reset full TTL (start ticking)
    await this.opts.shortTerm.setTTL(taskId, updated.ttl)
  } else if (to === 'paused' && task.status === 'blocked') {
    // blocked → paused = stop clock
    await this.opts.shortTerm.clearTTL?.(taskId)
  }

  await this._emit(taskId, {
    type: 'taskcast:status',
    level: 'info',
    data: { status: to, result: updated.result, error: updated.error, reason: updated.reason },
  })

  if (to === 'failed' && updated.error) {
    this.opts.hooks?.onTaskFailed?.(updated, updated.error)
  }
  if (to === 'timeout') {
    this.opts.hooks?.onTaskTimeout?.(updated)
  }

  return updated
}
```

Add `import { isSuspended } from './state-machine.js'` to the imports.

**Step 4: Run tests to verify they pass**

Run: `cd packages/core && pnpm test -- --run engine`
Expected: All PASS.

**Step 5: Commit**

```bash
git add packages/core/src/engine.ts packages/core/tests/unit/engine.test.ts
git commit -m "feat(core): engine TTL logic, reason/resumeAt for suspended states"
```

---

### Task 4: Add clearTTL and listByStatus to memory adapters

**Files:**
- Modify: `packages/core/src/types.ts` (ShortTermStore interface)
- Modify: `packages/core/src/memory-adapters.ts`
- Test: `packages/core/tests/unit/memory-adapters.test.ts`

**Step 1: Write failing tests**

```typescript
describe('MemoryShortTermStore – clearTTL', () => {
  it('clearTTL is callable without error', async () => {
    const store = new MemoryShortTermStore()
    await expect(store.clearTTL!('task-1')).resolves.toBeUndefined()
  })
})

describe('MemoryShortTermStore – listByStatus', () => {
  it('returns tasks matching given statuses', async () => {
    const store = new MemoryShortTermStore()
    await store.saveTask({ id: 't1', status: 'paused', createdAt: 1, updatedAt: 1 } as Task)
    await store.saveTask({ id: 't2', status: 'running', createdAt: 1, updatedAt: 1 } as Task)
    await store.saveTask({ id: 't3', status: 'blocked', createdAt: 1, updatedAt: 1 } as Task)
    const result = await store.listByStatus!(['paused', 'blocked'])
    expect(result.map(t => t.id).sort()).toEqual(['t1', 't3'])
  })

  it('returns empty array when no matches', async () => {
    const store = new MemoryShortTermStore()
    await store.saveTask({ id: 't1', status: 'running', createdAt: 1, updatedAt: 1 } as Task)
    const result = await store.listByStatus!(['paused'])
    expect(result).toEqual([])
  })
})
```

**Step 2: Run tests to verify they fail**

Run: `cd packages/core && pnpm test -- --run memory-adapters`
Expected: FAIL.

**Step 3: Add optional methods to ShortTermStore interface**

In `types.ts`, add to `ShortTermStore`:

```typescript
  clearTTL?(taskId: string): Promise<void>
  listByStatus?(statuses: TaskStatus[]): Promise<Task[]>
```

**Step 4: Implement in MemoryShortTermStore**

In `memory-adapters.ts`:

```typescript
  async clearTTL(_taskId: string): Promise<void> {
    // no-op in memory adapter (setTTL is also no-op)
  }

  async listByStatus(statuses: TaskStatus[]): Promise<Task[]> {
    const result: Task[] = []
    for (const task of this.tasks.values()) {
      if (statuses.includes(task.status)) result.push({ ...task })
    }
    return result
  }
```

**Step 5: Run tests to verify they pass**

Run: `cd packages/core && pnpm test -- --run memory-adapters`
Expected: All PASS.

**Step 6: Commit**

```bash
git add packages/core/src/types.ts packages/core/src/memory-adapters.ts packages/core/tests/unit/memory-adapters.test.ts
git commit -m "feat(core): add clearTTL and listByStatus to ShortTermStore interface"
```

---

## Phase 3: Server REST Endpoints (TypeScript)

### Task 5: Update transition endpoint for paused/blocked

**Files:**
- Modify: `packages/server/src/routes/tasks.ts`
- Test: `packages/server/tests/tasks.test.ts`

**Step 1: Write failing tests**

Add to `tasks.test.ts`:

```typescript
describe('PATCH /tasks/:taskId/status – suspended states', () => {
  it('transitions running → paused with reason', async () => {
    const res = await app.request(`/tasks/${taskId}/status`, {
      method: 'PATCH',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ status: 'paused', reason: 'User requested' }),
    })
    expect(res.status).toBe(200)
    const task = await res.json()
    expect(task.status).toBe('paused')
    expect(task.reason).toBe('User requested')
  })

  it('transitions running → blocked with resumeAfterMs', async () => {
    const res = await app.request(`/tasks/${taskId}/status`, {
      method: 'PATCH',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ status: 'blocked', reason: 'Waiting approval', resumeAfterMs: 30000 }),
    })
    expect(res.status).toBe(200)
    const task = await res.json()
    expect(task.status).toBe('blocked')
    expect(task.resumeAt).toBeDefined()
  })

  it('allows ttl override on transition', async () => {
    const res = await app.request(`/tasks/${taskId}/status`, {
      method: 'PATCH',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ status: 'blocked', ttl: 1800 }),
    })
    expect(res.status).toBe(200)
    const task = await res.json()
    expect(task.ttl).toBe(1800)
  })
})
```

**Step 2: Run tests to verify they fail**

Run: `cd packages/server && pnpm test -- --run tasks`
Expected: FAIL — `paused` not in Zod enum.

**Step 3: Update Zod schema and handler**

In `tasks.ts`, update the PATCH handler's schema:

```typescript
const schema = z.object({
  status: z.enum(['running', 'paused', 'blocked', 'completed', 'failed', 'timeout', 'cancelled']),
  result: z.record(z.unknown()).optional(),
  error: z.object({
    code: z.string().optional(),
    message: z.string(),
    details: z.record(z.unknown()).optional(),
  }).optional(),
  reason: z.string().optional(),
  ttl: z.number().int().positive().optional(),
  resumeAfterMs: z.number().int().positive().optional(),
})
```

Update the payload construction:

```typescript
const payload: {
  result?: Record<string, unknown>
  error?: TaskError
  reason?: string
  ttl?: number
  resumeAfterMs?: number
} = {}
if (parsed.data.result !== undefined) payload.result = parsed.data.result
if (parsed.data.error !== undefined) { /* existing error mapping */ }
if (parsed.data.reason !== undefined) payload.reason = parsed.data.reason
if (parsed.data.ttl !== undefined) payload.ttl = parsed.data.ttl
if (parsed.data.resumeAfterMs !== undefined) payload.resumeAfterMs = parsed.data.resumeAfterMs
```

**Step 4: Run tests to verify they pass**

Run: `cd packages/server && pnpm test -- --run tasks`
Expected: All PASS.

**Step 5: Commit**

```bash
git add packages/server/src/routes/tasks.ts packages/server/tests/tasks.test.ts
git commit -m "feat(server): support paused/blocked in transition endpoint with reason/ttl/resumeAfterMs"
```

---

### Task 6: Add resolve, request, and signal endpoints

**Files:**
- Modify: `packages/server/src/routes/tasks.ts`
- Test: `packages/server/tests/tasks.test.ts`

**Step 1: Write failing tests**

```typescript
describe('POST /tasks/:taskId/resolve', () => {
  it('resolves a blocked task', async () => {
    // Setup: create task, transition to running, then blocked
    // ...
    const res = await app.request(`/tasks/${taskId}/resolve`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ data: { answer: 'approved' } }),
    })
    expect(res.status).toBe(200)
    const task = await res.json()
    expect(task.status).toBe('running')
  })

  it('rejects resolve on non-blocked task', async () => {
    const res = await app.request(`/tasks/${taskId}/resolve`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ data: {} }),
    })
    expect(res.status).toBe(400)
  })

  it('requires task:resolve permission', async () => {
    // Test with auth that lacks task:resolve scope
  })
})

describe('GET /tasks/:taskId/request', () => {
  it('returns blocked request when task is blocked', async () => {
    // Setup: create task, transition to blocked with blockedRequest
    const res = await app.request(`/tasks/${taskId}/request`)
    expect(res.status).toBe(200)
    const body = await res.json()
    expect(body.type).toBeDefined()
  })

  it('returns 404 when task is not blocked', async () => {
    const res = await app.request(`/tasks/${taskId}/request`)
    expect(res.status).toBe(404)
  })
})

describe('POST /tasks/:taskId/signal', () => {
  it('accepts signal and returns 202', async () => {
    const res = await app.request(`/tasks/${taskId}/signal`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ type: 'user_input', data: { message: 'hello' } }),
    })
    expect(res.status).toBe(202)
  })

  it('requires task:signal permission', async () => {
    // Test with auth that lacks task:signal scope
  })
})
```

**Step 2: Run tests to verify they fail**

Run: `cd packages/server && pnpm test -- --run tasks`
Expected: FAIL — endpoints don't exist.

**Step 3: Implement endpoints**

In `tasks.ts`, add three new routes inside `createTasksRouter`:

```typescript
// POST /tasks/:taskId/resolve
router.post('/:taskId/resolve', async (c) => {
  const { taskId } = c.req.param()
  const auth = c.get('auth')
  if (!checkScope(auth, 'task:resolve', taskId)) return c.json({ error: 'Forbidden' }, 403)

  const task = await engine.getTask(taskId)
  if (!task) return c.json({ error: 'Task not found' }, 404)
  if (task.status !== 'blocked') return c.json({ error: 'Task is not blocked' }, 400)

  const body = await c.req.json()
  const schema = z.object({ data: z.unknown() })
  const parsed = schema.safeParse(body)
  if (!parsed.success) return c.json({ error: parsed.error.flatten() }, 400)

  try {
    const updated = await engine.transitionTask(taskId, 'running')

    // Emit resolved event
    await engine.publishEvent(taskId, {
      type: 'taskcast:resolved',
      level: 'info',
      data: { resolution: parsed.data.data },
    })

    return c.json(updated)
  } catch (err) {
    const msg = err instanceof Error ? err.message : String(err)
    return c.json({ error: msg }, 400)
  }
})

// GET /tasks/:taskId/request
router.get('/:taskId/request', async (c) => {
  const { taskId } = c.req.param()
  const auth = c.get('auth')
  if (!checkScope(auth, 'task:resolve', taskId)) return c.json({ error: 'Forbidden' }, 403)

  const task = await engine.getTask(taskId)
  if (!task) return c.json({ error: 'Task not found' }, 404)
  if (task.status !== 'blocked' || !task.blockedRequest) {
    return c.json({ error: 'No active blocked request' }, 404)
  }

  return c.json(task.blockedRequest)
})

// POST /tasks/:taskId/signal
router.post('/:taskId/signal', async (c) => {
  const { taskId } = c.req.param()
  const auth = c.get('auth')
  if (!checkScope(auth, 'task:signal', taskId)) return c.json({ error: 'Forbidden' }, 403)

  const task = await engine.getTask(taskId)
  if (!task) return c.json({ error: 'Task not found' }, 404)

  const body = await c.req.json()
  const schema = z.object({ type: z.string(), data: z.unknown() })
  const parsed = schema.safeParse(body)
  if (!parsed.success) return c.json({ error: parsed.error.flatten() }, 400)

  // Emit as a task event so SSE subscribers and workers get it
  await engine.publishEvent(taskId, {
    type: `taskcast:signal`,
    level: 'info',
    data: { signalType: parsed.data.type, data: parsed.data.data },
  })

  return c.json({ ok: true }, 202)
})
```

**Step 4: Run tests to verify they pass**

Run: `cd packages/server && pnpm test -- --run tasks`
Expected: All PASS.

**Step 5: Commit**

```bash
git add packages/server/src/routes/tasks.ts packages/server/tests/tasks.test.ts
git commit -m "feat(server): add resolve, request, signal REST endpoints"
```

---

### Task 7: Verify SSE behavior with suspended states

**Files:**
- Test: `packages/server/tests/sse.test.ts`

The `TERMINAL` set in `sse.ts` (line 53) already excludes paused/blocked — no code change needed. But we need tests to confirm.

**Step 1: Write tests**

```typescript
describe('SSE – suspended states', () => {
  it('keeps SSE stream open when task transitions to paused', async () => {
    // Create task, start SSE subscription, transition to paused
    // Verify stream stays open and receives the status event
  })

  it('keeps SSE stream open when task transitions to blocked', async () => {
    // Same as above for blocked
  })

  it('closes SSE stream when paused task is cancelled', async () => {
    // Create task → running → paused → cancelled
    // Verify stream receives taskcast.done
  })
})
```

**Step 2: Run tests**

Run: `cd packages/server && pnpm test -- --run sse`
Expected: All PASS (no code changes needed).

**Step 3: Commit tests**

```bash
git add packages/server/tests/sse.test.ts
git commit -m "test(server): verify SSE stays open for suspended states"
```

---

## Phase 4: Blocked Request Flow (TypeScript)

### Task 8: Add blockedRequest support in engine

**Files:**
- Modify: `packages/core/src/engine.ts`
- Test: `packages/core/tests/unit/engine.test.ts`

**Step 1: Write failing tests**

```typescript
describe('transitionTask – blockedRequest', () => {
  it('stores blockedRequest when transitioning to blocked', async () => {
    const engine = createTestEngine()
    const task = await engine.createTask({})
    await engine.transitionTask(task.id, 'running')
    const blocked = await engine.transitionTask(task.id, 'blocked', {
      reason: 'Need approval',
      blockedRequest: { type: 'approval', data: { question: 'Deploy to prod?' } },
    })
    expect(blocked.blockedRequest).toEqual({ type: 'approval', data: { question: 'Deploy to prod?' } })
  })

  it('clears blockedRequest when leaving blocked', async () => {
    const engine = createTestEngine()
    const task = await engine.createTask({})
    await engine.transitionTask(task.id, 'running')
    await engine.transitionTask(task.id, 'blocked', {
      blockedRequest: { type: 'approval', data: {} },
    })
    const running = await engine.transitionTask(task.id, 'running')
    expect(running.blockedRequest).toBeUndefined()
  })
})
```

**Step 2: Run tests to verify they fail**

Run: `cd packages/core && pnpm test -- --run engine`
Expected: FAIL — `blockedRequest` not in payload type.

**Step 3: Implement blockedRequest handling**

Add `blockedRequest` to the transition payload type:

```typescript
payload?: {
  result?: Task['result']
  error?: Task['error']
  reason?: string
  ttl?: number
  resumeAfterMs?: number
  blockedRequest?: BlockedRequest
}
```

In the transition logic, add:

```typescript
// BlockedRequest: set when entering blocked, clear when leaving
const newBlockedRequest = to === 'blocked'
  ? (payload?.blockedRequest ?? task.blockedRequest)
  : undefined
```

And include in `updated` task:

```typescript
...(newBlockedRequest !== undefined ? { blockedRequest: newBlockedRequest } : {}),
```

And in the cleanup:

```typescript
if (to !== 'blocked') {
  delete updated.blockedRequest
}
```

**Step 4: Run tests to verify they pass**

Run: `cd packages/core && pnpm test -- --run engine`
Expected: All PASS.

**Step 5: Commit**

```bash
git add packages/core/src/engine.ts packages/core/tests/unit/engine.test.ts
git commit -m "feat(core): store/clear blockedRequest on blocked transitions"
```

---

### Task 9: Add CreateTaskInput worker fields

**Files:**
- Modify: `packages/core/src/engine.ts` (CreateTaskInput)
- Modify: `packages/server/src/routes/tasks.ts` (CreateTaskSchema)
- Test: `packages/core/tests/unit/engine.test.ts`

**Step 1: Write failing tests**

```typescript
it('creates task with workerId and workerOnly', async () => {
  const engine = createTestEngine()
  const task = await engine.createTask({ workerId: 'w1', workerOnly: true })
  expect(task.workerId).toBe('w1')
  expect(task.workerOnly).toBe(true)
})
```

**Step 2: Implement**

Add to `CreateTaskInput` in `engine.ts`:

```typescript
workerId?: string
workerOnly?: boolean
```

And in `createTask`:

```typescript
...(input.workerId !== undefined && { workerId: input.workerId }),
...(input.workerOnly !== undefined && { workerOnly: input.workerOnly }),
```

Update `CreateTaskSchema` in server's `tasks.ts`:

```typescript
workerId: z.string().optional(),
workerOnly: z.boolean().optional(),
```

And pass through:

```typescript
if (d.workerId !== undefined) input.workerId = d.workerId
if (d.workerOnly !== undefined) input.workerOnly = d.workerOnly
```

**Step 3: Run tests**

Run: `cd packages/core && pnpm test -- --run engine`
Expected: All PASS.

**Step 4: Commit**

```bash
git add packages/core/src/engine.ts packages/server/src/routes/tasks.ts packages/core/tests/unit/engine.test.ts
git commit -m "feat(core,server): support workerId and workerOnly on task creation"
```

---

## Phase 5: Task Scheduler (TypeScript)

### Task 10: Implement wake-up timer scheduler

**Files:**
- Create: `packages/core/src/scheduler.ts`
- Test: `packages/core/tests/unit/scheduler.test.ts`

**Step 1: Write failing tests**

```typescript
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest'
import { TaskScheduler } from '../../src/scheduler.js'
import { TaskEngine } from '../../src/engine.js'
import { MemoryBroadcastProvider, MemoryShortTermStore } from '../../src/memory-adapters.js'

describe('TaskScheduler – wake-up timer', () => {
  it('auto-resumes blocked task when resumeAt has passed', async () => {
    const shortTerm = new MemoryShortTermStore()
    const broadcast = new MemoryBroadcastProvider()
    const engine = new TaskEngine({ shortTerm, broadcast })

    const task = await engine.createTask({})
    await engine.transitionTask(task.id, 'running')
    await engine.transitionTask(task.id, 'blocked', { resumeAfterMs: 1 }) // 1ms, immediate

    await new Promise(r => setTimeout(r, 10)) // wait for resumeAt to pass

    const scheduler = new TaskScheduler({ engine, shortTerm, checkIntervalMs: 100 })
    await scheduler.tick() // manual tick

    const updated = await engine.getTask(task.id)
    expect(updated!.status).toBe('running')
  })

  it('does not resume blocked task before resumeAt', async () => {
    const shortTerm = new MemoryShortTermStore()
    const broadcast = new MemoryBroadcastProvider()
    const engine = new TaskEngine({ shortTerm, broadcast })

    const task = await engine.createTask({})
    await engine.transitionTask(task.id, 'running')
    await engine.transitionTask(task.id, 'blocked', { resumeAfterMs: 60000 })

    const scheduler = new TaskScheduler({ engine, shortTerm, checkIntervalMs: 100 })
    await scheduler.tick()

    const updated = await engine.getTask(task.id)
    expect(updated!.status).toBe('blocked') // still blocked
  })

  it('does not resume blocked task without resumeAt', async () => {
    const shortTerm = new MemoryShortTermStore()
    const broadcast = new MemoryBroadcastProvider()
    const engine = new TaskEngine({ shortTerm, broadcast })

    const task = await engine.createTask({})
    await engine.transitionTask(task.id, 'running')
    await engine.transitionTask(task.id, 'blocked')

    const scheduler = new TaskScheduler({ engine, shortTerm, checkIntervalMs: 100 })
    await scheduler.tick()

    const updated = await engine.getTask(task.id)
    expect(updated!.status).toBe('blocked')
  })
})
```

**Step 2: Run tests to verify they fail**

Run: `cd packages/core && pnpm test -- --run scheduler`
Expected: FAIL — module doesn't exist.

**Step 3: Implement scheduler**

Create `packages/core/src/scheduler.ts`:

```typescript
import type { TaskEngine } from './engine.js'
import type { ShortTermStore, LongTermStore } from './types.js'

export interface TaskSchedulerOptions {
  engine: TaskEngine
  shortTerm: ShortTermStore
  longTerm?: LongTermStore
  checkIntervalMs?: number
  pausedColdAfterMs?: number
  blockedColdAfterMs?: number
}

export class TaskScheduler {
  private engine: TaskEngine
  private shortTerm: ShortTermStore
  private longTerm?: LongTermStore
  private checkIntervalMs: number
  private pausedColdAfterMs: number
  private blockedColdAfterMs: number
  private timer?: ReturnType<typeof setInterval>

  constructor(opts: TaskSchedulerOptions) {
    this.engine = opts.engine
    this.shortTerm = opts.shortTerm
    this.longTerm = opts.longTerm
    this.checkIntervalMs = opts.checkIntervalMs ?? 60_000
    this.pausedColdAfterMs = opts.pausedColdAfterMs ?? 5 * 60_000
    this.blockedColdAfterMs = opts.blockedColdAfterMs ?? 30 * 60_000
  }

  start(): void {
    this.timer = setInterval(() => this.tick().catch(() => {}), this.checkIntervalMs)
  }

  stop(): void {
    if (this.timer) clearInterval(this.timer)
  }

  async tick(): Promise<void> {
    await this._checkWakeUpTimers()
    // TODO: cold/hot demotion in a later task
  }

  private async _checkWakeUpTimers(): Promise<void> {
    if (!this.shortTerm.listByStatus) return

    const blockedTasks = await this.shortTerm.listByStatus(['blocked'])
    const now = Date.now()

    for (const task of blockedTasks) {
      if (task.resumeAt && task.resumeAt <= now) {
        try {
          await this.engine.transitionTask(task.id, 'running')
        } catch {
          // Task may have been transitioned by someone else — ignore
        }
      }
    }
  }
}
```

Export from `packages/core/src/index.ts` (or wherever the package re-exports).

**Step 4: Run tests to verify they pass**

Run: `cd packages/core && pnpm test -- --run scheduler`
Expected: All PASS.

**Step 5: Commit**

```bash
git add packages/core/src/scheduler.ts packages/core/tests/unit/scheduler.test.ts
git commit -m "feat(core): add TaskScheduler with wake-up timer for blocked tasks"
```

---

### Task 11: Add cold/hot demotion to scheduler

**Files:**
- Modify: `packages/core/src/scheduler.ts`
- Test: `packages/core/tests/unit/scheduler.test.ts`

**Step 1: Write failing tests**

```typescript
describe('TaskScheduler – cold/hot demotion', () => {
  it('emits taskcast:cold event for paused task past threshold', async () => {
    const shortTerm = new MemoryShortTermStore()
    const broadcast = new MemoryBroadcastProvider()
    const engine = new TaskEngine({ shortTerm, broadcast })

    const task = await engine.createTask({})
    await engine.transitionTask(task.id, 'running')
    await engine.transitionTask(task.id, 'paused')

    // Manually set updatedAt to past
    const stored = await shortTerm.getTask(task.id)
    stored!.updatedAt = Date.now() - 10 * 60_000 // 10 minutes ago
    await shortTerm.saveTask(stored!)

    const events: string[] = []
    engine.subscribe(task.id, (e) => events.push(e.type))

    const scheduler = new TaskScheduler({
      engine, shortTerm,
      pausedColdAfterMs: 5 * 60_000,
    })
    await scheduler.tick()

    expect(events).toContain('taskcast:cold')
  })
})
```

**Step 2: Implement cold/hot demotion**

Add to `TaskScheduler.tick()`:

```typescript
async tick(): Promise<void> {
  await this._checkWakeUpTimers()
  await this._checkColdDemotion()
}

private async _checkColdDemotion(): Promise<void> {
  if (!this.shortTerm.listByStatus) return

  const suspended = await this.shortTerm.listByStatus(['paused', 'blocked'])
  const now = Date.now()

  for (const task of suspended) {
    const age = now - task.updatedAt
    const threshold = task.status === 'paused'
      ? this.pausedColdAfterMs
      : this.blockedColdAfterMs

    if (age >= threshold) {
      // Emit cold event
      try {
        await this.engine.publishEvent(task.id, {
          type: 'taskcast:cold',
          level: 'info',
          data: {},
        })
      } catch {
        // ignore
      }
      // TODO: actually remove from ShortTermStore (future task)
    }
  }
}
```

**Step 3: Run tests**

Run: `cd packages/core && pnpm test -- --run scheduler`
Expected: All PASS.

**Step 4: Commit**

```bash
git add packages/core/src/scheduler.ts packages/core/tests/unit/scheduler.test.ts
git commit -m "feat(core): scheduler cold/hot demotion for suspended tasks"
```

---

## Phase 6: Server-SDK Updates (TypeScript)

### Task 12: Add resolve, signal, and block methods to server-sdk

**Files:**
- Modify: `packages/server-sdk/src/client.ts`
- Test: `packages/server-sdk/tests/` (create if needed)

**Step 1: Write failing tests**

```typescript
describe('TaskcastServerClient – new methods', () => {
  it('resolveBlocked sends POST /tasks/:id/resolve', async () => {
    // Use mock fetch
    const mockFetch = vi.fn().mockResolvedValue(new Response(JSON.stringify({ id: 't1', status: 'running' })))
    const client = new TaskcastServerClient({ baseUrl: 'http://localhost', fetch: mockFetch })
    await client.resolveBlocked('t1', { answer: 'approved' })
    expect(mockFetch).toHaveBeenCalledWith(
      'http://localhost/tasks/t1/resolve',
      expect.objectContaining({ method: 'POST' }),
    )
  })

  it('signal sends POST /tasks/:id/signal', async () => {
    const mockFetch = vi.fn().mockResolvedValue(new Response(JSON.stringify({ ok: true })))
    const client = new TaskcastServerClient({ baseUrl: 'http://localhost', fetch: mockFetch })
    await client.signal('t1', 'user_input', { message: 'hello' })
    expect(mockFetch).toHaveBeenCalledWith(
      'http://localhost/tasks/t1/signal',
      expect.objectContaining({ method: 'POST' }),
    )
  })

  it('getBlockedRequest sends GET /tasks/:id/request', async () => {
    const mockFetch = vi.fn().mockResolvedValue(new Response(JSON.stringify({ type: 'approval', data: {} })))
    const client = new TaskcastServerClient({ baseUrl: 'http://localhost', fetch: mockFetch })
    const req = await client.getBlockedRequest('t1')
    expect(req.type).toBe('approval')
  })

  it('transitionTask accepts reason and resumeAfterMs', async () => {
    const mockFetch = vi.fn().mockResolvedValue(new Response(JSON.stringify({ id: 't1', status: 'blocked' })))
    const client = new TaskcastServerClient({ baseUrl: 'http://localhost', fetch: mockFetch })
    await client.transitionTask('t1', 'blocked', { reason: 'test', resumeAfterMs: 30000 })
    const body = JSON.parse(mockFetch.mock.calls[0][1].body)
    expect(body.reason).toBe('test')
    expect(body.resumeAfterMs).toBe(30000)
  })
})
```

**Step 2: Implement new methods**

In `client.ts`:

```typescript
async transitionTask(
  taskId: string,
  status: TaskStatus,
  payload?: {
    result?: Record<string, unknown>
    error?: TaskError
    reason?: string
    ttl?: number
    resumeAfterMs?: number
  },
): Promise<Task> {
  return this._request<Task>('PATCH', `/tasks/${taskId}/status`, {
    status,
    ...payload,
  })
}

async resolveBlocked(taskId: string, data: unknown): Promise<Task> {
  return this._request<Task>('POST', `/tasks/${taskId}/resolve`, { data })
}

async getBlockedRequest(taskId: string): Promise<BlockedRequest> {
  return this._request<BlockedRequest>('GET', `/tasks/${taskId}/request`)
}

async signal(taskId: string, signalType: string, data: unknown): Promise<void> {
  await this._request('POST', `/tasks/${taskId}/signal`, { type: signalType, data })
}
```

Add `BlockedRequest` import from `@taskcast/core`.

**Step 3: Run tests**

Run: `cd packages/server-sdk && pnpm test -- --run`
Expected: All PASS.

**Step 4: Commit**

```bash
git add packages/server-sdk/src/client.ts packages/server-sdk/tests/
git commit -m "feat(server-sdk): add resolveBlocked, signal, getBlockedRequest methods"
```

---

## Phase 7: WebSocket Worker Protocol (TypeScript)

### Task 13: Create WebSocket worker route

**Files:**
- Create: `packages/server/src/routes/workers.ts`
- Modify: `packages/server/src/index.ts`
- Test: `packages/server/tests/workers.test.ts`

This is the most complex new component. The implementation involves:

1. WebSocket upgrade endpoint at `/workers/:workerId/ws`
2. Message handling for worker → TaskCast (publish, transition, block)
3. Event forwarding for TaskCast → Worker (status_changed, blocked_resolved, signal, resume_timeout, task_cold)
4. Worker registry to track connected workers by workerId

**Step 1: Write failing tests**

Create `packages/server/tests/workers.test.ts` with tests for:
- WebSocket connection establishment
- Worker publish message → engine.publishEvent called
- Worker transition message → engine.transitionTask called
- Worker block message → engine.transitionTask called with blockedRequest
- Status change event forwarded to worker WebSocket
- Signal forwarded to worker WebSocket
- Permission check for worker:connect scope

**Step 2: Implement worker route**

Create `packages/server/src/routes/workers.ts`:

- Export `createWorkersRouter(engine: TaskEngine)` function
- Implement Hono WebSocket upgrade handler
- Maintain a `Map<string, WebSocket>` of connected workers
- On incoming message: parse JSON, dispatch to engine methods
- Subscribe to task events for the worker's tasks and forward relevant ones

Note: Hono's WebSocket support varies by runtime. Use `hono/ws` adapter or implement with the native `upgradeWebSocket` helper based on the target runtime (Node.js / Bun).

**Step 3: Mount in server index**

In `packages/server/src/index.ts`:

```typescript
import { createWorkersRouter } from './routes/workers.js'

// In createTaskcastApp:
app.route('/workers', createWorkersRouter(opts.engine))
```

**Step 4: Run tests**

Run: `cd packages/server && pnpm test -- --run workers`
Expected: All PASS.

**Step 5: Commit**

```bash
git add packages/server/src/routes/workers.ts packages/server/src/index.ts packages/server/tests/workers.test.ts
git commit -m "feat(server): WebSocket worker bidirectional communication protocol"
```

---

## Phase 8: Integration Tests (TypeScript)

### Task 14: End-to-end suspended state lifecycle tests

**Files:**
- Modify: `packages/core/tests/integration/engine-full.test.ts`

Add tests for:
- Full lifecycle: pending → running → paused → running → blocked → running → completed
- Blocked with resolve: running → blocked (with request) → resolve → running
- Blocked with wake-up: running → blocked (resumeAfterMs: 1) → auto-resume via scheduler
- TTL behavior: paused stops clock, blocked keeps clock, resume resets
- Concurrent: 10 simultaneous pause/resume transitions on the same task

**Step 1: Write tests, Step 2: Run, Step 3: Fix if needed, Step 4: Commit**

```bash
git commit -m "test(core): integration tests for suspended state lifecycle"
```

---

### Task 15: Server integration tests

**Files:**
- Modify: `packages/server/tests/tasks.test.ts`
- Modify: `packages/server/tests/sse.test.ts`

Add integration tests covering:
- REST: create task → transition to blocked → resolve → verify running
- REST: create task → transition to blocked → signal → verify signal event
- SSE: subscribe → task paused → verify stream stays open → resume → verify events arrive
- Auth: verify new permission scopes (task:resolve, task:signal, worker:connect)

**Commit:**

```bash
git commit -m "test(server): integration tests for suspended states and new endpoints"
```

---

## Phase 9: Rust Sync

Per CLAUDE.md, both implementations must stay in sync. The Rust changes mirror the TypeScript implementation:

### Task 16: Rust types and state machine

**Files:**
- Modify: `rust/taskcast-core/src/types.rs` — add Paused/Blocked to TaskStatus, add BlockedRequest, new Task fields, new PermissionScope variants
- Modify: `rust/taskcast-core/src/state_machine.rs` — update ALLOWED_TRANSITIONS, add SUSPENDED_STATUSES, is_suspended()
- Update tests in both files

**Commit:** `feat(rust-core): add paused/blocked statuses and state machine transitions`

### Task 17: Rust engine TTL and reason logic

**Files:**
- Modify: `rust/taskcast-core/src/engine.rs` — extend TransitionPayload, TTL logic, reason/resumeAt/blockedRequest handling
- Modify: `rust/taskcast-core/src/memory_adapters.rs` — add clear_ttl, list_by_status
- Update tests

**Commit:** `feat(rust-core): engine TTL logic and suspended state fields`

### Task 18: Rust scheduler

**Files:**
- Create: `rust/taskcast-core/src/scheduler.rs` — TaskScheduler with wake-up timer and cold/hot demotion
- Update `rust/taskcast-core/src/lib.rs` — export scheduler

**Commit:** `feat(rust-core): add TaskScheduler for wake-up timers and cold/hot demotion`

### Task 19: Rust REST endpoints

**Files:**
- Modify: `rust/taskcast-server/src/routes/tasks.rs` — update TransitionBody, add resolve/request/signal handlers
- Modify: `rust/taskcast-server/src/routes/sse.rs` — verify suspended states flow through non-terminal path
- Modify route registration in app setup

**Commit:** `feat(rust-server): resolve, request, signal endpoints for suspended states`

### Task 20: Rust WebSocket worker protocol

**Files:**
- Create: `rust/taskcast-server/src/routes/workers.rs` — Axum WebSocket handler
- Update route registration

**Commit:** `feat(rust-server): WebSocket worker bidirectional communication`

### Task 21: Rust storage adapter migrations

**Files:**
- Modify: `rust/taskcast-sqlite/` — migration, row_helpers, short_term, long_term
- Modify: `rust/taskcast-postgres/` — migration, store
- Update all related tests

**Commit:** `feat(rust-storage): add suspended state fields to SQLite and Postgres adapters`

### Task 22: Rust CLI scheduler flags

**Files:**
- Modify: `rust/taskcast-cli/src/main.rs` — add scheduler CLI flags, start scheduler in main

**Commit:** `feat(rust-cli): add scheduler configuration flags`

---

## Phase 10: Final Verification

### Task 23: Full test suite and type check

**Step 1: Run all TypeScript tests**

```bash
pnpm test
```

Expected: All pass.

**Step 2: Run TypeScript type check**

```bash
pnpm lint
```

Expected: No errors.

**Step 3: Run all Rust tests**

```bash
cd rust && cargo test --workspace
```

Expected: All pass.

**Step 4: Commit any fixes**

```bash
git commit -m "fix: address test failures from full suite run"
```

---

## Summary

| Phase | Tasks | Focus |
|-------|-------|-------|
| 1 | 1-2 | Core state machine (types, transitions) |
| 2 | 3-4 | Engine TTL/reason + memory adapters |
| 3 | 5-7 | Server REST endpoints + SSE verification |
| 4 | 8-9 | Blocked request flow + worker fields |
| 5 | 10-11 | Task scheduler (wake-up + cold/hot) |
| 6 | 12 | Server-SDK new methods |
| 7 | 13 | WebSocket worker protocol |
| 8 | 14-15 | Integration tests |
| 9 | 16-22 | Rust sync (all components) |
| 10 | 23 | Final verification |

Total: ~23 tasks, each with TDD cycle (test → fail → implement → pass → commit).
