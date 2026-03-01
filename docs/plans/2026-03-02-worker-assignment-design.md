# Worker Assignment Design

## Overview

Add an optional task assignment mechanism to Taskcast. Tasks can be externally managed (current behavior) or distributed to workers by the system. Workers connect via long-polling or WebSocket and accept tasks based on configurable rules.

## Assignment Modes

Each task specifies an `assignMode` (with a global default). Four modes:

| Mode | ID | Behavior |
|------|----|----------|
| External | `external` | Current behavior. No worker system. External API calls manage task status. |
| Pull | `pull` | Worker long-polls for matching tasks. Receiving a task = accepting it. Worker can decline. |
| WS-Offer | `ws-offer` | System pushes task to best-matching Worker via WebSocket. Worker must ACK to accept. |
| WS-Race | `ws-race` | System broadcasts task to all matching Workers. First to claim wins. |

## State Machine Extension

```
pending → assigned → running → completed|failed|timeout|cancelled
   ↓         ↓
 cancelled  pending (decline/reclaim)
```

- `pending → assigned`: Worker accepts task (claim/ACK)
- `assigned → running`: Worker starts execution
- `assigned → pending`: Worker declines, task returns to pool
- `pending → running`: **External mode preserves direct transition** for backward compatibility

## Task Type Extensions

```typescript
interface Task {
  // ...existing fields

  tags?: string[]
  assignMode?: 'external' | 'pull' | 'ws-offer' | 'ws-race'
  cost?: number                      // resource slots consumed, default 1
  assignedWorker?: string            // assigned workerId
  disconnectPolicy?: 'reassign' | 'mark' | 'fail'
}
```

## Worker Model

```typescript
interface Worker {
  id: string
  status: 'idle' | 'busy' | 'draining' | 'offline'
  matchRule: WorkerMatchRule
  capacity: number                   // total resource slots
  usedSlots: number                  // currently occupied slots
  weight: number                     // scheduling weight 0-100, default 50
  connectionMode: 'pull' | 'websocket'
  connectedAt: number
  lastHeartbeatAt: number
  metadata?: Record<string, unknown>
}

interface WorkerMatchRule {
  taskTypes?: string[]               // wildcard patterns, e.g. ["llm.*"]
  tags?: TagMatcher
}

interface TagMatcher {
  all?: string[]                     // task must have ALL of these (AND)
  any?: string[]                     // task must have at least ONE of these (OR)
  none?: string[]                    // task must have NONE of these (NOT)
}
```

Match logic: `taskType matches AND all satisfied AND any satisfied AND none satisfied`

### Worker Status Flow

```
(connect) → idle ←→ busy (usedSlots = capacity)
               ↓
            draining (no new tasks, finish existing)
               ↓
            offline (disconnect/timeout)
```

### Capacity / Cost Model

Workers declare a `capacity` (total resource slots). Tasks declare a `cost` (slots consumed, default 1).

Assignment check: `worker.usedSlots + task.cost <= worker.capacity`

Example: A worker with `capacity=100` can run 100 cost=1 tasks, or 10 cost=10 tasks, or any mix.

## Worker Assignment State

Separate from task events — stored in ShortTermStore for atomic operations and high-frequency queries.

```typescript
interface WorkerAssignment {
  taskId: string
  workerId: string
  cost: number
  assignedAt: number
  status: 'offered' | 'assigned' | 'running'
}
```

## Protocol Details

### Pull Mode

```
Worker                          Server
  │                               │
  ├── GET /workers/pull ──────────┤  (matchRule, capacity, weight, workerId)
  │   (long-poll hold)            │
  │                               ├── matching pending task?
  │                               │   yes → assign, pending→assigned
  │   ◄── 200 { task } ──────────┤
  │                               │
  │   (decline if needed)         │
  ├── POST /tasks/:id/decline ───┤  → assigned→pending, optional blacklist
  │                               │
  │   (start execution)           │
  ├── PATCH /tasks/:id/status ───┤  { status: "running" }
  │                               │
  │   (complete)                  │
  ├── PATCH /tasks/:id/status ───┤  { status: "completed"|"failed" }
```

Worker can pass `weight` in each pull request to dynamically adjust scheduling priority.

### WS-Offer Mode

```
Worker                          Server
  │                               │
  ├── ws connect ─────────────────┤
  ├── { type: "register", ... }   │
  │   ◄── { type: "registered",  │
  │         workerId }            │
  │                               │
  │   ◄── { type: "offer",       │  system selects best worker
  │         taskId, task }        │
  │                               │
  ├── { type: "accept", taskId } ─┤  or { type: "decline", taskId, blacklist? }
  │   ◄── { type: "assigned",    │  confirmation ACK
  │         taskId }              │
  │                               │
  │   (update weight anytime)     │
  ├── { type: "update",          │
  │     weight: 80 }              │
```

Offer worker selection priority: `highest weight → most available slots → earliest connected`

If worker doesn't respond to offer (timeout), offer passes to next candidate.

### WS-Race Mode

```
Worker A, B, C                  Server
  │                               │
  │── ws connect + register ──────┤
  │                               │
  │   ◄── { type: "available",   │  broadcast to all matching workers
  │         taskId, task }        │
  │                               │
  A── { type: "claim", taskId } ──┤
  B── { type: "claim", taskId } ──┤  (near-simultaneous)
  │                               │
  │                               ├── atomic: A wins
  A  ◄── { type: "claimed",      │
  │       taskId, success: true } │
  B  ◄── { type: "claimed",      │
  │       taskId, success: false }│
```

### WebSocket Message Protocol

```typescript
// Client → Server
type WorkerMessage =
  | { type: 'register'; matchRule: WorkerMatchRule; capacity: number;
      weight?: number; workerId?: string }
  | { type: 'update'; weight?: number; capacity?: number; matchRule?: WorkerMatchRule }
  | { type: 'accept'; taskId: string }
  | { type: 'decline'; taskId: string; blacklist?: boolean }
  | { type: 'claim'; taskId: string }
  | { type: 'drain' }
  | { type: 'pong' }

// Server → Client
type ServerMessage =
  | { type: 'registered'; workerId: string }
  | { type: 'offer'; taskId: string; task: TaskSummary }
  | { type: 'available'; taskId: string; task: TaskSummary }
  | { type: 'assigned'; taskId: string }
  | { type: 'claimed'; taskId: string; success: boolean }
  | { type: 'declined'; taskId: string }
  | { type: 'revoked'; taskId: string; reason: string }
  | { type: 'ping' }
  | { type: 'error'; message: string; code?: string }
```

`TaskSummary` is a lightweight view of Task (id, type, tags, cost, params) to minimize wire overhead. Workers can fetch full task via REST if needed.

### Decline & Blacklist

Universal across all modes:
- `decline(taskId, { blacklist?: boolean })`: decline task, status → pending
- If `blacklist=true`, worker ID is added to task's exclusion list
- Blacklist stored in task metadata: `task.metadata._blacklistedWorkers: string[]`
- Future assignments skip blacklisted workers

## Heartbeat & Disconnect

- Heartbeat interval and timeout are globally configurable (default 30s interval, 90s timeout)
- Pull mode: each pull request auto-renews heartbeat
- WebSocket mode: server sends `ping`, worker replies `pong`

### Disconnect Policies

Configurable per task or task type:

| Policy | ID | Behavior |
|--------|----|----------|
| Reassign | `reassign` | Wait grace period, then revert to pending and reassign |
| Mark only | `mark` | Mark worker disconnected, wait for external intervention |
| Fail | `fail` | Mark task as failed with worker disconnect error |

Default: `reassign` with 30s grace period.

## Authentication

### Scopes

New permission scopes:

| Scope | Purpose |
|-------|---------|
| `worker:connect` | Connect as a worker (pull or websocket) |
| `worker:manage` | Admin operations (list workers, force disconnect, reclaim tasks) |

### Worker Identity

Worker ID resolution priority:
1. `jwt.workerId` — JWT payload specifies workerId (controlled environments)
2. `registration.workerId` — Worker self-declares during registration
3. Auto-generated ULID

If JWT specifies `workerId`, a mismatched registration ID is rejected.

### JWT Token ID

`AuthContext` extracts the standard `jti` (JWT ID) claim for future token revocation support:

```typescript
interface AuthContext {
  sub?: string
  jti?: string              // JWT unique ID for revocation
  workerId?: string          // from JWT or registration
  taskIds: string[] | '*'
  scope: PermissionScope[]
}
```

### WebSocket Auth

Connection-time authentication: token in WebSocket upgrade request (query parameter or header), validated before upgrade. No re-validation during connection lifetime. Token refresh is a future enhancement.

## Audit System

### Task Audit Events

Task lifecycle changes emit `taskcast:audit` events through the existing event system:

```typescript
// Reuses TaskEvent — inherits id, taskId, index, timestamp
{
  type: 'taskcast:audit',
  level: 'info',
  data: {
    action: 'created' | 'assigned' | 'declined' | 'reassigned'
           | 'status_changed' | 'worker_disconnected' | 'reclaimed',
    from?: TaskStatus,
    to?: TaskStatus,
    workerId?: string,
    reason?: string,
    metadata?: Record<string, unknown>
  }
}
```

Flows through existing write path: ShortTermStore (sync) → Broadcast (sync) → LongTermStore (async). SSE subscribers see audit events (filterable by `types=taskcast:audit`).

### Worker Audit Events

Separate from task events — workers are a different domain entity.

```typescript
interface WorkerAuditEvent {
  id: string
  workerId: string
  timestamp: number
  action: 'connected' | 'disconnected' | 'updated' | 'task_assigned'
         | 'task_declined' | 'task_reclaimed' | 'draining' | 'heartbeat_timeout'
         | 'pull_request'   // worker attempted a pull (matched or not)
  data?: Record<string, unknown>
}
```

Stored in LongTermStore only (async, non-blocking). Volume is low — connect/disconnect events are infrequent for long-lived workers.

LongTermStore extension:

```typescript
interface LongTermStore {
  // ...existing

  saveWorkerEvent(event: WorkerAuditEvent): Promise<void>
  getWorkerEvents(workerId: string, opts?: EventQueryOptions): Promise<WorkerAuditEvent[]>
}
```

Retention managed through existing cleanup mechanism (e.g., 30-day retention).

## Core Layer Architecture

### WorkerManager (packages/core)

New class in `@taskcast/core`, peer to `TaskEngine`, sharing storage adapters:

```typescript
interface WorkerManagerOptions {
  engine: TaskEngine
  shortTerm: ShortTermStore
  broadcast: BroadcastProvider
  longTerm?: LongTermStore
  hooks?: TaskcastHooks
  defaults?: {
    assignMode?: AssignMode
    heartbeatIntervalMs?: number      // default 30000
    heartbeatTimeoutMs?: number       // default 90000
    offerTimeoutMs?: number           // default 10000
    disconnectPolicy?: DisconnectPolicy
    disconnectGraceMs?: number        // default 30000
  }
}

class WorkerManager {
  registerWorker(config: WorkerRegistration): Worker
  unregisterWorker(workerId: string): void
  updateWorker(workerId: string, update: Partial<WorkerUpdate>): void
  heartbeat(workerId: string): void

  dispatchTask(taskId: string): DispatchResult
  claimTask(taskId: string, workerId: string): ClaimResult
  declineTask(taskId: string, workerId: string, opts?: DeclineOptions): void

  waitForTask(workerId: string, signal?: AbortSignal): Promise<Task>

  getWorker(workerId: string): Worker | null
  listWorkers(filter?: WorkerFilter): Worker[]
  getWorkerTasks(workerId: string): Task[]
}
```

WorkerManager holds **no connections** — it is pure logic. WebSocket/HTTP connection management lives in the server layer.

### TaskEngine Changes

```typescript
class TaskEngine {
  async createTask(input): Promise<Task> {
    // ...existing logic
    this.opts.hooks?.onTaskCreated?.(task)      // NEW
    return task
  }

  async transitionTask(taskId, to, payload?): Promise<Task> {
    // ...existing logic
    this.opts.hooks?.onTaskTransitioned?.(task, from, to)  // NEW
    return task
  }

  async listTasks(filter: TaskFilter): Promise<Task[]>     // NEW
}
```

### Storage Interface Extensions

#### ShortTermStore

```typescript
interface ShortTermStore {
  // ...existing

  // Task query
  listTasks(filter: TaskFilter): Promise<Task[]>

  // Worker state
  saveWorker(worker: Worker): Promise<void>
  getWorker(workerId: string): Promise<Worker | null>
  listWorkers(filter?: WorkerFilter): Promise<Worker[]>
  deleteWorker(workerId: string): Promise<void>

  // Atomic claim (Redis: Lua script; Memory: synchronous)
  claimTask(taskId: string, workerId: string, cost: number): Promise<boolean>

  // Worker assignments
  addAssignment(assignment: WorkerAssignment): Promise<void>
  removeAssignment(taskId: string): Promise<void>
  getWorkerAssignments(workerId: string): Promise<WorkerAssignment[]>
  getTaskAssignment(taskId: string): Promise<WorkerAssignment | null>
}
```

#### LongTermStore

```typescript
interface LongTermStore {
  // ...existing

  saveWorkerEvent(event: WorkerAuditEvent): Promise<void>
  getWorkerEvents(workerId: string, opts?: EventQueryOptions): Promise<WorkerAuditEvent[]>
}
```

### Hooks Extension

```typescript
interface TaskcastHooks {
  // ...existing

  onTaskCreated?(task: Task): void
  onTaskTransitioned?(task: Task, from: TaskStatus, to: TaskStatus): void
  onWorkerConnected?(worker: Worker): void
  onWorkerDisconnected?(worker: Worker, reason: string): void
  onTaskAssigned?(task: Task, worker: Worker): void
  onTaskDeclined?(task: Task, worker: Worker, blacklisted: boolean): void
}
```

## Server Layer (packages/server)

### New Routes

```
GET    /workers              → List all workers (worker:manage)
GET    /workers/:workerId    → Get single worker
DELETE /workers/:workerId    → Force disconnect (worker:manage)

GET    /workers/pull         → Long-poll for task (worker:connect)
GET    /workers/ws           → WebSocket upgrade (worker:connect)

POST   /tasks/:taskId/decline → Decline task (worker:connect, own tasks only)
```

### Server Factory Extension

```typescript
interface TaskcastServerOptions {
  engine: TaskEngine
  workerManager?: WorkerManager      // optional — omit to disable worker system
  auth?: AuthConfig
}
```

## Configuration

```typescript
interface TaskcastConfig {
  // ...existing

  workers?: {
    enabled?: boolean
    defaults?: {
      assignMode?: 'external' | 'pull' | 'ws-offer' | 'ws-race'
      heartbeatIntervalMs?: number
      heartbeatTimeoutMs?: number
      offerTimeoutMs?: number
      disconnectPolicy?: 'reassign' | 'mark' | 'fail'
      disconnectGraceMs?: number
    }
  }
}
```

## Backward Compatibility

- Worker system is entirely opt-in. If `workerManager` is not provided to the server, all worker routes return 404.
- Tasks without `assignMode` default to `external` (unless global default is configured).
- `pending → running` direct transition remains valid for external mode.
- Existing SSE, REST, webhook behavior is unchanged.
- Storage adapters that don't implement new methods throw "not implemented" — existing adapters work until worker features are used.