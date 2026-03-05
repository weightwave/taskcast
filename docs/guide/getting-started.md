# Getting Started

This guide will help you get Taskcast running in under 5 minutes — creating your first task, pushing streaming events, and subscribing to them.

## Installation

### Option 1: Standalone Server (Recommended for Quick Start)

**Node.js (npx) — no installation required:**

```bash
npx @taskcast/cli
```

**Native Rust binary — optimal performance, zero Node.js dependency:**

```bash
# Homebrew (macOS / Linux)
brew tap weightwave/tap
brew install taskcast
taskcast-rs

# Or download a pre-built binary from GitHub Releases
# https://github.com/weightwave/taskcast/releases

# Or run via Docker
docker run -p 3721:3721 mwr1998/taskcast-rs
```

Both versions produce identical behavior. The service runs at `http://localhost:3721` by default.

### Option 2: Embed in Your Project

```bash
pnpm add @taskcast/core @taskcast/server
```

## Your First Task

### 1. Start the Server

```bash
npx @taskcast/cli
```

You should see output similar to:

```
Taskcast server listening on http://localhost:3721
  Auth: none
  Broadcast: memory
  ShortTerm: memory
  LongTerm: not configured
```

### 2. Create a Task

```bash
curl -X POST http://localhost:3721/tasks \
  -H "Content-Type: application/json" \
  -d '{
    "type": "llm.chat",
    "params": { "prompt": "Hello, world!" }
  }'
```

Response:

```json
{
  "id": "01HXXXXXXXXXXXXXXXXXXX",
  "type": "llm.chat",
  "status": "pending",
  "params": { "prompt": "Hello, world!" },
  "createdAt": 1700000000000,
  "updatedAt": 1700000000000
}
```

Note the returned `id` — you will need it in the following steps.

### 3. Subscribe to Task Events (in Another Terminal)

```bash
curl -N http://localhost:3721/tasks/{taskId}/events
```

The connection will hang and wait, because the task is still in `pending` status.

### 4. Start the Task

```bash
curl -X PATCH http://localhost:3721/tasks/{taskId}/status \
  -H "Content-Type: application/json" \
  -d '{ "status": "running" }'
```

The subscribing terminal will receive a status change event.

### 5. Send Streaming Messages

```bash
# Send the first message
curl -X POST http://localhost:3721/tasks/{taskId}/events \
  -H "Content-Type: application/json" \
  -d '{
    "type": "llm.delta",
    "level": "info",
    "data": { "delta": "你好" },
    "seriesId": "response",
    "seriesMode": "accumulate"
  }'

# Send the second message (it will be accumulated into the same series)
curl -X POST http://localhost:3721/tasks/{taskId}/events \
  -H "Content-Type: application/json" \
  -d '{
    "type": "llm.delta",
    "level": "info",
    "data": { "delta": "世界！" },
    "seriesId": "response",
    "seriesMode": "accumulate"
  }'
```

The subscribing terminal will receive these events in real time.

> **Note:** In `accumulate` mode, the field defaults to `delta` but can be customized via `seriesAccField`.

### 6. Complete the Task

```bash
curl -X PATCH http://localhost:3721/tasks/{taskId}/status \
  -H "Content-Type: application/json" \
  -d '{
    "status": "completed",
    "result": { "output": "你好世界！" }
  }'
```

The subscription connection will receive the completion event and close automatically.

## Usage Examples

### Pattern 1: Backend + Worker Integrated (Self-Managed)

The backend creates tasks, processes them directly, and streams results — all within the same process. No separate worker needed. Best for simple deployments where the API server also does the work.

```typescript
import { TaskEngine, MemoryBroadcastProvider, MemoryShortTermStore } from '@taskcast/core'
import { createTaskcastApp } from '@taskcast/server'
import { Hono } from 'hono'

// Create the task engine with in-memory adapters (swap to Redis/SQLite for production)
const engine = new TaskEngine({
  broadcast: new MemoryBroadcastProvider(),   // real-time event fan-out to all SSE subscribers
  shortTermStore: new MemoryShortTermStore(), // task state + event buffer (sync writes ensure ordering)
})

const app = new Hono()
// Mount the Taskcast HTTP routes — provides REST API + SSE endpoints under /taskcast
app.route('/taskcast', createTaskcastApp({ engine }))

// Your API endpoint — creates and handles tasks directly
app.post('/api/chat', async (c) => {
  const { prompt } = await c.req.json()

  // Create a task in "pending" status. The client can start subscribing immediately —
  // SSE will hold the connection and auto-stream once the task transitions to "running".
  const task = await engine.createTask({
    type: 'llm.chat',       // task type, used for event filtering (supports wildcards like "llm.*")
    params: { prompt },      // arbitrary params, passed through to consumers
    ttl: 600,                // auto-timeout after 10 minutes if not completed
  })

  // Process in background — this server IS the worker.
  // The client receives the taskId immediately and subscribes via SSE.
  processChat(task.id, prompt)
  return c.json({ taskId: task.id })
})

async function processChat(taskId: string, prompt: string) {
  // pending → running: SSE subscribers waiting on this task will start receiving events
  await engine.transitionTask(taskId, 'running')

  for await (const chunk of callLLM(prompt)) {
    // Publish a streaming event. seriesMode: 'accumulate' means the engine merges
    // all deltas into a single series entry (like ChatCompletion streaming).
    // Late-joining subscribers see the accumulated result, not individual chunks.
    await engine.publishEvent(taskId, {
      type: 'llm.delta',
      level: 'info',
      data: { delta: chunk },
      seriesId: 'response',       // groups events into a named series
      seriesMode: 'accumulate',   // 'accumulate' | 'latest' | 'keep-all'
    })
  }

  // running → completed: SSE connections receive the completion event and close automatically.
  // Only one terminal transition is allowed (concurrent-safe).
  await engine.transitionTask(taskId, 'completed', {
    result: { output: 'full response text' },
  })
}
```

The client subscribes to `GET /taskcast/tasks/{taskId}/events` (SSE) to receive streamed results. If the task is still `pending`, the connection holds and auto-streams when it transitions to `running`. If the task is already `completed`, the client receives the full history replay then closes.

### Pattern 2: Backend + Worker Separated

The backend creates tasks via the HTTP SDK. Independent worker processes connect to the Taskcast service and pick up tasks for processing. Best for scaling workers independently from the API server.

**Step 1 — Start a standalone Taskcast service:**

```bash
npx @taskcast/cli
# or: taskcast-rs
```

**Step 2 — Backend creates tasks (task producer):**

```typescript
import { TaskcastServerClient } from '@taskcast/server-sdk'

const taskcast = new TaskcastServerClient({
  baseUrl: 'http://taskcast-service:3721',
  token: process.env.TASKCAST_TOKEN, // JWT with task:create + event:subscribe scopes
})

// Create a task — it stays in "pending" until a worker picks it up.
// assignMode tells the engine how to distribute this task to workers.
const task = await taskcast.createTask({
  type: 'llm.chat',
  params: { prompt: 'Tell me a story' },
  assignMode: 'pull',    // 'pull' = worker long-polls; 'ws-offer' = server pushes to WS worker;
                         // 'ws-race' = server offers to multiple WS workers, first accept wins
})

// Return taskId to the client — they subscribe via SSE to receive streaming results
return { taskId: task.id }
```

**Step 3a — Worker pulls tasks (long-polling):**

```typescript
const TASKCAST_URL = 'http://taskcast-service:3721'
const WORKER_ID = 'worker-1'

async function workerLoop() {
  while (true) {
    // Long-poll: the server holds the connection until a matching task is available
    // or the timeout expires. On match, the task is atomically assigned to this worker
    // (pending → assigned) so no other worker can claim it.
    const res = await fetch(
      `${TASKCAST_URL}/workers/pull?workerId=${WORKER_ID}&timeout=30000`,
      { headers: { Authorization: `Bearer ${WORKER_TOKEN}` } },
    )

    if (res.status === 204) continue // timeout, no task matched — retry

    const task = await res.json() // { id, type, params, ... }
    await processAndComplete(task.id, task.params)
  }
}

async function processAndComplete(taskId: string, params: Record<string, unknown>) {
  // assigned → running: tells subscribers that processing has started
  await fetch(`${TASKCAST_URL}/tasks/${taskId}/status`, {
    method: 'PATCH',
    headers: { 'Content-Type': 'application/json', Authorization: `Bearer ${WORKER_TOKEN}` },
    body: JSON.stringify({ status: 'running' }),
  })

  // Publish streaming events — each event is broadcast to all SSE subscribers in real time.
  // seriesMode: 'accumulate' merges deltas so late-joiners see the full text so far.
  for await (const chunk of callLLM(params.prompt as string)) {
    await fetch(`${TASKCAST_URL}/tasks/${taskId}/events`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json', Authorization: `Bearer ${WORKER_TOKEN}` },
      body: JSON.stringify({
        type: 'llm.delta', level: 'info',
        data: { delta: chunk },
        seriesId: 'response', seriesMode: 'accumulate',
      }),
    })
  }

  // running → completed: SSE subscribers receive the terminal event and disconnect.
  // The worker's capacity slot is automatically freed for the next task.
  await fetch(`${TASKCAST_URL}/tasks/${taskId}/status`, {
    method: 'PATCH',
    headers: { 'Content-Type': 'application/json', Authorization: `Bearer ${WORKER_TOKEN}` },
    body: JSON.stringify({ status: 'completed', result: { output: 'full text' } }),
  })
}
```

**Step 3b — Or use WebSocket workers:**

```typescript
const ws = new WebSocket('ws://taskcast-service:3721/workers/ws')

ws.addEventListener('open', () => {
  // Register this worker with the server. matchRule filters which tasks
  // are offered to this worker; capacity limits concurrent assignments.
  ws.send(JSON.stringify({
    type: 'register',
    matchRule: { types: ['llm.*'] }, // only accept tasks with type matching "llm.*"
    capacity: 5,                     // max 5 concurrent tasks
  }))
})

ws.addEventListener('message', async (event) => {
  const msg = JSON.parse(event.data)

  if (msg.type === 'offer') {
    // Server offers a task to this worker (ws-offer mode: exclusive offer;
    // ws-race mode: offered to multiple workers, first accept wins).
    // msg.task contains { id, type, params, tags, cost }.
    ws.send(JSON.stringify({ type: 'accept', taskId: msg.task.id }))
    await processAndComplete(msg.task.id, msg.task.params)
  }
})
```

**Step 4 — Client subscribes (browser):**

```bash
pnpm add @taskcast/client
```

```typescript
import { TaskcastClient } from '@taskcast/client'

const client = new TaskcastClient({
  baseUrl: 'http://taskcast-service:3721', // or behind an API gateway that handles auth
  token: 'user-jwt-token',
})

// Subscribe to the task's SSE event stream.
// filter supports wildcard matching to receive only events you care about.
await client.subscribe(taskId, {
  filter: { types: ['llm.*'] },  // only receive events with type matching "llm.*"
  onEvent: (envelope) => {
    // envelope contains the full event envelope: { eventId, type, level, data, seriesId, ... }
    document.getElementById('output')!.textContent += envelope.data.delta
  },
  onDone: (reason) => {
    // reason: 'completed' | 'failed' | 'timeout' | 'cancelled'
    console.log('Task completed:', reason)
  },
})
```

### React Integration

```bash
pnpm add @taskcast/react
```

```typescript
import { useTaskEvents } from '@taskcast/react'

function ChatStream({ taskId }: { taskId: string }) {
  const { events, isDone, doneReason, error } = useTaskEvents(taskId, {
    baseUrl: 'http://localhost:3721',
    filter: { types: ['llm.*'] },
  })

  if (error) return <div>Error: {error.message}</div>

  return (
    <div>
      {events.map((e) => (
        <span key={e.eventId}>{e.data.delta}</span>
      ))}
      {isDone && <p>Completed: {doneReason}</p>}
    </div>
  )
}
```

## Next Steps

- [Core Concepts](./concepts.md) — Deep dive into task lifecycle, series messages, and the three-tier storage model
- [Deployment Guide](./deployment.md) — Production configuration, Redis/PostgreSQL integration
- [REST API](../api/rest.md) — Complete API reference
- [SSE Subscription](../api/sse.md) — SSE protocol details