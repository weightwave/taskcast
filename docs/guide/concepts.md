# Core Concepts

## Task

A task is the central entity in Taskcast, representing a long-lifecycle asynchronous operation that needs to be tracked.

### Task Lifecycle

```
pending → running → completed
                  → failed
                  → timeout
                  → cancelled
pending → cancelled
```

**Key rules:**

- State transitions are forward-only; a task cannot revert to a previous state.
- Terminal states (`completed`, `failed`, `timeout`, `cancelled`) are immutable once reached.
- Concurrency-safe — if multiple requests attempt to transition a task to a terminal state simultaneously, only one will succeed and the rest will receive an error.
- Tasks with a `ttl` set are automatically transitioned to the `timeout` state when the deadline is exceeded.

### Task Properties

```typescript
{
  id: string          // ULID (auto-generated) or user-supplied
  type: string        // Task type, e.g. "llm.chat", "agent.run" — used for filtering and cleanup rule matching
  status: TaskStatus  // Current status
  params: object      // Task input parameters (written at creation time, read-only thereafter)
  result: object      // Result on successful completion (only in "completed" state)
  error: TaskError    // Failure details (only in "failed" / "timeout" states)
  metadata: object    // Custom metadata
  ttl: number         // Timeout in seconds; the task transitions to "timeout" automatically when exceeded
}
```

## Event (TaskEvent)

Events are immutable messages published to a task. Each event has:

- **type** — A user-defined type string that supports wildcard filtering. Examples: `llm.delta`, `tool.call`, `agent.thought`
- **level** — Log level: `debug`, `info`, `warn`, or `error`
- **data** — Arbitrary JSON payload
- **index** — A monotonically increasing sequence number scoped to the task
- **clientId** (optional) — Identifies the sender, used for sequence ordering
- **clientSeq** (optional) — Client-assigned monotonically increasing sequence number

### Client Sequence Ordering

When publishing events with `clientId` and `clientSeq`, Taskcast guarantees that events are written in `clientSeq` order within each `(taskId, clientId)` pair. This is useful when a client sends events over an unreliable or concurrent transport where arrival order is not guaranteed.

- **Hold mode** (default) — Out-of-order events are held until the gap is filled (configurable timeout, default 30s)
- **Fast-fail mode** — Out-of-order events are immediately rejected with `409 seq_gap`

Different `clientId` values are completely independent. Omitting both `clientId` and `clientSeq` bypasses ordering entirely (backward-compatible).

### Built-in Events

Whenever a task's status changes, Taskcast automatically injects a built-in event with `type: "taskcast:status"`. Clients can opt in or out of receiving these events.

## Series Messages (Series)

Series messages are a defining feature of Taskcast, designed specifically for streaming scenarios. Events sharing the same `seriesId` are grouped and processed together.

### keep-all (default)

Every event is stored independently with no merging. Use this when you need a complete history.

```
Event 1: { seriesId: "s1", data: { text: "Hello" } }
Event 2: { seriesId: "s1", data: { text: " world" } }
Stored:  [Event 1, Event 2]  ← both retained
```

### accumulate

Text accumulation mode. The `data.delta` field of each new event is appended to the series' existing value (like ChatCompletion streaming). The field name defaults to `delta` but can be customized via `seriesAccField` (e.g. `"text"` or `"content"`).

```
Event 1: { seriesId: "s1", data: { delta: "Hello" }, seriesMode: "accumulate" }
Event 2: { seriesId: "s1", data: { delta: " world" }, seriesMode: "accumulate" }
Short-term store: deltas → ["Hello", " world"]
Long-term store:  accumulated → "Hello world"
```

**This is the most common mode for LLM streaming output.** SSE subscribers choose their preferred format via the `seriesFormat` query parameter:

- **`seriesFormat=delta`** (default) — Each event carries the original incremental delta. Ideal for streaming UIs that concatenate chunks as they arrive.
- **`seriesFormat=accumulated`** — Each event carries the full accumulated value up to that point. Useful for simple displays.

When a client connects mid-stream or after a refresh, it receives a single **snapshot** containing the full accumulated text, followed by subsequent events in the chosen format. This avoids replaying every individual delta.

### latest

Only the most recent value is retained. Ideal for progress bars, status indicators, and any scenario where only the current value matters.

```
Event 1: { seriesId: "progress", data: { percent: 30 }, seriesMode: "latest" }
Event 2: { seriesId: "progress", data: { percent: 60 }, seriesMode: "latest" }
Stored:  only Event 2 is kept
```

## Three-Layer Storage

Taskcast abstracts storage into three distinct layers, each with a separate responsibility. Different implementations can be chosen for each layer based on your requirements.

### Broadcast Layer (BroadcastProvider)

**Responsibility:** Real-time message fan-out with no persistence guarantee.

When an event is published, the broadcast layer pushes it to all online SSE subscribers. This is a fire-and-forget operation.

| Implementation | Use case |
|----------------|----------|
| In-memory (default) | Single-process development and testing |
| Redis Pub/Sub | Multi-process / multi-instance production deployments |

### Short-Term Store (ShortTermStore)

**Responsibility:** Event buffering and task state caching.

This is the core layer that ensures data reliability. All events are synchronously written to the short-term store before being broadcast, guaranteeing that reconnecting clients can retrieve historical events. Automatic TTL-based expiry is supported.

| Implementation | Use case |
|----------------|----------|
| In-memory (default) | Single-process development and testing |
| Redis | Production environments requiring multi-process sharing and persistence |

### Long-Term Store (LongTermStore) — Optional

**Responsibility:** Permanent archival.

For scenarios requiring long-term retention of task history, a long-term store can be configured. Events are written asynchronously (without blocking the main flow), making this layer suitable for after-the-fact auditing and analysis.

| Implementation | Use case |
|----------------|----------|
| PostgreSQL | Permanent retention and complex queries |
| Not configured | Short-lived tasks that do not require long-term storage |

### Write Flow

```
Publish event
  → Series processing (accumulate: compute delta + accumulated)
  → Write delta to short-term store (synchronous, ordered)
  → Broadcast delta + transient accumulated value (synchronous, real-time)
  → Write accumulated value to long-term store (asynchronous, non-blocking)
```

For `accumulate` mode: the short-term store holds deltas (ephemeral), the long-term store holds accumulated values (self-contained). For `keep-all` and `latest`, both stores hold the same data.

## Event Filtering

Both SSE subscriptions and webhooks support event filtering via `SubscribeFilter`.

### Type Filtering (types)

Wildcard matching is supported:

```
"llm.*"       → matches llm.delta, llm.done, llm.error
"tool.*"      → matches tool.call, tool.result
"*"           → matches all types
"llm.delta"   → exact match
```

### Level Filtering (levels)

Filter by log level: `debug`, `info`, `warn`, `error`.

### Resume from Checkpoint (since)

Three ways to specify where to resume:

| Method | Purpose | Example use case |
|--------|---------|-----------------|
| `since.id` | Resume after a specific event ID | Precise resume across different filters |
| `since.index` | Resume after the Nth filtered event | Reconnect with the same filter |
| `since.timestamp` | Resume after a specific timestamp | Time-based recovery |

## Cleanup Rules

Taskcast supports configurable cleanup rules that automatically remove data from completed tasks:

```yaml
cleanup:
  rules:
    # Clean up debug-level events for LLM tasks after 1 hour
    - match:
        taskTypes: ["llm.*"]
      trigger:
        afterMs: 3600000
      target: events
      eventFilter:
        levels: [debug]

    # Clean up all completed tasks after 7 days
    - trigger:
        afterMs: 604800000
      target: all
```

**Cleanup targets (`target`):**
- `events` — Delete only events; the task record is retained.
- `task` — Delete only the task record.
- `all` — Delete the task record and all its events.

## Next Steps

- [Deployment Guide](./deployment.md) — Production environment configuration
- [REST API](../api/rest.md) — Full API reference
- [Authentication & Authorization](../api/authentication.md) — JWT authentication configuration