# Taskcast — AI Agent Context

> Unified long-lifecycle task tracking service for LLM streaming outputs, streaming agents, and async workflows.

## Problem Statement

Traditional SSE connections lose state on page refresh. Multiple clients cannot subscribe to the same task stream. There is no unified way to track, persist, and replay long-running async task events.

## Project Overview

- **Monorepo:** pnpm workspace, 9 packages, all TypeScript + ESM
- **Framework:** Hono (HTTP), Vitest (testing)
- **Runtime:** Node.js / Bun compatible

## Design Philosophy

### SDK-First

Core logic (`@taskcast/core`) has zero HTTP/infrastructure dependencies. The engine can be embedded into any server. Storage adapters are pluggable interfaces.

### Three-Layer Storage

```
BroadcastProvider   → real-time fan-out, no persistence (Redis pub/sub | memory)
ShortTermStore      → ordered event buffer + task state cache (Redis | memory)
LongTermStore       → permanent archive (PostgreSQL, optional)
```

Write path: `event → series merge → ShortTerm (sync) → Broadcast (sync) → LongTerm (async)`

Each layer is independently configurable and optional (except ShortTermStore which is required).

### Concurrent Safety

- Status transitions use optimistic concurrency — only one wins in a race
- Series message merging is atomic within the engine
- State machine validation happens at the engine level

## Package Map

```
packages/
├── core/          @taskcast/core         Pure engine, zero HTTP deps
│   ├── src/engine.ts                     TaskEngine orchestrator
│   ├── src/types.ts                      ALL type definitions
│   ├── src/state-machine.ts              Status transition rules
│   ├── src/filter.ts                     Wildcard event filtering
│   ├── src/series.ts                     Series merge logic
│   ├── src/cleanup.ts                    Cleanup rule matching
│   ├── src/config.ts                     Config loading + env interpolation
│   └── src/memory-adapters.ts            In-memory adapters for testing
│
├── server/        @taskcast/server       Hono HTTP server
│   ├── src/routes/tasks.ts               REST endpoints
│   ├── src/routes/sse.ts                 SSE streaming
│   ├── src/auth.ts                       JWT + custom middleware
│   └── src/webhook.ts                    Webhook delivery + HMAC
│
├── server-sdk/    @taskcast/server-sdk   HTTP client (server-to-server)
├── client/        @taskcast/client       Browser SSE client
├── react/         @taskcast/react        useTaskEvents hook
├── cli/           @taskcast/cli          npx taskcast standalone server
├── redis/         @taskcast/redis        Redis broadcast + short-term store
├── postgres/      @taskcast/postgres     PostgreSQL long-term store
└── sentry/        @taskcast/sentry       Sentry error monitoring hooks
```

## Core Concepts

### Task

A stateful entity with lifecycle: `pending → running → completed|failed|timeout|cancelled`.

- Status transitions validated by state machine (no backward, no double-terminal)
- `ttl` triggers automatic timeout
- `params` is read-only after creation; `result` set on completion; `error` set on failure

### TaskEvent

Immutable event published to a task:
- `type`: user-defined, supports wildcard filtering (e.g. `llm.*`)
- `level`: debug / info / warn / error
- `data`: arbitrary JSON
- `seriesId` + `seriesMode`: group streaming events

### Series Modes

| Mode | Behavior | Use Case |
|------|----------|----------|
| `keep-all` | Store all independently | Full history |
| `accumulate` | Concatenate `data.text` | LLM streaming text |
| `latest` | Replace previous | Progress bars |

### SSE Subscription Behavior

| Task Status | Behavior |
|-------------|----------|
| `pending` | Hold connection, auto-replay + stream when running |
| `running` | Replay history (filtered), then stream live |
| terminal | Replay history, then close |

Supports resumption via `since.id`, `since.index`, or `since.timestamp`.

## Type Reference

```typescript
type TaskStatus = 'pending' | 'running' | 'completed' | 'failed' | 'timeout' | 'cancelled'

interface Task {
  id: string; type?: string; status: TaskStatus
  params?: Record<string, unknown>; result?: Record<string, unknown>
  error?: { code?: string; message: string; details?: Record<string, unknown> }
  metadata?: Record<string, unknown>
  createdAt: number; updatedAt: number; completedAt?: number
  ttl?: number; authConfig?: TaskAuthConfig
  webhooks?: WebhookConfig[]; cleanup?: { rules: CleanupRule[] }
}

interface TaskEvent {
  id: string; taskId: string; index: number; timestamp: number
  type: string; level: 'debug' | 'info' | 'warn' | 'error'
  data: unknown; seriesId?: string; seriesMode?: 'keep-all' | 'accumulate' | 'latest'
}
```

## Storage Adapter Interfaces

```typescript
interface BroadcastProvider {
  publish(channel: string, event: TaskEvent): Promise<void>
  subscribe(channel: string, handler: (event: TaskEvent) => void): () => void
}

interface ShortTermStore {
  saveTask(task: Task): Promise<void>
  getTask(taskId: string): Promise<Task | null>
  appendEvent(taskId: string, event: TaskEvent): Promise<void>
  getEvents(taskId: string, opts: EventQueryOptions): Promise<TaskEvent[]>
  setTTL(taskId: string, ttl: number): Promise<void>
}

interface LongTermStore {
  saveTask(task: Task): Promise<void>
  getTask(taskId: string): Promise<Task | null>
  saveEvent(event: TaskEvent): Promise<void>
  getEvents(taskId: string, opts: EventQueryOptions): Promise<TaskEvent[]>
}
```

## HTTP API

```
POST   /tasks                       → Create task (201)
GET    /tasks/:taskId               → Get task (200)
PATCH  /tasks/:taskId/status        → Transition status (200)
DELETE /tasks/:taskId               → Delete task (204)
POST   /tasks/:taskId/events        → Publish event(s) (201)
GET    /tasks/:taskId/events        → SSE subscribe
GET    /tasks/:taskId/events/history → Query history (200)
```

## Auth & Permissions

Modes: `none` | `jwt` | `custom`

Scopes: `task:create`, `task:manage`, `event:publish`, `event:subscribe`, `event:history`, `webhook:create`, `*`

JWT payload: `{ sub?, taskIds: string[] | '*', scope: PermissionScope[], exp? }`

## Configuration

Priority: CLI args > Environment variables > Config file > Defaults

Config formats: `taskcast.config.ts` > `.js` > `.mjs` > `.yaml` > `.json`

Key env vars: `TASKCAST_PORT`, `TASKCAST_AUTH_MODE`, `TASKCAST_JWT_SECRET`, `TASKCAST_REDIS_URL`, `TASKCAST_POSTGRES_URL`, `SENTRY_DSN`

## Testing Principles

- **Coverage target: 100% where practical. Minimum: 90%.**
- Every bug fix must include a regression test
- Test bad cases: invalid inputs, edge cases, race conditions, error states, boundary values
- Unit tests use memory adapters, integration tests use testcontainers
- Concurrent tests verify safety under parallel access

## Documentation

| Location | Content |
|----------|---------|
| `docs/plan.md` | Original project vision |
| `docs/plans/` | Design specs and implementation plans |
| `docs/guide/` | Human-readable guides (getting-started, concepts, deployment) |
| `docs/api/` | API reference (rest, sse, authentication, webhooks) |
| `docs/skill/` | Claude Code skill for external project integration |
| `CLAUDE.md` | Claude Code development instructions |
| `AGENT.md` | This file — general AI context |

## Planned Rust Rewrite

Server-side packages → Rust (Axum + Tokio + sqlx). Client-side stays TypeScript. See `docs/plans/2026-02-28-rust-rewrite-design.md`.

**IMPORTANT: When changing any server-side feature, both the Node.js (TypeScript) and Rust implementations MUST be updated simultaneously.** The two implementations must stay in sync at all times.
