# Swagger / OpenAPI Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Add OpenAPI 3.1 spec generation and Swagger UI to both the TypeScript (Hono) and Rust (Axum) server implementations.

**Architecture:** Both sides generate OpenAPI specs from code — TS via `@hono/zod-openapi` (reuses existing Zod schemas), Rust via `utoipa` derive macros. Each side serves `/openapi.json` (spec) and `/docs` (interactive UI). The two implementations are independent; no shared spec file.

**Tech Stack:** TS: `@hono/zod-openapi`, `@scalar/hono-api-reference`. Rust: `utoipa`, `utoipa-axum`, `utoipa-scalar` (or `utoipa-swagger-ui`).

---

## Phase 1: TypeScript — OpenAPI Integration

### Task 1: Install TS dependencies

**Files:**
- Modify: `packages/server/package.json`

**Step 1: Install packages**

Run:
```bash
cd packages/server && pnpm add @hono/zod-openapi @scalar/hono-api-reference
```

**Step 2: Verify installation**

Run: `cd packages/server && pnpm list @hono/zod-openapi @scalar/hono-api-reference`
Expected: Both packages listed with versions

**Step 3: Commit**

```bash
git add packages/server/package.json pnpm-lock.yaml
git commit -m "feat(server): add @hono/zod-openapi and @scalar/hono-api-reference deps"
```

---

### Task 2: Extract Zod schemas into shared schemas file

Currently, Zod schemas are defined inline inside route handlers. Extract them into a shared file so both route handlers and OpenAPI route definitions can reference them.

**Files:**
- Create: `packages/server/src/schemas.ts`
- Modify: `packages/server/src/routes/tasks.ts` (remove inline schemas, import from schemas.ts)

**Step 1: Create `packages/server/src/schemas.ts`**

Extract and define all request/response schemas here. Include OpenAPI metadata (`.openapi()` calls for descriptions, examples):

```typescript
import { z } from '@hono/zod-openapi'

// ─── Shared Enums ─────────────────────────────────────────────────────────

export const TaskStatusSchema = z.enum([
  'pending', 'assigned', 'running', 'paused', 'blocked',
  'completed', 'failed', 'timeout', 'cancelled',
])

export const LevelSchema = z.enum(['debug', 'info', 'warn', 'error'])
export const SeriesModeSchema = z.enum(['keep-all', 'accumulate', 'latest'])
export const AssignModeSchema = z.enum(['external', 'pull', 'ws-offer', 'ws-race'])
export const DisconnectPolicySchema = z.enum(['reassign', 'mark', 'fail'])

// ─── Task Error ───────────────────────────────────────────────────────────

export const TaskErrorSchema = z.object({
  code: z.string().optional(),
  message: z.string(),
  details: z.record(z.unknown()).optional(),
}).openapi('TaskError')

// ─── Task ─────────────────────────────────────────────────────────────────

export const TaskSchema = z.object({
  id: z.string(),
  type: z.string().optional(),
  status: TaskStatusSchema,
  params: z.record(z.unknown()).optional(),
  result: z.record(z.unknown()).optional(),
  error: TaskErrorSchema.optional(),
  metadata: z.record(z.unknown()).optional(),
  createdAt: z.number(),
  updatedAt: z.number(),
  completedAt: z.number().optional(),
  ttl: z.number().optional(),
  tags: z.array(z.string()).optional(),
  assignMode: AssignModeSchema.optional(),
  cost: z.number().optional(),
  assignedWorker: z.string().optional(),
  disconnectPolicy: DisconnectPolicySchema.optional(),
}).openapi('Task')

// ─── Task Event ───────────────────────────────────────────────────────────

export const TaskEventSchema = z.object({
  id: z.string(),
  taskId: z.string(),
  index: z.number(),
  timestamp: z.number(),
  type: z.string(),
  level: LevelSchema,
  data: z.unknown(),
  seriesId: z.string().optional(),
  seriesMode: SeriesModeSchema.optional(),
}).openapi('TaskEvent')

// ─── Worker ───────────────────────────────────────────────────────────────

export const WorkerSchema = z.object({
  id: z.string(),
  status: z.enum(['idle', 'busy', 'draining', 'offline']),
  capacity: z.number(),
  usedSlots: z.number(),
  weight: z.number(),
  connectionMode: z.enum(['pull', 'websocket']),
  connectedAt: z.number(),
  lastHeartbeatAt: z.number(),
  metadata: z.record(z.unknown()).optional(),
}).openapi('Worker')

// ─── Request Bodies ───────────────────────────────────────────────────────

export const CreateTaskSchema = z.object({
  id: z.string().optional(),
  type: z.string().optional(),
  params: z.record(z.unknown()).optional(),
  metadata: z.record(z.unknown()).optional(),
  ttl: z.number().int().positive().optional(),
  webhooks: z.array(z.unknown()).optional(),
  cleanup: z.object({ rules: z.array(z.unknown()) }).optional(),
  tags: z.array(z.string()).optional(),
  assignMode: AssignModeSchema.optional(),
  cost: z.number().int().positive().optional(),
  disconnectPolicy: DisconnectPolicySchema.optional(),
}).openapi('CreateTaskInput')

export const TransitionSchema = z.object({
  status: TaskStatusSchema,
  result: z.record(z.unknown()).optional(),
  error: TaskErrorSchema.optional(),
  reason: z.string().optional(),
  ttl: z.number().int().positive().optional(),
  resumeAfterMs: z.number().int().positive().optional(),
}).openapi('TransitionInput')

export const PublishEventSchema = z.object({
  type: z.string(),
  level: LevelSchema,
  data: z.unknown(),
  seriesId: z.string().optional(),
  seriesMode: SeriesModeSchema.optional(),
}).openapi('PublishEventInput')

export const DeclineSchema = z.object({
  workerId: z.string(),
  blacklist: z.boolean().optional(),
}).openapi('DeclineInput')

// ─── Error Response ───────────────────────────────────────────────────────

export const ErrorSchema = z.object({
  error: z.string(),
}).openapi('Error')
```

**Step 2: Update tasks.ts to import from schemas.ts**

Remove the inline `CreateTaskSchema`, `PublishEventSchema`, and transition schema definitions. Import from `../schemas.js`.

**Step 3: Update workers.ts to import DeclineSchema**

Remove inline `DeclineSchema`, import from `../schemas.js`.

**Step 4: Run tests to verify nothing broke**

Run: `cd packages/server && pnpm test`
Expected: All existing tests pass

**Step 5: Commit**

```bash
git add packages/server/src/schemas.ts packages/server/src/routes/tasks.ts packages/server/src/routes/workers.ts
git commit -m "refactor(server): extract Zod schemas into shared schemas.ts"
```

---

### Task 3: Convert app factory to OpenAPIHono

**Files:**
- Modify: `packages/server/src/index.ts`

**Step 1: Replace Hono with OpenAPIHono**

Change the app factory to use `OpenAPIHono` instead of `Hono`. The rest of the route mounting stays the same — `OpenAPIHono` extends `Hono`, so `.route()`, `.get()`, `.use()` all still work.

```typescript
import { OpenAPIHono } from '@hono/zod-openapi'
// ... keep all other imports

export function createTaskcastApp(opts: TaskcastServerOptions): OpenAPIHono {
  const app = new OpenAPIHono()
  // ... health route stays as-is
  // ... auth middleware stays as-is
  // ... route mounting stays as-is

  // Add OpenAPI spec endpoint
  app.doc('/openapi.json', {
    openapi: '3.1.0',
    info: {
      title: 'Taskcast API',
      version: '0.3.0',
      description: 'Unified long-lifecycle task tracking service for LLM streaming, agents, and async workloads.',
    },
    security: [{ Bearer: [] }],
  })

  // Add Scalar UI
  app.get('/docs', apiReference({
    spec: { url: '/openapi.json' },
  }))

  return app
}
```

Import `apiReference` from `@scalar/hono-api-reference`.

**Step 2: Run tests**

Run: `cd packages/server && pnpm test`
Expected: All existing tests pass (OpenAPIHono is backward compatible)

**Step 3: Commit**

```bash
git add packages/server/src/index.ts
git commit -m "feat(server): switch to OpenAPIHono, add /openapi.json and /docs endpoints"
```

---

### Task 4: Convert task routes to OpenAPI routes

**Files:**
- Modify: `packages/server/src/routes/tasks.ts`

**Step 1: Convert each route handler**

Replace `router.post('/', handler)` with `createRoute()` + `router.openapi(route, handler)` for each of the 5 task routes:

1. `POST /` → create task
2. `GET /:taskId` → get task
3. `PATCH /:taskId/status` → transition
4. `POST /:taskId/events` → publish events
5. `GET /:taskId/events/history` → event history

For each route, define the route spec using `createRoute()`:

```typescript
import { createRoute, OpenAPIHono, z } from '@hono/zod-openapi'
import {
  CreateTaskSchema, TransitionSchema, PublishEventSchema,
  TaskSchema, TaskEventSchema, ErrorSchema,
} from '../schemas.js'

const createTaskRoute = createRoute({
  method: 'post',
  path: '/',
  tags: ['Tasks'],
  summary: 'Create a new task',
  security: [{ Bearer: [] }],
  request: {
    body: { content: { 'application/json': { schema: CreateTaskSchema } } },
  },
  responses: {
    201: { description: 'Task created', content: { 'application/json': { schema: TaskSchema } } },
    400: { description: 'Validation error', content: { 'application/json': { schema: ErrorSchema } } },
    403: { description: 'Forbidden', content: { 'application/json': { schema: ErrorSchema } } },
  },
})
```

The handler function signature changes slightly — `c` gets typed validation, but you can keep the existing logic. If the existing handler does manual `.json()` parsing + Zod safeParse, you can simplify since `@hono/zod-openapi` validates automatically. However, to minimize risk, keep the existing validation logic and just wrap the route registration.

**Important:** The router must change from `new Hono()` to `new OpenAPIHono()`.

**Step 2: Run tests**

Run: `cd packages/server && pnpm test`
Expected: All existing task tests pass

**Step 3: Commit**

```bash
git add packages/server/src/routes/tasks.ts
git commit -m "feat(server): convert task routes to OpenAPI route definitions"
```

---

### Task 5: Convert SSE route to OpenAPI route

**Files:**
- Modify: `packages/server/src/routes/sse.ts`

SSE streaming doesn't fit neatly into OpenAPI request/response. But we can still document it as a GET endpoint that returns `text/event-stream`. The handler stays as-is (streamSSE).

**Step 1: Add OpenAPI route definition for SSE**

```typescript
import { createRoute, OpenAPIHono } from '@hono/zod-openapi'

const sseRoute = createRoute({
  method: 'get',
  path: '/:taskId/events',
  tags: ['Events'],
  summary: 'Subscribe to task events via SSE',
  description: 'Server-Sent Events stream. Replays history then streams live events. Closes on terminal status.',
  security: [{ Bearer: [] }],
  request: {
    params: z.object({ taskId: z.string() }),
    query: z.object({
      types: z.string().optional().openapi({ description: 'Comma-separated type filter (wildcard support)' }),
      levels: z.string().optional().openapi({ description: 'Comma-separated level filter' }),
      includeStatus: z.string().optional().openapi({ description: 'Include taskcast:status events (default: true)' }),
      wrap: z.string().optional().openapi({ description: 'Wrap in SSEEnvelope (default: true)' }),
      'since.id': z.string().optional(),
      'since.index': z.string().optional(),
      'since.timestamp': z.string().optional(),
    }),
  },
  responses: {
    200: { description: 'SSE event stream (text/event-stream)' },
    403: { description: 'Forbidden' },
    404: { description: 'Task not found' },
  },
})
```

The handler stays as-is but is registered with `router.openapi(sseRoute, handler)`.

**Step 2: Run tests**

Run: `cd packages/server && pnpm test`
Expected: SSE tests pass

**Step 3: Commit**

```bash
git add packages/server/src/routes/sse.ts
git commit -m "feat(server): convert SSE route to OpenAPI route definition"
```

---

### Task 6: Convert worker routes to OpenAPI routes

**Files:**
- Modify: `packages/server/src/routes/workers.ts`

**Step 1: Convert all 5 worker routes**

Same pattern as tasks: define `createRoute()` for each, register with `router.openapi()`.

Routes:
1. `GET /` → list workers (tag: Workers)
2. `GET /pull` → long-poll for task (tag: Workers)
3. `GET /:workerId` → get worker (tag: Workers)
4. `DELETE /:workerId` → delete worker (tag: Workers)
5. `POST /tasks/:taskId/decline` → decline task (tag: Workers)

**Step 2: Run tests**

Run: `cd packages/server && pnpm test`
Expected: Worker tests pass

**Step 3: Commit**

```bash
git add packages/server/src/routes/workers.ts
git commit -m "feat(server): convert worker routes to OpenAPI route definitions"
```

---

### Task 7: Write TS OpenAPI smoke test

**Files:**
- Create: `packages/server/tests/openapi.test.ts`

**Step 1: Write the test**

```typescript
import { describe, it, expect, beforeAll } from 'vitest'
import { createTaskcastApp } from '../src/index.js'
import { TaskEngine, createMemoryBroadcast, createMemoryShortTermStore } from '@taskcast/core'

describe('OpenAPI', () => {
  let app: ReturnType<typeof createTaskcastApp>

  beforeAll(async () => {
    const engine = new TaskEngine({
      broadcast: createMemoryBroadcast(),
      shortTermStore: createMemoryShortTermStore(),
    })
    app = createTaskcastApp({ engine, auth: { mode: 'none' } })
  })

  it('GET /openapi.json returns valid OpenAPI spec', async () => {
    const res = await app.request('/openapi.json')
    expect(res.status).toBe(200)
    const spec = await res.json()
    expect(spec.openapi).toBe('3.1.0')
    expect(spec.info.title).toBe('Taskcast API')
    expect(spec.paths).toBeDefined()
    // Check key paths exist
    expect(spec.paths['/tasks']).toBeDefined()
    expect(spec.paths['/tasks/{taskId}']).toBeDefined()
    expect(spec.paths['/health']).toBeDefined()
  })

  it('GET /docs returns HTML', async () => {
    const res = await app.request('/docs')
    expect(res.status).toBe(200)
    const ct = res.headers.get('content-type')
    expect(ct).toContain('text/html')
  })
})
```

**Step 2: Run test to verify it fails**

Run: `cd packages/server && pnpm test openapi`
Expected: Tests should pass if previous tasks completed correctly. If not, debug.

**Step 3: Commit**

```bash
git add packages/server/tests/openapi.test.ts
git commit -m "test(server): add OpenAPI spec and docs smoke tests"
```

---

### Task 8: Re-export OpenAPIHono type and update package exports

**Files:**
- Modify: `packages/server/src/index.ts`

**Step 1: Update exports**

Ensure `createTaskcastApp` return type is properly exported and the schemas are also available for consumers:

```typescript
export { TaskSchema, TaskEventSchema, WorkerSchema } from './schemas.js'
```

**Step 2: Build and verify**

Run: `cd packages/server && pnpm build`
Expected: Clean build with no errors

**Step 3: Commit**

```bash
git add packages/server/src/index.ts
git commit -m "feat(server): export OpenAPI schemas for consumers"
```

---

## Phase 2: Rust — OpenAPI Integration

### Task 9: Add Rust utoipa dependencies

**Files:**
- Modify: `rust/taskcast-server/Cargo.toml`
- Modify: `rust/taskcast-core/Cargo.toml`

**Step 1: Add utoipa to taskcast-core**

Core types need `ToSchema` derive, so utoipa must be a dependency of `taskcast-core`:

```toml
# In rust/taskcast-core/Cargo.toml [dependencies]
utoipa = { version = "5", features = ["preserve_order"] }
```

**Step 2: Add utoipa + UI to taskcast-server**

```toml
# In rust/taskcast-server/Cargo.toml [dependencies]
utoipa = { version = "5", features = ["axum_extras", "preserve_order"] }
utoipa-axum = "0.2"
utoipa-scalar = { version = "0.3", features = ["axum"] }
```

**Step 3: Build to verify**

Run: `cd rust && cargo build -p taskcast-server`
Expected: Compiles successfully

**Step 4: Commit**

```bash
git add rust/taskcast-core/Cargo.toml rust/taskcast-server/Cargo.toml Cargo.lock
git commit -m "feat(rust): add utoipa dependencies for OpenAPI support"
```

---

### Task 10: Add ToSchema derives to core types

**Files:**
- Modify: `rust/taskcast-core/src/types.rs`

**Step 1: Add `#[derive(utoipa::ToSchema)]` to key types**

Add the derive macro to these types (they already have `Serialize`/`Deserialize`):
- `TaskStatus`
- `TaskError`
- `Task`
- `Level`
- `SeriesMode`
- `TaskEvent`
- `SSEEnvelope`
- `AssignMode`
- `DisconnectPolicy`
- `Worker`, `WorkerStatus`, `WorkerMatchRule`, `ConnectionMode`
- `WebhookConfig`, `RetryConfig`, `BackoffStrategy`
- `CleanupConfig`, `CleanupRule`, `CleanupRuleMatch`, `CleanupTarget`, `CleanupTrigger`, `CleanupEventFilter`
- `TaskAuthConfig`, `TaskAuthRule`, `TaskAuthRuleMatch`, `TaskAuthRuleRequire`
- `PermissionScope`
- `SinceCursor`, `SubscribeFilter`, `EventQueryOptions`
- `TagMatcher`

Example for Task:
```rust
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Task {
    // ... fields unchanged
}
```

For enums with custom serde like `#[serde(rename_all = "kebab-case")]`, utoipa handles this automatically.

**Step 2: Build to verify**

Run: `cd rust && cargo build -p taskcast-core`
Expected: Compiles with no errors

**Step 3: Commit**

```bash
git add rust/taskcast-core/src/types.rs
git commit -m "feat(rust/core): add ToSchema derives to all API types"
```

---

### Task 11: Add ToSchema to server request/response types

**Files:**
- Modify: `rust/taskcast-server/src/routes/tasks.rs`
- Modify: `rust/taskcast-server/src/routes/sse.rs`
- Modify: `rust/taskcast-server/src/routes/workers.rs`

**Step 1: Add derives to request body types**

In `tasks.rs`:
```rust
#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct CreateTaskBody { ... }

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct TransitionBody { ... }

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct TaskErrorBody { ... }

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct PublishEventBody { ... }
```

In `sse.rs` — add `IntoParams` to query struct:
```rust
#[derive(Debug, Deserialize, utoipa::IntoParams)]
pub struct SseQuery { ... }
```

In `workers.rs`:
```rust
#[derive(Debug, Deserialize, utoipa::IntoParams)]
pub struct PullQuery { ... }

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct DeclineBody { ... }
```

Also in `tasks.rs`:
```rust
#[derive(Debug, Deserialize, utoipa::IntoParams)]
pub struct HistoryQuery { ... }
```

**Step 2: Build**

Run: `cd rust && cargo build -p taskcast-server`

**Step 3: Commit**

```bash
git add rust/taskcast-server/src/routes/
git commit -m "feat(rust/server): add ToSchema/IntoParams to request types"
```

---

### Task 12: Add utoipa::path macros to task handlers

**Files:**
- Modify: `rust/taskcast-server/src/routes/tasks.rs`

**Step 1: Annotate each handler**

Add `#[utoipa::path(...)]` above each handler function:

```rust
#[utoipa::path(
    post,
    path = "/tasks",
    tag = "Tasks",
    summary = "Create a new task",
    security(("Bearer" = [])),
    request_body = CreateTaskBody,
    responses(
        (status = 201, description = "Task created", body = Task),
        (status = 400, description = "Validation error"),
        (status = 403, description = "Forbidden"),
    )
)]
pub async fn create_task(...) -> ... { ... }

#[utoipa::path(
    get,
    path = "/tasks/{task_id}",
    tag = "Tasks",
    summary = "Get task by ID",
    security(("Bearer" = [])),
    params(("task_id" = String, Path, description = "Task ID")),
    responses(
        (status = 200, description = "Task details", body = Task),
        (status = 404, description = "Task not found"),
        (status = 403, description = "Forbidden"),
    )
)]
pub async fn get_task(...) -> ... { ... }

#[utoipa::path(
    patch,
    path = "/tasks/{task_id}/status",
    tag = "Tasks",
    summary = "Transition task status",
    security(("Bearer" = [])),
    params(("task_id" = String, Path, description = "Task ID")),
    request_body = TransitionBody,
    responses(
        (status = 200, description = "Updated task", body = Task),
        (status = 400, description = "Invalid transition"),
        (status = 404, description = "Task not found"),
        (status = 403, description = "Forbidden"),
    )
)]
pub async fn transition_task(...) -> ... { ... }

#[utoipa::path(
    post,
    path = "/tasks/{task_id}/events",
    tag = "Events",
    summary = "Publish events to a task",
    security(("Bearer" = [])),
    params(("task_id" = String, Path, description = "Task ID")),
    responses(
        (status = 201, description = "Events published"),
        (status = 400, description = "Validation error"),
        (status = 404, description = "Task not found"),
        (status = 403, description = "Forbidden"),
    )
)]
pub async fn publish_events(...) -> ... { ... }

#[utoipa::path(
    get,
    path = "/tasks/{task_id}/events/history",
    tag = "Events",
    summary = "Query event history",
    security(("Bearer" = [])),
    params(("task_id" = String, Path, description = "Task ID"), HistoryQuery),
    responses(
        (status = 200, description = "Event list", body = Vec<TaskEvent>),
        (status = 404, description = "Task not found"),
        (status = 403, description = "Forbidden"),
    )
)]
pub async fn get_event_history(...) -> ... { ... }
```

**Step 2: Build**

Run: `cd rust && cargo build -p taskcast-server`

**Step 3: Commit**

```bash
git add rust/taskcast-server/src/routes/tasks.ts
git commit -m "feat(rust/server): add utoipa::path annotations to task handlers"
```

---

### Task 13: Add utoipa::path macros to SSE and worker handlers

**Files:**
- Modify: `rust/taskcast-server/src/routes/sse.rs`
- Modify: `rust/taskcast-server/src/routes/workers.rs`

**Step 1: Annotate SSE handler**

```rust
#[utoipa::path(
    get,
    path = "/tasks/{task_id}/events",
    tag = "Events",
    summary = "Subscribe to task events via SSE",
    description = "Server-Sent Events stream. Replays history then streams live events.",
    security(("Bearer" = [])),
    params(("task_id" = String, Path, description = "Task ID"), SseQuery),
    responses(
        (status = 200, description = "SSE event stream (text/event-stream)"),
        (status = 404, description = "Task not found"),
        (status = 403, description = "Forbidden"),
    )
)]
pub async fn sse_events(...) -> ... { ... }
```

**Step 2: Annotate all 5 worker handlers**

```rust
#[utoipa::path(get, path = "/workers", tag = "Workers", summary = "List all workers",
    security(("Bearer" = [])),
    responses((status = 200, description = "Worker list", body = Vec<Worker>), (status = 403, description = "Forbidden")))]
pub async fn list_workers(...) { ... }

#[utoipa::path(get, path = "/workers/pull", tag = "Workers", summary = "Long-poll for task assignment",
    security(("Bearer" = [])), params(PullQuery),
    responses((status = 200, description = "Task assigned"), (status = 204, description = "Timeout, no task"), (status = 403, description = "Forbidden")))]
pub async fn pull_task(...) { ... }

#[utoipa::path(get, path = "/workers/{worker_id}", tag = "Workers", summary = "Get worker by ID",
    security(("Bearer" = [])), params(("worker_id" = String, Path, description = "Worker ID")),
    responses((status = 200, description = "Worker details", body = Worker), (status = 404, description = "Not found"), (status = 403, description = "Forbidden")))]
pub async fn get_worker(...) { ... }

#[utoipa::path(delete, path = "/workers/{worker_id}", tag = "Workers", summary = "Delete worker",
    security(("Bearer" = [])), params(("worker_id" = String, Path, description = "Worker ID")),
    responses((status = 204, description = "Deleted"), (status = 404, description = "Not found"), (status = 403, description = "Forbidden")))]
pub async fn delete_worker(...) { ... }

#[utoipa::path(post, path = "/workers/tasks/{task_id}/decline", tag = "Workers", summary = "Worker declines a task",
    security(("Bearer" = [])), params(("task_id" = String, Path, description = "Task ID")),
    request_body = DeclineBody,
    responses((status = 200, description = "Declined"), (status = 403, description = "Forbidden")))]
pub async fn decline_task(...) { ... }
```

**Step 3: Build**

Run: `cd rust && cargo build -p taskcast-server`

**Step 4: Commit**

```bash
git add rust/taskcast-server/src/routes/sse.rs rust/taskcast-server/src/routes/workers.rs
git commit -m "feat(rust/server): add utoipa::path annotations to SSE and worker handlers"
```

---

### Task 14: Create OpenAPI struct and mount Swagger UI

**Files:**
- Create: `rust/taskcast-server/src/openapi.rs`
- Modify: `rust/taskcast-server/src/app.rs`
- Modify: `rust/taskcast-server/src/lib.rs` (add `mod openapi;`)

**Step 1: Create openapi.rs**

```rust
use utoipa::openapi::security::{HttpAuthScheme, HttpBuilder, SecurityScheme};
use utoipa::{Modify, OpenApi};

use crate::routes::{sse, tasks, workers};

#[derive(OpenApi)]
#[openapi(
    info(
        title = "Taskcast API",
        version = "0.3.0",
        description = "Unified long-lifecycle task tracking service for LLM streaming, agents, and async workloads."
    ),
    paths(
        tasks::create_task,
        tasks::get_task,
        tasks::transition_task,
        tasks::publish_events,
        tasks::get_event_history,
        sse::sse_events,
        workers::list_workers,
        workers::pull_task,
        workers::get_worker,
        workers::delete_worker,
        workers::decline_task,
    ),
    components(schemas(
        taskcast_core::Task,
        taskcast_core::TaskStatus,
        taskcast_core::TaskError,
        taskcast_core::TaskEvent,
        taskcast_core::Level,
        taskcast_core::SeriesMode,
        taskcast_core::Worker,
        taskcast_core::WorkerStatus,
        taskcast_core::AssignMode,
        taskcast_core::DisconnectPolicy,
        taskcast_core::SSEEnvelope,
        tasks::CreateTaskBody,
        tasks::TransitionBody,
        tasks::TaskErrorBody,
        tasks::PublishEventBody,
        workers::DeclineBody,
    )),
    modifiers(&SecurityAddon),
    tags(
        (name = "Tasks", description = "Task lifecycle management"),
        (name = "Events", description = "Task event publishing and streaming"),
        (name = "Workers", description = "Worker management and task assignment"),
    )
)]
pub struct ApiDoc;

struct SecurityAddon;

impl Modify for SecurityAddon {
    fn modify(&self, openapi: &mut utoipa::openapi::OpenApi) {
        let components = openapi.components.get_or_insert_with(Default::default);
        components.add_security_scheme(
            "Bearer",
            SecurityScheme::Http(
                HttpBuilder::new()
                    .scheme(HttpAuthScheme::Bearer)
                    .bearer_format("JWT")
                    .description(Some("JWT Bearer token"))
                    .build(),
            ),
        );
    }
}
```

**Step 2: Modify app.rs to mount spec + UI**

```rust
use utoipa::OpenApi;
use utoipa_scalar::{Scalar, Servable};
use crate::openapi::ApiDoc;

// In create_app(), before the auth middleware layer:
let spec = ApiDoc::openapi();
app = app
    .route("/openapi.json", get(|| async move {
        axum::Json(spec.clone())
    }))
    .merge(Scalar::with_url("/docs", ApiDoc::openapi()));
```

Alternatively, serve the spec as a closure that captures the generated ApiDoc.

**Step 3: Build**

Run: `cd rust && cargo build -p taskcast-server`

**Step 4: Commit**

```bash
git add rust/taskcast-server/src/openapi.rs rust/taskcast-server/src/app.rs rust/taskcast-server/src/lib.rs
git commit -m "feat(rust/server): create OpenAPI doc struct and mount Swagger UI"
```

---

### Task 15: Write Rust OpenAPI smoke test

**Files:**
- Create test in: `rust/taskcast-server/tests/openapi_test.rs` or add to existing test file

**Step 1: Write test**

```rust
#[tokio::test]
async fn test_openapi_spec_endpoint() {
    // Set up a test app with memory adapters
    let engine = /* create test engine with memory adapters */;
    let app = create_app(Arc::new(engine), AuthMode::None, None);

    let response = app
        .oneshot(Request::builder().uri("/openapi.json").body(Body::empty()).unwrap())
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let spec: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(spec["openapi"], "3.1.0");
    assert_eq!(spec["info"]["title"], "Taskcast API");
    assert!(spec["paths"].is_object());
    assert!(spec["paths"]["/tasks"].is_object());
}

#[tokio::test]
async fn test_docs_returns_html() {
    let engine = /* create test engine */;
    let app = create_app(Arc::new(engine), AuthMode::None, None);

    let response = app
        .oneshot(Request::builder().uri("/docs").body(Body::empty()).unwrap())
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let ct = response.headers().get("content-type").unwrap().to_str().unwrap();
    assert!(ct.contains("text/html"));
}
```

**Step 2: Run tests**

Run: `cd rust && cargo test -p taskcast-server openapi`

**Step 3: Commit**

```bash
git add rust/taskcast-server/tests/
git commit -m "test(rust/server): add OpenAPI spec and docs smoke tests"
```

---

## Phase 3: Final Verification

### Task 16: Full test suite pass

**Step 1: Run TS tests**

Run: `pnpm test`
Expected: All tests pass

**Step 2: Run Rust tests**

Run: `cd rust && cargo test`
Expected: All tests pass

**Step 3: Build both**

Run: `pnpm build && cd rust && cargo build`
Expected: Clean builds

**Step 4: Manual smoke test (optional)**

Start the TS server locally, open `/docs` in browser. Start the Rust server, open `/docs`. Verify both show interactive Swagger UI with all routes.

**Step 5: Final commit (if any fixups needed)**

```bash
git add -A
git commit -m "chore: fixups from full test run"
```
