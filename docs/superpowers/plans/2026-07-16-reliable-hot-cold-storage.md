# Reliable Hot/Cold Storage Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make PostgreSQL authoritative for complete history, release proven-durable task data from Redis safely, rehydrate cold tasks before mutation, and turn task TTL into a durable timeout transition.

**Architecture:** A core storage coordinator fences each task with a Redis epoch/write gate and a tokenized per-task lock. It streams a manifest-backed archive barrier into PostgreSQL before deleting hot keys, then restores only mutation state plus a bounded replay window on later writes. PostgreSQL supplies canonical history and multi-replica TTL claims; Redis becomes a bounded active/tail tier. Every externally visible server change is implemented in TypeScript and Rust in the same task.

**Tech Stack:** TypeScript ESM, Vitest, Hono, Redis Lua/ioredis, postgres.js, Rust, Tokio, Axum, sqlx, shared SQL migrations, SSE.

---

## Non-negotiable constraints

- Do not add `maxEvents`, silent truncation, or event-type denoising.
- Do not infer complete history from a non-empty Redis list.
- Do not delete Redis keys until PostgreSQL has committed and read-back verified the archive watermark.
- Do not enable release while an old writer can bypass the write fence.
- Keep TypeScript and Rust HTTP/JSON/SSE behavior identical at every implementation checkpoint.
- Production has `TASKCAST_AUTO_MIGRATE=FALSE`; migration is a manual release gate.

## File map

- Add shared lifecycle migration `migrations/postgres/003_storage_lifecycle.sql`.
- Add core storage types/contracts and a `storage-coordinator` in both `packages/core` and `rust/taskcast-core`.
- Extend Redis, PostgreSQL, memory, and SQLite adapters; split-tier release is active for Redis+PostgreSQL, while single-file SQLite keeps local fenced writes/TTL but reports hot/cold release unsupported.
- Add TypeScript/Rust release routes, canonical history, subscribe-before-snapshot SSE, TTL workers, config, readiness, metrics, and regression tests.
- Add a changeset only after both stacks pass parity tests.

### Task 1: Define parity types, fenced mutation contracts, and timeout transitions

**Files:**

- Modify: `packages/core/src/types.ts`
- Modify: `packages/core/src/state-machine.ts`
- Modify: `packages/core/src/index.ts`
- Create: `packages/core/tests/unit/storage-contract.test.ts`
- Modify: `packages/core/tests/unit/state-machine.test.ts`
- Modify: `rust/taskcast-core/src/types.rs`
- Modify: `rust/taskcast-core/src/state_machine.rs`
- Modify: `rust/taskcast-core/src/lib.rs`
- Create: `rust/taskcast-core/tests/storage_contract.rs`

- [ ] **Step 1: Write failing TypeScript and Rust contract tests**

Test camelCase/Serde parity for all new public response types and assert `pending`, `assigned`, `running`, `paused`, and `blocked` can transition to `timeout`, while terminal states cannot.

- [ ] **Step 2: Run and confirm missing-type/transition failures**

Run:

```bash
pnpm exec vitest run packages/core/tests/unit/storage-contract.test.ts packages/core/tests/unit/state-machine.test.ts
cargo test -p taskcast-core --test storage_contract
cargo test -p taskcast-core state_machine
```

Expected: FAIL because the storage contract does not exist and several non-terminal timeout transitions are currently forbidden.

- [ ] **Step 3: Add matching TypeScript/Rust domain types**

Use these semantics in both languages:

```ts
export type StorageState = "hot" | "releasing" | "cold";

export interface TaskStorageMetadata {
  taskId: string;
  storageState: StorageState;
  storageEpoch: number;
  activeReleaseGeneration: string | null;
  archiveWatermark: number;
  lastEventAt: number | null;
  coldAt: number | null;
  executionDeadlineAt: number | null;
  taskVersion: number;
}

export interface HotWriteToken {
  taskId: string;
  storageEpoch: number;
}
export interface StorageLease {
  taskId: string;
  lockToken: string;
  generation: string;
  storageEpoch: number;
}
export interface ReleasePreconditions {
  expectedLastEventIndex: number;
  inactiveSince: number;
}
export interface ReleaseResult {
  taskId: string;
  storageState: StorageState;
  archiveWatermark: number;
  released: boolean;
}
export interface ArchiveSourceManifest {
  priorWatermark: number;
  targetWatermark: number;
  sourceEntryCount: number;
  sourceDigest: string;
  seriesStateDigest: string;
  expectedBatchOrdinals: number[];
}

export interface DurableSeriesState {
  taskId: string;
  seriesId: string;
  mode: "latest" | "accumulate";
  event: TaskEvent;
  throughIndex: number;
}

export interface CanonicalHistoryEntry {
  event: TaskEvent;
  seriesThroughIndex?: number;
}
```

Add `ArchiveGeneration`, `ArchiveBatchReceipt`, `ArchiveSourcePage` with an opaque scan cursor, `RehydrateSnapshot`, `TtlClaim`, and `TerminalProjection` with the same field names/serialization. Add a retryable `StorageFenceConflictError`, `StorageBusyError`, `StorageIntegrityError`, and `StorageReleaseUnsupportedError` in both stacks.

- [ ] **Step 4: Extend store traits explicitly**

`ShortTermStore` gains tokenized storage lock methods, fence inspection/close/reopen, `commitEventFenced`, `saveTaskFenced`, bounded source-page reads, atomic `deleteTaskStorageFenced`, atomic `restoreHotTaskFenced`, writer readiness registration, and task-storage presence inspection.

`LongTermStore` gains metadata CAS, archive generation/batch/finalize/read-back methods, replay-window/series reads, overdue TTL claim/terminalize methods, terminal projection claims, and durable assignment save/delete. Use matching method semantics in Rust traits. A default capability flag may explicitly report `supportsHotColdRelease = false`; a store must never silently claim support for methods it does not implement.

- [ ] **Step 5: Pass contract/state tests**

Run:

```bash
pnpm exec vitest run packages/core/tests/unit/storage-contract.test.ts packages/core/tests/unit/state-machine.test.ts
cargo test -p taskcast-core --test storage_contract
cargo test -p taskcast-core state_machine
```

Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add packages/core/src packages/core/tests/unit/storage-contract.test.ts packages/core/tests/unit/state-machine.test.ts rust/taskcast-core/src rust/taskcast-core/tests/storage_contract.rs
git commit -m "feat(core): define fenced storage lifecycle contracts"
```

### Task 2: Implement reference memory fencing and single-tier SQLite behavior

**Files:**

- Modify: `packages/core/src/memory-adapters.ts`
- Modify: `packages/core/tests/unit/memory-adapters.test.ts`
- Modify: `rust/taskcast-core/src/memory_adapters.rs`
- Create: `rust/taskcast-core/tests/memory_storage_lifecycle.rs`
- Modify: `packages/sqlite/src/short-term.ts`
- Modify: `packages/sqlite/src/long-term.ts`
- Modify: `packages/sqlite/src/index.ts`
- Create: `packages/sqlite/migrations/002_storage_lifecycle.sql`
- Modify: `packages/sqlite/tests/short-term.test.ts`
- Modify: `packages/sqlite/tests/long-term.test.ts`
- Modify: `packages/sqlite/tests/factory.test.ts`
- Modify: `rust/taskcast-sqlite/src/short_term.rs`
- Modify: `rust/taskcast-sqlite/src/long_term.rs`
- Modify: `rust/taskcast-sqlite/src/lib.rs`
- Create: `rust/taskcast-sqlite/migrations/002_storage_lifecycle.sql`
- Modify: `rust/taskcast-sqlite/tests/short_term.rs`
- Modify: `rust/taskcast-sqlite/tests/long_term.rs`
- Modify: `rust/taskcast-sqlite/tests/factory.rs`

- [ ] **Step 1: Write adapter capability and fencing tests**

For separate memory stores, test local tokenized lock serialization, epoch-fenced event/task mutation, archive source paging, delete, restore, and TTL metadata. For SQLite, assert `supportsHotColdRelease=false`, release returns `StorageReleaseUnsupportedError`, but task writes/index allocation/series updates and local deadline claims are transactional and fenced.

- [ ] **Step 2: Run and confirm trait/capability failures**

Run:

```bash
pnpm exec vitest run packages/core/tests/unit/memory-adapters.test.ts packages/sqlite/tests/short-term.test.ts packages/sqlite/tests/long-term.test.ts packages/sqlite/tests/factory.test.ts
cargo test -p taskcast-core --test memory_storage_lifecycle
cargo test -p taskcast-sqlite
```

Expected: FAIL because the adapters do not implement the new contract.

- [ ] **Step 3: Implement deterministic in-memory reference behavior**

Use a per-task async mutex and monotonically increasing epoch. Keep distinct hot and long-term maps so core release/recovery tests can delete hot data while retaining durable data. Implement the same manifest/digest fixtures used by production adapters, making the memory implementation the pure behavioral oracle.

- [ ] **Step 4: Add SQLite lifecycle metadata without pretending it is split storage**

Add lifecycle/deadline/version columns and local assignment/outbox tables to the SQLite schema. Update both factory migration runners to apply migration 002 idempotently: inspect `PRAGMA table_info` before each additive column for existing databases, then create new tables/indexes with `IF NOT EXISTS`.

Because short- and long-term SQLite adapters share one database/table set, deleting “short-term” rows would delete authoritative history. Report hot/cold release unsupported, but implement fenced event/task transactions and restart-safe local TTL claim/outbox behavior.

- [ ] **Step 5: Pass adapter tests and full typechecks**

Run:

```bash
pnpm exec vitest run packages/core/tests/unit/memory-adapters.test.ts packages/sqlite/tests/short-term.test.ts packages/sqlite/tests/long-term.test.ts packages/sqlite/tests/factory.test.ts
pnpm lint
cargo test -p taskcast-core --test memory_storage_lifecycle
cargo test -p taskcast-sqlite
```

Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add packages/core/src/memory-adapters.ts packages/core/tests/unit/memory-adapters.test.ts rust/taskcast-core/src/memory_adapters.rs rust/taskcast-core/tests/memory_storage_lifecycle.rs packages/sqlite rust/taskcast-sqlite
git commit -m "feat(storage): add local lifecycle reference adapters"
```

### Task 3: Add PostgreSQL lifecycle, archive, TTL, assignment, and outbox schema

**Files:**

- Create: `migrations/postgres/003_storage_lifecycle.sql`
- Modify: `packages/postgres/tests/integration/migration-compat.test.ts`
- Modify: `packages/postgres/tests/integration/migration-runner.test.ts`
- Modify: `rust/taskcast-postgres/tests/store_tests.rs`

- [ ] **Step 1: Write migration assertions first**

Assert a migrated v2 database gains lifecycle columns/indexes without scanning or rewriting `taskcast_events`, and TypeScript migration checksums remain sqlx-compatible.

- [ ] **Step 2: Run and confirm schema failures**

Run:

```bash
pnpm exec vitest run packages/postgres/tests/integration/migration-compat.test.ts packages/postgres/tests/integration/migration-runner.test.ts
cargo test -p taskcast-postgres --test store_tests migration
```

Expected: FAIL because migration 003 is absent.

- [ ] **Step 3: Add task metadata columns and indexes**

Add to `taskcast_tasks`:

```sql
storage_state TEXT NOT NULL DEFAULT 'hot',
storage_epoch BIGINT NOT NULL DEFAULT 1,
active_release_generation TEXT,
archive_watermark BIGINT NOT NULL DEFAULT -1,
last_event_at BIGINT,
cold_at BIGINT,
execution_deadline_at BIGINT,
task_version BIGINT NOT NULL DEFAULT 0,
ttl_claim_token TEXT,
ttl_claim_until BIGINT,
release_requested_at BIGINT,
release_expected_index BIGINT,
release_inactive_since BIGINT
```

Add a storage-state check, `(storage_state,last_event_at)` index, partial non-terminal deadline index, and release-request index. Convert `taskcast_events.idx` to `BIGINT` without a table rewrite if PostgreSQL confirms the cast is metadata-safe; otherwise keep INTEGER in this migration and document the separate online migration.

- [ ] **Step 4: Add coordination tables**

Create:

- `taskcast_archive_generations` keyed by `(task_id,generation)` with epoch, manifest, status, timestamps;
- `taskcast_archive_batches` keyed by `(task_id,generation,ordinal)` with previous/current digest, source coverage, entry count;
- `taskcast_series_state` keyed by `(task_id,series_id)` with canonical event JSON/mode and `through_index`;
- `taskcast_durable_assignments` keyed by task ID with worker/cost/assignment ID;
- `taskcast_terminal_outbox` keyed by assignment/event identity with claim token/until and projection status.

Archive tables store receipts/manifests, not duplicate event payload blobs. Add only targeted indexes for incomplete generations, due TTL rows, and pending projections.

- [ ] **Step 5: Pass migration compatibility tests**

Run:

```bash
pnpm exec vitest run packages/postgres/tests/integration/migration-compat.test.ts packages/postgres/tests/integration/migration-runner.test.ts
cargo test -p taskcast-postgres --test store_tests migration
```

Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add migrations/postgres/003_storage_lifecycle.sql packages/postgres/tests/integration/migration-compat.test.ts packages/postgres/tests/integration/migration-runner.test.ts rust/taskcast-postgres/tests/store_tests.rs
git commit -m "feat(postgres): add storage lifecycle schema"
```

### Task 4: Implement the manifest-backed PostgreSQL archive barrier in both stacks

**Files:**

- Modify: `packages/postgres/src/long-term.ts`
- Modify: `packages/postgres/tests/long-term.test.ts`
- Create: `packages/postgres/tests/integration/archive-barrier.test.ts`
- Modify: `rust/taskcast-postgres/src/store.rs`
- Modify: `rust/taskcast-postgres/tests/store_tests.rs`

- [ ] **Step 1: Write identical barrier behavior tests**

Cover idempotent begin/resume, bounded numbered batches, response loss after finalize, conflicting event ID/index content, missing middle ordinal, duplicate/reordered ordinal, broken previous digest, wrong count/source digest, wrong series digest, stale generation, and watermark monotonicity.

- [ ] **Step 2: Run and confirm missing-method failures**

Run:

```bash
pnpm exec vitest run packages/postgres/tests/long-term.test.ts packages/postgres/tests/integration/archive-barrier.test.ts
cargo test -p taskcast-postgres --test store_tests archive
```

Expected: FAIL because PostgreSQL currently has only fire-and-forget-compatible `saveEvent` methods.

- [ ] **Step 3: Implement idempotent generation and batch receipts**

`beginArchive()` inserts or validates an existing generation against `(taskId,watermark,storageEpoch,generation,manifest)`. `archiveBatch()` transactionally upserts canonical event/series state and inserts a receipt; replay with identical digest succeeds, conflicting content fails with `StorageIntegrityError`. Each latest/accumulate update advances `taskcast_series_state.through_index` to the source event index, so history merging can distinguish an accumulated snapshot from later Redis deltas.

Use SHA-256 over a stable UTF-8 canonical encoding shared by TS/Rust: ordered scalar fields, canonical JSON data, and newline-delimited event records. Add cross-language digest fixtures so neither runtime relies on object key insertion order.

- [ ] **Step 4: Finalize only a complete digest chain**

In one PostgreSQL transaction, lock generation/task rows, verify every expected ordinal exactly once, verify chained digests/count/index-ID coverage/series digest, save final task and series state, then advance `archive_watermark`. Never infer completion from event row count because `latest` and `accumulate` are compacted.

All final state changes compare `active_release_generation`, storage epoch, and expected `releasing` state. A stale executor may leave harmless uncommitted receipts but cannot finalize.

- [ ] **Step 5: Pass both adapter suites**

Run:

```bash
pnpm exec vitest run packages/postgres/tests/long-term.test.ts packages/postgres/tests/integration/archive-barrier.test.ts
cargo test -p taskcast-postgres --test store_tests archive
```

Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add packages/postgres/src/long-term.ts packages/postgres/tests/long-term.test.ts packages/postgres/tests/integration/archive-barrier.test.ts rust/taskcast-postgres/src/store.rs rust/taskcast-postgres/tests/store_tests.rs
git commit -m "feat(postgres): add verifiable archive barrier"
```

### Task 5: Add atomic Redis write fences, locks, source scans, deletion, and rehydration

**Files:**

- Modify: `packages/redis/src/short-term.ts`
- Modify: `packages/redis/tests/short-term.test.ts`
- Create: `packages/redis/tests/storage-lifecycle.test.ts`
- Modify: `rust/taskcast-redis/src/short_term.rs`
- Modify: `rust/taskcast-redis/tests/short_term_tests.rs`
- Modify: `rust/taskcast-redis/tests/concurrent.rs`

- [ ] **Step 1: Write fence/lock/race tests**

Test tokenized acquire/renew/compare-delete, stale token rejection, fence close racing a writer that already read the old epoch, all task-key deletion, expired lock recovery, stale executor delete/reopen rejection, atomic replay-window restore, and writer-readiness TTL expiry.

- [ ] **Step 2: Run and confirm current multi-command behavior fails**

Run:

```bash
pnpm exec vitest run packages/redis/tests/short-term.test.ts packages/redis/tests/storage-lifecycle.test.ts
cargo test -p taskcast-redis --test short_term_tests --test concurrent
```

Expected: FAIL because no fence/lock keys exist and current index/series/event writes are separate commands.

- [ ] **Step 3: Add task-specific keys**

```ts
fence: `${prefix}:writeFence:${taskId}`,
storageLock: `${prefix}:storageLock:${taskId}`,
hotWindow: `${prefix}:hotWindow:${taskId}`,
writers: `${prefix}:storageWriters`,
writer: `${prefix}:storageWriter:${instanceId}`,
```

Fence JSON contains `acceptingWrites`, `storageEpoch`, and active generation. The lock value contains token/generation and uses PX expiry. Writer readiness entries contain protocol version/build and expire unless heartbeated.

- [ ] **Step 4: Replace event writes with one fenced Lua commit**

`commitEventFenced()` validates an open fence and expected epoch, allocates the next index, applies keep-all/latest/accumulate semantics, appends the delta/replacement, updates series state, and returns the raw plus accumulated event in one script. A fence conflict consumes no index and returns a retryable typed error.

`saveTaskFenced()` similarly checks the epoch in the same script as the task update. Keep unfenced methods only for initial task creation and controlled archive import; engine mutation paths must stop calling them.

- [ ] **Step 5: Implement bounded source pages and atomic lifecycle scripts**

`readArchiveSourcePage(cursor,watermark,limit)` uses an opaque list-offset cursor and bounded LRANGE windows, then validates strictly increasing source indexes up to the watermark. It must not assume list position equals global index because latest-series replacement makes indexes sparse. `closeWriteFence` captures high watermark. Delete script checks lock token, generation, epoch, and closed fence before deleting task/events/index/fence/hot-window/all series keys. Restore script installs task, replay window, series latest, `nextIndex=max+1`, hot-window bounds, incremented epoch, and open fence atomically.

- [ ] **Step 6: Pass TS/Rust Redis suites**

Run:

```bash
pnpm exec vitest run packages/redis/tests/short-term.test.ts packages/redis/tests/storage-lifecycle.test.ts
cargo test -p taskcast-redis --test short_term_tests --test concurrent
```

Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add packages/redis/src/short-term.ts packages/redis/tests/short-term.test.ts packages/redis/tests/storage-lifecycle.test.ts rust/taskcast-redis/src/short_term.rs rust/taskcast-redis/tests/short_term_tests.rs rust/taskcast-redis/tests/concurrent.rs
git commit -m "feat(redis): fence and release task storage atomically"
```

### Task 6: Implement the core release coordinator and crash recovery

**Files:**

- Create: `packages/core/src/storage-coordinator.ts`
- Modify: `packages/core/src/engine.ts`
- Modify: `packages/core/src/index.ts`
- Create: `packages/core/tests/unit/storage-coordinator.test.ts`
- Create: `packages/core/tests/integration/storage-release.test.ts`
- Create: `rust/taskcast-core/src/storage_coordinator.rs`
- Modify: `rust/taskcast-core/src/engine.rs`
- Modify: `rust/taskcast-core/src/lib.rs`
- Create: `rust/taskcast-core/tests/storage_release.rs`

- [ ] **Step 1: Write release protocol tests**

Cover already-cold idempotency, expected-index/inactive-since conflicts, PostgreSQL outage, manifest failure, publish racing fence close, lock renewal loss, process crash before and after Redis deletion, stale executor wake-up, and failure cleanup reopening a new epoch.

- [ ] **Step 2: Run and confirm missing release operation**

Run:

```bash
pnpm exec vitest run packages/core/tests/unit/storage-coordinator.test.ts packages/core/tests/integration/storage-release.test.ts
cargo test -p taskcast-core --test storage_release
```

Expected: FAIL because `releaseTaskStorage` does not exist.

- [ ] **Step 3: Implement bounded manifest sealing**

`describeArchiveSource()` makes two bounded passes while the fence is closed: one to compute source/index-ID and series digests/count/ordinals, then one to upload numbered batches. It holds at most `archiveBatchSize` events in memory. Abort permanently after lock renewal failure.

- [ ] **Step 4: Implement the exact release state machine**

```ts
async releaseTaskStorage(taskId: string, pre: ReleasePreconditions): Promise<ReleaseResult>
```

Acquire lease/generation; verify all writer readiness; close fence/capture watermark; validate preconditions; CAS PostgreSQL `hot -> releasing`; seal/upload/finalize archive; read back watermark/generation; atomically delete Redis; CAS `releasing -> cold`; release lock. A failure while still owning the generation reopens with a new epoch and returns durable state to hot. After lease loss, perform no cleanup mutation.

Recovery acquires a new generation, invalidates the old one, inspects durable watermark and Redis presence, then either finishes `cold` after proven deletion or reopens `hot`. Never mark cold solely because metadata says `releasing`.

- [ ] **Step 5: Make every normal engine mutation call `ensureTaskHotForWrite`**

Creation initializes hot metadata/fence. `publishEvent` and `transitionTask` obtain a `HotWriteToken`, use fenced adapter methods, and retry a fence conflict through the gate. Replace the async long-term event write as a correctness source with best-effort low-latency dual-write plus the release barrier as the deletion proof.

- [ ] **Step 6: Pass core release tests**

Run:

```bash
pnpm exec vitest run packages/core/tests/unit/storage-coordinator.test.ts packages/core/tests/integration/storage-release.test.ts packages/core/tests/unit/concurrent-publish.test.ts
cargo test -p taskcast-core --test storage_release --test concurrent_publish
```

Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add packages/core/src/storage-coordinator.ts packages/core/src/engine.ts packages/core/src/index.ts packages/core/tests/unit/storage-coordinator.test.ts packages/core/tests/integration/storage-release.test.ts rust/taskcast-core/src/storage_coordinator.rs rust/taskcast-core/src/engine.rs rust/taskcast-core/src/lib.rs rust/taskcast-core/tests/storage_release.rs
git commit -m "feat(core): release hot storage behind durable watermark"
```

### Task 7: Rehydrate cold tasks before writes without rehydrating reads

**Files:**

- Modify: `packages/core/src/storage-coordinator.ts`
- Modify: `packages/core/src/engine.ts`
- Create: `packages/core/tests/integration/storage-rehydration.test.ts`
- Modify: `rust/taskcast-core/src/storage_coordinator.rs`
- Modify: `rust/taskcast-core/src/engine.rs`
- Create: `rust/taskcast-core/tests/storage_rehydration.rs`

- [ ] **Step 1: Write cold-write tests**

Release a task, publish later, and assert the next global index is `durableMax+1`; latest/accumulate series continue correctly; only the configured last 1,000 events return to Redis; a read leaves it cold; concurrent rehydrators yield one epoch; stale pre-rehydrate writer is rejected.

- [ ] **Step 2: Run and confirm index-zero recreation**

Run:

```bash
pnpm exec vitest run packages/core/tests/integration/storage-rehydration.test.ts
cargo test -p taskcast-core --test storage_rehydration
```

Expected: FAIL before rehydration implementation.

- [ ] **Step 3: Implement rehydration under a new generation**

Load durable task/metadata/max index/series state and `rehydrateReplayEvents` recent canonical events from PostgreSQL. Increment epoch and call atomic `restoreHotTaskFenced`; CAS durable state to hot and clear cold/generation. On any failure, keep durable state cold and reject the original mutation as retryable.

For split storage only. SQLite uses its single durable database and never deletes its mutation state; its release call returns `storage_release_not_supported`, but its fenced mutation and TTL behavior remain tested.

- [ ] **Step 4: Pass rehydration tests and commit**

Run:

```bash
pnpm exec vitest run packages/core/tests/integration/storage-rehydration.test.ts packages/core/tests/unit/engine-series.test.ts
cargo test -p taskcast-core --test storage_rehydration --test engine_series
```

```bash
git add packages/core/src/storage-coordinator.ts packages/core/src/engine.ts packages/core/tests/integration/storage-rehydration.test.ts rust/taskcast-core/src/storage_coordinator.rs rust/taskcast-core/src/engine.rs rust/taskcast-core/tests/storage_rehydration.rs
git commit -m "feat(core): rehydrate cold tasks before mutation"
```

### Task 8: Make PostgreSQL the canonical history baseline with Redis tail overlay

**Files:**

- Create: `packages/core/src/canonical-history.ts`
- Modify: `packages/core/src/engine.ts`
- Modify: `packages/core/src/index.ts`
- Create: `packages/core/tests/unit/canonical-history.test.ts`
- Create: `packages/core/tests/integration/hot-cold-history.test.ts`
- Create: `rust/taskcast-core/src/canonical_history.rs`
- Modify: `rust/taskcast-core/src/engine.rs`
- Modify: `rust/taskcast-core/src/lib.rs`
- Create: `rust/taskcast-core/tests/hot_cold_history.rs`

- [ ] **Step 1: Write the current bug regression first**

Place indexes 0–9 in PostgreSQL and only 8–10 in Redis. `getEvents()` must return canonical 0–10, not Redis 8–10. Add conflicts with same index/different ID/data, async-tail read visibility, cold-only reads, `since.id/index/timestamp`, limits, and latest/accumulate hot-cold equality.

- [ ] **Step 2: Run and confirm Redis hides durable history**

Run:

```bash
pnpm exec vitest run packages/core/tests/unit/canonical-history.test.ts packages/core/tests/integration/hot-cold-history.test.ts
cargo test -p taskcast-core --test hot_cold_history
```

Expected: FAIL because current `getEvents()` returns Redis whenever it is non-empty.

- [ ] **Step 3: Implement canonical merge rules**

When long-term storage exists, always query PostgreSQL as baseline. If metadata is hot, read the Redis replay/tail window and merge by global index/event identity. Identical overlap deduplicates; conflicting identity/content raises `StorageIntegrityError`. For a durable `latest`/`accumulate` snapshot, ignore Redis deltas at or below its `throughIndex` and apply only later deltas; this prevents double accumulation while preserving read-after-write visibility. Apply requested series representation and caller cursor/limit only after the canonical stream is assembled.

For bounded `limit`, implement a page merge that fetches PostgreSQL and Redis pages until it can prove the earliest requested `limit` events; do not materialize a 600k history merely to return 100 events. Without long-term storage, preserve short-term behavior.

- [ ] **Step 4: Make series-latest reads durable-aware**

Late-join accumulation collapse must use PostgreSQL series state plus hot overlay, not short-term-only `getSeriesLatest`, so cold and hot snapshots match byte-for-byte.

- [ ] **Step 5: Pass TS/Rust history tests and commit**

Run:

```bash
pnpm exec vitest run packages/core/tests/unit/canonical-history.test.ts packages/core/tests/integration/hot-cold-history.test.ts
cargo test -p taskcast-core --test hot_cold_history
```

```bash
git add packages/core/src/canonical-history.ts packages/core/src/engine.ts packages/core/src/index.ts packages/core/tests/unit/canonical-history.test.ts packages/core/tests/integration/hot-cold-history.test.ts rust/taskcast-core/src/canonical_history.rs rust/taskcast-core/src/engine.rs rust/taskcast-core/src/lib.rs rust/taskcast-core/tests/hot_cold_history.rs
git commit -m "fix(core): merge durable history with hot tail"
```

### Task 9: Fix SSE snapshot races in TypeScript and Rust

**Files:**

- Modify: `packages/server/src/routes/sse.ts`
- Modify: `packages/server/tests/sse.test.ts`
- Create: `packages/server/tests/hot-cold-sse.test.ts`
- Modify: `rust/taskcast-server/src/routes/sse.rs`
- Modify: `rust/taskcast-server/tests/sse_filter.rs`
- Create: `rust/taskcast-server/tests/hot_cold_sse.rs`

- [ ] **Step 1: Write publish-during-snapshot tests**

Block history fetch, connect SSE, publish while blocked, release snapshot, and assert the event appears once. Repeat for a cold task that rehydrates during connection, since cursors, terminal replay, and accumulated series. Compare exact TS/Rust SSE frames and IDs.

- [ ] **Step 2: Run and confirm snapshot-then-subscribe misses events**

Run:

```bash
pnpm exec vitest run packages/server/tests/sse.test.ts packages/server/tests/hot-cold-sse.test.ts
cargo test -p taskcast-server --test sse_filter --test hot_cold_sse
```

Expected: FAIL because both routes currently fetch history before subscribing.

- [ ] **Step 3: Subscribe before snapshot and buffer by global index**

Create the broadcast subscription first, buffer live events, then fetch canonical history. Send the filtered/collapsed snapshot, discard buffered events identical to snapshot entries, fail on conflicting same-index content, and drain higher-index buffered events before switching to live delivery. Keep one monotonic filtered index and existing `taskcast.done` semantics.

Do not rehydrate on SSE reads. A cold non-terminal task remains subscribed so a later writer's rehydration/publish is delivered.

- [ ] **Step 4: Pass exact-frame parity tests and commit**

Run:

```bash
pnpm exec vitest run packages/server/tests/sse.test.ts packages/server/tests/hot-cold-sse.test.ts packages/server/tests/sse-series-format.test.ts
cargo test -p taskcast-server --test sse_filter --test hot_cold_sse --test sse_series_format
```

```bash
git add packages/server/src/routes/sse.ts packages/server/tests/sse.test.ts packages/server/tests/hot-cold-sse.test.ts rust/taskcast-server/src/routes/sse.rs rust/taskcast-server/tests/sse_filter.rs rust/taskcast-server/tests/hot_cold_sse.rs
git commit -m "fix(server): subscribe before SSE history snapshot"
```

### Task 10: Add release HTTP APIs, writer readiness, and exact parity responses

**Files:**

- Modify: `packages/server/src/routes/tasks.ts`
- Modify: `packages/server/src/index.ts`
- Modify: `packages/server/src/schemas.ts`
- Create: `packages/server/tests/storage-release-routes.test.ts`
- Modify: `packages/server/tests/health-detail.test.ts`
- Modify: `rust/taskcast-server/src/routes/tasks.rs`
- Modify: `rust/taskcast-server/src/app.rs`
- Modify: `rust/taskcast-server/src/openapi.rs`
- Create: `rust/taskcast-server/tests/storage_release_routes.rs`
- Modify: `rust/taskcast-server/tests/health_detail.rs`

- [ ] **Step 1: Write route/auth/parity tests**

Test `POST /tasks/:taskId/storage/release` with `{expectedLastEventIndex,inactiveSince}`. Assert `task:manage` required, 404 missing, 409 stale precondition/busy, 503 adapter or writer-readiness outage, idempotent 200 already-cold, and identical camelCase body/OpenAPI in both stacks.

- [ ] **Step 2: Run and confirm 404**

Run:

```bash
pnpm exec vitest run packages/server/tests/storage-release-routes.test.ts
cargo test -p taskcast-server --test storage_release_routes
```

Expected: FAIL because the route is not mounted.

- [ ] **Step 3: Add explicit release request persistence**

Before attempting work, persist bounded release preconditions (`release_requested_at`, expected index, inactive cutoff). This makes a PostgreSQL outage after operator intent retryable by the retention worker. Clear the request only after success or a proven stale-precondition conflict.

Response shape is always `ReleaseResult`. Use stable JSON error codes `storage_precondition_failed`, `storage_busy`, `storage_integrity_error`, `storage_release_unsupported`, and `storage_unavailable`.

- [ ] **Step 4: Register every live writer**

On app start, heartbeat `{instanceId,storageProtocolVersion:2,build}` into short-term storage; stop heartbeat on `stop()`/graceful shutdown. Release checks that every unexpired writer is protocol 2. `/health/detail` reports readiness and incompatible writer IDs without exposing credentials.

- [ ] **Step 5: Pass route/readiness tests and commit**

Run:

```bash
pnpm exec vitest run packages/server/tests/storage-release-routes.test.ts packages/server/tests/health-detail.test.ts
cargo test -p taskcast-server --test storage_release_routes --test health_detail
```

```bash
git add packages/server/src/routes/tasks.ts packages/server/src/index.ts packages/server/src/schemas.ts packages/server/tests/storage-release-routes.test.ts packages/server/tests/health-detail.test.ts rust/taskcast-server/src/routes/tasks.rs rust/taskcast-server/src/app.rs rust/taskcast-server/src/openapi.rs rust/taskcast-server/tests/storage_release_routes.rs rust/taskcast-server/tests/health_detail.rs
git commit -m "feat(server): expose guarded storage release"
```

### Task 11: Implement durable multi-replica execution TTL and terminal projection

**Files:**

- Create: `packages/core/src/ttl-coordinator.ts`
- Modify: `packages/core/src/engine.ts`
- Modify: `packages/core/src/worker-manager.ts`
- Create: `packages/core/tests/integration/durable-ttl.test.ts`
- Modify: `packages/postgres/src/long-term.ts`
- Create: `packages/postgres/tests/integration/ttl-claims.test.ts`
- Create: `rust/taskcast-core/src/ttl_coordinator.rs`
- Modify: `rust/taskcast-core/src/engine.rs`
- Modify: `rust/taskcast-core/src/worker_manager.rs`
- Create: `rust/taskcast-core/tests/durable_ttl.rs`
- Modify: `rust/taskcast-postgres/src/store.rs`
- Modify: `rust/taskcast-postgres/tests/store_tests.rs`

- [ ] **Step 1: Write the durable TTL matrix**

Test timeout from all five non-terminal states, two replicas, restart with overdue rows, PostgreSQL failure, claim expiry/steal, completion/cancellation race, non-terminal version race, cold task timeout, crash after DB commit before Redis projection, and assigned/running capacity release exactly once.

- [ ] **Step 2: Run and confirm Redis expiry is not a transition**

Run:

```bash
pnpm exec vitest run packages/core/tests/integration/durable-ttl.test.ts packages/postgres/tests/integration/ttl-claims.test.ts
cargo test -p taskcast-core --test durable_ttl
cargo test -p taskcast-postgres --test store_tests ttl
```

Expected: FAIL because current TTL only expires Redis keys and PostgreSQL remains non-terminal.

- [ ] **Step 3: Store absolute deadlines durably at creation/update**

PostgreSQL computes deadline from database time plus `ttl`; changing/resuming TTL updates it transactionally. Pausing may clear/suspend the deadline according to existing semantics, but Redis `PERSIST/EXPIRE` is no longer the lifecycle source of truth. Local memory/SQLite scheduler stores the same absolute semantics for non-PostgreSQL use.

- [ ] **Step 4: Claim overdue rows with DB time and tokens**

Use short `FOR UPDATE SKIP LOCKED` batches to set unique `ttl_claim_token` and `ttl_claim_until` from PostgreSQL time, then release the scan transaction. Only the current token may continue/clear/reopen/project.

- [ ] **Step 5: Terminalize task/event/assignment/outbox in one transaction**

Ensure hot/fence under a TTL generation and reserve an index. PostgreSQL compare-and-sets task version/non-terminal status, writes the timeout status event at that index, deletes durable assignment, inserts one terminal outbox row keyed by assignment/event, and clears claim. A terminal race discards the reservation and projects the winner; a non-terminal version race reopens at a new epoch.

The outbox idempotently projects task/event/fence to Redis, broadcasts the event, removes Redis assignment, and releases worker capacity once. Add a repair sweep for terminal tasks with stale assignment state.

- [ ] **Step 6: Pass TTL/worker tests**

Run:

```bash
pnpm exec vitest run packages/core/tests/integration/durable-ttl.test.ts packages/postgres/tests/integration/ttl-claims.test.ts packages/core/tests/unit/worker-manager.test.ts
cargo test -p taskcast-core --test durable_ttl --test worker_manager_extended
cargo test -p taskcast-postgres --test store_tests ttl
```

Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add packages/core/src/ttl-coordinator.ts packages/core/src/engine.ts packages/core/src/worker-manager.ts packages/core/tests/integration/durable-ttl.test.ts packages/postgres/src/long-term.ts packages/postgres/tests/integration/ttl-claims.test.ts rust/taskcast-core/src/ttl_coordinator.rs rust/taskcast-core/src/engine.rs rust/taskcast-core/src/worker_manager.rs rust/taskcast-core/tests/durable_ttl.rs rust/taskcast-postgres/src/store.rs rust/taskcast-postgres/tests/store_tests.rs
git commit -m "fix(core): make execution TTL durable"
```

### Task 12: Wire config, TTL/release workers, retries, and observability in both CLIs

**Files:**

- Modify: `packages/core/src/config.ts`
- Modify: `packages/core/tests/unit/config.test.ts`
- Modify: `packages/server/src/index.ts`
- Modify: `packages/cli/src/commands/start.ts`
- Modify: `packages/cli/tests/startup.test.ts`
- Modify: `rust/taskcast-core/src/config.rs`
- Modify: `rust/taskcast-server/src/app.rs`
- Modify: `rust/taskcast-cli/src/commands/start.rs`
- Modify: `rust/taskcast-cli/tests/start_env_tests.rs`

- [ ] **Step 1: Write config parity tests**

Cover env and config-file behavior for:

```text
TASKCAST_HOT_RETENTION_ENABLED=false
TASKCAST_HOT_RETENTION_TERMINAL_SECONDS
TASKCAST_HOT_RETENTION_IDLE_SECONDS
TASKCAST_REHYDRATE_REPLAY_EVENTS=1000
TASKCAST_STORAGE_LOCK_TTL_SECONDS
TASKCAST_TTL_SWEEP_INTERVAL_SECONDS
TASKCAST_TTL_SWEEP_BATCH_SIZE
```

Reject zero/negative/NaN values consistently. Automatic release defaults off; durable TTL sweeping defaults on only when the long-term adapter advertises TTL support.

- [ ] **Step 2: Run and confirm config failures**

Run:

```bash
pnpm exec vitest run packages/core/tests/unit/config.test.ts packages/cli/tests/startup.test.ts
cargo test -p taskcast-core proptest_config
cargo test -p taskcast-cli --test start_env_tests
```

Expected: FAIL because settings are absent.

- [ ] **Step 3: Start independent bounded workers**

Wire TTL claim, terminal projection repair, incomplete release recovery, persisted release-request retry, and optional terminal-task retention. Each iteration has a small batch, overlap guard, bounded backoff, structured error, and stop hook. A PostgreSQL outage retains Redis and slows retries; it never deletes or marks cold.

Non-terminal tasks are not auto-released from silence. The embedding service must call the explicit release route after owner/session release; the persisted request lets Taskcast retry safely. Terminal tasks may be requested automatically after grace.

- [ ] **Step 4: Add structured metrics/log hooks**

Record hot/releasing/cold counts, bytes/events released, archive duration/failure, precondition conflicts, rehydration size, watermark mismatch, history source/latency, overdue TTL, race outcomes, projection repair, and unusually old/large hot tasks. Log task IDs/watermarks, never payloads by default.

- [ ] **Step 5: Pass startup/config tests and commit**

Run:

```bash
pnpm exec vitest run packages/core/tests/unit/config.test.ts packages/cli/tests/startup.test.ts
cargo test -p taskcast-core proptest_config
cargo test -p taskcast-cli --test start_env_tests
```

```bash
git add packages/core/src/config.ts packages/core/tests/unit/config.test.ts packages/server/src/index.ts packages/cli/src/commands/start.ts packages/cli/tests/startup.test.ts rust/taskcast-core/src/config.rs rust/taskcast-server/src/app.rs rust/taskcast-cli/src/commands/start.rs rust/taskcast-cli/tests/start_env_tests.rs
git commit -m "feat(cli): run durable storage lifecycle workers"
```

### Task 13: Add production incident regression, bounded-memory test, runbook, and changeset

**Files:**

- Create: `packages/core/tests/integration/incident-hot-cold.test.ts`
- Create: `packages/postgres/tests/integration/large-release.test.ts`
- Create: `rust/tests/hot_cold_parity.test.ts`
- Create: `docs/guide/hot-cold-storage.md`
- Create: `docs/guide/hot-cold-storage.zh.md`
- Create: `docs/runbooks/2026-07-16-production-hot-cold-rollout.md`
- Create: `.changeset/reliable-hot-cold-storage.md`

- [ ] **Step 1: Build a reduced `01KRK8Y78MA3SV416YNAV3E3KJ` fixture**

Create a pending/no-TTL task with many retry-cycle events, keep-all and compacted series, release it, query full history, then publish later. Assert Redis task keys disappear, PostgreSQL remains complete, rehydration restores bounded replay only, next index remains monotonic, and no `maxEvents` behavior exists.

- [ ] **Step 2: Prove bounded archive memory and unrelated-task progress**

Generate at least 600,000 small events through a streaming fixture without holding them all in the test process. Instrument batch size/resident growth, release concurrently with another task's publishes, and assert no archive batch exceeds configured size and unrelated work continues.

- [ ] **Step 3: Run cross-stack parity**

Run:

```bash
pnpm exec vitest run packages/core/tests/integration/incident-hot-cold.test.ts packages/postgres/tests/integration/large-release.test.ts rust/tests/hot_cold_parity.test.ts
pnpm test
pnpm lint
cargo test --workspace
```

Expected: all tests pass and the parity harness reports identical history, cursors, release responses, status codes, and SSE frames.

- [ ] **Step 4: Write operator documentation**

Document why the old mechanism failed: Redis was unbounded short-term storage, PostgreSQL was only async dual-write, reads preferred any Redis prefix/tail, and TTL expired keys without durable terminalization. Include config, release/rehydrate semantics, 409/503 handling, metrics, and explicit confirmation that no event cap was added.

- [ ] **Step 5: Write the production rollout runbook**

Exact order:

1. Back up/verify the target task archive and database.
2. Manually apply migration 003 because auto-migrate is false.
3. Deploy TS/Rust readers/writers with release and automatic retention disabled.
4. Drain old writers; verify writer-readiness reports only protocol 2.
5. Enable PostgreSQL-canonical history and sample hot/cold parity.
6. Enable operator release only.
7. After agent-pi quarantines poison messages and Team9 releases ownership, cancel the incident task and release its storage.
8. Run small oldest-first batches; pause on archive failures, Redis latency, or PostgreSQL pressure.
9. Enable terminal retention; keep non-terminal release explicit.
10. Enable TTL sweeper/projection separately after readiness is green.

Include SQL verification for new columns/indexes, watermark checks, Redis key checks, rollback conditions, and recovery commands that never delete unverified Redis data.

- [ ] **Step 6: Add a fixed-version changeset**

Create a minor changeset for `@taskcast/core`, `@taskcast/server`, `@taskcast/redis`, `@taskcast/postgres`, and `@taskcast/cli`; fixed versioning will bump all packages. Mention the new release endpoint, canonical history, and durable TTL.

- [ ] **Step 7: Final verification and commit**

```bash
pnpm test:coverage
pnpm build
pnpm lint
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
git diff --check
```

Expected: 100% required coverage and all CI-equivalent checks pass.

```bash
git add packages/core/tests/integration/incident-hot-cold.test.ts packages/postgres/tests/integration/large-release.test.ts rust/tests/hot_cold_parity.test.ts docs/guide/hot-cold-storage.md docs/guide/hot-cold-storage.zh.md docs/runbooks/2026-07-16-production-hot-cold-rollout.md .changeset/reliable-hot-cold-storage.md
git commit -m "test(storage): cover production hot-cold lifecycle"
```

## Acceptance verification

- [ ] Redis deletion is impossible without a verified PostgreSQL archive watermark.
- [ ] A partial Redis replay window cannot hide PostgreSQL history.
- [ ] Publish/release and publish/rehydrate races preserve every event and index.
- [ ] Reads never rehydrate cold tasks.
- [ ] Every non-terminal TTL produces one durable timeout event or loses safely to another transition.
- [ ] Assigned/running timeout releases durable assignment and worker capacity exactly once.
- [ ] Old writer readiness blocks release.
- [ ] TypeScript and Rust route/history/SSE parity is exact.
- [ ] No `maxEvents`, silent truncation, or event-type denoising exists.
