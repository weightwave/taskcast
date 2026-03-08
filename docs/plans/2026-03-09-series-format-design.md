# Series Format: Consumer-Controlled Accumulate Output

## Problem

The current `accumulate` series mode stores and broadcasts the **full accumulated text** in every event. This causes:

1. **Double accumulation** — frontends that iterate events and concatenate `data.delta` get duplicated text, because each event already contains the full accumulated value.
2. **Bandwidth waste** — for a 10 KB text streamed over 1000 chunks, the final event alone carries 10 KB, and the total data transferred is O(n^2).
3. **Rigid consumption** — consumers cannot choose between receiving deltas (for streaming UIs) or accumulated values (for simple displays).

Additionally, `processSeries` performs a non-atomic read-modify-write on `seriesLatest`. In distributed deployments where two POSTs for the same series hit different instances simultaneously, one accumulated value can overwrite the other, causing data loss.

## Design

### Core Idea

Change `accumulate` mode to **store deltas in ShortTermStore** and **maintain accumulated state in seriesLatest**. Let SSE subscribers choose their preferred format via a `seriesFormat` query parameter. Fix the underlying atomicity issue in series processing.

### Three-Layer Storage

| Layer | What accumulate stores | Purpose |
|---|---|---|
| **ShortTermStore** | Original delta | Serve real-time streams |
| **seriesLatest** | Accumulated value | Late-join snapshots, feed LongTermStore |
| **LongTermStore** | Accumulated value (from seriesLatest) | Archival queries, self-contained |

### Write Path

```
event arrives
  -> processSeries (accumulate mode):
      1. Atomically read-concat-write seriesLatest (Lua script for Redis)
      2. Returns SeriesResult { event: original delta, accumulated: full acc value }
  -> ShortTermStore.appendEvent(delta event)
  -> broadcast.publish(delta event + transient acc value)
  -> LongTermStore.saveEvent(accumulated event)  <- use acc value from SeriesResult
```

The broadcast event carries the accumulated value as a transient field (not persisted in ShortTermStore). SSE handlers for `accumulated` subscribers use this directly — zero extra store reads.

### Atomicity Fix

The current `processSeries` does non-atomic read-modify-write on `seriesLatest`:

```typescript
const prev = await store.getSeriesLatest(taskId, seriesId)  // READ
// ... compute merged
await store.setSeriesLatest(taskId, seriesId, merged)        // WRITE
```

In distributed deployments, concurrent POSTs to the same series can race and corrupt the accumulated value.

**Fix by adapter:**

- **Memory adapter** — Node.js is single-threaded; no real concurrency risk. No change needed.
- **Redis adapter** — Replace GET+SET with a Lua script that atomically reads, concatenates, and writes:

```lua
local prev = redis.call('GET', KEYS[1]) or '{}'
local prevObj = cjson.decode(prev)
local patch = cjson.decode(ARGV[1])
local field = ARGV[2]

local prevVal = prevObj[field] or ""
local newVal = patch[field] or ""
patch[field] = prevVal .. newVal

redis.call('SET', KEYS[1], cjson.encode(patch))
return cjson.encode(patch)
```

This approach also supports future extensions (e.g., JSON deep merge) since `cjson` is available in Redis Lua.

**Interface change:**

`ShortTermStore` gains a new method for atomic accumulate:

```typescript
interface ShortTermStore {
  // ... existing methods
  accumulateSeries(
    taskId: string,
    seriesId: string,
    event: TaskEvent,
    field: string,
  ): Promise<TaskEvent>  // returns event with accumulated field value
}
```

`processSeries` calls `accumulateSeries` instead of separate `getSeriesLatest` + `setSeriesLatest` for accumulate mode. This pushes the atomicity guarantee down to the adapter layer where it belongs.

### Performance Optimization

`processSeries` returns both the delta event and the accumulated value:

```typescript
interface SeriesResult {
  event: TaskEvent       // original delta (stored in ShortTermStore)
  accumulated?: unknown  // full accumulated value (transient, for broadcast + LongTermStore)
}
```

The engine attaches the accumulated value to the broadcast message. This means:

- **`delta` subscribers** — receive the delta event directly, ignore transient field. Zero overhead.
- **`accumulated` subscribers** — read the transient accumulated value. Zero store reads.
- **Late-join snapshot** — read `seriesLatest` once per series at connection time. One-time cost.

No per-event store reads at the SSE layer. The computation happens once at publish time and is shared across all subscribers.

### SSE `seriesFormat` Parameter

New query parameter on the SSE endpoint:

```
GET /tasks/{taskId}/events?seriesFormat=delta|accumulated
```

Default: `delta`.

#### Replay Scenarios (both formats)

| Scenario | `seriesFormat=delta` | `seriesFormat=accumulated` |
|---|---|---|
| Subscribe from start | Original delta stream | Each event's `data[field]` replaced with acc value |
| Late-join (series active) | One acc snapshot per series + subsequent deltas | One acc snapshot per series + subsequent acc events |
| Late-join (series no longer updating) | Single acc snapshot per series | Single acc snapshot per series |
| Terminal task replay | Single acc snapshot per series | Single acc snapshot per series |
| Reconnect with `since` cursor | Deltas from breakpoint (no collapse) | Accumulated events from breakpoint |
| Non-series events | Normal replay, unaffected | Normal replay, unaffected |
| Multiple series, mixed states | Each series independent: active → snapshot+stream, inactive → single snapshot | Same collapse behavior |

**Key rule:** Late-join and terminal replay **always collapse** accumulate series to a single snapshot, regardless of `seriesFormat`. The format only affects what happens after the snapshot (deltas vs accumulated events for subsequent live events).

### Late-Join Behavior

"Late-join" means the subscriber connects when ShortTermStore already has events for a series.

Both `seriesFormat=delta` and `seriesFormat=accumulated` share the same replay logic:

1. During history replay, **skip** all historical delta events for `accumulate` series
2. Emit one snapshot event per series from `seriesLatest` with `seriesSnapshot: true`
3. Switch to live event stream (delta or accumulated depending on `seriesFormat`)

The difference between formats only applies to **live events after the snapshot**:
- `delta`: subsequent events are original deltas
- `accumulated`: subsequent events have `data[field]` replaced with accumulated value

### `seriesSnapshot` Event Marker

A new optional field on `TaskEvent`:

```typescript
interface TaskEvent {
  // ... existing fields
  seriesSnapshot?: boolean  // true = accumulated snapshot, not an incremental delta
}
```

Consumers use this to distinguish snapshots from regular deltas. Only present on events generated by the late-join / terminal replay collapse mechanism.

### REST History Endpoint

**No `seriesFormat` parameter.** The history endpoint returns data as stored:
- From ShortTermStore (hot) -> deltas
- From LongTermStore (cold) -> accumulated values

No transformation applied. This avoids semantic confusion where requesting `seriesFormat=delta` might return accumulated data because ShortTermStore was already cleaned up.

### Series Mode Semantics

| Mode | ShortTermStore | LongTermStore | Intent |
|---|---|---|---|
| `keep-all` | Original events | Original events | Preserve full delta history |
| `accumulate` | Deltas | Accumulated | Server-side accumulation; deltas are ephemeral |
| `latest` | Latest only | Latest only | Only current value matters |

## Breaking Changes

- **Default format is `delta`** — existing consumers receiving accumulated values must add `seriesFormat=accumulated` to their SSE subscription.
- **ShortTermStore content changes** — accumulate events stored as deltas instead of accumulated values. REST history returns deltas for hot data.
- **ShortTermStore interface change** — new `accumulateSeries` method required on all adapter implementations.

## Scope of Changes

### Core (TypeScript + Rust)

- `series.ts` / `series.rs` — `processSeries` returns `SeriesResult` with delta event + accumulated value
- `engine.ts` / `engine.rs` — Attach accumulated value to broadcast; LongTermStore receives accumulated event
- `types.ts` / `types.rs` — Add `seriesSnapshot` to TaskEvent, add `seriesFormat` to SubscribeFilter, add `SeriesResult` type

### Storage Adapters (TypeScript + Rust)

- `ShortTermStore` interface — Add `accumulateSeries` method
- Memory adapter — Implement as simple read-concat-write (single-threaded, safe)
- Redis adapter — Implement via Lua script for atomic read-concat-write
- SQLite adapter — Implement via single SQL transaction

### Server (TypeScript + Rust)

- SSE route — Parse `seriesFormat`, late-join snapshot collapse logic, accumulated format using transient broadcast value
- REST route — No changes

### Client

- `client.ts` — Update SubscribeFilter type with `seriesFormat`
- `react` — Pass through `seriesFormat` parameter

### Documentation

- API reference — Document `seriesFormat` parameter
- User guide — Update accumulate mode description, series mode explanations
- Outdated content — Review and fix any references to old accumulate behavior

## Test Plan

### Unit Tests (core)

- `processSeries` returns `SeriesResult` with original delta and correct accumulated value
- `seriesLatest` correctly maintains accumulated state after each delta
- Different `seriesAccField` values work correctly
- Edge cases: empty series, first event, non-string field, null data

### Unit Tests (adapters)

- Memory adapter `accumulateSeries`: correct concatenation, returns accumulated event
- Redis adapter `accumulateSeries`: Lua script atomicity (mock concurrent calls)
- SQLite adapter `accumulateSeries`: transaction isolation

### Integration Tests (server)

- SSE `seriesFormat=delta`: subscriber from start receives delta stream
- SSE `seriesFormat=accumulated`: subscriber receives accumulated values
- SSE late-join (delta): receives `seriesSnapshot: true` snapshot + subsequent deltas
- SSE late-join (accumulated): receives snapshot + subsequent accumulated events
- SSE late-join with series no longer updating: single snapshot, no subsequent series events
- SSE terminal task replay: each series collapsed to single snapshot (both formats)
- SSE reconnect with `since` cursor (delta): deltas from breakpoint, no collapse
- SSE reconnect with `since` cursor (accumulated): accumulated events from breakpoint
- Multiple series interleaved: each series handled independently
- Non-series events unaffected by `seriesFormat`
- LongTermStore receives accumulated values (not deltas)
- REST history returns ShortTermStore raw data (deltas)

### Concurrency / Timing Tests

- Worker rapidly publishes N deltas; SSE subscriber receives all in order, no drops
- SSE subscriber joins **mid-stream** while worker is publishing: snapshot is complete, subsequent deltas have no gaps or duplicates
- Multiple series interleaved: each series accumulates independently and correctly
- Mixed subscribers (delta + accumulated) on same task: each receives correct format
- High-frequency publishing (100+ deltas in burst): seriesLatest stays consistent via atomic operations
- Subscriber disconnect + reconnect with `since` cursor: deltas resume from breakpoint, no loss, no duplication
- **Distributed concurrency**: two concurrent POSTs to same series — accumulated value is correct (no data loss from race condition)

### End-to-End Tests

- Full flow: create task -> worker publishes delta stream -> SSE clients subscribe with delta/accumulated -> verify final concatenation matches
- Mid-stream join: task has published half its deltas -> new client joins -> verify snapshot + remaining deltas == full accumulated result
- Series completion: series stops updating -> late-joiner gets single snapshot only
- Terminal task: task completes -> subscriber gets collapsed snapshots for all series