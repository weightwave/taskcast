# Getting Started

This guide will help you get Taskcast running in under 5 minutes — creating your first task, pushing streaming events, and subscribing to them.

## Installation

### Option 1: Standalone Server (Recommended for Quick Start)

No installation required. Start the server directly with npx:

```bash
npx taskcast
```

The service runs at `http://localhost:3721` by default.

### Option 2: Embed in Your Project

```bash
pnpm add @taskcast/core @taskcast/server
```

## Your First Task

### 1. Start the Server

```bash
npx taskcast
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
    "data": { "text": "你好" },
    "seriesId": "response",
    "seriesMode": "accumulate"
  }'

# Send the second message (it will be accumulated into the same series)
curl -X POST http://localhost:3721/tasks/{taskId}/events \
  -H "Content-Type: application/json" \
  -d '{
    "type": "llm.delta",
    "level": "info",
    "data": { "text": "世界！" },
    "seriesId": "response",
    "seriesMode": "accumulate"
  }'
```

The subscribing terminal will receive these events in real time.

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

## Using in Code

### Embedded Mode (Recommended for Production)

```typescript
import { TaskEngine, MemoryBroadcastProvider, MemoryShortTermStore } from '@taskcast/core'
import { createTaskcastApp } from '@taskcast/server'

// Create the engine
const engine = new TaskEngine({
  broadcast: new MemoryBroadcastProvider(),
  shortTermStore: new MemoryShortTermStore(),
})

// Create the HTTP application
const app = createTaskcastApp({ engine })

// Use it in your LLM call handler
async function handleChat(prompt: string) {
  const task = await engine.createTask({
    type: 'llm.chat',
    params: { prompt },
    ttl: 600, // 10-minute timeout
  })

  await engine.transitionTask(task.id, 'running')

  // Simulate streaming output
  for (const chunk of ['Hello, ', 'world!']) {
    await engine.publishEvent(task.id, {
      type: 'llm.delta',
      level: 'info',
      data: { text: chunk },
      seriesId: 'response',
      seriesMode: 'accumulate',
    })
  }

  await engine.transitionTask(task.id, 'completed', {
    result: { output: 'Hello, world!' },
  })

  return task.id
}
```

### Browser-Side Subscription

```bash
pnpm add @taskcast/client
```

```typescript
import { TaskcastClient } from '@taskcast/client'

const client = new TaskcastClient({
  baseUrl: 'http://localhost:3721',
})

await client.subscribe('task-id', {
  filter: {
    types: ['llm.*'],
    since: { index: 0 },
  },
  onEvent: (envelope) => {
    // Called for every event received
    document.getElementById('output')!.textContent += envelope.data.text
  },
  onDone: (reason) => {
    console.log('Task completed:', reason)
  },
  onError: (err) => {
    console.error('Subscription error:', err)
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
        <span key={e.eventId}>{e.data.text}</span>
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