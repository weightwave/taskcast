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
npx taskcast              # Start with defaults (port 3721, memory storage)
npx taskcast -p 8080      # Custom port
npx taskcast -c config.yaml  # With config file
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
const { broadcast, shortTerm } = createRedisAdapters(pub, sub, store)

const engine = new TaskEngine({ broadcast, shortTermStore: shortTerm })
```

### With PostgreSQL long-term storage:

```typescript
import { createPostgresAdapter } from '@taskcast/postgres'

const longTerm = await createPostgresAdapter({ url: process.env.DATABASE_URL! })
const engine = new TaskEngine({ broadcast, shortTermStore: shortTerm, longTermStore: longTerm })
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
  data: { text: 'Hello ' },
  seriesId: 'response',          // Group streaming chunks
  seriesMode: 'accumulate',      // Concatenate data.text across events
})

await engine.publishEvent(task.id, {
  type: 'llm.delta',
  level: 'info',
  data: { text: 'world!' },
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
| `accumulate` | Concatenate `data.text` across events | LLM streaming text |
| `latest` | Replace previous event in series | Progress bars, status indicators |

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
      {events.map((e) => <span key={e.eventId}>{e.data.text}</span>)}
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
await taskcast.publishEvent(task.id, { type: 'llm.delta', level: 'info', data: { text: 'Hi!' } })
await taskcast.transitionTask(task.id, 'completed', { result: { output: 'Hi!' } })
```

## REST API Quick Reference

```
POST   /tasks                       Create task
GET    /tasks/:taskId               Get task status
PATCH  /tasks/:taskId/status        Transition status
DELETE /tasks/:taskId               Delete task
POST   /tasks/:taskId/events        Publish event(s)
GET    /tasks/:taskId/events        SSE subscribe
GET    /tasks/:taskId/events/history Query history
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
