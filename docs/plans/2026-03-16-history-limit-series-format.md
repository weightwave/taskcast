# History Endpoint: `limit` + `seriesFormat` + Hot/Cold Routing

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add `limit` and `seriesFormat` query parameters to the history endpoint, with hot/cold routing that reads from ShortTermStore (hot tasks) or LongTermStore (cold/terminal tasks) as needed.

**Architecture:** Extract the accumulate-series collapse logic from SSE handlers into a shared `collapseAccumulateSeries()` function in core. Extend `engine.getEvents()` with LongTermStore fallback (matching the existing `getTask()` pattern). History endpoint gains `limit` (pushed to storage layer) and `seriesFormat` (applied after retrieval via collapse function). Both TypeScript and Rust implementations updated simultaneously.

**Tech Stack:** TypeScript (Vitest), Rust (Axum + tokio), in-memory adapters for unit tests.

---

## Design Decisions

These decisions were made in discussion and should not be revisited during implementation:

1. **Hot/cold routing:** `engine.getEvents()` tries ShortTermStore first; if empty, falls back to LongTermStore (if configured). This mirrors the existing `engine.getTask()` pattern.

2. **`limit`:** Pushed directly to the storage layer via `EventQueryOptions.limit` (already defined in types, just not wired to the endpoint).

3. **`seriesFormat`:** New query parameter for history endpoint. `delta` (default) returns events as-is. `accumulated` collapses accumulate-mode series into single snapshots.

4. **Series collapse logic:** Shared function `collapseAccumulateSeries()` used by both history endpoint and SSE handler. Takes a `getSeriesLatest` callback; if callback returns null (cold task, ShortTermStore empty), derives snapshot from last event in the series within the events array.

5. **Cold task + `seriesFormat=delta`:** LongTermStore stores accumulated values — deltas are irrecoverable. Silently return accumulated data. No error, no special header.

6. **`limit` + collapse interaction:** `limit` applies at storage layer (before collapse). After collapse, result may contain fewer events than `limit`. This is intentional — the alternative requires unbounded reads.

---

## Chunk 1: Core — `collapseAccumulateSeries` Function

### Task 1.1: TypeScript — `collapseAccumulateSeries` tests

**Files:**
- Create: `packages/core/tests/unit/series-collapse.test.ts`

- [ ] **Step 1: Write tests for collapseAccumulateSeries**

```typescript
import { describe, it, expect, vi } from 'vitest'
import { collapseAccumulateSeries } from '../../src/series.js'
import type { TaskEvent } from '../../src/types.js'

function makeEvent(overrides: Partial<TaskEvent> = {}): TaskEvent {
  return {
    id: 'evt-1',
    taskId: 'task-1',
    index: 0,
    timestamp: 1000,
    type: 'test',
    level: 'info',
    data: { text: 'hello' },
    ...overrides,
  }
}

describe('collapseAccumulateSeries', () => {
  it('returns events unchanged when no accumulate series present', async () => {
    const events = [
      makeEvent({ id: 'e1', index: 0 }),
      makeEvent({ id: 'e2', index: 1 }),
    ]
    const getLatest = vi.fn().mockResolvedValue(null)
    const result = await collapseAccumulateSeries(events, getLatest)
    expect(result).toEqual(events)
    expect(getLatest).not.toHaveBeenCalled()
  })

  it('collapses accumulate series into single snapshot using getSeriesLatest', async () => {
    const events = [
      makeEvent({ id: 'e1', index: 0, seriesId: 's1', seriesMode: 'accumulate', data: { delta: 'A' } }),
      makeEvent({ id: 'e2', index: 1, seriesId: 's1', seriesMode: 'accumulate', data: { delta: 'B' } }),
      makeEvent({ id: 'e3', index: 2, type: 'other', data: { x: 1 } }),
    ]
    const accSnapshot = makeEvent({
      id: 'e2', index: 1, seriesId: 's1', seriesMode: 'accumulate',
      data: { delta: 'AB' },
    })
    const getLatest = vi.fn().mockResolvedValue(accSnapshot)

    const result = await collapseAccumulateSeries(events, getLatest)

    expect(result).toHaveLength(2)
    expect(result[0]).toEqual({ ...accSnapshot, seriesSnapshot: true })
    expect(result[1]).toEqual(events[2])
    expect(getLatest).toHaveBeenCalledWith('task-1', 's1')
  })

  it('falls back to last event in array when getSeriesLatest returns null (cold task)', async () => {
    const events = [
      makeEvent({ id: 'e1', index: 0, seriesId: 's1', seriesMode: 'accumulate', data: { delta: 'A' } }),
      makeEvent({ id: 'e2', index: 1, seriesId: 's1', seriesMode: 'accumulate', data: { delta: 'AB' } }),
    ]
    const getLatest = vi.fn().mockResolvedValue(null)

    const result = await collapseAccumulateSeries(events, getLatest)

    expect(result).toHaveLength(1)
    expect(result[0]).toEqual({ ...events[1], seriesSnapshot: true })
  })

  it('handles multiple accumulate series independently', async () => {
    const events = [
      makeEvent({ id: 'e1', index: 0, seriesId: 's1', seriesMode: 'accumulate' }),
      makeEvent({ id: 'e2', index: 1, seriesId: 's2', seriesMode: 'accumulate' }),
      makeEvent({ id: 'e3', index: 2, seriesId: 's1', seriesMode: 'accumulate' }),
      makeEvent({ id: 'e4', index: 3, seriesId: 's2', seriesMode: 'accumulate' }),
    ]
    const getLatest = vi.fn()
      .mockResolvedValueOnce(makeEvent({ id: 'snap-s1', seriesId: 's1', data: { delta: 'S1-ACC' } }))
      .mockResolvedValueOnce(makeEvent({ id: 'snap-s2', seriesId: 's2', data: { delta: 'S2-ACC' } }))

    const result = await collapseAccumulateSeries(events, getLatest)

    expect(result).toHaveLength(2)
    expect(result[0].seriesSnapshot).toBe(true)
    expect(result[1].seriesSnapshot).toBe(true)
  })

  it('preserves keep-all and latest series events', async () => {
    const events = [
      makeEvent({ id: 'e1', index: 0, seriesId: 'ka', seriesMode: 'keep-all' }),
      makeEvent({ id: 'e2', index: 1, seriesId: 'lt', seriesMode: 'latest' }),
      makeEvent({ id: 'e3', index: 2, seriesId: 'acc', seriesMode: 'accumulate' }),
      makeEvent({ id: 'e4', index: 3, seriesId: 'acc', seriesMode: 'accumulate' }),
    ]
    const snapshot = makeEvent({ id: 'snap', seriesId: 'acc', data: { text: 'collapsed' } })
    const getLatest = vi.fn().mockResolvedValue(snapshot)

    const result = await collapseAccumulateSeries(events, getLatest)

    expect(result).toHaveLength(3)
    expect(result[0].id).toBe('e1') // keep-all preserved
    expect(result[1].id).toBe('e2') // latest preserved
    expect(result[2].seriesSnapshot).toBe(true) // accumulate collapsed
  })

  it('handles empty events array', async () => {
    const getLatest = vi.fn()
    const result = await collapseAccumulateSeries([], getLatest)
    expect(result).toEqual([])
  })
})
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cd packages/core && npx vitest run tests/unit/series-collapse.test.ts`
Expected: FAIL — `collapseAccumulateSeries` is not exported from `series.js`

- [ ] **Step 3: Implement collapseAccumulateSeries**

Modify: `packages/core/src/series.ts` — add function after existing `processSeries`:

```typescript
/**
 * Collapse accumulate-mode series events into single snapshot events.
 * Used by history endpoint and SSE late-join replay.
 *
 * @param events - Array of events to collapse
 * @param getSeriesLatest - Callback to get latest accumulated value for a series.
 *   For hot tasks, pass engine.getSeriesLatest. If returns null (cold task),
 *   falls back to last event in the events array for that series.
 */
export async function collapseAccumulateSeries(
  events: TaskEvent[],
  getSeriesLatest: (taskId: string, seriesId: string) => Promise<TaskEvent | null>,
): Promise<TaskEvent[]> {
  const accSeriesIds = new Set<string>()
  for (const e of events) {
    if (e.seriesMode === 'accumulate' && e.seriesId) {
      accSeriesIds.add(e.seriesId)
    }
  }

  if (accSeriesIds.size === 0) return events

  // Resolve snapshots for each accumulate series
  const snapshots = new Map<string, TaskEvent>()
  const taskId = events[0].taskId
  for (const sid of accSeriesIds) {
    const latest = await getSeriesLatest(taskId, sid)
    if (latest) {
      snapshots.set(sid, { ...latest, seriesSnapshot: true })
    } else {
      // Cold path: derive from last event in this series
      for (let i = events.length - 1; i >= 0; i--) {
        if (events[i].seriesId === sid) {
          snapshots.set(sid, { ...events[i], seriesSnapshot: true })
          break
        }
      }
    }
  }

  // Replace series events with snapshots (first occurrence only)
  const emitted = new Set<string>()
  const result: TaskEvent[] = []
  for (const event of events) {
    if (event.seriesMode === 'accumulate' && event.seriesId && accSeriesIds.has(event.seriesId)) {
      if (!emitted.has(event.seriesId)) {
        const snapshot = snapshots.get(event.seriesId)
        if (snapshot) {
          result.push(snapshot)
          emitted.add(event.seriesId)
        }
      }
      // Skip remaining events in this accumulate series
    } else {
      result.push(event)
    }
  }

  return result
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cd packages/core && npx vitest run tests/unit/series-collapse.test.ts`
Expected: All 6 tests PASS

- [ ] **Step 5: Commit**

```bash
git add packages/core/src/series.ts packages/core/tests/unit/series-collapse.test.ts
git commit -m "feat(core): add collapseAccumulateSeries function for history endpoint"
```

---

### Task 1.2: Rust — `collapse_accumulate_series` tests + implementation

**Files:**
- Modify: `rust/taskcast-core/src/series.rs`
- Create: `rust/taskcast-core/tests/series_collapse.rs`

- [ ] **Step 1: Write tests**

Create `rust/taskcast-core/tests/series_collapse.rs`:

```rust
use taskcast_core::types::{TaskEvent, SeriesMode, Level};
use taskcast_core::series::collapse_accumulate_series;

fn make_event(id: &str, index: u64, overrides: Option<EventOverrides>) -> TaskEvent {
    let o = overrides.unwrap_or_default();
    TaskEvent {
        id: id.to_string(),
        task_id: o.task_id.unwrap_or_else(|| "task-1".to_string()),
        index,
        timestamp: 1000.0 + index as f64,
        r#type: o.event_type.unwrap_or_else(|| "test".to_string()),
        level: Level::Info,
        data: serde_json::json!({ "text": "hello" }),
        series_id: o.series_id,
        series_mode: o.series_mode,
        series_acc_field: None,
        series_snapshot: None,
        _accumulated_data: None,
    }
}

#[derive(Default)]
struct EventOverrides {
    task_id: Option<String>,
    event_type: Option<String>,
    series_id: Option<String>,
    series_mode: Option<SeriesMode>,
}

#[tokio::test]
async fn returns_unchanged_when_no_accumulate_series() {
    let events = vec![
        make_event("e1", 0, None),
        make_event("e2", 1, None),
    ];
    let called = std::sync::atomic::AtomicBool::new(false);
    let get_latest = |_task_id: &str, _series_id: &str| {
        called.store(true, std::sync::atomic::Ordering::SeqCst);
        Box::pin(async { Ok(None) }) as std::pin::Pin<Box<dyn std::future::Future<Output = Result<Option<TaskEvent>, Box<dyn std::error::Error + Send + Sync>>> + Send>>
    };
    let result = collapse_accumulate_series(&events, get_latest).await.unwrap();
    assert_eq!(result.len(), 2);
    assert!(!called.load(std::sync::atomic::Ordering::SeqCst));
}

#[tokio::test]
async fn collapses_accumulate_series_with_snapshot() {
    let events = vec![
        make_event("e1", 0, Some(EventOverrides {
            series_id: Some("s1".to_string()),
            series_mode: Some(SeriesMode::Accumulate),
            ..Default::default()
        })),
        make_event("e2", 1, Some(EventOverrides {
            series_id: Some("s1".to_string()),
            series_mode: Some(SeriesMode::Accumulate),
            ..Default::default()
        })),
        make_event("e3", 2, None),
    ];

    let mut snapshot = make_event("e2", 1, Some(EventOverrides {
        series_id: Some("s1".to_string()),
        series_mode: Some(SeriesMode::Accumulate),
        ..Default::default()
    }));
    snapshot.data = serde_json::json!({ "delta": "AB" });

    let snapshot_clone = snapshot.clone();
    let get_latest = move |_task_id: &str, _series_id: &str| {
        let s = snapshot_clone.clone();
        Box::pin(async move { Ok(Some(s)) }) as std::pin::Pin<Box<dyn std::future::Future<Output = Result<Option<TaskEvent>, Box<dyn std::error::Error + Send + Sync>>> + Send>>
    };

    let result = collapse_accumulate_series(&events, get_latest).await.unwrap();
    assert_eq!(result.len(), 2);
    assert_eq!(result[0].series_snapshot, Some(true));
    assert_eq!(result[1].id, "e3");
}

#[tokio::test]
async fn falls_back_to_last_event_when_get_latest_returns_none() {
    let events = vec![
        make_event("e1", 0, Some(EventOverrides {
            series_id: Some("s1".to_string()),
            series_mode: Some(SeriesMode::Accumulate),
            ..Default::default()
        })),
        make_event("e2", 1, Some(EventOverrides {
            series_id: Some("s1".to_string()),
            series_mode: Some(SeriesMode::Accumulate),
            ..Default::default()
        })),
    ];
    let get_latest = |_task_id: &str, _series_id: &str| {
        Box::pin(async { Ok(None) }) as std::pin::Pin<Box<dyn std::future::Future<Output = Result<Option<TaskEvent>, Box<dyn std::error::Error + Send + Sync>>> + Send>>
    };

    let result = collapse_accumulate_series(&events, get_latest).await.unwrap();
    assert_eq!(result.len(), 1);
    assert_eq!(result[0].id, "e2"); // last event in series
    assert_eq!(result[0].series_snapshot, Some(true));
}

#[tokio::test]
async fn handles_empty_events() {
    let events: Vec<TaskEvent> = vec![];
    let get_latest = |_task_id: &str, _series_id: &str| {
        Box::pin(async { Ok(None) }) as std::pin::Pin<Box<dyn std::future::Future<Output = Result<Option<TaskEvent>, Box<dyn std::error::Error + Send + Sync>>> + Send>>
    };
    let result = collapse_accumulate_series(&events, get_latest).await.unwrap();
    assert!(result.is_empty());
}
```

> **Note:** The exact callback type may need adjustment during implementation. The key behaviors to test are the same as the TS version. Adapt the closure types to match whatever signature `collapse_accumulate_series` ends up using (likely `async fn` with `dyn Fn` or an `async_trait`-based approach).

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p taskcast-core --test series_collapse`
Expected: FAIL — `collapse_accumulate_series` not found

- [ ] **Step 3: Implement collapse_accumulate_series in Rust**

Modify: `rust/taskcast-core/src/series.rs` — add after existing `process_series`:

```rust
use std::collections::{HashMap, HashSet};
use std::future::Future;

/// Collapse accumulate-mode series events into single snapshot events.
///
/// For each accumulate series found in `events`:
/// 1. Call `get_series_latest` to get the definitive accumulated value (hot task path)
/// 2. If it returns None (cold task), use the last event in that series from the array
/// 3. Replace all events in that series with a single snapshot (seriesSnapshot = true)
///
/// Non-accumulate events (keep-all, latest, no series) are passed through unchanged.
pub async fn collapse_accumulate_series<F, Fut>(
    events: &[TaskEvent],
    get_series_latest: F,
) -> Result<Vec<TaskEvent>, Box<dyn std::error::Error + Send + Sync>>
where
    F: Fn(&str, &str) -> Fut,
    Fut: Future<Output = Result<Option<TaskEvent>, Box<dyn std::error::Error + Send + Sync>>> + Send,
{
    if events.is_empty() {
        return Ok(vec![]);
    }

    let mut acc_series_ids = HashSet::new();
    for e in events {
        if e.series_mode.as_ref() == Some(&SeriesMode::Accumulate) {
            if let Some(ref sid) = e.series_id {
                acc_series_ids.insert(sid.clone());
            }
        }
    }

    if acc_series_ids.is_empty() {
        return Ok(events.to_vec());
    }

    let task_id = &events[0].task_id;
    let mut snapshots = HashMap::new();
    for sid in &acc_series_ids {
        let latest = get_series_latest(task_id, sid).await?;
        if let Some(mut snap) = latest {
            snap.series_snapshot = Some(true);
            snapshots.insert(sid.clone(), snap);
        } else {
            // Cold path: derive from last event in this series
            for e in events.iter().rev() {
                if e.series_id.as_deref() == Some(sid) {
                    let mut snap = e.clone();
                    snap.series_snapshot = Some(true);
                    snapshots.insert(sid.clone(), snap);
                    break;
                }
            }
        }
    }

    let mut emitted = HashSet::new();
    let mut result = Vec::new();
    for event in events {
        if event.series_mode.as_ref() == Some(&SeriesMode::Accumulate) {
            if let Some(ref sid) = event.series_id {
                if acc_series_ids.contains(sid) {
                    if !emitted.contains(sid) {
                        if let Some(snapshot) = snapshots.get(sid) {
                            result.push(snapshot.clone());
                            emitted.insert(sid.clone());
                        }
                    }
                    continue; // Skip remaining events in this accumulate series
                }
            }
        }
        result.push(event.clone());
    }

    Ok(result)
}
```

Also export it from `rust/taskcast-core/src/lib.rs`.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p taskcast-core --test series_collapse`
Expected: All tests PASS

- [ ] **Step 5: Commit**

```bash
git add rust/taskcast-core/src/series.rs rust/taskcast-core/src/lib.rs rust/taskcast-core/tests/series_collapse.rs
git commit -m "feat(rust-core): add collapse_accumulate_series function for history endpoint"
```

---

### Task 1.3: Core — Engine `getEvents` LongTermStore Fallback (TS)

**Files:**
- Modify: `packages/core/src/engine.ts:300-302`
- Modify: `packages/core/tests/unit/engine.test.ts`

- [ ] **Step 1: Write tests for getEvents fallback**

Add to `packages/core/tests/unit/engine.test.ts`, in the existing `getEvents` describe block:

```typescript
it('falls back to longTermStore when shortTermStore returns empty', async () => {
  const store = new MemoryShortTermStore()
  const broadcast = new MemoryBroadcastProvider()
  const longTermEvents: TaskEvent[] = [
    {
      id: 'lt-evt-1', taskId: 'cold-task', index: 0, timestamp: 1000,
      type: 'test', level: 'info', data: { text: 'from long term' },
    },
  ]
  const longTermStore = makeLongTermStore({
    getEvents: vi.fn().mockResolvedValue(longTermEvents),
  })
  const engine = new TaskEngine({ shortTermStore: store, broadcast, longTermStore })

  const events = await engine.getEvents('cold-task')
  expect(events).toEqual(longTermEvents)
  expect(longTermStore.getEvents).toHaveBeenCalledWith('cold-task', undefined)
})

it('returns shortTermStore events when available (does not call longTermStore)', async () => {
  const store = new MemoryShortTermStore()
  const broadcast = new MemoryBroadcastProvider()
  const longTermStore = makeLongTermStore()
  const engine = new TaskEngine({ shortTermStore: store, broadcast, longTermStore })

  const task = await engine.createTask({})
  await engine.transitionTask(task.id, 'running')
  await engine.publishEvent(task.id, { type: 'test', level: 'info', data: {} })

  const events = await engine.getEvents(task.id)
  expect(events.length).toBeGreaterThan(0)
  expect(longTermStore.getEvents).not.toHaveBeenCalled()
})

it('returns empty when both stores have no events and no longTermStore configured', async () => {
  const { engine } = makeEngine()
  const events = await engine.getEvents('nonexistent')
  expect(events).toEqual([])
})

it('passes opts through to longTermStore fallback', async () => {
  const store = new MemoryShortTermStore()
  const broadcast = new MemoryBroadcastProvider()
  const longTermStore = makeLongTermStore({
    getEvents: vi.fn().mockResolvedValue([]),
  })
  const engine = new TaskEngine({ shortTermStore: store, broadcast, longTermStore })

  const opts = { since: { id: 'some-id' }, limit: 10 }
  await engine.getEvents('cold-task', opts)
  expect(longTermStore.getEvents).toHaveBeenCalledWith('cold-task', opts)
})
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cd packages/core && npx vitest run tests/unit/engine.test.ts -t "falls back to longTermStore"`
Expected: FAIL — current implementation returns `[]` from shortTermStore, doesn't try longTermStore

- [ ] **Step 3: Implement getEvents fallback**

Modify `packages/core/src/engine.ts` lines 300-302:

```typescript
async getEvents(taskId: string, opts?: EventQueryOptions): Promise<TaskEvent[]> {
  const fromShort = await this.shortTermStore.getEvents(taskId, opts)
  if (fromShort.length > 0) return fromShort
  if (this.longTermStore) {
    return this.longTermStore.getEvents(taskId, opts)
  }
  return []
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cd packages/core && npx vitest run tests/unit/engine.test.ts`
Expected: All tests PASS

- [ ] **Step 5: Commit**

```bash
git add packages/core/src/engine.ts packages/core/tests/unit/engine.test.ts
git commit -m "feat(core): engine.getEvents falls back to LongTermStore for cold tasks"
```

---

### Task 1.4: Core — Engine `get_events` LongTermStore Fallback (Rust)

**Files:**
- Modify: `rust/taskcast-core/src/engine.rs:415-421`

- [ ] **Step 1: Add test for get_events fallback**

Add to the `#[cfg(test)]` module in `rust/taskcast-core/src/engine.rs`:

```rust
#[tokio::test]
async fn get_events_falls_back_to_long_term_store() {
    let long_term_store = Arc::new(MockLongTermStore::new());
    // Pre-populate long_term_store with an event
    let event = TaskEvent {
        id: "lt-evt-1".to_string(),
        task_id: "cold-task".to_string(),
        index: 0,
        timestamp: 1000.0,
        r#type: "test".to_string(),
        level: Level::Info,
        data: serde_json::json!({"text": "from long term"}),
        series_id: None,
        series_mode: None,
        series_acc_field: None,
        series_snapshot: None,
        _accumulated_data: None,
    };
    long_term_store.events.write().await.push(event.clone());

    let engine = make_engine_with_long_term(Arc::clone(&long_term_store) as Arc<dyn LongTermStore>);

    let events = engine.get_events("cold-task", None).await.unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].id, "lt-evt-1");
}

#[tokio::test]
async fn get_events_prefers_short_term_store() {
    let long_term_store = Arc::new(MockLongTermStore::new());
    let engine = make_engine_with_long_term(Arc::clone(&long_term_store) as Arc<dyn LongTermStore>);

    let task = engine.create_task(CreateTaskInput {
        r#type: Some("test".to_string()),
        ..Default::default()
    }).await.unwrap();
    engine.transition_task(&task.id, TaskStatus::Running, None).await.unwrap();
    engine.publish_event(&task.id, PublishEventInput {
        r#type: "test".to_string(),
        level: Level::Info,
        data: serde_json::json!({}),
        ..Default::default()
    }).await.unwrap();

    let events = engine.get_events(&task.id, None).await.unwrap();
    assert!(!events.is_empty());
    // long_term_store.get_events should not have been called — but we can't easily
    // assert that with the current MockLongTermStore. The key check is that events come
    // from short_term_store (they have the published event).
}
```

> **Note:** The `MockLongTermStore` currently returns all events regardless of `task_id`. You may need to update its `get_events` to filter by `task_id` for the fallback test to work correctly. Add a `task_id` field to stored events and filter accordingly.

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p taskcast-core get_events_falls_back`
Expected: FAIL — current implementation doesn't check long_term_store

- [ ] **Step 3: Implement get_events fallback**

Modify `rust/taskcast-core/src/engine.rs` lines 415-421:

```rust
pub async fn get_events(
    &self,
    task_id: &str,
    opts: Option<EventQueryOptions>,
) -> Result<Vec<TaskEvent>, EngineError> {
    let from_short = self.short_term_store.get_events(task_id, opts.clone()).await?;
    if !from_short.is_empty() {
        return Ok(from_short);
    }
    if let Some(ref long_term_store) = self.long_term_store {
        return Ok(long_term_store.get_events(task_id, opts).await?);
    }
    Ok(vec![])
}
```

> **Note:** `EventQueryOptions` must derive `Clone` (check it already does — yes, it has `#[derive(Clone)]`).

- [ ] **Step 4: Fix MockLongTermStore to filter by task_id**

Update the `get_events` impl in `MockLongTermStore` to filter:

```rust
async fn get_events(&self, task_id: &str, _opts: Option<EventQueryOptions>) -> Result<Vec<TaskEvent>, Box<dyn std::error::Error + Send + Sync>> {
    let events = self.events.read().await;
    Ok(events.iter().filter(|e| e.task_id == task_id).cloned().collect())
}
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p taskcast-core -- get_events`
Expected: All tests PASS. Also run full suite: `cargo test -p taskcast-core` to ensure no regressions.

- [ ] **Step 6: Commit**

```bash
git add rust/taskcast-core/src/engine.rs
git commit -m "feat(rust-core): engine.get_events falls back to LongTermStore for cold tasks"
```

---

## Chunk 2: History Endpoint — `limit` + `seriesFormat`

### Task 2.1: TypeScript — History endpoint tests

**Files:**
- Modify: `packages/server/tests/tasks.test.ts`

- [ ] **Step 1: Write tests for limit and seriesFormat on history endpoint**

Add to the existing history tests in `packages/server/tests/tasks.test.ts`:

```typescript
describe('GET /tasks/:taskId/events/history — limit + seriesFormat', () => {
  // NOTE: transitionTask('running') emits a taskcast:status event at index 0.
  // All event counts below account for this extra event.

  it('respects limit parameter', async () => {
    const { app, engine } = makeApp()
    const task = await engine.createTask({})
    await engine.transitionTask(task.id, 'running')
    // Publish 5 events. With the status event, total = 6.
    for (let i = 0; i < 5; i++) {
      await engine.publishEvent(task.id, { type: 'progress', level: 'info', data: { i } })
    }

    const res = await app.request(`/tasks/${task.id}/events/history?limit=3`)
    expect(res.status).toBe(200)
    const events = await res.json()
    expect(events).toHaveLength(3)
    // First 3 events: taskcast:status (index=0), progress(i=0, index=1), progress(i=1, index=2)
    expect(events[0].type).toBe('taskcast:status')
    expect(events[1].data.i).toBe(0)
    expect(events[2].data.i).toBe(1)
  })

  it('returns all events when limit is not specified', async () => {
    const { app, engine } = makeApp()
    const task = await engine.createTask({})
    await engine.transitionTask(task.id, 'running')
    for (let i = 0; i < 5; i++) {
      await engine.publishEvent(task.id, { type: 'progress', level: 'info', data: { i } })
    }

    const res = await app.request(`/tasks/${task.id}/events/history`)
    const events = await res.json()
    // 1 status + 5 published = 6
    expect(events).toHaveLength(6)
  })

  it('collapses accumulate series when seriesFormat=accumulated', async () => {
    const { app, engine } = makeApp()
    const task = await engine.createTask({})
    await engine.transitionTask(task.id, 'running')

    // Publish 3 accumulate deltas
    for (const delta of ['A', 'B', 'C']) {
      await engine.publishEvent(task.id, {
        type: 'llm.token', level: 'info',
        data: { delta }, seriesId: 'tokens',
        seriesMode: 'accumulate', seriesAccField: 'delta',
      })
    }

    // Also publish a non-series event
    await engine.publishEvent(task.id, { type: 'log', level: 'info', data: { msg: 'done' } })

    const res = await app.request(`/tasks/${task.id}/events/history?seriesFormat=accumulated`)
    expect(res.status).toBe(200)
    const events = await res.json()

    // 1 status + 1 collapsed snapshot + 1 non-series = 3 events
    expect(events).toHaveLength(3)
    const snapshot = events.find((e: any) => e.seriesId === 'tokens')
    expect(snapshot.seriesSnapshot).toBe(true)
    expect(snapshot.data.delta).toBe('ABC')
  })

  it('returns raw deltas when seriesFormat=delta (default)', async () => {
    const { app, engine } = makeApp()
    const task = await engine.createTask({})
    await engine.transitionTask(task.id, 'running')
    for (const delta of ['A', 'B']) {
      await engine.publishEvent(task.id, {
        type: 'llm.token', level: 'info',
        data: { delta }, seriesId: 'tokens',
        seriesMode: 'accumulate', seriesAccField: 'delta',
      })
    }

    const res = await app.request(`/tasks/${task.id}/events/history?seriesFormat=delta`)
    const events = await res.json()
    // 1 status + 2 deltas = 3
    expect(events).toHaveLength(3)
    const deltas = events.filter((e: any) => e.type === 'llm.token')
    expect(deltas[0].data.delta).toBe('A')
    expect(deltas[1].data.delta).toBe('B')
  })

  it('combines limit and seriesFormat=accumulated', async () => {
    const { app, engine } = makeApp()
    const task = await engine.createTask({})
    await engine.transitionTask(task.id, 'running')
    // Events: status(0), acc(1), acc(2), acc(3), log(4), log(5) = 6 total
    for (const delta of ['A', 'B', 'C']) {
      await engine.publishEvent(task.id, {
        type: 'llm.token', level: 'info',
        data: { delta }, seriesId: 'tokens',
        seriesMode: 'accumulate', seriesAccField: 'delta',
      })
    }
    for (let i = 0; i < 2; i++) {
      await engine.publishEvent(task.id, { type: 'log', level: 'info', data: { i } })
    }

    // limit=5 at storage → first 5 events: status + 3 accumulate + 1 log
    // After collapse → status + 1 snapshot + 1 log = 3 events
    const res = await app.request(`/tasks/${task.id}/events/history?limit=5&seriesFormat=accumulated`)
    const events = await res.json()
    expect(events.length).toBeLessThanOrEqual(5)
    const snapshot = events.find((e: any) => e.seriesSnapshot === true)
    expect(snapshot).toBeDefined()
  })

  it('treats invalid seriesFormat as delta', async () => {
    const { app, engine } = makeApp()
    const task = await engine.createTask({})
    await engine.transitionTask(task.id, 'running')
    await engine.publishEvent(task.id, {
      type: 'llm.token', level: 'info',
      data: { delta: 'A' }, seriesId: 's1',
      seriesMode: 'accumulate', seriesAccField: 'delta',
    })

    const res = await app.request(`/tasks/${task.id}/events/history?seriesFormat=invalid`)
    const events = await res.json()
    // No collapse — raw deltas returned (1 status + 1 delta = 2)
    expect(events).toHaveLength(2)
    expect(events.find((e: any) => e.seriesSnapshot)).toBeUndefined()
  })

  it('handles limit with since cursor', async () => {
    const { app, engine } = makeApp()
    const task = await engine.createTask({})
    await engine.transitionTask(task.id, 'running')
    const first = await engine.publishEvent(task.id, { type: 'a', level: 'info', data: null })
    await engine.publishEvent(task.id, { type: 'b', level: 'info', data: null })
    await engine.publishEvent(task.id, { type: 'c', level: 'info', data: null })

    // since.id=first, limit=1 → return 1 event after 'first' (which is 'b')
    const res = await app.request(`/tasks/${task.id}/events/history?since.id=${first.id}&limit=1`)
    const events = await res.json()
    expect(events).toHaveLength(1)
    expect(events[0].type).toBe('b')
  })
})
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cd packages/server && npx vitest run tests/tasks.test.ts -t "limit"`
Expected: FAIL — `limit` and `seriesFormat` query params not recognized / not implemented

- [ ] **Step 3: Update history endpoint route definition**

Modify: `packages/server/src/routes/tasks.ts` — update the `eventHistoryRoute` query schema:

```typescript
query: z.object({
  'since.id': z.string().optional(),
  'since.index': z.string().optional(),
  'since.timestamp': z.string().optional(),
  types: z.string().optional(),
  levels: z.string().optional(),
  limit: z.string().optional().openapi({ description: 'Maximum number of events to return' }),
  seriesFormat: z.string().optional().openapi({ description: 'Series format: delta (default) or accumulated' }),
}),
```

- [ ] **Step 4: Update history endpoint handler**

Modify the handler for `eventHistoryRoute` to parse limit + seriesFormat and apply collapse:

Add import at top of file:
```typescript
import { collapseAccumulateSeries } from '@taskcast/core'
```

Update handler:
```typescript
register(eventHistoryRoute, async (c) => {
  const taskId = c.req.param('taskId') as string
  const auth = c.get('auth')
  if (!checkScope(auth, 'event:history', taskId)) return c.json({ error: 'Forbidden' }, 403)

  const task = await engine.getTask(taskId)
  if (!task) return c.json({ error: 'Task not found' }, 404)

  const sinceIndex = c.req.query('since.index')
  const sinceTimestamp = c.req.query('since.timestamp')
  const sinceId = c.req.query('since.id')
  const limitStr = c.req.query('limit')
  const seriesFormat = c.req.query('seriesFormat') ?? 'delta'

  let since: SinceCursor | undefined
  if (sinceId !== undefined || sinceIndex !== undefined || sinceTimestamp !== undefined) {
    since = {}
    if (sinceId !== undefined) since.id = sinceId
    if (sinceIndex !== undefined) since.index = Number(sinceIndex)
    if (sinceTimestamp !== undefined) since.timestamp = Number(sinceTimestamp)
  }

  const limit = limitStr !== undefined ? Number(limitStr) : undefined
  const opts: EventQueryOptions | undefined =
    since !== undefined || limit !== undefined
      ? { ...(since !== undefined && { since }), ...(limit !== undefined && { limit }) }
      : undefined

  let events = await engine.getEvents(taskId, opts)

  if (seriesFormat === 'accumulated') {
    events = await collapseAccumulateSeries(
      events,
      (tid, sid) => engine.getSeriesLatest(tid, sid),
    )
  }

  return c.json(events)
})
```

- [ ] **Step 5: Verify collapseAccumulateSeries is exported from @taskcast/core**

Check `packages/core/src/index.ts` and add export if missing:
```typescript
export { processSeries, collapseAccumulateSeries } from './series.js'
```

- [ ] **Step 6: Run tests to verify they pass**

Run: `cd packages/server && npx vitest run tests/tasks.test.ts`
Expected: All tests PASS (both new and existing)

- [ ] **Step 7: Commit**

```bash
git add packages/core/src/index.ts packages/server/src/routes/tasks.ts packages/server/tests/tasks.test.ts
git commit -m "feat(server): add limit + seriesFormat to history endpoint"
```

---

### Task 2.2: Rust — History endpoint tests + implementation

**Files:**
- Modify: `rust/taskcast-server/src/routes/tasks.rs`
- Create or modify: `rust/taskcast-server/tests/history.rs`

- [ ] **Step 1: Write tests for limit and seriesFormat on Rust history endpoint**

Create `rust/taskcast-server/tests/history_limit_series.rs` (or add to existing test file). Tests should mirror the TS tests:

1. `limit` parameter caps returned events
2. `seriesFormat=accumulated` collapses accumulate series
3. `seriesFormat=delta` (default) returns raw events
4. `limit` + `seriesFormat=accumulated` combined

> **Note:** Follow the existing test patterns in `rust/taskcast-server/tests/` (e.g., `sse_last_event_id.rs`). Use the test helper to create an app instance with in-memory stores.

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p taskcast-server --test history_limit_series`
Expected: FAIL

- [ ] **Step 3: Update HistoryQuery struct to include limit and seriesFormat**

Modify `rust/taskcast-server/src/routes/tasks.rs`:

```rust
#[derive(Debug, Deserialize, utoipa::IntoParams)]
pub struct HistoryQuery {
    #[serde(rename = "since.index")]
    pub since_index: Option<u64>,
    #[serde(rename = "since.timestamp")]
    pub since_timestamp: Option<f64>,
    #[serde(rename = "since.id")]
    pub since_id: Option<String>,
    pub limit: Option<u64>,
    #[serde(rename = "seriesFormat")]
    pub series_format: Option<String>,
}
```

- [ ] **Step 4: Update get_event_history handler**

```rust
pub async fn get_event_history(
    State(engine): State<Arc<TaskEngine>>,
    Extension(auth): Extension<AuthContext>,
    Path(task_id): Path<String>,
    Query(query): Query<HistoryQuery>,
) -> Result<impl IntoResponse, AppError> {
    if !check_scope(&auth, taskcast_core::PermissionScope::EventHistory, Some(&task_id)) {
        return Err(AppError::Forbidden);
    }

    let _task = engine.get_task(&task_id).await?
        .ok_or_else(|| AppError::NotFound("Task not found".to_string()))?;

    let since = if query.since_id.is_some() || query.since_index.is_some() || query.since_timestamp.is_some() {
        Some(SinceCursor {
            id: query.since_id,
            index: query.since_index,
            timestamp: query.since_timestamp,
        })
    } else {
        None
    };

    let opts = if since.is_some() || query.limit.is_some() {
        Some(EventQueryOptions { since, limit: query.limit })
    } else {
        None
    };

    let mut events = engine.get_events(&task_id, opts).await?;

    let series_format = query.series_format.as_deref().unwrap_or("delta");
    if series_format == "accumulated" {
        let engine_ref = Arc::clone(&engine);
        events = taskcast_core::series::collapse_accumulate_series(
            &events,
            |tid: &str, sid: &str| {
                let eng = Arc::clone(&engine_ref);
                let tid = tid.to_string();
                let sid = sid.to_string();
                async move {
                    eng.get_series_latest(&tid, &sid).await
                        .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)
                }
            },
        ).await.map_err(|e| AppError::BadRequest(format!("Series collapse error: {e}")))?;
    }

    Ok(axum::Json(events))
}
```

> **Note:** `AppError` does not have an `Internal` variant. Available variants: `Engine`, `BadRequest`, `NotFound`, `Forbidden`, `MissingToken`, `InvalidToken`. The code above uses `BadRequest` as a pragmatic fallback for collapse errors. Consider adding an `Internal(String)` variant to `AppError` if this pattern becomes common.

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p taskcast-server --test history_limit_series`
Expected: All tests PASS

- [ ] **Step 6: Commit**

```bash
git add rust/taskcast-server/src/routes/tasks.rs rust/taskcast-server/tests/history_limit_series.rs
git commit -m "feat(rust-server): add limit + seriesFormat to history endpoint"
```

---

## Chunk 3: SSE — Refactor to Use Shared Collapse + Add `limit`

### Task 3.1: TypeScript — Refactor SSE series collapse

**Files:**
- Modify: `packages/server/src/routes/sse.ts:190-228`

- [ ] **Step 1: Identify existing SSE series collapse code to replace**

Read `packages/server/src/routes/sse.ts` lines 190-228. This is the late-join collapse logic that should be replaced with `collapseAccumulateSeries()`.

- [ ] **Step 2: Replace inline collapse with shared function call**

The current code block (roughly lines 190-228) does:
- Detect accumulate series in history
- Fetch snapshots via `engine.getSeriesLatest`
- Build `replayEvents` with snapshots replacing series events

Replace with:

```typescript
let replayEvents: TaskEvent[]
const hasSinceCursor = !!filter.since

if (!hasSinceCursor) {
  replayEvents = await collapseAccumulateSeries(
    history,
    (tid, sid) => engine.getSeriesLatest(tid, sid),
  )
} else {
  replayEvents = history
}
```

Add import at top:
```typescript
import { collapseAccumulateSeries } from '@taskcast/core'
```

- [ ] **Step 3: Run ALL existing SSE tests to verify no regressions**

Run: `cd packages/server && npx vitest run tests/sse.test.ts tests/sse-series-format.test.ts`
Expected: All tests PASS — behavior is identical

- [ ] **Step 4: Commit**

```bash
git add packages/server/src/routes/sse.ts
git commit -m "refactor(server): SSE uses shared collapseAccumulateSeries function"
```

---

### Task 3.2: TypeScript — Add `limit` to SSE history replay

**Files:**
- Modify: `packages/server/src/routes/sse.ts`
- Modify: `packages/core/src/types.ts` (add `limit` to `SubscribeFilter` if needed)
- Add tests to: `packages/server/tests/sse.test.ts`

- [ ] **Step 1: Write test for SSE limit on history replay**

```typescript
describe('SSE limit on history replay', () => {
  it('limits history replay events when limit param is set', async () => {
    // Use the SSE test helper pattern from existing tests to create a running task
    const task = await createRunningTask()
    // Publish 10 events
    for (let i = 0; i < 10; i++) {
      await engine.publishEvent(task.id, { type: 'progress', level: 'info', data: { i } })
    }

    const res = await app.request(`/tasks/${task.id}/events?limit=3`)
    // Parse SSE stream, expect only 3 history events before live stream begins
    // (plus any taskcast:status events depending on includeStatus)
    const events = parseSSEStream(res)
    const progressEvents = events.filter(e => e.type === 'progress')
    expect(progressEvents).toHaveLength(3)
  })
})
```

> **Note:** Adapt test to use the actual SSE parsing pattern established in existing tests. The exact assertion depends on whether `includeStatus` events are also limited.

- [ ] **Step 2: Add `limit` query parameter to SSE route**

Add to the SSE query schema:
```typescript
limit: z.string().optional().openapi({ description: 'Maximum number of history events to replay on connect' }),
```

Parse in handler and pass to `engine.getEvents()`:
```typescript
const limitStr = c.req.query('limit')
const limit = limitStr !== undefined ? Number(limitStr) : undefined
```

Pass `limit` in the `engine.getEvents()` call within the SSE handler.

- [ ] **Step 3: Decide interaction with Last-Event-ID**

When reconnecting with `Last-Event-ID`:
- `limit` caps events replayed since the cursor
- This prevents unbounded replay after long disconnections
- No special handling needed — `limit` is already in `EventQueryOptions` alongside `since`

- [ ] **Step 4: Run tests**

Run: `cd packages/server && npx vitest run tests/sse.test.ts`
Expected: All tests PASS

- [ ] **Step 5: Commit**

```bash
git add packages/server/src/routes/sse.ts packages/server/tests/sse.test.ts
git commit -m "feat(server): add limit parameter to SSE history replay"
```

---

### Task 3.3: Rust — Refactor SSE series collapse

**Files:**
- Modify: `rust/taskcast-server/src/routes/sse.rs`

- [ ] **Step 1: Replace inline collapse with shared function**

In `rust/taskcast-server/src/routes/sse.rs`, replace the late-join collapse block (roughly lines 271-317) with:

```rust
let replay_events = if !has_since_cursor {
    let engine_ref = Arc::clone(&engine_clone);
    let task_id_for_collapse = task_id_clone.clone();
    taskcast_core::series::collapse_accumulate_series(
        &history,
        |tid: &str, sid: &str| {
            let eng = Arc::clone(&engine_ref);
            let tid = tid.to_string();
            let sid = sid.to_string();
            async move {
                eng.get_series_latest(&tid, &sid).await
                    .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)
            }
        },
    ).await.unwrap_or(history)
} else {
    history
};
```

- [ ] **Step 2: Run all existing Rust SSE tests**

Run: `cargo test -p taskcast-server`
Expected: All tests PASS

- [ ] **Step 3: Commit**

```bash
git add rust/taskcast-server/src/routes/sse.rs
git commit -m "refactor(rust-server): SSE uses shared collapse_accumulate_series function"
```

---

### Task 3.4: Rust — Add `limit` to SSE history replay

**Files:**
- Modify: `rust/taskcast-server/src/routes/sse.rs`
- Add tests

- [ ] **Step 1: Add `limit` field to SseQuery**

```rust
pub limit: Option<String>,
```

- [ ] **Step 2: Parse limit and pass to engine.get_events()**

In the handler, parse the limit and include it in the `EventQueryOptions`:

```rust
let limit = query.limit.as_ref().and_then(|s| s.parse::<u64>().ok());
```

Include `limit` in the `since_opts` construction.

- [ ] **Step 3: Write and run tests**

Mirror the TS SSE limit test.

- [ ] **Step 4: Run all Rust server tests**

Run: `cargo test -p taskcast-server`
Expected: All PASS

- [ ] **Step 5: Commit**

```bash
git add rust/taskcast-server/src/routes/sse.rs rust/taskcast-server/tests/
git commit -m "feat(rust-server): add limit parameter to SSE history replay"
```

---

## Chunk 4: Final Verification + Documentation

### Task 4.1: Full test suite verification

- [ ] **Step 1: Run full TypeScript test suite**

Run: `pnpm test`
Expected: All packages pass

- [ ] **Step 2: Run full Rust test suite**

Run: `cargo test --workspace`
Expected: All packages pass

- [ ] **Step 3: Run TypeScript type check**

Run: `pnpm lint`
Expected: No type errors

- [ ] **Step 4: Run Rust clippy**

Run: `cargo clippy --workspace -- -D warnings`
Expected: No warnings

### Task 4.2: Update API documentation

**Files:**
- Modify: `docs/api/` — update history endpoint docs with new params
- Modify: `docs/api/*.zh.md` — Chinese equivalents if they exist for history

- [ ] **Step 1: Update REST API docs**

Add `limit` and `seriesFormat` query parameters to the history endpoint documentation.

- [ ] **Step 2: Update SSE docs**

Add `limit` query parameter to the SSE endpoint documentation.

- [ ] **Step 3: Commit**

```bash
git add docs/
git commit -m "docs: add limit + seriesFormat params to history and SSE endpoint docs"
```

### Task 4.3: Changeset

- [ ] **Step 1: Create changeset**

Run: `pnpm changeset`

Select: `@taskcast/core`, `@taskcast/server` — minor bump
Summary: "Add `limit` and `seriesFormat` query parameters to history endpoint. Engine falls back to LongTermStore for cold tasks. SSE refactored to use shared series collapse logic."

- [ ] **Step 2: Commit changeset**

```bash
git add .changeset/
git commit -m "chore: add changeset for limit + seriesFormat feature"
```
