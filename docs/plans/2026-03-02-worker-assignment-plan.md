# Worker Assignment Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Add optional task assignment mechanism with four modes (external, pull, ws-offer, ws-race) to Taskcast.

**Architecture:** Extend `@taskcast/core` with new types, state machine changes, and `WorkerManager` class. Extend `@taskcast/server` with worker HTTP routes and WebSocket support. Extend storage adapters (memory, Redis, Postgres) with worker-related methods.

**Tech Stack:** TypeScript, Hono, WebSocket (via `hono/ws` or `ws` library), Zod, vitest, ioredis, postgres.js

**Design Doc:** `docs/plans/2026-03-02-worker-assignment-design.md`

---

## Phase 1: Core Type Extensions & State Machine

### Task 1.1: Extend types.ts — New types and interfaces

**Files:**
- Modify: `packages/core/src/types.ts`

**Step 1: Add new types after existing PermissionScope**

Add these types to `packages/core/src/types.ts`:

After `PermissionScope` type (line 54), add the new worker scopes:

```typescript
export type PermissionScope =
  | 'task:create'
  | 'task:manage'
  | 'event:publish'
  | 'event:subscribe'
  | 'event:history'
  | 'webhook:create'
  | 'worker:connect'    // NEW
  | 'worker:manage'     // NEW
  | '*'
```

After `CleanupRule` interface (before Task), add worker-related types:

```typescript
// ─── Worker Assignment ──────────────────────────────────────────────────────

export type AssignMode = 'external' | 'pull' | 'ws-offer' | 'ws-race'

export type DisconnectPolicy = 'reassign' | 'mark' | 'fail'

export type WorkerStatus = 'idle' | 'busy' | 'draining' | 'offline'

export interface TagMatcher {
  all?: string[]
  any?: string[]
  none?: string[]
}

export interface WorkerMatchRule {
  taskTypes?: string[]
  tags?: TagMatcher
}

export interface Worker {
  id: string
  status: WorkerStatus
  matchRule: WorkerMatchRule
  capacity: number
  usedSlots: number
  weight: number
  connectionMode: 'pull' | 'websocket'
  connectedAt: number
  lastHeartbeatAt: number
  metadata?: Record<string, unknown>
}

export type WorkerAssignmentStatus = 'offered' | 'assigned' | 'running'

export interface WorkerAssignment {
  taskId: string
  workerId: string
  cost: number
  assignedAt: number
  status: WorkerAssignmentStatus
}

export interface WorkerAuditEvent {
  id: string
  workerId: string
  timestamp: number
  action:
    | 'connected'
    | 'disconnected'
    | 'updated'
    | 'task_assigned'
    | 'task_declined'
    | 'task_reclaimed'
    | 'draining'
    | 'heartbeat_timeout'
    | 'pull_request'
  data?: Record<string, unknown>
}
```

**Step 2: Extend Task interface**

Add new optional fields to the Task interface:

```typescript
export interface Task {
  // ...existing fields (id through cleanup)
  tags?: string[]
  assignMode?: AssignMode
  cost?: number
  assignedWorker?: string
  disconnectPolicy?: DisconnectPolicy
}
```

**Step 3: Extend ShortTermStore interface**

Add worker methods to ShortTermStore:

```typescript
export interface ShortTermStore {
  // ...existing methods

  // Task query
  listTasks(filter: TaskFilter): Promise<Task[]>

  // Worker state
  saveWorker(worker: Worker): Promise<void>
  getWorker(workerId: string): Promise<Worker | null>
  listWorkers(filter?: WorkerFilter): Promise<Worker[]>
  deleteWorker(workerId: string): Promise<void>

  // Atomic claim
  claimTask(taskId: string, workerId: string, cost: number): Promise<boolean>

  // Worker assignments
  addAssignment(assignment: WorkerAssignment): Promise<void>
  removeAssignment(taskId: string): Promise<void>
  getWorkerAssignments(workerId: string): Promise<WorkerAssignment[]>
  getTaskAssignment(taskId: string): Promise<WorkerAssignment | null>
}

export interface TaskFilter {
  status?: TaskStatus[]
  types?: string[]
  tags?: TagMatcher
  assignMode?: AssignMode[]
  excludeTaskIds?: string[]
  limit?: number
}

export interface WorkerFilter {
  status?: WorkerStatus[]
  connectionMode?: ('pull' | 'websocket')[]
}
```

**Step 4: Extend LongTermStore interface**

```typescript
export interface LongTermStore {
  // ...existing methods

  saveWorkerEvent(event: WorkerAuditEvent): Promise<void>
  getWorkerEvents(workerId: string, opts?: EventQueryOptions): Promise<WorkerAuditEvent[]>
}
```

**Step 5: Extend TaskcastHooks interface**

```typescript
export interface TaskcastHooks {
  // ...existing hooks

  onTaskCreated?(task: Task): void
  onTaskTransitioned?(task: Task, from: TaskStatus, to: TaskStatus): void
  onWorkerConnected?(worker: Worker): void
  onWorkerDisconnected?(worker: Worker, reason: string): void
  onTaskAssigned?(task: Task, worker: Worker): void
  onTaskDeclined?(task: Task, worker: Worker, blacklisted: boolean): void
}
```

**Step 6: Commit**

```bash
git add packages/core/src/types.ts
git commit -m "feat(core): add worker assignment type definitions"
```

---

### Task 1.2: Extend state machine — Add `assigned` status

**Files:**
- Modify: `packages/core/src/state-machine.ts`
- Test: `packages/core/tests/unit/state-machine.test.ts`

**Step 1: Write failing tests**

Add these tests to `packages/core/tests/unit/state-machine.test.ts`:

```typescript
describe('assigned status', () => {
  it('allows pending → assigned', () => {
    expect(canTransition('pending', 'assigned')).toBe(true)
  })

  it('allows assigned → running', () => {
    expect(canTransition('assigned', 'running')).toBe(true)
  })

  it('allows assigned → pending (decline)', () => {
    expect(canTransition('assigned', 'pending')).toBe(true)
  })

  it('allows assigned → cancelled', () => {
    expect(canTransition('assigned', 'cancelled')).toBe(true)
  })

  it('rejects assigned → completed', () => {
    expect(canTransition('assigned', 'completed')).toBe(false)
  })

  it('rejects assigned → failed', () => {
    expect(canTransition('assigned', 'failed')).toBe(false)
  })

  it('assigned is not terminal', () => {
    expect(isTerminal('assigned')).toBe(false)
  })

  it('applyTransition works for assigned transitions', () => {
    expect(applyTransition('pending', 'assigned')).toBe('assigned')
    expect(applyTransition('assigned', 'running')).toBe('running')
    expect(applyTransition('assigned', 'pending')).toBe('pending')
  })
})
```

**Step 2: Run tests to verify they fail**

```bash
cd packages/core && pnpm test -- tests/unit/state-machine.test.ts
```

Expected: FAIL — `'assigned'` is not a valid `TaskStatus`.

**Step 3: Update TaskStatus type**

In `packages/core/src/types.ts`, add `'assigned'`:

```typescript
export type TaskStatus =
  | 'pending'
  | 'assigned'    // NEW
  | 'running'
  | 'completed'
  | 'failed'
  | 'timeout'
  | 'cancelled'
```

**Step 4: Update state machine**

In `packages/core/src/state-machine.ts`:

```typescript
const ALLOWED_TRANSITIONS: Record<TaskStatus, TaskStatus[]> = {
  pending: ['assigned', 'running', 'cancelled'],   // added 'assigned'
  assigned: ['running', 'pending', 'cancelled'],    // NEW
  running: ['completed', 'failed', 'timeout', 'cancelled'],
  completed: [],
  failed: [],
  timeout: [],
  cancelled: [],
}
```

**Step 5: Run tests to verify they pass**

```bash
cd packages/core && pnpm test -- tests/unit/state-machine.test.ts
```

Expected: ALL PASS

**Step 6: Commit**

```bash
git add packages/core/src/types.ts packages/core/src/state-machine.ts packages/core/tests/unit/state-machine.test.ts
git commit -m "feat(core): add assigned status to state machine"
```

---

### Task 1.3: Extend TaskEngine — Add hooks and new fields in createTask

**Files:**
- Modify: `packages/core/src/engine.ts`
- Test: `packages/core/tests/unit/engine.test.ts`

**Step 1: Write failing tests**

Add to `packages/core/tests/unit/engine.test.ts`:

```typescript
describe('worker assignment fields', () => {
  it('createTask persists tags, assignMode, cost, disconnectPolicy', async () => {
    const { engine } = makeEngine()
    const task = await engine.createTask({
      type: 'llm.chat',
      tags: ['gpu', 'us-east'],
      assignMode: 'ws-offer',
      cost: 5,
      disconnectPolicy: 'reassign',
    })
    expect(task.tags).toEqual(['gpu', 'us-east'])
    expect(task.assignMode).toBe('ws-offer')
    expect(task.cost).toBe(5)
    expect(task.disconnectPolicy).toBe('reassign')
  })

  it('createTask omits undefined worker fields', async () => {
    const { engine } = makeEngine()
    const task = await engine.createTask({})
    expect(task).not.toHaveProperty('tags')
    expect(task).not.toHaveProperty('assignMode')
    expect(task).not.toHaveProperty('cost')
    expect(task).not.toHaveProperty('disconnectPolicy')
  })
})

describe('lifecycle hooks', () => {
  it('calls onTaskCreated after task creation', async () => {
    const onTaskCreated = vi.fn()
    const store = new MemoryShortTermStore()
    const broadcast = new MemoryBroadcastProvider()
    const engine = new TaskEngine({ shortTerm: store, broadcast, hooks: { onTaskCreated } })
    const task = await engine.createTask({ type: 'test' })
    expect(onTaskCreated).toHaveBeenCalledWith(task)
  })

  it('calls onTaskTransitioned after status change', async () => {
    const onTaskTransitioned = vi.fn()
    const store = new MemoryShortTermStore()
    const broadcast = new MemoryBroadcastProvider()
    const engine = new TaskEngine({ shortTerm: store, broadcast, hooks: { onTaskTransitioned } })
    const task = await engine.createTask({ type: 'test' })
    await engine.transitionTask(task.id, 'running')
    expect(onTaskTransitioned).toHaveBeenCalledWith(
      expect.objectContaining({ status: 'running' }),
      'pending',
      'running',
    )
  })
})
```

**Step 2: Run tests to verify they fail**

```bash
cd packages/core && pnpm test -- tests/unit/engine.test.ts
```

**Step 3: Update CreateTaskInput and createTask**

In `packages/core/src/engine.ts`, update `CreateTaskInput`:

```typescript
export interface CreateTaskInput {
  id?: string
  type?: string
  params?: Record<string, unknown>
  metadata?: Record<string, unknown>
  ttl?: number
  webhooks?: Task['webhooks']
  cleanup?: Task['cleanup']
  authConfig?: Task['authConfig']
  tags?: string[]
  assignMode?: Task['assignMode']
  cost?: number
  disconnectPolicy?: Task['disconnectPolicy']
}
```

In `createTask` method, add the new fields to the task object construction:

```typescript
  ...(input.tags !== undefined && { tags: input.tags }),
  ...(input.assignMode !== undefined && { assignMode: input.assignMode }),
  ...(input.cost !== undefined && { cost: input.cost }),
  ...(input.disconnectPolicy !== undefined && { disconnectPolicy: input.disconnectPolicy }),
```

After `return task` at the end but before the return, add the hook call:

```typescript
    // After: if (task.ttl) await this.opts.shortTerm.setTTL(task.id, task.ttl)
    this.opts.hooks?.onTaskCreated?.(task)
    return task
```

**Step 4: Add onTaskTransitioned hook call in transitionTask**

In `transitionTask`, after the existing hook calls (after the `if (to === 'timeout')` block), add:

```typescript
    this.opts.hooks?.onTaskTransitioned?.(updated, task.status, to)
```

Note: `task.status` is the old status (before transition), `to` is the new status.

**Step 5: Run tests to verify they pass**

```bash
cd packages/core && pnpm test -- tests/unit/engine.test.ts
```

**Step 6: Run full core test suite**

```bash
cd packages/core && pnpm test
```

Expected: ALL PASS (existing tests must still pass)

**Step 7: Commit**

```bash
git add packages/core/src/engine.ts packages/core/tests/unit/engine.test.ts
git commit -m "feat(core): add worker fields to createTask and lifecycle hooks"
```

---

### Task 1.4: Update server-side Zod schemas for new fields

**Files:**
- Modify: `packages/server/src/routes/tasks.ts`
- Test: `packages/server/tests/tasks.test.ts`

**Step 1: Write failing tests**

Add to `packages/server/tests/tasks.test.ts`:

```typescript
describe('worker assignment fields', () => {
  it('POST /tasks accepts tags, assignMode, cost, disconnectPolicy', async () => {
    const { app } = makeApp()
    const res = await app.request('/tasks', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({
        type: 'llm.chat',
        tags: ['gpu'],
        assignMode: 'ws-offer',
        cost: 5,
        disconnectPolicy: 'reassign',
      }),
    })
    expect(res.status).toBe(201)
    const body = await res.json()
    expect(body.tags).toEqual(['gpu'])
    expect(body.assignMode).toBe('ws-offer')
    expect(body.cost).toBe(5)
    expect(body.disconnectPolicy).toBe('reassign')
  })

  it('POST /tasks rejects invalid assignMode', async () => {
    const { app } = makeApp()
    const res = await app.request('/tasks', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ assignMode: 'invalid' }),
    })
    expect(res.status).toBe(400)
  })

  it('PATCH status accepts assigned status', async () => {
    const { app, engine } = makeApp()
    const task = await engine.createTask({ type: 'test' })
    const res = await app.request(`/tasks/${task.id}/status`, {
      method: 'PATCH',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ status: 'assigned' }),
    })
    expect(res.status).toBe(200)
    const body = await res.json()
    expect(body.status).toBe('assigned')
  })
})
```

**Step 2: Run tests to verify they fail**

```bash
cd packages/server && pnpm test -- tests/tasks.test.ts
```

**Step 3: Update Zod schemas**

In `packages/server/src/routes/tasks.ts`:

Update `CreateTaskSchema`:
```typescript
const CreateTaskSchema = z.object({
  id: z.string().optional(),
  type: z.string().optional(),
  params: z.record(z.unknown()).optional(),
  metadata: z.record(z.unknown()).optional(),
  ttl: z.number().int().positive().optional(),
  webhooks: z.array(z.unknown()).optional(),
  cleanup: z.object({ rules: z.array(z.unknown()) }).optional(),
  tags: z.array(z.string()).optional(),
  assignMode: z.enum(['external', 'pull', 'ws-offer', 'ws-race']).optional(),
  cost: z.number().int().positive().optional(),
  disconnectPolicy: z.enum(['reassign', 'mark', 'fail']).optional(),
})
```

Update the input construction in POST handler:
```typescript
    if (d.tags !== undefined) input.tags = d.tags
    if (d.assignMode !== undefined) input.assignMode = d.assignMode
    if (d.cost !== undefined) input.cost = d.cost
    if (d.disconnectPolicy !== undefined) input.disconnectPolicy = d.disconnectPolicy
```

Update PATCH status schema to include `assigned`:
```typescript
    const schema = z.object({
      status: z.enum(['assigned', 'running', 'completed', 'failed', 'timeout', 'cancelled']),
      // ...rest unchanged
    })
```

**Step 4: Run tests**

```bash
cd packages/server && pnpm test -- tests/tasks.test.ts
```

**Step 5: Commit**

```bash
git add packages/server/src/routes/tasks.ts packages/server/tests/tasks.test.ts
git commit -m "feat(server): accept worker assignment fields in REST API"
```

---

## Phase 2: Memory Adapters & Worker Matching

### Task 2.1: Implement worker matching logic

**Files:**
- Create: `packages/core/src/worker-matching.ts`
- Test: `packages/core/tests/unit/worker-matching.test.ts`

**Step 1: Write failing tests**

Create `packages/core/tests/unit/worker-matching.test.ts`:

```typescript
import { describe, it, expect } from 'vitest'
import { matchesTag, matchesWorkerRule } from '../../src/worker-matching.js'
import type { Task, Worker, WorkerMatchRule } from '../../src/types.js'

describe('matchesTag', () => {
  it('matches when task has all required tags', () => {
    expect(matchesTag(['gpu', 'us-east', 'large'], { all: ['gpu', 'us-east'] })).toBe(true)
  })

  it('rejects when task missing a required tag', () => {
    expect(matchesTag(['gpu'], { all: ['gpu', 'us-east'] })).toBe(false)
  })

  it('matches when task has at least one "any" tag', () => {
    expect(matchesTag(['eu-west'], { any: ['us-east', 'eu-west'] })).toBe(true)
  })

  it('rejects when task has none of the "any" tags', () => {
    expect(matchesTag(['ap-south'], { any: ['us-east', 'eu-west'] })).toBe(false)
  })

  it('rejects when task has a "none" tag', () => {
    expect(matchesTag(['gpu', 'deprecated'], { none: ['deprecated'] })).toBe(false)
  })

  it('matches when task has no "none" tags', () => {
    expect(matchesTag(['gpu'], { none: ['deprecated'] })).toBe(true)
  })

  it('combines all/any/none correctly', () => {
    const matcher = { all: ['gpu'], any: ['us-east', 'eu-west'], none: ['deprecated'] }
    expect(matchesTag(['gpu', 'us-east'], matcher)).toBe(true)
    expect(matchesTag(['gpu', 'ap-south'], matcher)).toBe(false) // no any match
    expect(matchesTag(['us-east'], matcher)).toBe(false) // missing all
    expect(matchesTag(['gpu', 'us-east', 'deprecated'], matcher)).toBe(false) // has none
  })

  it('empty matcher matches everything', () => {
    expect(matchesTag(['anything'], {})).toBe(true)
    expect(matchesTag([], {})).toBe(true)
  })

  it('undefined tags treated as empty array', () => {
    expect(matchesTag(undefined, { all: ['gpu'] })).toBe(false)
    expect(matchesTag(undefined, {})).toBe(true)
  })
})

describe('matchesWorkerRule', () => {
  const makeTask = (overrides: Partial<Task> = {}): Task => ({
    id: 'task-1', status: 'pending', createdAt: 1000, updatedAt: 1000,
    ...overrides,
  })

  it('matches by taskType wildcard', () => {
    const rule: WorkerMatchRule = { taskTypes: ['llm.*'] }
    expect(matchesWorkerRule(makeTask({ type: 'llm.chat' }), rule)).toBe(true)
    expect(matchesWorkerRule(makeTask({ type: 'image.gen' }), rule)).toBe(false)
  })

  it('matches by exact taskType', () => {
    const rule: WorkerMatchRule = { taskTypes: ['llm.chat'] }
    expect(matchesWorkerRule(makeTask({ type: 'llm.chat' }), rule)).toBe(true)
    expect(matchesWorkerRule(makeTask({ type: 'llm.delta' }), rule)).toBe(false)
  })

  it('matches with "*" taskType', () => {
    const rule: WorkerMatchRule = { taskTypes: ['*'] }
    expect(matchesWorkerRule(makeTask({ type: 'anything' }), rule)).toBe(true)
  })

  it('matches by tags', () => {
    const rule: WorkerMatchRule = { tags: { all: ['gpu'] } }
    expect(matchesWorkerRule(makeTask({ tags: ['gpu', 'fast'] }), rule)).toBe(true)
    expect(matchesWorkerRule(makeTask({ tags: ['cpu'] }), rule)).toBe(false)
  })

  it('combines taskType and tags', () => {
    const rule: WorkerMatchRule = { taskTypes: ['llm.*'], tags: { all: ['gpu'] } }
    expect(matchesWorkerRule(makeTask({ type: 'llm.chat', tags: ['gpu'] }), rule)).toBe(true)
    expect(matchesWorkerRule(makeTask({ type: 'llm.chat', tags: ['cpu'] }), rule)).toBe(false)
    expect(matchesWorkerRule(makeTask({ type: 'image.gen', tags: ['gpu'] }), rule)).toBe(false)
  })

  it('empty rule matches everything', () => {
    expect(matchesWorkerRule(makeTask({ type: 'anything' }), {})).toBe(true)
  })

  it('task with no type matches when rule has no taskTypes', () => {
    expect(matchesWorkerRule(makeTask(), {})).toBe(true)
  })

  it('task with no type does not match when rule has taskTypes', () => {
    expect(matchesWorkerRule(makeTask(), { taskTypes: ['llm.*'] })).toBe(false)
  })
})
```

**Step 2: Run tests to verify they fail**

```bash
cd packages/core && pnpm test -- tests/unit/worker-matching.test.ts
```

**Step 3: Implement worker matching**

Create `packages/core/src/worker-matching.ts`:

```typescript
import { matchesType } from './filter.js'
import type { Task, TagMatcher, WorkerMatchRule } from './types.js'

export function matchesTag(
  taskTags: string[] | undefined,
  matcher: TagMatcher,
): boolean {
  const tags = taskTags ?? []

  if (matcher.all && matcher.all.length > 0) {
    if (!matcher.all.every((t) => tags.includes(t))) return false
  }

  if (matcher.any && matcher.any.length > 0) {
    if (!matcher.any.some((t) => tags.includes(t))) return false
  }

  if (matcher.none && matcher.none.length > 0) {
    if (matcher.none.some((t) => tags.includes(t))) return false
  }

  return true
}

export function matchesWorkerRule(task: Task, rule: WorkerMatchRule): boolean {
  if (rule.taskTypes && rule.taskTypes.length > 0) {
    if (!task.type) return false
    if (!matchesType(task.type, rule.taskTypes)) return false
  }

  if (rule.tags) {
    if (!matchesTag(task.tags, rule.tags)) return false
  }

  return true
}
```

**Step 4: Export from index**

Add to `packages/core/src/index.ts`:

```typescript
export * from './worker-matching.js'
```

**Step 5: Run tests**

```bash
cd packages/core && pnpm test -- tests/unit/worker-matching.test.ts
```

**Step 6: Commit**

```bash
git add packages/core/src/worker-matching.ts packages/core/tests/unit/worker-matching.test.ts packages/core/src/index.ts
git commit -m "feat(core): add worker matching logic (taskType wildcard + tags all/any/none)"
```

---

### Task 2.2: Extend MemoryShortTermStore with worker methods

**Files:**
- Modify: `packages/core/src/memory-adapters.ts`
- Test: `packages/core/tests/unit/memory-adapters.test.ts`

**Step 1: Write failing tests**

Add to `packages/core/tests/unit/memory-adapters.test.ts`:

```typescript
import type { Worker, WorkerAssignment, Task } from '../../src/types.js'

const makeWorker = (overrides: Partial<Worker> = {}): Worker => ({
  id: 'worker-1',
  status: 'idle',
  matchRule: {},
  capacity: 10,
  usedSlots: 0,
  weight: 50,
  connectionMode: 'websocket',
  connectedAt: 1000,
  lastHeartbeatAt: 1000,
  ...overrides,
})

describe('MemoryShortTermStore — worker methods', () => {
  it('saveWorker and getWorker', async () => {
    const store = new MemoryShortTermStore()
    const worker = makeWorker()
    await store.saveWorker(worker)
    expect(await store.getWorker('worker-1')).toEqual(worker)
  })

  it('getWorker returns null for missing worker', async () => {
    const store = new MemoryShortTermStore()
    expect(await store.getWorker('nope')).toBeNull()
  })

  it('listWorkers returns all workers', async () => {
    const store = new MemoryShortTermStore()
    await store.saveWorker(makeWorker({ id: 'w1' }))
    await store.saveWorker(makeWorker({ id: 'w2', status: 'busy' }))
    const all = await store.listWorkers()
    expect(all).toHaveLength(2)
  })

  it('listWorkers filters by status', async () => {
    const store = new MemoryShortTermStore()
    await store.saveWorker(makeWorker({ id: 'w1', status: 'idle' }))
    await store.saveWorker(makeWorker({ id: 'w2', status: 'busy' }))
    const idle = await store.listWorkers({ status: ['idle'] })
    expect(idle).toHaveLength(1)
    expect(idle[0]!.id).toBe('w1')
  })

  it('deleteWorker removes worker', async () => {
    const store = new MemoryShortTermStore()
    await store.saveWorker(makeWorker())
    await store.deleteWorker('worker-1')
    expect(await store.getWorker('worker-1')).toBeNull()
  })

  it('listTasks filters by status', async () => {
    const store = new MemoryShortTermStore()
    await store.saveTask({ id: 't1', status: 'pending', createdAt: 1, updatedAt: 1 } as Task)
    await store.saveTask({ id: 't2', status: 'running', createdAt: 2, updatedAt: 2 } as Task)
    const pending = await store.listTasks({ status: ['pending'] })
    expect(pending).toHaveLength(1)
    expect(pending[0]!.id).toBe('t1')
  })

  it('listTasks filters by tags (all)', async () => {
    const store = new MemoryShortTermStore()
    await store.saveTask({ id: 't1', status: 'pending', createdAt: 1, updatedAt: 1, tags: ['gpu'] } as Task)
    await store.saveTask({ id: 't2', status: 'pending', createdAt: 2, updatedAt: 2, tags: ['cpu'] } as Task)
    const result = await store.listTasks({ tags: { all: ['gpu'] } })
    expect(result).toHaveLength(1)
    expect(result[0]!.id).toBe('t1')
  })

  it('listTasks excludes taskIds', async () => {
    const store = new MemoryShortTermStore()
    await store.saveTask({ id: 't1', status: 'pending', createdAt: 1, updatedAt: 1 } as Task)
    await store.saveTask({ id: 't2', status: 'pending', createdAt: 2, updatedAt: 2 } as Task)
    const result = await store.listTasks({ excludeTaskIds: ['t1'] })
    expect(result).toHaveLength(1)
    expect(result[0]!.id).toBe('t2')
  })

  it('listTasks respects limit', async () => {
    const store = new MemoryShortTermStore()
    await store.saveTask({ id: 't1', status: 'pending', createdAt: 1, updatedAt: 1 } as Task)
    await store.saveTask({ id: 't2', status: 'pending', createdAt: 2, updatedAt: 2 } as Task)
    const result = await store.listTasks({ limit: 1 })
    expect(result).toHaveLength(1)
  })
})

describe('MemoryShortTermStore — assignment methods', () => {
  it('claimTask succeeds when task is pending and worker has capacity', async () => {
    const store = new MemoryShortTermStore()
    await store.saveTask({ id: 't1', status: 'pending', createdAt: 1, updatedAt: 1 } as Task)
    await store.saveWorker(makeWorker({ id: 'w1', capacity: 10, usedSlots: 0 }))
    const ok = await store.claimTask('t1', 'w1', 1)
    expect(ok).toBe(true)
    const worker = await store.getWorker('w1')
    expect(worker!.usedSlots).toBe(1)
  })

  it('claimTask fails when worker has no capacity', async () => {
    const store = new MemoryShortTermStore()
    await store.saveTask({ id: 't1', status: 'pending', createdAt: 1, updatedAt: 1 } as Task)
    await store.saveWorker(makeWorker({ id: 'w1', capacity: 5, usedSlots: 5 }))
    const ok = await store.claimTask('t1', 'w1', 1)
    expect(ok).toBe(false)
  })

  it('claimTask fails when task is not pending', async () => {
    const store = new MemoryShortTermStore()
    await store.saveTask({ id: 't1', status: 'running', createdAt: 1, updatedAt: 1 } as Task)
    await store.saveWorker(makeWorker({ id: 'w1', capacity: 10, usedSlots: 0 }))
    const ok = await store.claimTask('t1', 'w1', 1)
    expect(ok).toBe(false)
  })

  it('addAssignment and getTaskAssignment', async () => {
    const store = new MemoryShortTermStore()
    const assignment: WorkerAssignment = {
      taskId: 't1', workerId: 'w1', cost: 1, assignedAt: 1000, status: 'assigned',
    }
    await store.addAssignment(assignment)
    expect(await store.getTaskAssignment('t1')).toEqual(assignment)
  })

  it('getWorkerAssignments returns all assignments for a worker', async () => {
    const store = new MemoryShortTermStore()
    await store.addAssignment({ taskId: 't1', workerId: 'w1', cost: 1, assignedAt: 1000, status: 'assigned' })
    await store.addAssignment({ taskId: 't2', workerId: 'w1', cost: 2, assignedAt: 1001, status: 'running' })
    await store.addAssignment({ taskId: 't3', workerId: 'w2', cost: 1, assignedAt: 1002, status: 'assigned' })
    const w1 = await store.getWorkerAssignments('w1')
    expect(w1).toHaveLength(2)
  })

  it('removeAssignment deletes by taskId', async () => {
    const store = new MemoryShortTermStore()
    await store.addAssignment({ taskId: 't1', workerId: 'w1', cost: 1, assignedAt: 1000, status: 'assigned' })
    await store.removeAssignment('t1')
    expect(await store.getTaskAssignment('t1')).toBeNull()
  })
})
```

**Step 2: Run tests to verify they fail**

```bash
cd packages/core && pnpm test -- tests/unit/memory-adapters.test.ts
```

**Step 3: Implement the methods**

Add to `packages/core/src/memory-adapters.ts`:

Import new types at top:
```typescript
import type {
  Task, TaskEvent, BroadcastProvider, ShortTermStore, EventQueryOptions,
  Worker, WorkerAssignment, TaskFilter, WorkerFilter,
} from './types.js'
import { matchesTag } from './worker-matching.js'
import { matchesType } from './filter.js'
```

Add new private fields and methods to `MemoryShortTermStore`:

```typescript
  private workers = new Map<string, Worker>()
  private assignmentsByTask = new Map<string, WorkerAssignment>()
  private assignmentsByWorker = new Map<string, Set<string>>()  // workerId → Set<taskId>

  async listTasks(filter: TaskFilter): Promise<Task[]> {
    let result = [...this.tasks.values()]
    if (filter.status) result = result.filter((t) => filter.status!.includes(t.status))
    if (filter.types) result = result.filter((t) => t.type !== undefined && matchesType(t.type, filter.types))
    if (filter.tags) result = result.filter((t) => matchesTag(t.tags, filter.tags!))
    if (filter.assignMode) result = result.filter((t) => t.assignMode !== undefined && filter.assignMode!.includes(t.assignMode))
    if (filter.excludeTaskIds) {
      const exclude = new Set(filter.excludeTaskIds)
      result = result.filter((t) => !exclude.has(t.id))
    }
    if (filter.limit) result = result.slice(0, filter.limit)
    return result
  }

  async saveWorker(worker: Worker): Promise<void> {
    this.workers.set(worker.id, { ...worker })
  }

  async getWorker(workerId: string): Promise<Worker | null> {
    return this.workers.get(workerId) ?? null
  }

  async listWorkers(filter?: WorkerFilter): Promise<Worker[]> {
    let result = [...this.workers.values()]
    if (filter?.status) result = result.filter((w) => filter.status!.includes(w.status))
    if (filter?.connectionMode) result = result.filter((w) => filter.connectionMode!.includes(w.connectionMode))
    return result
  }

  async deleteWorker(workerId: string): Promise<void> {
    this.workers.delete(workerId)
    this.assignmentsByWorker.delete(workerId)
  }

  async claimTask(taskId: string, workerId: string, cost: number): Promise<boolean> {
    const task = this.tasks.get(taskId)
    if (!task || task.status !== 'pending') return false
    const worker = this.workers.get(workerId)
    if (!worker || worker.usedSlots + cost > worker.capacity) return false
    worker.usedSlots += cost
    return true
  }

  async addAssignment(assignment: WorkerAssignment): Promise<void> {
    this.assignmentsByTask.set(assignment.taskId, { ...assignment })
    if (!this.assignmentsByWorker.has(assignment.workerId)) {
      this.assignmentsByWorker.set(assignment.workerId, new Set())
    }
    this.assignmentsByWorker.get(assignment.workerId)!.add(assignment.taskId)
  }

  async removeAssignment(taskId: string): Promise<void> {
    const assignment = this.assignmentsByTask.get(taskId)
    if (assignment) {
      this.assignmentsByWorker.get(assignment.workerId)?.delete(taskId)
      this.assignmentsByTask.delete(taskId)
    }
  }

  async getWorkerAssignments(workerId: string): Promise<WorkerAssignment[]> {
    const taskIds = this.assignmentsByWorker.get(workerId)
    if (!taskIds) return []
    return [...taskIds]
      .map((id) => this.assignmentsByTask.get(id))
      .filter((a): a is WorkerAssignment => a !== undefined)
  }

  async getTaskAssignment(taskId: string): Promise<WorkerAssignment | null> {
    return this.assignmentsByTask.get(taskId) ?? null
  }
```

**Step 4: Run tests**

```bash
cd packages/core && pnpm test -- tests/unit/memory-adapters.test.ts
```

**Step 5: Run full core suite**

```bash
cd packages/core && pnpm test
```

**Step 6: Commit**

```bash
git add packages/core/src/memory-adapters.ts packages/core/tests/unit/memory-adapters.test.ts
git commit -m "feat(core): add worker and assignment methods to MemoryShortTermStore"
```

---

## Phase 3: WorkerManager Core Logic

### Task 3.1: WorkerManager — Worker registration & lifecycle

**Files:**
- Create: `packages/core/src/worker-manager.ts`
- Test: `packages/core/tests/unit/worker-manager.test.ts`

**Step 1: Write failing tests**

Create `packages/core/tests/unit/worker-manager.test.ts`:

```typescript
import { describe, it, expect, vi, beforeEach } from 'vitest'
import { WorkerManager } from '../../src/worker-manager.js'
import { TaskEngine } from '../../src/engine.js'
import { MemoryShortTermStore, MemoryBroadcastProvider } from '../../src/memory-adapters.js'
import type { Worker } from '../../src/types.js'

function makeSetup() {
  const store = new MemoryShortTermStore()
  const broadcast = new MemoryBroadcastProvider()
  const engine = new TaskEngine({ shortTerm: store, broadcast })
  const manager = new WorkerManager({ engine, shortTerm: store, broadcast })
  return { store, broadcast, engine, manager }
}

describe('WorkerManager — registration', () => {
  it('registerWorker creates a worker with idle status', async () => {
    const { manager, store } = makeSetup()
    const worker = await manager.registerWorker({
      id: 'w1',
      matchRule: { taskTypes: ['llm.*'] },
      capacity: 10,
      weight: 50,
      connectionMode: 'websocket',
    })
    expect(worker.status).toBe('idle')
    expect(worker.usedSlots).toBe(0)
    expect(await store.getWorker('w1')).toEqual(worker)
  })

  it('registerWorker generates id if not provided', async () => {
    const { manager } = makeSetup()
    const worker = await manager.registerWorker({
      matchRule: {},
      capacity: 10,
      connectionMode: 'pull',
    })
    expect(worker.id).toBeTruthy()
    expect(worker.id.length).toBeGreaterThan(0)
  })

  it('registerWorker defaults weight to 50', async () => {
    const { manager } = makeSetup()
    const worker = await manager.registerWorker({
      matchRule: {},
      capacity: 10,
      connectionMode: 'websocket',
    })
    expect(worker.weight).toBe(50)
  })

  it('unregisterWorker removes worker and marks offline', async () => {
    const { manager, store } = makeSetup()
    await manager.registerWorker({ id: 'w1', matchRule: {}, capacity: 10, connectionMode: 'websocket' })
    await manager.unregisterWorker('w1')
    expect(await store.getWorker('w1')).toBeNull()
  })

  it('updateWorker changes weight', async () => {
    const { manager, store } = makeSetup()
    await manager.registerWorker({ id: 'w1', matchRule: {}, capacity: 10, connectionMode: 'websocket' })
    await manager.updateWorker('w1', { weight: 80 })
    const w = await store.getWorker('w1')
    expect(w!.weight).toBe(80)
  })

  it('heartbeat updates lastHeartbeatAt', async () => {
    const { manager, store } = makeSetup()
    await manager.registerWorker({ id: 'w1', matchRule: {}, capacity: 10, connectionMode: 'websocket' })
    const before = (await store.getWorker('w1'))!.lastHeartbeatAt
    // Small delay to ensure time changes
    await new Promise((r) => setTimeout(r, 10))
    await manager.heartbeat('w1')
    const after = (await store.getWorker('w1'))!.lastHeartbeatAt
    expect(after).toBeGreaterThanOrEqual(before)
  })

  it('listWorkers delegates to store', async () => {
    const { manager } = makeSetup()
    await manager.registerWorker({ id: 'w1', matchRule: {}, capacity: 10, connectionMode: 'websocket' })
    await manager.registerWorker({ id: 'w2', matchRule: {}, capacity: 5, connectionMode: 'pull' })
    const all = await manager.listWorkers()
    expect(all).toHaveLength(2)
  })

  it('getWorker returns null for unknown worker', async () => {
    const { manager } = makeSetup()
    expect(await manager.getWorker('nope')).toBeNull()
  })
})
```

**Step 2: Run tests to verify they fail**

```bash
cd packages/core && pnpm test -- tests/unit/worker-manager.test.ts
```

**Step 3: Implement WorkerManager (registration part)**

Create `packages/core/src/worker-manager.ts`:

```typescript
import { ulid } from 'ulidx'
import type {
  Task,
  TaskStatus,
  Worker,
  WorkerAssignment,
  WorkerFilter,
  BroadcastProvider,
  ShortTermStore,
  LongTermStore,
  TaskcastHooks,
  AssignMode,
  DisconnectPolicy,
} from './types.js'
import type { TaskEngine } from './engine.js'

export interface WorkerManagerOptions {
  engine: TaskEngine
  shortTerm: ShortTermStore
  broadcast: BroadcastProvider
  longTerm?: LongTermStore
  hooks?: TaskcastHooks
  defaults?: WorkerManagerDefaults
}

export interface WorkerManagerDefaults {
  assignMode?: AssignMode
  heartbeatIntervalMs?: number
  heartbeatTimeoutMs?: number
  offerTimeoutMs?: number
  disconnectPolicy?: DisconnectPolicy
  disconnectGraceMs?: number
}

export interface WorkerRegistration {
  id?: string
  matchRule: Worker['matchRule']
  capacity: number
  weight?: number
  connectionMode: Worker['connectionMode']
  metadata?: Record<string, unknown>
}

export interface WorkerUpdate {
  weight?: number
  capacity?: number
  matchRule?: Worker['matchRule']
}

export class WorkerManager {
  private opts: WorkerManagerOptions

  constructor(opts: WorkerManagerOptions) {
    this.opts = opts
  }

  async registerWorker(config: WorkerRegistration): Promise<Worker> {
    const now = Date.now()
    const worker: Worker = {
      id: config.id ?? ulid(),
      status: 'idle',
      matchRule: config.matchRule,
      capacity: config.capacity,
      usedSlots: 0,
      weight: config.weight ?? 50,
      connectionMode: config.connectionMode,
      connectedAt: now,
      lastHeartbeatAt: now,
      ...(config.metadata !== undefined && { metadata: config.metadata }),
    }
    await this.opts.shortTerm.saveWorker(worker)
    this.opts.hooks?.onWorkerConnected?.(worker)
    return worker
  }

  async unregisterWorker(workerId: string): Promise<void> {
    const worker = await this.opts.shortTerm.getWorker(workerId)
    if (!worker) return
    await this.opts.shortTerm.deleteWorker(workerId)
    this.opts.hooks?.onWorkerDisconnected?.(worker, 'unregistered')
  }

  async updateWorker(workerId: string, update: WorkerUpdate): Promise<Worker | null> {
    const worker = await this.opts.shortTerm.getWorker(workerId)
    if (!worker) return null
    if (update.weight !== undefined) worker.weight = update.weight
    if (update.capacity !== undefined) worker.capacity = update.capacity
    if (update.matchRule !== undefined) worker.matchRule = update.matchRule
    await this.opts.shortTerm.saveWorker(worker)
    return worker
  }

  async heartbeat(workerId: string): Promise<void> {
    const worker = await this.opts.shortTerm.getWorker(workerId)
    if (!worker) return
    worker.lastHeartbeatAt = Date.now()
    await this.opts.shortTerm.saveWorker(worker)
  }

  async getWorker(workerId: string): Promise<Worker | null> {
    return this.opts.shortTerm.getWorker(workerId)
  }

  async listWorkers(filter?: WorkerFilter): Promise<Worker[]> {
    return this.opts.shortTerm.listWorkers(filter)
  }
}
```

**Step 4: Export from index**

Add to `packages/core/src/index.ts`:

```typescript
export * from './worker-manager.js'
```

**Step 5: Run tests**

```bash
cd packages/core && pnpm test -- tests/unit/worker-manager.test.ts
```

**Step 6: Commit**

```bash
git add packages/core/src/worker-manager.ts packages/core/tests/unit/worker-manager.test.ts packages/core/src/index.ts
git commit -m "feat(core): add WorkerManager with registration and lifecycle"
```

---

### Task 3.2: WorkerManager — Task dispatch, claim & decline

**Files:**
- Modify: `packages/core/src/worker-manager.ts`
- Test: `packages/core/tests/unit/worker-manager.test.ts`

**Step 1: Write failing tests**

Add to the existing worker-manager test file:

```typescript
describe('WorkerManager — dispatch & claim', () => {
  it('dispatchTask finds best matching worker by weight', async () => {
    const { manager, engine, store } = makeSetup()
    await manager.registerWorker({ id: 'w1', matchRule: { taskTypes: ['llm.*'] }, capacity: 10, weight: 30, connectionMode: 'websocket' })
    await manager.registerWorker({ id: 'w2', matchRule: { taskTypes: ['llm.*'] }, capacity: 10, weight: 80, connectionMode: 'websocket' })
    const task = await engine.createTask({ type: 'llm.chat', assignMode: 'ws-offer' })
    const result = await manager.dispatchTask(task.id)
    expect(result.matched).toBe(true)
    expect(result.workerId).toBe('w2')  // highest weight
  })

  it('dispatchTask skips workers with no capacity', async () => {
    const { manager, engine } = makeSetup()
    await manager.registerWorker({ id: 'w1', matchRule: { taskTypes: ['*'] }, capacity: 1, weight: 90, connectionMode: 'websocket' })
    // Fill up w1
    const t0 = await engine.createTask({ type: 'test', assignMode: 'ws-offer' })
    await manager.claimTask(t0.id, 'w1')

    await manager.registerWorker({ id: 'w2', matchRule: { taskTypes: ['*'] }, capacity: 10, weight: 50, connectionMode: 'websocket' })
    const task = await engine.createTask({ type: 'test', assignMode: 'ws-offer' })
    const result = await manager.dispatchTask(task.id)
    expect(result.workerId).toBe('w2')
  })

  it('dispatchTask returns no match when no workers match', async () => {
    const { manager, engine } = makeSetup()
    await manager.registerWorker({ id: 'w1', matchRule: { taskTypes: ['image.*'] }, capacity: 10, connectionMode: 'websocket' })
    const task = await engine.createTask({ type: 'llm.chat', assignMode: 'ws-offer' })
    const result = await manager.dispatchTask(task.id)
    expect(result.matched).toBe(false)
  })

  it('dispatchTask skips blacklisted workers', async () => {
    const { manager, engine } = makeSetup()
    await manager.registerWorker({ id: 'w1', matchRule: { taskTypes: ['*'] }, capacity: 10, weight: 90, connectionMode: 'websocket' })
    await manager.registerWorker({ id: 'w2', matchRule: { taskTypes: ['*'] }, capacity: 10, weight: 50, connectionMode: 'websocket' })
    const task = await engine.createTask({
      type: 'test',
      assignMode: 'ws-offer',
      metadata: { _blacklistedWorkers: ['w1'] },
    })
    const result = await manager.dispatchTask(task.id)
    expect(result.workerId).toBe('w2')
  })

  it('claimTask transitions task to assigned and creates assignment', async () => {
    const { manager, engine, store } = makeSetup()
    await manager.registerWorker({ id: 'w1', matchRule: { taskTypes: ['*'] }, capacity: 10, connectionMode: 'websocket' })
    const task = await engine.createTask({ type: 'test', assignMode: 'ws-offer', cost: 3 })
    const result = await manager.claimTask(task.id, 'w1')
    expect(result.success).toBe(true)
    const updated = await engine.getTask(task.id)
    expect(updated!.status).toBe('assigned')
    expect(updated!.assignedWorker).toBe('w1')
    const assignment = await store.getTaskAssignment(task.id)
    expect(assignment).toBeTruthy()
    expect(assignment!.cost).toBe(3)
  })

  it('claimTask fails for non-pending task', async () => {
    const { manager, engine } = makeSetup()
    await manager.registerWorker({ id: 'w1', matchRule: { taskTypes: ['*'] }, capacity: 10, connectionMode: 'websocket' })
    const task = await engine.createTask({ type: 'test' })
    await engine.transitionTask(task.id, 'running')
    const result = await manager.claimTask(task.id, 'w1')
    expect(result.success).toBe(false)
  })
})

describe('WorkerManager — decline', () => {
  it('declineTask returns task to pending', async () => {
    const { manager, engine, store } = makeSetup()
    await manager.registerWorker({ id: 'w1', matchRule: { taskTypes: ['*'] }, capacity: 10, connectionMode: 'websocket' })
    const task = await engine.createTask({ type: 'test', assignMode: 'ws-offer', cost: 2 })
    await manager.claimTask(task.id, 'w1')
    await manager.declineTask(task.id, 'w1')
    const updated = await engine.getTask(task.id)
    expect(updated!.status).toBe('pending')
    expect(updated!.assignedWorker).toBeUndefined()
    const worker = await store.getWorker('w1')
    expect(worker!.usedSlots).toBe(0)
  })

  it('declineTask with blacklist adds worker to exclusion list', async () => {
    const { manager, engine } = makeSetup()
    await manager.registerWorker({ id: 'w1', matchRule: { taskTypes: ['*'] }, capacity: 10, connectionMode: 'websocket' })
    const task = await engine.createTask({ type: 'test', assignMode: 'ws-offer' })
    await manager.claimTask(task.id, 'w1')
    await manager.declineTask(task.id, 'w1', { blacklist: true })
    const updated = await engine.getTask(task.id)
    const bl = (updated!.metadata as Record<string, unknown>)?._blacklistedWorkers as string[]
    expect(bl).toContain('w1')
  })
})
```

**Step 2: Run tests to verify they fail**

```bash
cd packages/core && pnpm test -- tests/unit/worker-manager.test.ts
```

**Step 3: Implement dispatch, claim, decline**

Add to `WorkerManager` class in `packages/core/src/worker-manager.ts`:

Add import at top:
```typescript
import { matchesWorkerRule } from './worker-matching.js'
```

Add type definitions:
```typescript
export interface DispatchResult {
  matched: boolean
  workerId?: string
}

export interface ClaimResult {
  success: boolean
  reason?: string
}

export interface DeclineOptions {
  blacklist?: boolean
}
```

Add methods to WorkerManager:

```typescript
  async dispatchTask(taskId: string): Promise<DispatchResult> {
    const task = await this.opts.engine.getTask(taskId)
    if (!task || task.status !== 'pending') return { matched: false }

    const blacklist = (task.metadata?._blacklistedWorkers as string[]) ?? []
    const workers = await this.opts.shortTerm.listWorkers({ status: ['idle', 'busy'] })

    const candidates = workers.filter((w) => {
      if (blacklist.includes(w.id)) return false
      if (w.usedSlots + (task.cost ?? 1) > w.capacity) return false
      return matchesWorkerRule(task, w.matchRule)
    })

    if (candidates.length === 0) return { matched: false }

    // Sort by: weight desc → available slots desc → connectedAt asc
    candidates.sort((a, b) => {
      if (b.weight !== a.weight) return b.weight - a.weight
      const aSlots = a.capacity - a.usedSlots
      const bSlots = b.capacity - b.usedSlots
      if (bSlots !== aSlots) return bSlots - aSlots
      return a.connectedAt - b.connectedAt
    })

    return { matched: true, workerId: candidates[0]!.id }
  }

  async claimTask(taskId: string, workerId: string): Promise<ClaimResult> {
    const task = await this.opts.engine.getTask(taskId)
    if (!task) return { success: false, reason: 'Task not found' }
    if (task.status !== 'pending') return { success: false, reason: `Task status is ${task.status}` }

    const cost = task.cost ?? 1
    const claimed = await this.opts.shortTerm.claimTask(taskId, workerId, cost)
    if (!claimed) return { success: false, reason: 'Claim failed (no capacity or task changed)' }

    // Transition to assigned
    const updated = await this.opts.engine.transitionTask(taskId, 'assigned')

    // Set assignedWorker on the task
    const withWorker: Task = { ...updated, assignedWorker: workerId }
    await this.opts.shortTerm.saveTask(withWorker)
    if (this.opts.longTerm) await this.opts.longTerm.saveTask(withWorker)

    // Record assignment
    await this.opts.shortTerm.addAssignment({
      taskId,
      workerId,
      cost,
      assignedAt: Date.now(),
      status: 'assigned',
    })

    // Update worker status
    const worker = await this.opts.shortTerm.getWorker(workerId)
    if (worker) {
      worker.status = worker.usedSlots >= worker.capacity ? 'busy' : 'idle'
      await this.opts.shortTerm.saveWorker(worker)
      this.opts.hooks?.onTaskAssigned?.(withWorker, worker)
    }

    return { success: true }
  }

  async declineTask(taskId: string, workerId: string, opts?: DeclineOptions): Promise<void> {
    const assignment = await this.opts.shortTerm.getTaskAssignment(taskId)
    if (!assignment || assignment.workerId !== workerId) return

    // Remove assignment
    await this.opts.shortTerm.removeAssignment(taskId)

    // Restore worker capacity
    const worker = await this.opts.shortTerm.getWorker(workerId)
    if (worker) {
      worker.usedSlots = Math.max(0, worker.usedSlots - assignment.cost)
      worker.status = 'idle'
      await this.opts.shortTerm.saveWorker(worker)
    }

    // Transition task back to pending
    await this.opts.engine.transitionTask(taskId, 'pending')

    // Clear assignedWorker and optionally blacklist
    const task = await this.opts.engine.getTask(taskId)
    if (task) {
      const metadata = { ...(task.metadata ?? {}) }
      delete (task as Record<string, unknown>).assignedWorker
      if (opts?.blacklist) {
        const bl = (metadata._blacklistedWorkers as string[]) ?? []
        metadata._blacklistedWorkers = [...bl, workerId]
      }
      const updated: Task = { ...task, metadata, assignedWorker: undefined }
      // Remove assignedWorker key if undefined
      delete (updated as Record<string, unknown>).assignedWorker
      await this.opts.shortTerm.saveTask(updated)
      if (this.opts.longTerm) await this.opts.longTerm.saveTask(updated)
      if (worker) this.opts.hooks?.onTaskDeclined?.(updated, worker, opts?.blacklist ?? false)
    }
  }

  async getWorkerTasks(workerId: string): Promise<WorkerAssignment[]> {
    return this.opts.shortTerm.getWorkerAssignments(workerId)
  }
```

**Step 4: Run tests**

```bash
cd packages/core && pnpm test -- tests/unit/worker-manager.test.ts
```

**Step 5: Run full core suite**

```bash
cd packages/core && pnpm test
```

**Step 6: Commit**

```bash
git add packages/core/src/worker-manager.ts packages/core/tests/unit/worker-manager.test.ts
git commit -m "feat(core): add task dispatch, claim, and decline to WorkerManager"
```

---

## Phase 4: Server Layer — Worker HTTP Routes

### Task 4.1: Worker REST routes

**Files:**
- Create: `packages/server/src/routes/workers.ts`
- Test: `packages/server/tests/workers.test.ts`
- Modify: `packages/server/src/index.ts`

**Step 1: Write failing tests**

Create `packages/server/tests/workers.test.ts`:

```typescript
import { describe, it, expect } from 'vitest'
import { Hono } from 'hono'
import {
  TaskEngine,
  MemoryBroadcastProvider,
  MemoryShortTermStore,
  WorkerManager,
} from '@taskcast/core'
import { createWorkersRouter } from '../src/routes/workers.js'
import type { AuthContext } from '../src/auth.js'

function makeApp() {
  const store = new MemoryShortTermStore()
  const broadcast = new MemoryBroadcastProvider()
  const engine = new TaskEngine({ shortTerm: store, broadcast })
  const manager = new WorkerManager({ engine, shortTerm: store, broadcast })
  const app = new Hono()
  app.use('*', async (c, next) => {
    const auth: AuthContext = { taskIds: '*', scope: ['*'] }
    c.set('auth', auth)
    await next()
  })
  app.route('/workers', createWorkersRouter(manager, engine))
  return { app, engine, manager, store }
}

describe('GET /workers', () => {
  it('returns empty list initially', async () => {
    const { app } = makeApp()
    const res = await app.request('/workers')
    expect(res.status).toBe(200)
    expect(await res.json()).toEqual([])
  })

  it('returns registered workers', async () => {
    const { app, manager } = makeApp()
    await manager.registerWorker({ id: 'w1', matchRule: {}, capacity: 10, connectionMode: 'websocket' })
    const res = await app.request('/workers')
    expect(res.status).toBe(200)
    const body = await res.json()
    expect(body).toHaveLength(1)
    expect(body[0].id).toBe('w1')
  })
})

describe('GET /workers/:workerId', () => {
  it('returns 404 for unknown worker', async () => {
    const { app } = makeApp()
    const res = await app.request('/workers/nope')
    expect(res.status).toBe(404)
  })

  it('returns worker details', async () => {
    const { app, manager } = makeApp()
    await manager.registerWorker({ id: 'w1', matchRule: {}, capacity: 10, connectionMode: 'websocket' })
    const res = await app.request('/workers/w1')
    expect(res.status).toBe(200)
    const body = await res.json()
    expect(body.id).toBe('w1')
    expect(body.capacity).toBe(10)
  })
})

describe('DELETE /workers/:workerId', () => {
  it('removes worker', async () => {
    const { app, manager, store } = makeApp()
    await manager.registerWorker({ id: 'w1', matchRule: {}, capacity: 10, connectionMode: 'websocket' })
    const res = await app.request('/workers/w1', { method: 'DELETE' })
    expect(res.status).toBe(204)
    expect(await store.getWorker('w1')).toBeNull()
  })

  it('returns 404 for unknown worker', async () => {
    const { app } = makeApp()
    const res = await app.request('/workers/nope', { method: 'DELETE' })
    expect(res.status).toBe(404)
  })
})

describe('POST /tasks/:taskId/decline', () => {
  it('declines an assigned task', async () => {
    const { app, engine, manager } = makeApp()
    await manager.registerWorker({ id: 'w1', matchRule: { taskTypes: ['*'] }, capacity: 10, connectionMode: 'websocket' })
    const task = await engine.createTask({ type: 'test', assignMode: 'ws-offer' })
    await manager.claimTask(task.id, 'w1')
    const res = await app.request(`/workers/tasks/${task.id}/decline`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ workerId: 'w1' }),
    })
    expect(res.status).toBe(200)
    const updated = await engine.getTask(task.id)
    expect(updated!.status).toBe('pending')
  })
})

describe('scope enforcement', () => {
  it('requires worker:manage for GET /workers', async () => {
    const store = new MemoryShortTermStore()
    const broadcast = new MemoryBroadcastProvider()
    const engine = new TaskEngine({ shortTerm: store, broadcast })
    const manager = new WorkerManager({ engine, shortTerm: store, broadcast })
    const app = new Hono()
    app.use('*', async (c, next) => {
      c.set('auth', { taskIds: '*', scope: ['worker:connect'] } as AuthContext)
      await next()
    })
    app.route('/workers', createWorkersRouter(manager, engine))
    const res = await app.request('/workers')
    expect(res.status).toBe(403)
  })
})
```

**Step 2: Run tests to verify they fail**

```bash
cd packages/server && pnpm test -- tests/workers.test.ts
```

**Step 3: Implement workers router**

Create `packages/server/src/routes/workers.ts`:

```typescript
import { Hono } from 'hono'
import { z } from 'zod'
import { checkScope } from '../auth.js'
import type { WorkerManager, TaskEngine } from '@taskcast/core'

const DeclineSchema = z.object({
  workerId: z.string(),
  blacklist: z.boolean().optional(),
})

export function createWorkersRouter(manager: WorkerManager, engine: TaskEngine) {
  const router = new Hono()

  router.get('/', async (c) => {
    const auth = c.get('auth')
    if (!checkScope(auth, 'worker:manage')) return c.json({ error: 'Forbidden' }, 403)
    const workers = await manager.listWorkers()
    return c.json(workers)
  })

  router.get('/:workerId', async (c) => {
    const { workerId } = c.req.param()
    const auth = c.get('auth')
    if (!checkScope(auth, 'worker:manage')) return c.json({ error: 'Forbidden' }, 403)
    const worker = await manager.getWorker(workerId)
    if (!worker) return c.json({ error: 'Worker not found' }, 404)
    return c.json(worker)
  })

  router.delete('/:workerId', async (c) => {
    const { workerId } = c.req.param()
    const auth = c.get('auth')
    if (!checkScope(auth, 'worker:manage')) return c.json({ error: 'Forbidden' }, 403)
    const worker = await manager.getWorker(workerId)
    if (!worker) return c.json({ error: 'Worker not found' }, 404)
    await manager.unregisterWorker(workerId)
    return c.body(null, 204)
  })

  router.post('/tasks/:taskId/decline', async (c) => {
    const { taskId } = c.req.param()
    const auth = c.get('auth')
    if (!checkScope(auth, 'worker:connect')) return c.json({ error: 'Forbidden' }, 403)

    const body = await c.req.json()
    const parsed = DeclineSchema.safeParse(body)
    if (!parsed.success) return c.json({ error: parsed.error.flatten() }, 400)

    await manager.declineTask(taskId, parsed.data.workerId, {
      blacklist: parsed.data.blacklist,
    })
    return c.json({ ok: true })
  })

  return router
}
```

**Step 4: Update server factory**

In `packages/server/src/index.ts`, add optional worker manager support:

```typescript
import { createWorkersRouter } from './routes/workers.js'
import type { TaskEngine, WorkerManager } from '@taskcast/core'

export interface TaskcastServerOptions {
  engine: TaskEngine
  workerManager?: WorkerManager
  auth?: AuthConfig
}

export function createTaskcastApp(opts: TaskcastServerOptions): Hono {
  const app = new Hono()
  app.get('/health', (c) => c.json({ ok: true }))
  app.use('*', createAuthMiddleware(opts.auth ?? { mode: 'none' }))
  app.route('/tasks', createTasksRouter(opts.engine))
  app.route('/tasks', createSSERouter(opts.engine))
  if (opts.workerManager) {
    app.route('/workers', createWorkersRouter(opts.workerManager, opts.engine))
  }
  return app
}
```

Also export the new router from `packages/server/src/index.ts`:
```typescript
export { createWorkersRouter } from './routes/workers.js'
```

**Step 5: Run tests**

```bash
cd packages/server && pnpm test -- tests/workers.test.ts
```

**Step 6: Run full server suite**

```bash
cd packages/server && pnpm test
```

**Step 7: Commit**

```bash
git add packages/server/src/routes/workers.ts packages/server/src/index.ts packages/server/tests/workers.test.ts
git commit -m "feat(server): add worker REST routes and integrate into server factory"
```

---

## Phase 5: Worker Pull Mode

### Task 5.1: Pull mode — long-poll endpoint

**Files:**
- Modify: `packages/core/src/worker-manager.ts`
- Modify: `packages/server/src/routes/workers.ts`
- Test: `packages/core/tests/unit/worker-manager.test.ts`
- Test: `packages/server/tests/workers.test.ts`

**Step 1: Write failing test for core waitForTask**

Add to `packages/core/tests/unit/worker-manager.test.ts`:

```typescript
describe('WorkerManager — pull mode', () => {
  it('waitForTask resolves when matching task is created', async () => {
    const { manager, engine } = makeSetup()
    await manager.registerWorker({ id: 'w1', matchRule: { taskTypes: ['llm.*'] }, capacity: 10, connectionMode: 'pull' })

    const promise = manager.waitForTask('w1')
    // Create a matching task after a short delay
    setTimeout(async () => {
      await engine.createTask({ type: 'llm.chat', assignMode: 'pull' })
    }, 50)
    const task = await promise
    expect(task.type).toBe('llm.chat')
    expect(task.status).toBe('assigned')
  })

  it('waitForTask resolves immediately if pending task exists', async () => {
    const { manager, engine } = makeSetup()
    await engine.createTask({ type: 'llm.chat', assignMode: 'pull' })
    await manager.registerWorker({ id: 'w1', matchRule: { taskTypes: ['llm.*'] }, capacity: 10, connectionMode: 'pull' })
    const task = await manager.waitForTask('w1')
    expect(task.type).toBe('llm.chat')
  })

  it('waitForTask can be aborted', async () => {
    const { manager } = makeSetup()
    await manager.registerWorker({ id: 'w1', matchRule: {}, capacity: 10, connectionMode: 'pull' })
    const controller = new AbortController()
    setTimeout(() => controller.abort(), 50)
    await expect(manager.waitForTask('w1', controller.signal)).rejects.toThrow('aborted')
  })
})
```

**Step 2: Implement waitForTask in WorkerManager**

Add to `WorkerManager`:

```typescript
  async waitForTask(workerId: string, signal?: AbortSignal): Promise<Task> {
    const worker = await this.opts.shortTerm.getWorker(workerId)
    if (!worker) throw new Error('Worker not found')

    // Check existing pending tasks first
    const pending = await this.opts.shortTerm.listTasks({
      status: ['pending'],
      assignMode: ['pull'],
    })
    for (const task of pending) {
      if (!matchesWorkerRule(task, worker.matchRule)) continue
      const blacklist = (task.metadata?._blacklistedWorkers as string[]) ?? []
      if (blacklist.includes(workerId)) continue
      const result = await this.claimTask(task.id, workerId)
      if (result.success) {
        const claimed = await this.opts.engine.getTask(task.id)
        return claimed!
      }
    }

    // Wait for new tasks via broadcast on a well-known channel
    return new Promise<Task>((resolve, reject) => {
      if (signal?.aborted) {
        reject(new Error('aborted'))
        return
      }

      const channel = 'taskcast:worker:new-task'
      const unsub = this.opts.broadcast.subscribe(channel, async (event) => {
        const taskId = event.data as string
        const task = await this.opts.engine.getTask(taskId)
        if (!task || task.status !== 'pending') return
        if (task.assignMode !== 'pull') return
        const currentWorker = await this.opts.shortTerm.getWorker(workerId)
        if (!currentWorker) return
        if (!matchesWorkerRule(task, currentWorker.matchRule)) return
        const blacklist = (task.metadata?._blacklistedWorkers as string[]) ?? []
        if (blacklist.includes(workerId)) return
        const result = await this.claimTask(taskId, workerId)
        if (result.success) {
          unsub()
          const claimed = await this.opts.engine.getTask(taskId)
          resolve(claimed!)
        }
      })

      signal?.addEventListener('abort', () => {
        unsub()
        reject(new Error('aborted'))
      })
    })
  }

  /** Call this when a new task is created with assignMode != 'external' */
  async notifyNewTask(taskId: string): Promise<void> {
    const event = {
      id: ulid(),
      taskId: 'system',
      index: 0,
      timestamp: Date.now(),
      type: 'taskcast:worker:new-task',
      level: 'info' as const,
      data: taskId,
    }
    await this.opts.broadcast.publish('taskcast:worker:new-task', event)
  }
```

Note: The `onTaskCreated` hook set up in Phase 1 should call `notifyNewTask` when the task's assignMode is not `external`. This wiring happens when constructing the WorkerManager — see the integration in the server factory or in a setup helper.

**Step 3: Add pull endpoint to workers router**

Add to `packages/server/src/routes/workers.ts`:

```typescript
const PullSchema = z.object({
  workerId: z.string(),
  matchRule: z.object({
    taskTypes: z.array(z.string()).optional(),
    tags: z.object({
      all: z.array(z.string()).optional(),
      any: z.array(z.string()).optional(),
      none: z.array(z.string()).optional(),
    }).optional(),
  }).optional(),
  capacity: z.number().int().positive().optional(),
  weight: z.number().int().min(0).max(100).optional(),
})

  router.get('/pull', async (c) => {
    const auth = c.get('auth')
    if (!checkScope(auth, 'worker:connect')) return c.json({ error: 'Forbidden' }, 403)

    const workerId = c.req.query('workerId')
    if (!workerId) return c.json({ error: 'workerId query param required' }, 400)

    // Update weight if provided
    const weight = c.req.query('weight')
    if (weight) await manager.updateWorker(workerId, { weight: Number(weight) })

    // Heartbeat
    await manager.heartbeat(workerId)

    try {
      const controller = new AbortController()
      // Timeout after 30s for long-poll
      const timeout = setTimeout(() => controller.abort(), 30000)
      c.req.raw.signal.addEventListener('abort', () => {
        clearTimeout(timeout)
        controller.abort()
      })
      const task = await manager.waitForTask(workerId, controller.signal)
      clearTimeout(timeout)
      return c.json(task)
    } catch {
      return c.json(null, 204)
    }
  })
```

**Step 4: Run tests**

```bash
cd packages/core && pnpm test -- tests/unit/worker-manager.test.ts
cd packages/server && pnpm test -- tests/workers.test.ts
```

**Step 5: Commit**

```bash
git add packages/core/src/worker-manager.ts packages/core/tests/unit/worker-manager.test.ts packages/server/src/routes/workers.ts packages/server/tests/workers.test.ts
git commit -m "feat: add pull mode with long-poll support"
```

---

## Phase 6: WebSocket Support

### Task 6.1: WebSocket handler for ws-offer and ws-race

**Files:**
- Create: `packages/server/src/routes/worker-ws.ts`
- Modify: `packages/server/src/routes/workers.ts`
- Test: `packages/server/tests/worker-ws.test.ts`

This is the most complex part. The implementation involves:

1. WebSocket upgrade handler on `GET /workers/ws`
2. Message parsing (register, accept, decline, claim, update, drain, pong)
3. Server-initiated messages (offer, available, assigned, claimed, ping, error)
4. Heartbeat ping/pong loop
5. Integration with WorkerManager for dispatch

**Step 1: Write tests**

Create `packages/server/tests/worker-ws.test.ts`. Since Hono's built-in WebSocket testing is limited, test the message handler logic as unit tests using mock WebSocket objects:

```typescript
import { describe, it, expect, vi } from 'vitest'
import { WorkerWSHandler } from '../src/routes/worker-ws.js'
import { TaskEngine, MemoryBroadcastProvider, MemoryShortTermStore, WorkerManager } from '@taskcast/core'

function makeSetup() {
  const store = new MemoryShortTermStore()
  const broadcast = new MemoryBroadcastProvider()
  const engine = new TaskEngine({ shortTerm: store, broadcast })
  const manager = new WorkerManager({ engine, shortTerm: store, broadcast })
  return { store, broadcast, engine, manager }
}

function mockWS() {
  const sent: unknown[] = []
  return {
    send: vi.fn((data: string) => sent.push(JSON.parse(data))),
    close: vi.fn(),
    sent,
  }
}

describe('WorkerWSHandler', () => {
  it('handles register message', async () => {
    const { manager } = makeSetup()
    const ws = mockWS()
    const handler = new WorkerWSHandler(manager, ws as any)
    await handler.handleMessage(JSON.stringify({
      type: 'register',
      matchRule: { taskTypes: ['llm.*'] },
      capacity: 10,
      weight: 50,
    }))
    expect(ws.sent).toHaveLength(1)
    expect(ws.sent[0].type).toBe('registered')
    expect(ws.sent[0].workerId).toBeTruthy()
  })

  it('handles accept message for offered task', async () => {
    const { manager, engine } = makeSetup()
    const ws = mockWS()
    const handler = new WorkerWSHandler(manager, ws as any)
    await handler.handleMessage(JSON.stringify({
      type: 'register',
      matchRule: { taskTypes: ['*'] },
      capacity: 10,
    }))
    const task = await engine.createTask({ type: 'test', assignMode: 'ws-offer' })
    // Simulate offer
    handler.offerTask(task)
    await handler.handleMessage(JSON.stringify({ type: 'accept', taskId: task.id }))
    const msgs = ws.sent.filter((m: any) => m.type === 'assigned')
    expect(msgs).toHaveLength(1)
  })

  it('handles decline message', async () => {
    const { manager, engine } = makeSetup()
    const ws = mockWS()
    const handler = new WorkerWSHandler(manager, ws as any)
    await handler.handleMessage(JSON.stringify({
      type: 'register',
      matchRule: { taskTypes: ['*'] },
      capacity: 10,
    }))
    const task = await engine.createTask({ type: 'test', assignMode: 'ws-offer' })
    handler.offerTask(task)
    await handler.handleMessage(JSON.stringify({ type: 'accept', taskId: task.id }))
    await handler.handleMessage(JSON.stringify({ type: 'decline', taskId: task.id }))
    const msgs = ws.sent.filter((m: any) => m.type === 'declined')
    expect(msgs).toHaveLength(1)
  })

  it('handles update message', async () => {
    const { manager, store } = makeSetup()
    const ws = mockWS()
    const handler = new WorkerWSHandler(manager, ws as any)
    await handler.handleMessage(JSON.stringify({
      type: 'register',
      matchRule: {},
      capacity: 10,
    }))
    const workerId = ws.sent[0].workerId
    await handler.handleMessage(JSON.stringify({ type: 'update', weight: 80 }))
    const worker = await store.getWorker(workerId)
    expect(worker!.weight).toBe(80)
  })

  it('sends error for unknown message type', async () => {
    const { manager } = makeSetup()
    const ws = mockWS()
    const handler = new WorkerWSHandler(manager, ws as any)
    await handler.handleMessage(JSON.stringify({ type: 'unknown' }))
    expect(ws.sent[0].type).toBe('error')
  })
})
```

**Step 2: Implement WorkerWSHandler**

Create `packages/server/src/routes/worker-ws.ts`:

```typescript
import type { Task, WorkerManager } from '@taskcast/core'

interface WSLike {
  send(data: string): void
  close(): void
}

type TaskSummary = {
  id: string
  type?: string
  tags?: string[]
  cost?: number
  params?: Record<string, unknown>
}

function toSummary(task: Task): TaskSummary {
  const s: TaskSummary = { id: task.id }
  if (task.type !== undefined) s.type = task.type
  if (task.tags !== undefined) s.tags = task.tags
  if (task.cost !== undefined) s.cost = task.cost
  if (task.params !== undefined) s.params = task.params
  return s
}

export class WorkerWSHandler {
  private workerId: string | null = null
  private pendingOffers = new Map<string, Task>()

  constructor(
    private manager: WorkerManager,
    private ws: WSLike,
  ) {}

  private send(msg: Record<string, unknown>): void {
    this.ws.send(JSON.stringify(msg))
  }

  async handleMessage(raw: string): Promise<void> {
    let msg: Record<string, unknown>
    try {
      msg = JSON.parse(raw) as Record<string, unknown>
    } catch {
      this.send({ type: 'error', message: 'Invalid JSON' })
      return
    }

    switch (msg.type) {
      case 'register':
        await this.handleRegister(msg)
        break
      case 'update':
        await this.handleUpdate(msg)
        break
      case 'accept':
        await this.handleAccept(msg as { type: string; taskId: string })
        break
      case 'decline':
        await this.handleDecline(msg as { type: string; taskId: string; blacklist?: boolean })
        break
      case 'claim':
        await this.handleClaim(msg as { type: string; taskId: string })
        break
      case 'drain':
        await this.handleDrain()
        break
      case 'pong':
        if (this.workerId) await this.manager.heartbeat(this.workerId)
        break
      default:
        this.send({ type: 'error', message: `Unknown message type: ${String(msg.type)}` })
    }
  }

  private async handleRegister(msg: Record<string, unknown>): Promise<void> {
    const worker = await this.manager.registerWorker({
      id: msg.workerId as string | undefined,
      matchRule: (msg.matchRule as any) ?? {},
      capacity: (msg.capacity as number) ?? 10,
      weight: msg.weight as number | undefined,
      connectionMode: 'websocket',
    })
    this.workerId = worker.id
    this.send({ type: 'registered', workerId: worker.id })
  }

  private async handleUpdate(msg: Record<string, unknown>): Promise<void> {
    if (!this.workerId) {
      this.send({ type: 'error', message: 'Not registered' })
      return
    }
    await this.manager.updateWorker(this.workerId, {
      weight: msg.weight as number | undefined,
      capacity: msg.capacity as number | undefined,
      matchRule: msg.matchRule as any,
    })
  }

  private async handleAccept(msg: { type: string; taskId: string }): Promise<void> {
    if (!this.workerId) {
      this.send({ type: 'error', message: 'Not registered' })
      return
    }
    const result = await this.manager.claimTask(msg.taskId, this.workerId)
    if (result.success) {
      this.pendingOffers.delete(msg.taskId)
      this.send({ type: 'assigned', taskId: msg.taskId })
    } else {
      this.send({ type: 'error', message: result.reason ?? 'Claim failed', taskId: msg.taskId })
    }
  }

  private async handleDecline(msg: { type: string; taskId: string; blacklist?: boolean }): Promise<void> {
    if (!this.workerId) return
    this.pendingOffers.delete(msg.taskId)
    await this.manager.declineTask(msg.taskId, this.workerId, { blacklist: msg.blacklist })
    this.send({ type: 'declined', taskId: msg.taskId })
  }

  private async handleClaim(msg: { type: string; taskId: string }): Promise<void> {
    if (!this.workerId) {
      this.send({ type: 'error', message: 'Not registered' })
      return
    }
    const result = await this.manager.claimTask(msg.taskId, this.workerId)
    this.send({ type: 'claimed', taskId: msg.taskId, success: result.success })
  }

  private async handleDrain(): Promise<void> {
    if (!this.workerId) return
    await this.manager.updateWorker(this.workerId, {})
    // Mark as draining in store
    const worker = await this.manager.getWorker(this.workerId)
    if (worker) {
      worker.status = 'draining'
      // Direct store update — WorkerManager could expose a drain method
    }
  }

  offerTask(task: Task): void {
    this.pendingOffers.set(task.id, task)
    this.send({ type: 'offer', taskId: task.id, task: toSummary(task) })
  }

  broadcastAvailable(task: Task): void {
    this.send({ type: 'available', taskId: task.id, task: toSummary(task) })
  }

  get registeredWorkerId(): string | null {
    return this.workerId
  }

  async handleDisconnect(): Promise<void> {
    if (this.workerId) {
      await this.manager.unregisterWorker(this.workerId)
    }
  }
}
```

**Step 3: Run tests**

```bash
cd packages/server && pnpm test -- tests/worker-ws.test.ts
```

**Step 4: Commit**

```bash
git add packages/server/src/routes/worker-ws.ts packages/server/tests/worker-ws.test.ts
git commit -m "feat(server): add WebSocket message handler for worker connections"
```

---

## Phase 7: Audit Events

### Task 7.1: Emit audit events from WorkerManager

**Files:**
- Modify: `packages/core/src/worker-manager.ts`
- Test: `packages/core/tests/unit/worker-manager.test.ts`

**Step 1: Write failing tests**

Add to worker-manager tests:

```typescript
describe('WorkerManager — audit events', () => {
  it('emits taskcast:audit event on claim', async () => {
    const { manager, engine, store } = makeSetup()
    await manager.registerWorker({ id: 'w1', matchRule: { taskTypes: ['*'] }, capacity: 10, connectionMode: 'websocket' })
    const task = await engine.createTask({ type: 'test', assignMode: 'ws-offer' })
    await manager.claimTask(task.id, 'w1')
    const events = await store.getEvents(task.id)
    const auditEvents = events.filter((e) => e.type === 'taskcast:audit')
    expect(auditEvents.length).toBeGreaterThanOrEqual(1)
    const assignEvent = auditEvents.find((e) => (e.data as any).action === 'assigned')
    expect(assignEvent).toBeTruthy()
    expect((assignEvent!.data as any).workerId).toBe('w1')
  })

  it('emits taskcast:audit event on decline', async () => {
    const { manager, engine, store } = makeSetup()
    await manager.registerWorker({ id: 'w1', matchRule: { taskTypes: ['*'] }, capacity: 10, connectionMode: 'websocket' })
    const task = await engine.createTask({ type: 'test', assignMode: 'ws-offer' })
    await manager.claimTask(task.id, 'w1')
    await manager.declineTask(task.id, 'w1')
    const events = await store.getEvents(task.id)
    const declineEvents = events.filter((e) => (e.data as any)?.action === 'declined')
    expect(declineEvents).toHaveLength(1)
  })

  it('writes worker audit event to longTerm on register', async () => {
    const longTerm = {
      saveTask: vi.fn().mockResolvedValue(undefined),
      getTask: vi.fn().mockResolvedValue(null),
      saveEvent: vi.fn().mockResolvedValue(undefined),
      getEvents: vi.fn().mockResolvedValue([]),
      saveWorkerEvent: vi.fn().mockResolvedValue(undefined),
      getWorkerEvents: vi.fn().mockResolvedValue([]),
    }
    const store = new MemoryShortTermStore()
    const broadcast = new MemoryBroadcastProvider()
    const engine = new TaskEngine({ shortTerm: store, broadcast })
    const manager = new WorkerManager({ engine, shortTerm: store, broadcast, longTerm })
    await manager.registerWorker({ id: 'w1', matchRule: {}, capacity: 10, connectionMode: 'websocket' })
    await Promise.resolve()
    await Promise.resolve()
    expect(longTerm.saveWorkerEvent).toHaveBeenCalledWith(
      expect.objectContaining({ workerId: 'w1', action: 'connected' }),
    )
  })
})
```

**Step 2: Implement audit event emission**

Add helper method to WorkerManager:

```typescript
  private async emitTaskAudit(taskId: string, action: string, data?: Record<string, unknown>): Promise<void> {
    await this.opts.engine.publishEvent(taskId, {
      type: 'taskcast:audit',
      level: 'info',
      data: { action, ...data },
    }).catch(() => {
      // Task may be in terminal state; audit is best-effort
    })
  }

  private emitWorkerAudit(action: WorkerAuditEvent['action'], workerId: string, data?: Record<string, unknown>): void {
    if (!this.opts.longTerm) return
    const event: WorkerAuditEvent = {
      id: ulid(),
      workerId,
      timestamp: Date.now(),
      action,
      ...(data !== undefined && { data }),
    }
    this.opts.longTerm.saveWorkerEvent(event).catch(() => {})
  }
```

Add import for `WorkerAuditEvent` type at top.

Then call these in the appropriate methods:
- `registerWorker`: `this.emitWorkerAudit('connected', worker.id)`
- `unregisterWorker`: `this.emitWorkerAudit('disconnected', workerId, { reason })`
- `claimTask` (on success): `await this.emitTaskAudit(taskId, 'assigned', { workerId })` and `this.emitWorkerAudit('task_assigned', workerId, { taskId })`
- `declineTask`: `await this.emitTaskAudit(taskId, 'declined', { workerId, blacklisted: opts?.blacklist ?? false })` and `this.emitWorkerAudit('task_declined', workerId, { taskId })`

Note: `emitTaskAudit` must handle the case where the task is in `assigned` status — `publishEvent` checks `isTerminal` but `assigned` is not terminal, so this should work. However, after `declineTask` transitions the task back to `pending`, the audit event needs to be emitted BEFORE the transition or use `_emit` directly. Consider emitting the audit event before the state change in `declineTask`.

**Step 3: Run tests**

```bash
cd packages/core && pnpm test -- tests/unit/worker-manager.test.ts
```

**Step 4: Run full suite**

```bash
cd packages/core && pnpm test
```

**Step 5: Commit**

```bash
git add packages/core/src/worker-manager.ts packages/core/tests/unit/worker-manager.test.ts
git commit -m "feat(core): emit task and worker audit events from WorkerManager"
```

---

## Phase 8: Redis Adapter Extensions

### Task 8.1: Add worker methods to RedisShortTermStore

**Files:**
- Modify: `packages/redis/src/short-term.ts`
- Test: `packages/redis/tests/short-term.test.ts`

This task adds all new ShortTermStore methods to the Redis adapter. Key implementation details:

- Worker state: Redis Hash at `taskcast:worker:{id}` (JSON-serialized)
- Worker list: Redis Set at `taskcast:workers` (all worker IDs)
- Assignments by task: Redis Hash at `taskcast:assignment:{taskId}`
- Assignments by worker: Redis Set at `taskcast:worker-assignments:{workerId}`
- `claimTask`: Lua script for atomicity (check task status + worker capacity + update both)
- `listTasks`: Scan all task keys with status filter (NOTE: for production scale, consider a secondary index set like `taskcast:tasks:pending`)

**Step 1: Write integration tests using testcontainers**

Add worker-specific tests to `packages/redis/tests/short-term.test.ts` following the existing pattern (GenericContainer redis:7-alpine, beforeAll/afterAll, flushall in beforeEach).

**Step 2: Implement the methods**

Follow the same patterns as existing Redis code: JSON.stringify/parse for objects, pipeline for multi-key operations, Lua scripts for atomicity.

**Step 3: Run tests**

```bash
cd packages/redis && pnpm test -- tests/short-term.test.ts
```

**Step 4: Commit**

```bash
git add packages/redis/src/short-term.ts packages/redis/tests/short-term.test.ts
git commit -m "feat(redis): add worker and assignment methods to RedisShortTermStore"
```

---

### Task 8.2: Add claimTask Lua script

**Files:**
- Modify: `packages/redis/src/short-term.ts`

The `claimTask` Lua script must atomically:
1. Read task status — if not 'pending', return 0
2. Read worker capacity and usedSlots — if usedSlots + cost > capacity, return 0
3. Update worker usedSlots += cost
4. Return 1

```lua
local taskKey = KEYS[1]
local workerKey = KEYS[2]
local cost = tonumber(ARGV[1])

local taskJson = redis.call('GET', taskKey)
if not taskJson then return 0 end
local task = cjson.decode(taskJson)
if task.status ~= 'pending' then return 0 end

local workerJson = redis.call('GET', workerKey)
if not workerJson then return 0 end
local worker = cjson.decode(workerJson)
if worker.usedSlots + cost > worker.capacity then return 0 end

worker.usedSlots = worker.usedSlots + cost
redis.call('SET', workerKey, cjson.encode(worker))
return 1
```

---

## Phase 9: Postgres Adapter Extensions

### Task 9.1: Add worker audit event methods to PostgresLongTermStore

**Files:**
- Create: `packages/postgres/migrations/002_workers.sql`
- Modify: `packages/postgres/src/long-term.ts`
- Test: `packages/postgres/tests/long-term.test.ts`

**Step 1: Create migration**

```sql
CREATE TABLE IF NOT EXISTS taskcast_worker_events (
  id TEXT PRIMARY KEY,
  worker_id TEXT NOT NULL,
  timestamp BIGINT NOT NULL,
  action TEXT NOT NULL,
  data JSONB,
  created_at TIMESTAMPTZ DEFAULT now()
);

CREATE INDEX idx_worker_events_worker_id ON taskcast_worker_events (worker_id, timestamp DESC);
CREATE INDEX idx_worker_events_action ON taskcast_worker_events (action);
```

**Step 2: Implement saveWorkerEvent and getWorkerEvents**

Follow existing patterns in `packages/postgres/src/long-term.ts` (tagged template SQL, null-check for optional fields).

**Step 3: Write integration tests**

Add tests to `packages/postgres/tests/long-term.test.ts` using existing testcontainer setup. Load both migrations (001 + 002).

**Step 4: Commit**

```bash
git add packages/postgres/migrations/002_workers.sql packages/postgres/src/long-term.ts packages/postgres/tests/long-term.test.ts
git commit -m "feat(postgres): add worker audit event storage"
```

---

## Phase 10: Auth Extensions

### Task 10.1: Add jti and workerId to AuthContext

**Files:**
- Modify: `packages/server/src/auth.ts`
- Test: `packages/server/tests/auth.test.ts`

**Step 1: Write failing test**

```typescript
it('extracts jti and workerId from JWT payload', async () => {
  // Create JWT with jti and workerId claims
  // Verify AuthContext includes them
})
```

**Step 2: Extend AuthContext**

```typescript
export interface AuthContext {
  sub?: string
  jti?: string
  workerId?: string
  taskIds: string[] | '*'
  scope: PermissionScope[]
}
```

**Step 3: Extract jti and workerId in JWT verification**

```typescript
if (payload.jti !== undefined) ctx.jti = payload.jti as string
if (payload['workerId'] !== undefined) ctx.workerId = payload['workerId'] as string
```

**Step 4: Run tests**

```bash
cd packages/server && pnpm test -- tests/auth.test.ts
```

**Step 5: Commit**

```bash
git add packages/server/src/auth.ts packages/server/tests/auth.test.ts
git commit -m "feat(server): extract jti and workerId from JWT tokens"
```

---

## Phase 11: Integration & Full Suite

### Task 11.1: End-to-end integration test

**Files:**
- Create: `packages/core/tests/integration/worker-assignment.test.ts`

Write an integration test that exercises the full flow:
1. Create TaskEngine + WorkerManager with memory adapters
2. Register two workers with different rules
3. Create tasks with different assignModes
4. Verify correct dispatch, claim, decline, re-dispatch
5. Test concurrent claim race condition (10 workers, 1 task)

### Task 11.2: Run all tests across all packages

```bash
pnpm test
pnpm lint
```

Fix any type errors or test failures.

### Task 11.3: Final commit

```bash
git add -A
git commit -m "feat: complete worker assignment system integration"
```

---

## Summary

| Phase | Tasks | Focus |
|-------|-------|-------|
| 1 | 1.1–1.4 | Types, state machine, engine hooks, server schemas |
| 2 | 2.1–2.2 | Worker matching logic, memory adapter extensions |
| 3 | 3.1–3.2 | WorkerManager: registration, dispatch, claim, decline |
| 4 | 4.1 | Server REST routes for workers |
| 5 | 5.1 | Pull mode with long-polling |
| 6 | 6.1 | WebSocket handler |
| 7 | 7.1 | Audit events |
| 8 | 8.1–8.2 | Redis adapter extensions |
| 9 | 9.1 | Postgres migration + adapter |
| 10 | 10.1 | Auth extensions (jti, workerId) |
| 11 | 11.1–11.3 | Integration tests, full suite, final commit |
