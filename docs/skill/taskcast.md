---
name: taskcast
description: Use when integrating Taskcast for long-lifecycle task tracking with SSE streaming — covers installation, embedded/remote setup, API usage, React hooks, and configuration
---

# Taskcast Integration Guide

Taskcast is a unified long-lifecycle task tracking service for LLM streaming, agents, and async workflows. It provides persistent state, resumable SSE subscriptions, and multi-client fan-out.

## When to Use Taskcast

- You need to track long-running async tasks (LLM generation, agent execution, batch processing)
- You need SSE streaming that survives page refresh
- Multiple clients need to subscribe to the same task
- You need task lifecycle management (pending → running → completed/failed)

## Installation

Choose packages based on your deployment mode:

**Embedded mode** (mount into your server):
```bash
pnpm add @taskcast/core @taskcast/server
# Optional adapters:
pnpm add @taskcast/redis    # Production: Redis broadcast + short-term store
pnpm add @taskcast/postgres  # Optional: PostgreSQL long-term archive
pnpm add @taskcast/sentry    # Optional: Sentry error monitoring
```

**Remote mode** (connect to standalone server):
```bash
# Server side (producer):
pnpm add @taskcast/server-sdk

# Browser side (consumer):
pnpm add @taskcast/client
# or for React:
pnpm add @taskcast/react
```

**Standalone server:**
```bash
npx @taskcast/cli              # Start with defaults (port 3721, memory storage)
npx @taskcast/cli -p 8080      # Custom port
npx @taskcast/cli -c config.yaml  # With config file
```

## Embedded Mode Setup

```typescript
import { TaskEngine, MemoryBroadcastProvider, MemoryShortTermStore } from '@taskcast/core'
import { createTaskcastApp } from '@taskcast/server'

const engine = new TaskEngine({
  broadcast: new MemoryBroadcastProvider(),
  shortTermStore: new MemoryShortTermStore(),
})

const app = createTaskcastApp({ engine })

// Mount to your Hono app:
// mainApp.route('/taskcast', app)
// Or serve directly:
// export default app
```

### With Redis (production):

```typescript
import { createRedisAdapters } from '@taskcast/redis'
import Redis from 'ioredis'

const pub = new Redis(process.env.REDIS_URL!)
const sub = new Redis(process.env.REDIS_URL!)
const store = new Redis(process.env.REDIS_URL!)
const { broadcast, shortTermStore } = createRedisAdapters(pub, sub, store)

const engine = new TaskEngine({ broadcast, shortTermStore })
```

### With PostgreSQL long-term storage:

```typescript
import { createPostgresAdapter } from '@taskcast/postgres'

const longTermStore = await createPostgresAdapter({ url: process.env.DATABASE_URL! })
const engine = new TaskEngine({ broadcast, shortTermStore, longTermStore })
```

### With JWT authentication:

```typescript
const app = createTaskcastApp({
  engine,
  auth: {
    mode: 'jwt',
    jwt: { algorithm: 'HS256', secret: process.env.JWT_SECRET! },
  },
})
```

## Creating and Managing Tasks

```typescript
// Create a task
const task = await engine.createTask({
  type: 'llm.chat',              // Task type for filtering/cleanup
  params: { prompt: 'Hello' },   // Input params (read-only after creation)
  ttl: 3600,                     // Auto-timeout after 1 hour
})

// Transition to running
await engine.transitionTask(task.id, 'running')

// Publish streaming events
await engine.publishEvent(task.id, {
  type: 'llm.delta',
  level: 'info',
  data: { delta: 'Hello ' },
  seriesId: 'response',          // Group streaming chunks
  seriesMode: 'accumulate',      // Concatenate data.delta across events
})

await engine.publishEvent(task.id, {
  type: 'llm.delta',
  level: 'info',
  data: { delta: 'world!' },
  seriesId: 'response',
  seriesMode: 'accumulate',
})

// Complete the task
await engine.transitionTask(task.id, 'completed', {
  result: { output: 'Hello world!' },
})
```

## Series Modes

Events with the same `seriesId` are grouped:

| Mode | Behavior | Use Case |
|------|----------|----------|
| `keep-all` | Store all events independently | Full history needed |
| `accumulate` | Concatenate `data.delta` across events (field customizable via `seriesAccField`). Short-term store holds deltas; long-term store holds accumulated values. | LLM streaming text |
| `latest` | Replace previous event in series | Progress bars, status indicators |

### Series Format (SSE)

SSE subscribers can choose how `accumulate` series events are delivered via `seriesFormat` query parameter:

| Format | Behavior |
|--------|----------|
| `delta` (default) | Each event carries the original incremental delta |
| `accumulated` | Each event carries the full accumulated value |

Late-joining subscribers always receive a single snapshot per series (`seriesSnapshot: true`), regardless of format. Reconnection with a `since` cursor does NOT collapse — events resume from the breakpoint.

## Browser SSE Subscription

```typescript
import { TaskcastClient } from '@taskcast/client'

const client = new TaskcastClient({
  baseUrl: 'http://localhost:3721',
  token: jwtToken,  // Optional
})

await client.subscribe(taskId, {
  filter: {
    types: ['llm.*'],          // Wildcard type matching
    levels: ['info', 'warn'],  // Level filtering
    since: { index: 0 },       // Resume from beginning
    seriesFormat: 'delta',     // 'delta' (default) or 'accumulated'
  },
  onEvent: (envelope) => {
    // envelope.filteredIndex, envelope.data, envelope.type, etc.
    console.log(envelope.data)
  },
  onDone: (reason) => {
    // reason: 'completed' | 'failed' | 'timeout' | 'cancelled'
    console.log('Task done:', reason)
  },
  onError: (err) => console.error(err),
})
```

## React Integration

```typescript
import { useTaskEvents } from '@taskcast/react'

function TaskStream({ taskId }: { taskId: string }) {
  const { events, isDone, doneReason, error } = useTaskEvents(taskId, {
    baseUrl: 'http://localhost:3721',
    token: jwtToken,
    filter: { types: ['llm.*'] },
  })

  return (
    <div>
      {events.map((e) => <span key={e.eventId}>{e.data.delta}</span>)}
      {isDone && <p>Done: {doneReason}</p>}
      {error && <p>Error: {error.message}</p>}
    </div>
  )
}
```

## Remote Mode (Server SDK)

```typescript
import { TaskcastServerClient } from '@taskcast/server-sdk'

const taskcast = new TaskcastServerClient({
  baseUrl: 'http://taskcast-service:3721',
  token: serviceToken,
})

const task = await taskcast.createTask({ type: 'llm.chat', params: { prompt: 'Hi' } })
await taskcast.transitionTask(task.id, 'running')
await taskcast.publishEvent(task.id, { type: 'llm.delta', level: 'info', data: { delta: 'Hi!' } })
await taskcast.transitionTask(task.id, 'completed', { result: { output: 'Hi!' } })
```

## REST API Quick Reference

```
POST   /tasks                       Create task
GET    /tasks/:taskId               Get task status
PATCH  /tasks/:taskId/status        Transition status
DELETE /tasks/:taskId               Delete task (planned)
POST   /tasks/:taskId/events        Publish event(s) (?clientId&clientSeq for ordering)
GET    /tasks/:taskId/events        SSE subscribe (?seriesFormat=delta|accumulated)
GET    /tasks/:taskId/events/history Query history
GET    /tasks/:taskId/seq/:clientId  Query expected seq for a client
```

## Task Status Lifecycle

```
pending → running → completed | failed | timeout | cancelled
pending → cancelled
```

No backward transitions. Only one terminal transition allowed (concurrent-safe).

## Configuration (YAML)

```yaml
port: 3721
auth:
  mode: jwt
  jwt:
    algorithm: HS256
    secret: ${JWT_SECRET}
adapters:
  broadcast: { provider: redis, url: "${REDIS_URL}" }
  shortTerm: { provider: redis, url: "${REDIS_URL}" }
  longTerm: { provider: postgres, url: "${DATABASE_URL}" }
```

## Key Environment Variables

| Variable | Description |
|----------|-------------|
| `TASKCAST_PORT` | Server port (default: 3721) |
| `TASKCAST_AUTH_MODE` | none / jwt / custom |
| `TASKCAST_JWT_SECRET` | JWT HMAC secret |
| `TASKCAST_REDIS_URL` | Redis connection URL |
| `TASKCAST_POSTGRES_URL` | PostgreSQL connection URL |

## JWT Payload Format

```json
{
  "sub": "user-id",
  "taskIds": ["task-1"] or "*",
  "scope": ["event:subscribe", "event:history"],
  "exp": 1700003600
}
```

Scopes: `task:create`, `task:manage`, `event:publish`, `event:subscribe`, `event:history`, `webhook:create`, `*`

## Debugging

### CLI Quick Checks

```bash
taskcast ping                          # Server reachable?
taskcast doctor                        # Storage + auth + connectivity
taskcast tasks list --status running   # Any stuck tasks?
taskcast tasks inspect <taskId>        # Full task details + recent events
taskcast logs <taskId>                 # Real-time event stream for one task
taskcast tail                          # Watch all tasks globally
```

### Common Errors and Fixes

| Error | Cause | Fix |
|-------|-------|-----|
| `Task not found: <id>` | Task expired (TTL) or was cleaned up | Check TTL settings; query long-term store if configured |
| `Cannot publish to task in terminal status` | Task already completed/failed/cancelled | Check task status before publishing; create a new task |
| `Invalid transition: pending → completed` | Must go through `running` first | Call `transitionTask(id, 'running')` before `transitionTask(id, 'completed')` |
| `403 Forbidden` | JWT missing required scope | Check token scopes; need `event:publish`, `task:create`, etc. |
| SSE connects but no events appear | Task still in `pending` status | Transition to `running` — SSE holds connection until task is running |
| `ECONNREFUSED` | Server not running or wrong port | Run `taskcast ping` to verify; check with `taskcast doctor` |

### Verbose Server Mode

Start the server with `--verbose` to see every request:

```bash
taskcast start --verbose
# [2026-03-07 14:32:01] POST   /tasks                    → 201  12ms
# [2026-03-07 14:32:02] PATCH  /tasks/01JXX../status     → 200   3ms
```

### State Machine Reference

```
pending → running → completed | failed | timeout | cancelled
pending → assigned → running (worker assignment)
pending → cancelled
running → paused → running (resumable)
running → blocked → running (after resolve)
```

## Agent Workflow Patterns

### Agent as Producer (streaming output)

```typescript
import { TaskcastServerClient } from '@taskcast/server-sdk'

const taskcast = new TaskcastServerClient({ baseUrl: 'http://localhost:3721' })

const task = await taskcast.createTask({ type: 'llm.chat', params: { prompt } })
await taskcast.transitionTask(task.id, 'running')

try {
  for await (const chunk of llmStream) {
    await taskcast.publishEvent(task.id, {
      type: 'llm.delta',
      level: 'info',
      data: { delta: chunk.text },
      seriesId: 'response',
      seriesMode: 'accumulate',
    })
  }
  await taskcast.transitionTask(task.id, 'completed', {
    result: { output: fullText },
  })
} catch (err) {
  await taskcast.transitionTask(task.id, 'failed', {
    error: { message: err.message, code: 'LLM_ERROR' },
  })
}
```

### Agent as Orchestrator (subtask management)

```typescript
// Create subtasks
const subtasks = await Promise.all(
  steps.map(step =>
    taskcast.createTask({ type: 'agent.step', params: step })
  )
)

// Monitor all subtasks
const poll = setInterval(async () => {
  const results = await Promise.all(
    subtasks.map(t => taskcast.getTask(t.id))
  )
  const allDone = results.every(t =>
    ['completed', 'failed'].includes(t.status)
  )
  if (allDone) {
    clearInterval(poll)
    // Process results...
  }
}, 1000)
```

### Error Recovery Pattern

```typescript
try {
  await taskcast.transitionTask(taskId, 'running')
  // ... do work ...
  await taskcast.transitionTask(taskId, 'completed', { result })
} catch (err) {
  // Always transition to failed so subscribers know
  await taskcast.transitionTask(taskId, 'failed', {
    error: {
      message: err.message,
      code: err.code ?? 'UNKNOWN',
      details: { stack: err.stack },
    },
  }).catch(() => {}) // task may already be terminal
}
```

## Node Management (CLI)

Manage connections to multiple Taskcast servers:

```bash
# Add connections
taskcast node add local --url http://localhost:3721
taskcast node add prod --url https://tc.example.com --token <jwt> --token-type jwt
taskcast node add staging --url https://s.tc.io --token <admin-token> --token-type admin

# Switch default target
taskcast node use prod

# All commands now target prod
taskcast tasks list
taskcast logs <taskId>

# Override per-command
taskcast tasks list --node local
```

Token types:
- `jwt` — Used directly as Bearer token
- `admin` — Exchanged for JWT via `/admin/token` endpoint automatically
