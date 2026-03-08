# Series Format Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Let SSE subscribers choose between receiving original deltas or accumulated values for `accumulate` series mode, with atomic series processing and late-join snapshot collapse.

**Architecture:** Change `processSeries` to return both delta and accumulated events. Store deltas in ShortTermStore, accumulated in LongTermStore. Add `accumulateSeries` atomic method to storage adapters. SSE route parses `seriesFormat` parameter and applies late-join collapse logic.

**Tech Stack:** TypeScript (core/server/client/react), Rust (core/server/redis), Vitest, Redis Lua scripts, Hono SSE, Axum SSE

**Design doc:** `docs/plans/2026-03-09-series-format-design.md`

---

## Task 1: TypeScript Core Types

**Files:**
- Modify: `packages/core/src/types.ts`

**Step 1: Add `seriesSnapshot` to TaskEvent (line ~183)**

```typescript
export interface TaskEvent {
  id: string
  taskId: string
  index: number
  timestamp: number
  type: string
  level: Level
  data: unknown
  seriesId?: string
  seriesMode?: SeriesMode
  seriesAccField?: string
  seriesSnapshot?: boolean          // NEW: true = accumulated snapshot, not delta
  /** Transient: accumulated data attached during broadcast, not persisted in ShortTermStore */
  _accumulatedData?: unknown        // NEW: for SSE accumulated format
}
```

**Step 2: Add `seriesSnapshot` and `_accumulatedData` to SSEEnvelope (line ~197)**

```typescript
export interface SSEEnvelope {
  filteredIndex: number
  rawIndex: number
  eventId: string
  taskId: string
  type: string
  timestamp: number
  level: Level
  data: unknown
  seriesId?: string
  seriesMode?: SeriesMode
  seriesAccField?: string
  seriesSnapshot?: boolean          // NEW
}
```

Note: `_accumulatedData` is NOT in SSEEnvelope — it's internal only.

**Step 3: Add `seriesFormat` to SubscribeFilter (line ~207)**

```typescript
export type SeriesFormat = 'delta' | 'accumulated'  // NEW

export interface SubscribeFilter {
  since?: SinceCursor
  types?: string[]
  levels?: Level[]
  includeStatus?: boolean
  wrap?: boolean
  seriesFormat?: SeriesFormat        // NEW: default 'delta'
}
```

**Step 4: Add `SeriesResult` type**

```typescript
export interface SeriesResult {
  /** The original delta event (stored in ShortTermStore) */
  event: TaskEvent
  /** The event with accumulated data (for LongTermStore + broadcast). Undefined for non-accumulate modes. */
  accumulatedEvent?: TaskEvent
}
```

**Step 5: Add `accumulateSeries` to ShortTermStore interface (line ~237)**

```typescript
export interface ShortTermStore {
  // ... existing methods ...
  getSeriesLatest(taskId: string, seriesId: string): Promise<TaskEvent | null>
  setSeriesLatest(taskId: string, seriesId: string, event: TaskEvent): Promise<void>
  replaceLastSeriesEvent(taskId: string, seriesId: string, event: TaskEvent): Promise<void>
  /** Atomically read previous accumulated value, concatenate with new delta, write back. Returns the accumulated event. */
  accumulateSeries(taskId: string, seriesId: string, event: TaskEvent, field: string): Promise<TaskEvent>  // NEW
  // ... rest ...
}
```

**Step 6: Export new types from core index**

Check `packages/core/src/index.ts` and ensure `SeriesFormat`, `SeriesResult` are exported.

**Step 7: Commit**

```
feat(core): add types for seriesFormat, seriesSnapshot, SeriesResult, accumulateSeries
```

---

## Task 2: TypeScript Series Processing (TDD)

**Files:**
- Modify: `packages/core/src/series.ts`
- Modify: `packages/core/tests/unit/series.test.ts`

**Step 1: Update test helper `makeStore` to include `accumulateSeries` mock**

In `packages/core/tests/unit/series.test.ts`, the `makeStore` helper (line 16) needs the new method. Add it alongside the other mocks. The mock should implement the actual accumulate logic (read prev from mock, concat, return accumulated event) so tests can verify the interaction:

```typescript
const makeStore = (latestEvent?: TaskEvent): ShortTermStore => {
  let storedLatest = latestEvent ?? null
  return {
    saveTask: vi.fn(),
    getTask: vi.fn(),
    nextIndex: vi.fn(),
    appendEvent: vi.fn(),
    getEvents: vi.fn(),
    setTTL: vi.fn(),
    getSeriesLatest: vi.fn().mockImplementation(() => Promise.resolve(storedLatest)),
    setSeriesLatest: vi.fn().mockImplementation((_tid, _sid, evt) => { storedLatest = evt; return Promise.resolve() }),
    replaceLastSeriesEvent: vi.fn(),
    accumulateSeries: vi.fn().mockImplementation((_tid, _sid, event, field) => {
      const prevData = (typeof storedLatest?.data === 'object' && storedLatest?.data !== null)
        ? storedLatest.data as Record<string, unknown> : {}
      const newData = (typeof event.data === 'object' && event.data !== null)
        ? event.data as Record<string, unknown> : {}
      let accEvent = event
      if (typeof prevData[field] === 'string' && typeof newData[field] === 'string') {
        accEvent = { ...event, data: { ...newData, [field]: prevData[field] + newData[field] } }
      }
      storedLatest = accEvent
      return Promise.resolve(accEvent)
    }),
    // ... include all other required ShortTermStore methods as vi.fn()
  } as unknown as ShortTermStore
}
```

Note: The mock needs ALL ShortTermStore methods. Check the interface and add any missing ones (listTasks, saveWorker, getWorker, listWorkers, deleteWorker, claimTask, addAssignment, removeAssignment, getWorkerAssignments, getTaskAssignment, clearTTL, listByStatus) as `vi.fn()`.

**Step 2: Write failing tests for new `processSeries` behavior**

Add a new describe block for the changed accumulate return value:

```typescript
describe('processSeries - accumulate returns SeriesResult', () => {
  it('returns delta event (original) and accumulated event', async () => {
    const prev = makeEvent({ data: { delta: 'hello ' }, seriesId: 's1', seriesMode: 'accumulate' })
    const store = makeStore(prev)
    const event = makeEvent({ data: { delta: 'world' }, seriesId: 's1', seriesMode: 'accumulate' })
    const result = await processSeries(event, store)
    // Delta event should be the ORIGINAL event, unchanged
    expect((result.event.data as { delta: string }).delta).toBe('world')
    // Accumulated event should have concatenated value
    expect((result.accumulatedEvent!.data as { delta: string }).delta).toBe('hello world')
  })

  it('returns delta event and accumulated event for first event (no previous)', async () => {
    const store = makeStore()
    const event = makeEvent({ data: { delta: 'start' }, seriesId: 's1', seriesMode: 'accumulate' })
    const result = await processSeries(event, store)
    expect((result.event.data as { delta: string }).delta).toBe('start')
    // First event: accumulated is same as delta
    expect((result.accumulatedEvent!.data as { delta: string }).delta).toBe('start')
  })

  it('calls accumulateSeries instead of getSeriesLatest+setSeriesLatest', async () => {
    const store = makeStore()
    const event = makeEvent({ data: { delta: 'test' }, seriesId: 's1', seriesMode: 'accumulate' })
    await processSeries(event, store)
    expect(store.accumulateSeries).toHaveBeenCalledWith('task-1', 's1', event, 'delta')
    expect(store.getSeriesLatest).not.toHaveBeenCalled()
    expect(store.setSeriesLatest).not.toHaveBeenCalled()
  })

  it('uses custom seriesAccField for accumulateSeries call', async () => {
    const store = makeStore()
    const event = makeEvent({ data: { content: 'test' }, seriesId: 's1', seriesMode: 'accumulate', seriesAccField: 'content' })
    await processSeries(event, store)
    expect(store.accumulateSeries).toHaveBeenCalledWith('task-1', 's1', event, 'content')
  })

  it('keep-all returns SeriesResult with no accumulatedEvent', async () => {
    const store = makeStore()
    const event = makeEvent({ seriesId: 's1', seriesMode: 'keep-all' })
    const result = await processSeries(event, store)
    expect(result.event).toEqual(event)
    expect(result.accumulatedEvent).toBeUndefined()
  })

  it('latest returns SeriesResult with no accumulatedEvent', async () => {
    const store = makeStore()
    const event = makeEvent({ seriesId: 's1', seriesMode: 'latest', data: { delta: 'new' } })
    const result = await processSeries(event, store)
    expect(result.event).toEqual(event)
    expect(result.accumulatedEvent).toBeUndefined()
  })

  it('no seriesId returns SeriesResult with no accumulatedEvent', async () => {
    const store = makeStore()
    const event = makeEvent()
    const result = await processSeries(event, store)
    expect(result.event).toEqual(event)
    expect(result.accumulatedEvent).toBeUndefined()
  })
})
```

**Step 3: Run tests to verify they fail**

```bash
cd packages/core && pnpm test -- --run tests/unit/series.test.ts
```

Expected: FAIL — `processSeries` still returns `TaskEvent` not `SeriesResult`.

**Step 4: Implement new `processSeries`**

Rewrite `packages/core/src/series.ts`:

```typescript
import type { TaskEvent, ShortTermStore, SeriesResult } from './types.js'

export async function processSeries(
  event: TaskEvent,
  store: ShortTermStore,
): Promise<SeriesResult> {
  if (!event.seriesId || !event.seriesMode) {
    return { event }
  }

  const { seriesId, seriesMode, taskId } = event

  if (seriesMode === 'keep-all') {
    return { event }
  }

  if (seriesMode === 'accumulate') {
    const field = event.seriesAccField ?? 'delta'
    const accumulatedEvent = await store.accumulateSeries(taskId, seriesId, event, field)
    return { event, accumulatedEvent }
  }

  // 'latest'
  await store.replaceLastSeriesEvent(taskId, seriesId, event)
  return { event }
}
```

**Step 5: Update existing accumulate tests**

The existing tests (lines 39-98) test `processSeries` return value expecting the accumulated event directly. They need updating since the return type changed. For each existing test:
- Change `result` to `result.event` for delta checks
- Add `result.accumulatedEvent` checks where appropriate
- Remove tests that checked `setSeriesLatest` was called (it's now inside `accumulateSeries`)

Example fix for "concatenates delta field when previous exists" (line 40):
```typescript
it('concatenates delta field when previous exists', async () => {
  const prev = makeEvent({ data: { delta: 'hello ' }, seriesId: 's1', seriesMode: 'accumulate' })
  const store = makeStore(prev)
  const event = makeEvent({ data: { delta: 'world' }, seriesId: 's1', seriesMode: 'accumulate' })
  const result = await processSeries(event, store)
  // Delta event is original
  expect((result.event.data as { delta: string }).delta).toBe('world')
  // Accumulated event has concatenated value
  expect((result.accumulatedEvent!.data as { delta: string }).delta).toBe('hello world')
  expect(store.accumulateSeries).toHaveBeenCalledWith('task-1', 's1', event, 'delta')
})
```

**Step 6: Run tests to verify they pass**

```bash
cd packages/core && pnpm test -- --run tests/unit/series.test.ts
```

Expected: PASS

**Step 7: Commit**

```
feat(core): processSeries returns SeriesResult with delta + accumulated events
```

---

## Task 3: TypeScript Memory Adapter

**Files:**
- Modify: `packages/core/src/memory-adapters.ts`
- Test: `packages/core/tests/unit/memory-adapters.test.ts` (create if doesn't exist, or add to existing)

**Step 1: Write failing test for `accumulateSeries`**

```typescript
describe('MemoryShortTermStore.accumulateSeries', () => {
  it('returns accumulated event with concatenated field', async () => {
    const store = new MemoryShortTermStore()
    // Set up previous accumulated state
    const prev: TaskEvent = { id: '1', taskId: 't1', index: 0, timestamp: 1, type: 'x', level: 'info', data: { delta: 'hello ' }, seriesId: 's1', seriesMode: 'accumulate' }
    await store.setSeriesLatest('t1', 's1', prev)

    const event: TaskEvent = { id: '2', taskId: 't1', index: 1, timestamp: 2, type: 'x', level: 'info', data: { delta: 'world' }, seriesId: 's1', seriesMode: 'accumulate' }
    const result = await store.accumulateSeries('t1', 's1', event, 'delta')

    expect((result.data as { delta: string }).delta).toBe('hello world')
    // seriesLatest should be updated
    const latest = await store.getSeriesLatest('t1', 's1')
    expect((latest!.data as { delta: string }).delta).toBe('hello world')
  })

  it('returns event as-is when no previous exists', async () => {
    const store = new MemoryShortTermStore()
    const event: TaskEvent = { id: '1', taskId: 't1', index: 0, timestamp: 1, type: 'x', level: 'info', data: { delta: 'start' }, seriesId: 's1', seriesMode: 'accumulate' }
    const result = await store.accumulateSeries('t1', 's1', event, 'delta')
    expect((result.data as { delta: string }).delta).toBe('start')
  })

  it('handles non-string field gracefully', async () => {
    const store = new MemoryShortTermStore()
    const prev: TaskEvent = { id: '1', taskId: 't1', index: 0, timestamp: 1, type: 'x', level: 'info', data: { count: 1 }, seriesId: 's1', seriesMode: 'accumulate' }
    await store.setSeriesLatest('t1', 's1', prev)

    const event: TaskEvent = { id: '2', taskId: 't1', index: 1, timestamp: 2, type: 'x', level: 'info', data: { count: 2 }, seriesId: 's1', seriesMode: 'accumulate' }
    const result = await store.accumulateSeries('t1', 's1', event, 'delta')
    // No string concat possible, returns event data unchanged
    expect(result.data).toEqual({ count: 2 })
  })
})
```

**Step 2: Run to verify fail**

```bash
cd packages/core && pnpm test -- --run tests/unit/memory-adapters.test.ts
```

**Step 3: Implement `accumulateSeries` in memory adapter**

Add to `MemoryShortTermStore` class in `packages/core/src/memory-adapters.ts` (after `setSeriesLatest`, around line 91):

```typescript
async accumulateSeries(taskId: string, seriesId: string, event: TaskEvent, field: string): Promise<TaskEvent> {
  const key = `${taskId}:${seriesId}`
  const prev = this.seriesLatest.get(key) ?? null

  let accumulated = event
  if (prev !== null) {
    const prevData = (typeof prev.data === 'object' && prev.data !== null)
      ? prev.data as Record<string, unknown> : {}
    const newData = (typeof event.data === 'object' && event.data !== null)
      ? event.data as Record<string, unknown> : {}
    if (typeof prevData[field] === 'string' && typeof newData[field] === 'string') {
      accumulated = {
        ...event,
        data: { ...newData, [field]: prevData[field] + newData[field] },
      }
    }
  }

  this.seriesLatest.set(key, { ...accumulated })
  return accumulated
}
```

**Step 4: Run tests to verify pass**

**Step 5: Commit**

```
feat(core): add accumulateSeries to MemoryShortTermStore
```

---

## Task 4: TypeScript Engine Changes

**Files:**
- Modify: `packages/core/src/engine.ts`
- Test: `packages/core/tests/unit/engine.test.ts`

**Step 1: Write failing test for engine emit behavior**

Add tests that verify:
1. ShortTermStore receives the delta event (not accumulated)
2. Broadcast receives the delta event with `_accumulatedData` attached
3. LongTermStore receives the accumulated event

```typescript
describe('engine._emit series format', () => {
  it('stores delta in ShortTermStore and accumulated in LongTermStore', async () => {
    // Setup engine with mock stores
    // Publish two accumulate events to same series
    // Verify shortTermStore.appendEvent received delta
    // Verify longTermStore.saveEvent received accumulated
  })

  it('attaches _accumulatedData to broadcast event', async () => {
    // Setup engine with mock broadcast
    // Publish accumulate event
    // Verify broadcast.publish received event with _accumulatedData
  })
})
```

**Step 2: Update `_emit` in `packages/core/src/engine.ts` (line 308)**

```typescript
private async _emit(taskId: string, input: PublishEventInput): Promise<TaskEvent> {
  const index = await this.shortTermStore.nextIndex(taskId)
  const raw: TaskEvent = {
    id: ulid(),
    taskId,
    index,
    timestamp: Date.now(),
    type: input.type,
    level: input.level,
    data: input.data,
    ...(input.seriesId !== undefined && { seriesId: input.seriesId }),
    ...(input.seriesMode !== undefined && { seriesMode: input.seriesMode }),
    ...(input.seriesAccField !== undefined && { seriesAccField: input.seriesAccField }),
  }

  const { event, accumulatedEvent } = await processSeries(raw, this.shortTermStore)
  await this.shortTermStore.appendEvent(taskId, event)

  // Attach accumulated data to broadcast for SSE accumulated subscribers
  const broadcastEvent = accumulatedEvent
    ? { ...event, _accumulatedData: accumulatedEvent.data }
    : event
  await this.broadcast.publish(taskId, broadcastEvent)

  if (this.longTermStore) {
    // LongTermStore gets accumulated event (or delta if non-accumulate)
    const storeEvent = accumulatedEvent ?? event
    this.longTermStore.saveEvent(storeEvent).catch((err) => {
      this.hooks?.onEventDropped?.(storeEvent, String(err))
    })
  }

  return event
}
```

Note: The `processSeries` import return type changes from `TaskEvent` to `SeriesResult`. Update the import.

**Step 3: Run all core tests**

```bash
cd packages/core && pnpm test -- --run
```

Fix any existing tests that break due to the return type change. Key files to check:
- `packages/core/tests/unit/engine.test.ts` — tests that check `publishEvent` return value
- `packages/core/tests/integration/lifecycle.test.ts` — series accumulate lifecycle tests
- `packages/core/tests/integration/engine-full.test.ts` — accumulate tests

The return value of `publishEvent` is now the **delta event**, not the accumulated event. Tests asserting accumulated values from `publishEvent` need updating.

**Step 4: Commit**

```
feat(core): engine stores delta in ShortTermStore, accumulated in LongTermStore
```

---

## Task 5: TypeScript SSE Route — seriesFormat Parsing + Accumulated Mode

**Files:**
- Modify: `packages/server/src/routes/sse.ts`

**Step 1: Add `seriesFormat` to `parseFilter` (line 65)**

```typescript
function parseFilter(query: Record<string, string | undefined>): SubscribeFilter {
  // ... existing code ...

  const seriesFormat = get('seriesFormat')
  if (seriesFormat === 'delta' || seriesFormat === 'accumulated') {
    filter.seriesFormat = seriesFormat
  }

  return filter
}
```

**Step 2: Update `toEnvelope` to include `seriesSnapshot` (line 95)**

```typescript
function toEnvelope(event: TaskEvent, filteredIndex: number): SSEEnvelope {
  const env: SSEEnvelope = {
    filteredIndex,
    rawIndex: event.index,
    eventId: event.id,
    taskId: event.taskId,
    type: event.type,
    timestamp: event.timestamp,
    level: event.level,
    data: event.data,
  }
  if (event.seriesId !== undefined) env.seriesId = event.seriesId
  if (event.seriesMode !== undefined) env.seriesMode = event.seriesMode
  if (event.seriesAccField !== undefined) env.seriesAccField = event.seriesAccField
  if (event.seriesSnapshot !== undefined) env.seriesSnapshot = event.seriesSnapshot  // NEW
  return env
}
```

**Step 3: Update `sendEvent` to handle accumulated format for live events**

In the SSE handler (line 141), modify `sendEvent` to swap data for accumulated subscribers:

```typescript
const seriesFormat = filter.seriesFormat ?? 'delta'

const sendEvent = async (event: TaskEvent, filteredIndex: number) => {
  let eventToSend = event
  // For accumulated format: replace data with accumulated data if available
  if (seriesFormat === 'accumulated' && event._accumulatedData !== undefined) {
    eventToSend = { ...event, data: event._accumulatedData }
  }
  // Strip transient field before sending
  const { _accumulatedData: _, ...cleanEvent } = eventToSend
  const payload = wrap ? toEnvelope(cleanEvent as TaskEvent, filteredIndex) : cleanEvent
  await stream.writeSSE({
    event: 'taskcast.event',
    data: JSON.stringify(payload),
    id: event.id,
  })
}
```

**Step 4: Commit**

```
feat(server): parse seriesFormat parameter, support accumulated format for live events
```

---

## Task 6: TypeScript SSE Route — Late-Join Snapshot Collapse

This is the most complex part. When replaying history, accumulate series events should be collapsed into a single snapshot.

**Files:**
- Modify: `packages/server/src/routes/sse.ts`
- Modify: `packages/core/src/filter.ts` (optional: add helper)

**Step 1: Implement snapshot collapse in history replay**

Replace the simple history replay (lines 157-168) with collapse logic:

```typescript
// Replay history
let history: TaskEvent[]
try {
  history = await engine.getEvents(taskId)
} catch {
  cleanup()
  return
}

const seriesFormat = filter.seriesFormat ?? 'delta'

// Determine which series need snapshot collapse
// A series needs collapse if: it uses accumulate mode AND has events in history
// (regardless of seriesFormat — both delta and accumulated modes collapse on late-join)
const hasAccumulateSeries = history.some(e => e.seriesMode === 'accumulate' && e.seriesId)
const hasSinceCursor = !!filter.since

if (hasAccumulateSeries && !hasSinceCursor) {
  // Collapse mode: replace accumulate series events with snapshots
  const seriesIds = new Set<string>()
  const collapsedHistory: TaskEvent[] = []

  // Collect all accumulate series IDs
  for (const event of history) {
    if (event.seriesMode === 'accumulate' && event.seriesId) {
      seriesIds.add(event.seriesId)
    }
  }

  // Get snapshots for each series
  const snapshots = new Map<string, TaskEvent>()
  for (const seriesId of seriesIds) {
    const latest = await engine.getSeriesLatest(taskId, seriesId)
    if (latest) {
      snapshots.set(seriesId, { ...latest, seriesSnapshot: true })
    }
  }

  // Build collapsed history: skip accumulate series events, insert snapshot at first occurrence position
  const emittedSnapshots = new Set<string>()
  for (const event of history) {
    if (event.seriesMode === 'accumulate' && event.seriesId && seriesIds.has(event.seriesId)) {
      // Replace first occurrence with snapshot, skip rest
      if (!emittedSnapshots.has(event.seriesId)) {
        const snapshot = snapshots.get(event.seriesId)
        if (snapshot) {
          collapsedHistory.push(snapshot)
          emittedSnapshots.add(event.seriesId)
        }
      }
      // Skip all other events in this series
    } else {
      collapsedHistory.push(event)
    }
  }

  const filtered = applyFilteredIndex(collapsedHistory, filter)
  for (const { event, filteredIndex } of filtered) {
    await sendEvent(event, filteredIndex)
  }
} else {
  // No collapse: normal replay (reconnect with since cursor, or no accumulate series)
  const filtered = applyFilteredIndex(history, filter)
  for (const { event, filteredIndex } of filtered) {
    await sendEvent(event, filteredIndex)
  }
}
```

**Important:** This requires `engine.getSeriesLatest()` to be exposed. Check if `TaskEngine` already exposes it. If not, add:

```typescript
// In engine.ts
async getSeriesLatest(taskId: string, seriesId: string): Promise<TaskEvent | null> {
  return this.shortTermStore.getSeriesLatest(taskId, seriesId)
}
```

**Step 2: Update nextFilteredIndex calculation**

The `nextFilteredIndex` (line 178) needs to account for the collapsed history:

```typescript
// After the replay block, filtered is available in both branches
// Refactor to: let filtered be assigned in both branches, then use it below
```

Refactor the replay section so `filtered` is accessible after both branches for computing `nextFilteredIndex`.

**Step 3: Commit**

```
feat(server): late-join snapshot collapse for accumulate series in SSE replay
```

---

## Task 7: TypeScript SSE Integration Tests

**Files:**
- Modify: `packages/server/tests/integration/sse-streaming.test.ts`
- May need: `packages/server/tests/sse.test.ts`

**Step 1: Test `seriesFormat=delta` from start**

```typescript
it('seriesFormat=delta receives original deltas', async () => {
  // Create task, publish 3 accumulate events
  // Subscribe with seriesFormat=delta
  // Verify each event has original delta, not accumulated
})
```

**Step 2: Test `seriesFormat=accumulated` from start**

```typescript
it('seriesFormat=accumulated receives accumulated values', async () => {
  // Create task, start SSE with seriesFormat=accumulated
  // Publish 3 accumulate events
  // Verify events have accumulated values: "a", "ab", "abc"
})
```

**Step 3: Test late-join snapshot (delta mode)**

```typescript
it('late-join receives snapshot then deltas', async () => {
  // Create task, transition to running
  // Publish 5 accumulate events
  // THEN subscribe with seriesFormat=delta (no since cursor)
  // Verify: first event is snapshot with seriesSnapshot=true and full accumulated value
  // Publish 2 more events
  // Verify: subsequent events are deltas
})
```

**Step 4: Test late-join snapshot (accumulated mode)**

```typescript
it('late-join accumulated receives snapshot then accumulated events', async () => {
  // Same setup as above but seriesFormat=accumulated
  // Verify: snapshot, then accumulated values for subsequent events
})
```

**Step 5: Test terminal task replay**

```typescript
it('terminal task collapses series to single snapshot', async () => {
  // Create task, publish events, transition to completed
  // Subscribe (task already terminal)
  // Verify: single snapshot per series, then taskcast.done
})
```

**Step 6: Test reconnect with `since` cursor does NOT collapse**

```typescript
it('reconnect with since cursor sends deltas without collapse', async () => {
  // Create task, publish events
  // Subscribe with since.index=2
  // Verify: receives delta events from index 3 onwards, NO snapshot
})
```

**Step 7: Test multiple series interleaved**

```typescript
it('multiple series each get independent snapshots', async () => {
  // Create task, publish events to series A and B interleaved
  // Late-join subscribe
  // Verify: one snapshot per series, non-series events preserved
})
```

**Step 8: Test mixed subscribers**

```typescript
it('delta and accumulated subscribers on same task receive correct formats', async () => {
  // Create task
  // Subscribe client A with seriesFormat=delta
  // Subscribe client B with seriesFormat=accumulated
  // Publish events
  // Verify A gets deltas, B gets accumulated values
})
```

**Step 9: Commit**

```
test(server): add SSE integration tests for seriesFormat and late-join snapshots
```

---

## Task 8: TypeScript Redis Adapter — Atomic accumulateSeries

**Files:**
- Modify: `packages/redis/src/short-term.ts`
- Test: `packages/redis/tests/short-term.test.ts`

**Step 1: Add Lua script for atomic accumulate**

```typescript
const ACCUMULATE_SERIES_SCRIPT = `
local key = KEYS[1]
local event_json = ARGV[1]
local field = ARGV[2]

local prev_json = redis.call('GET', key)
local event = cjson.decode(event_json)
local event_data = event['data']

if prev_json then
  local prev = cjson.decode(prev_json)
  local prev_data = prev['data']
  if type(prev_data) == 'table' and type(event_data) == 'table'
     and type(prev_data[field]) == 'string' and type(event_data[field]) == 'string' then
    event_data[field] = prev_data[field] .. event_data[field]
    event['data'] = event_data
  end
end

local result_json = cjson.encode(event)
redis.call('SET', key, result_json)
return result_json
`
```

**Step 2: Implement `accumulateSeries` method**

```typescript
async accumulateSeries(taskId: string, seriesId: string, event: TaskEvent, field: string): Promise<TaskEvent> {
  const key = this.seriesKey(taskId, seriesId)
  const eventJson = JSON.stringify(event)
  const resultJson = await this.redis.eval(
    ACCUMULATE_SERIES_SCRIPT,
    1,        // numkeys
    key,      // KEYS[1]
    eventJson, // ARGV[1]
    field,     // ARGV[2]
  ) as string
  // Also add to seriesIds set for TTL tracking
  await this.redis.sadd(this.seriesIdsKey(taskId), seriesId)
  return JSON.parse(resultJson) as TaskEvent
}
```

**Step 3: Write integration test (requires testcontainers Redis)**

```typescript
it('accumulateSeries atomically concatenates field', async () => {
  // Publish event 1 via accumulateSeries
  // Publish event 2 via accumulateSeries
  // Verify second result has concatenated value
  // Verify getSeriesLatest returns accumulated
})
```

**Step 4: Commit**

```
feat(redis): atomic accumulateSeries via Lua script
```

---

## Task 9: TypeScript SQLite Adapter — accumulateSeries

**Files:**
- Modify: `packages/sqlite/src/short-term.ts`
- Test: `packages/sqlite/tests/short-term.test.ts`

**Step 1: Implement `accumulateSeries` using SQL transaction**

The SQLite adapter can use a transaction since it's single-process:

```typescript
async accumulateSeries(taskId: string, seriesId: string, event: TaskEvent, field: string): Promise<TaskEvent> {
  // Read previous from taskcast_series_latest
  const prev = await this.getSeriesLatest(taskId, seriesId)

  let accumulated = event
  if (prev !== null) {
    const prevData = (typeof prev.data === 'object' && prev.data !== null)
      ? prev.data as Record<string, unknown> : {}
    const newData = (typeof event.data === 'object' && event.data !== null)
      ? event.data as Record<string, unknown> : {}
    if (typeof prevData[field] === 'string' && typeof newData[field] === 'string') {
      accumulated = {
        ...event,
        data: { ...newData, [field]: prevData[field] + newData[field] },
      }
    }
  }

  await this.setSeriesLatest(taskId, seriesId, accumulated)
  return accumulated
}
```

**Step 2: Write test and verify**

**Step 3: Commit**

```
feat(sqlite): add accumulateSeries method
```

---

## Task 10: TypeScript Client + React Updates

**Files:**
- Modify: `packages/client/src/client.ts`
- Modify: `packages/react/src/useTaskcast.ts`

**Step 1: Update client URL builder**

In `packages/client/src/client.ts`, find where SubscribeFilter is translated to URL params. Add `seriesFormat`:

```typescript
if (filter.seriesFormat) {
  params.set('seriesFormat', filter.seriesFormat)
}
```

**Step 2: Update React hook**

In `packages/react/src/useTaskcast.ts`, ensure `seriesFormat` is passed through the options. This should happen automatically if the hook passes `SubscribeFilter` to the client.

**Step 3: Commit**

```
feat(client): pass seriesFormat parameter in SSE subscription URL
```

---

## Task 11: Rust Core Types

**Files:**
- Modify: `rust/taskcast-core/src/types.rs`

**Step 1: Add `series_snapshot` to TaskEvent (line ~403)**

```rust
pub struct TaskEvent {
    // ... existing fields ...
    #[serde(skip_serializing_if = "Option::is_none")]
    pub series_acc_field: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub series_snapshot: Option<bool>,          // NEW
    #[serde(skip_serializing, skip_deserializing)]
    pub _accumulated_data: Option<serde_json::Value>,  // NEW: transient, not serialized
}
```

**Step 2: Add `SeriesFormat` enum**

```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub enum SeriesFormat {
    Delta,
    Accumulated,
}
```

**Step 3: Add `series_format` to `SubscribeFilter`**

```rust
pub struct SubscribeFilter {
    // ... existing fields ...
    pub series_format: Option<SeriesFormat>,  // NEW
}
```

**Step 4: Add `SeriesResult` struct**

```rust
pub struct SeriesResult {
    pub event: TaskEvent,
    pub accumulated_event: Option<TaskEvent>,
}
```

**Step 5: Add `series_snapshot` to SSEEnvelope**

```rust
pub struct SSEEnvelope {
    // ... existing fields ...
    #[serde(skip_serializing_if = "Option::is_none")]
    pub series_snapshot: Option<bool>,  // NEW
}
```

**Step 6: Add `accumulate_series` to `ShortTermStore` trait**

```rust
#[async_trait]
pub trait ShortTermStore: Send + Sync {
    // ... existing methods ...
    async fn accumulate_series(
        &self,
        task_id: &str,
        series_id: &str,
        event: TaskEvent,
        field: &str,
    ) -> Result<TaskEvent, Box<dyn std::error::Error + Send + Sync>>;
}
```

**Step 7: Commit**

```
feat(rust/core): add types for SeriesFormat, SeriesResult, series_snapshot, accumulate_series
```

---

## Task 12: Rust Series Processing + Engine

**Files:**
- Modify: `rust/taskcast-core/src/series.rs`
- Modify: `rust/taskcast-core/src/engine.rs`

**Step 1: Update `process_series` to return `SeriesResult`**

Mirror the TypeScript changes:
- `keep-all` and `latest`: return `SeriesResult { event, accumulated_event: None }`
- `accumulate`: call `store.accumulate_series()`, return both delta and accumulated

**Step 2: Update engine `emit` method**

Mirror TypeScript:
- `append_event(delta)`, `broadcast(delta + _accumulated_data)`, `save_event(accumulated)` to long-term store

**Step 3: Update existing Rust tests in `series.rs`**

The 45+ tests in `rust/taskcast-core/src/series.rs` (lines 84-520) need updating for the new return type.

**Step 4: Commit**

```
feat(rust/core): processSeries returns SeriesResult, engine stores delta/accumulated separately
```

---

## Task 13: Rust Redis Adapter — Atomic accumulateSeries

**Files:**
- Modify: `rust/taskcast-redis/src/short_term.rs`

**Step 1: Add Lua script (same as TypeScript)**

**Step 2: Implement `accumulate_series` method**

Use `redis::Script` with the Lua script for atomic execution.

**Step 3: Commit**

```
feat(rust/redis): atomic accumulate_series via Lua script
```

---

## Task 14: Rust SSE Route — seriesFormat + Late-Join

**Files:**
- Modify: `rust/taskcast-server/src/routes/sse.rs`

**Step 1: Add `series_format` to `SseQuery` (line ~53)**

```rust
pub struct SseQuery {
    // ... existing fields ...
    #[serde(rename = "seriesFormat")]
    pub series_format: Option<String>,
}
```

**Step 2: Parse `series_format` in `parse_filter`**

**Step 3: Implement late-join snapshot collapse in SSE handler**

Mirror the TypeScript logic from Task 6.

**Step 4: Handle accumulated format for live events**

Check `_accumulated_data` on broadcast events, swap data for accumulated subscribers.

**Step 5: Update `to_envelope` to include `series_snapshot`**

**Step 6: Commit**

```
feat(rust/server): seriesFormat parameter, late-join snapshot collapse, accumulated format
```

---

## Task 15: Concurrency & Timing Tests

**Files:**
- Create: `packages/server/tests/integration/series-concurrency.test.ts`

**Step 1: Worker publishes N deltas, SSE receives all in order**

```typescript
it('receives all deltas in order under rapid publishing', async () => {
  // Create + start task
  // Subscribe with seriesFormat=delta
  // Rapidly publish 100 accumulate events
  // Verify: all 100 received, in order, no gaps
})
```

**Step 2: Mid-stream join during active publishing**

```typescript
it('mid-stream join snapshot is complete with no gaps', async () => {
  // Create + start task
  // Publish 50 events
  // Subscribe (late-join)
  // Publish 50 more events
  // Verify: snapshot value == concat of first 50, then 50 deltas follow
  // Verify: snapshot + deltas == full accumulated result
})
```

**Step 3: Mixed delta + accumulated subscribers**

```typescript
it('mixed subscribers receive correct formats simultaneously', async () => {
  // Subscribe client A (delta) and client B (accumulated)
  // Publish 20 events
  // Verify A got deltas, B got accumulated values
  // Verify final state matches for both
})
```

**Step 4: Disconnect + reconnect with `since` cursor**

```typescript
it('reconnect with since cursor resumes without gaps or duplication', async () => {
  // Subscribe, receive 10 events, record last filteredIndex
  // Disconnect
  // Publish 5 more events
  // Reconnect with since.index = last filteredIndex
  // Verify: receives exactly the 5 new events, no snapshot
})
```

**Step 5: Multiple series interleaved**

```typescript
it('multiple series accumulate independently under interleaved publishing', async () => {
  // Publish alternating events to series A and B (20 each)
  // Late-join subscribe
  // Verify: snapshot A has full A text, snapshot B has full B text
})
```

**Step 6: Commit**

```
test(server): add concurrency and timing tests for series format
```

---

## Task 16: End-to-End Tests

**Files:**
- Create or modify: `packages/server/tests/e2e/series-format.test.ts`

Check if E2E test infrastructure exists. If so, add:

**Step 1: Full flow test**

```typescript
it('e2e: create task -> publish deltas -> verify delta and accumulated subscribers', async () => {
  // Start real server
  // Create task via REST
  // Connect SSE client A (delta) and B (accumulated)
  // Publish 10 accumulate events via REST
  // Complete task
  // Verify A received all deltas + done
  // Verify B received all accumulated values + done
  // Verify concat(A deltas) === last B value
})
```

**Step 2: Mid-stream join E2E**

```typescript
it('e2e: mid-stream join gets snapshot then deltas', async () => {
  // Start server, create task, publish 5 events
  // Connect SSE late-joiner
  // Publish 5 more events, complete task
  // Verify: snapshot + 5 deltas, final value matches
})
```

**Step 3: Series completion E2E**

```typescript
it('e2e: completed task returns single snapshot per series', async () => {
  // Create task, publish events, complete
  // Connect SSE
  // Verify: single snapshot per series + done
})
```

**Step 4: Commit**

```
test: add end-to-end tests for series format feature
```

---

## Task 17: Update Existing Tests

Existing tests that verify accumulate behavior need updating because:
1. `processSeries` now returns `SeriesResult` instead of `TaskEvent`
2. `publishEvent` now returns the delta event, not accumulated
3. Event history in ShortTermStore now contains deltas, not accumulated values

**Files to check and update:**
- `packages/core/tests/unit/engine.test.ts` (line ~278-293)
- `packages/core/tests/integration/lifecycle.test.ts` (line ~44-71)
- `packages/core/tests/integration/engine-full.test.ts` (line ~81-92)
- `packages/server/tests/tasks.test.ts` (line ~399-418)
- `packages/server/tests/sse.test.ts` (line ~165-180)
- `packages/core/tests/unit/cleanup.test.ts` (line ~130-148)
- `packages/sqlite/tests/short-term.test.ts`
- `packages/redis/tests/short-term.test.ts`

For each file:
1. Read the current test assertions
2. Update assertions that expect accumulated values to expect deltas
3. Ensure test mocks/stores include `accumulateSeries` method
4. Run and verify all pass

**Commit:**

```
test: update existing tests for series format changes
```

---

## Task 18: Documentation Updates

**Files to update:**
- `docs/guide/concepts.md` (lines 52-87: series section)
- `docs/guide/concepts.zh.md` (lines 52-87: Chinese version)
- `docs/guide/getting-started.md` (lines 110-128, 186-196, 275)
- `docs/api/rest.md` (lines 146-170: event publishing)
- `docs/api/sse.md` (lines 59, 107: envelope fields + new seriesFormat param)
- `docs/skill/taskcast.md` (lines 121-146, 283)

**For each doc:**

1. **Concepts guide**: Update accumulate mode description to explain delta storage, seriesFormat option, late-join snapshots, three-layer storage difference
2. **Getting started**: Update examples to mention `seriesFormat` parameter, explain default is `delta`
3. **REST API**: Note that `publishEvent` returns delta event for accumulate mode
4. **SSE API**: Document `seriesFormat` query parameter, `seriesSnapshot` field in envelope, late-join behavior
5. **Skill doc**: Update series mode table, add seriesFormat to SSE params

**Also update `CLAUDE.md`** if any architecture descriptions changed (e.g., write path, SSE behavior section).

**Commit:**

```
docs: update guides and API reference for series format feature
```

---

## Task 19: Final Verification

**Step 1: Run full test suite**

```bash
pnpm test
```

**Step 2: Run type check**

```bash
pnpm lint
```

**Step 3: Run Rust tests**

```bash
cd rust && cargo test
```

**Step 4: Run coverage**

```bash
pnpm test:coverage
```

Verify coverage meets 100% target. If any gaps, add missing tests.

**Step 5: Create changeset**

```bash
pnpm changeset
```

Select all affected packages, minor version bump (breaking change for consumers).

**Step 6: Final commit**

```
chore: add changeset for series format feature
```
