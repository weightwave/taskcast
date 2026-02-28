# SSE Subscription

The Taskcast SSE subscription endpoint provides a real-time event stream with support for history replay, resume-from-last-position, and event filtering.

## Endpoint

```
GET /tasks/:taskId/events
```

**Required permission:** `event:subscribe`

## Subscription Behavior

The behavior of an SSE connection depends on the current task state:

| Task state | Behavior |
|------------|----------|
| `pending` | Holds the connection. Once the task transitions to `running`, automatically replays history then streams live events. |
| `running` | Replays historical events (subject to filter criteria), then streams live events. Automatically closes when the task reaches a terminal state. |
| Terminal state | Replays historical events (by default), then sends a close signal. |
| Does not exist | Returns `404`. |

## Query Parameters

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `since.id` | string | — | Resume after the specified event ID (precise resume, filter-independent). |
| `since.index` | number | — | Resume after the Nth event in the filtered sequence (for reconnecting with the same filter). |
| `since.timestamp` | number | — | Resume after the specified timestamp (ms). |
| `types` | string | — | Comma-separated type filter; supports wildcards. e.g. `llm.*,tool.call` |
| `levels` | string | — | Comma-separated level filter. e.g. `info,warn,error` |
| `includeStatus` | boolean | `true` | Whether to include the built-in `taskcast:status` status events. |
| `wrap` | boolean | `true` | Whether to wrap each event in an SSEEnvelope. |

### Examples

```
# Subscribe to all events
GET /tasks/01HXXX/events

# Subscribe to LLM-related events only
GET /tasks/01HXXX/events?types=llm.*

# Resume from the 5th event, with info level and above only
GET /tasks/01HXXX/events?since.index=5&levels=info,warn,error

# Exclude status events, no envelope wrapping
GET /tasks/01HXXX/events?includeStatus=false&wrap=false
```

## Event Stream Format

### Regular event (wrap=true, default)

```
event: taskcast.event
id: 01HXXX001
data: {"filteredIndex":0,"rawIndex":0,"eventId":"01HXXX001","taskId":"01HXXX","type":"llm.delta","timestamp":1700000000000,"level":"info","data":{"text":"Hello"},"seriesId":"response","seriesMode":"accumulate"}

event: taskcast.event
id: 01HXXX002
data: {"filteredIndex":1,"rawIndex":1,"eventId":"01HXXX002","taskId":"01HXXX","type":"llm.delta","timestamp":1700000000100,"level":"info","data":{"text":" world!"}}
```

### Regular event (wrap=false)

```
event: taskcast.event
id: 01HXXX001
data: {"text":"Hello"}
```

### Status change event

```
event: taskcast.status
data: {"taskId":"01HXXX","status":"completed","result":{"output":"Hello world!"}}
```

### Close signal

Sent before the connection is closed when the task reaches a terminal state:

```
event: taskcast.done
data: {"reason":"completed"}
```

`reason` corresponds to the task's terminal state: `completed`, `failed`, `timeout`, or `cancelled`.

## SSEEnvelope Structure

When `wrap=true`, each event is wrapped in an envelope:

```typescript
interface SSEEnvelope {
  filteredIndex: number  // Position in the filtered sequence (0, 1, 2...), used for since.index resume
  rawIndex: number       // Raw global sequence number, for debugging
  eventId: string        // Event ULID
  taskId: string
  type: string           // Event type
  timestamp: number      // Timestamp in ms
  level: string
  data: unknown          // Event payload
  seriesId?: string
  seriesMode?: string
}
```

**About `filteredIndex`:** When filters are applied, `rawIndex` may not be contiguous (filtered-out events are skipped), whereas `filteredIndex` always increments sequentially from 0. Clients use `filteredIndex` together with `since.index` to implement resume-from-last-position.

## Resume from Last Position

### Scenario: Resuming after a page refresh

```javascript
// Track the last received filteredIndex
let lastIndex = -1

client.subscribe(taskId, {
  filter: { types: ['llm.*'] },
  onEvent: (envelope) => {
    lastIndex = envelope.filteredIndex
    // handle event...
  },
})

// After refresh, resume using the same filter + since.index
client.subscribe(taskId, {
  filter: {
    types: ['llm.*'],
    since: { index: lastIndex }, // continue from where we left off
  },
  onEvent: (envelope) => {
    // only events after lastIndex will be received
  },
})
```

### Scenario: Resuming across filter changes

If you change the filter criteria (e.g. switching from `llm.*` only to all events), `since.index` will no longer be accurate. Use `since.id` instead:

```javascript
client.subscribe(taskId, {
  filter: {
    // new filter criteria
    since: { id: lastEventId }, // continue after a specific event, regardless of filter changes
  },
})
```

## Authentication

The SSE endpoint authenticates via the `Authorization` request header:

```
GET /tasks/01HXXX/events
Authorization: Bearer <jwt-token>
```

The `taskIds` field in the JWT payload controls which task IDs are accessible, and the `scope` must include `event:subscribe`.

## Connection Management

- When a task reaches a terminal state, the server sends a `taskcast.done` event and closes the connection.
- When the client disconnects, the server automatically cleans up the subscription resources.
- Long-idle connections are not proactively closed by the server (heartbeating is maintained by the client's SSE mechanism).