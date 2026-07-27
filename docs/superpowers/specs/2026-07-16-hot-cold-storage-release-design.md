# Reliable Hot/Cold Storage and Execution TTL Design

**Status:** Approved design, implementation pending

**Date:** 2026-07-16

**Scope:** `@taskcast/core`, `@taskcast/server`, `@taskcast/redis`, `@taskcast/postgres`, CLI configuration, and the Rust server equivalents

## Summary

Taskcast currently treats Redis as both the active-task state store and an unbounded event buffer. PostgreSQL receives long-term event writes, but there is no protocol that proves those writes are durable and then releases the Redis copy. History reads also prefer Redis whenever Redis contains any events, so a partial hot copy can hide complete PostgreSQL history. Finally, task `ttl` is implemented as Redis key expiry rather than as a reliable lifecycle transition.

This design adds an explicit, concurrency-safe hot-to-cold release protocol. PostgreSQL becomes authoritative for history whenever a long-term store is configured. Redis remains the active write/state tier and bounded replay tier. A cold task is rehydrated before any later mutation, with its next event index and series state restored from PostgreSQL. Execution TTL becomes a scheduler-backed task timeout and is separated from storage retention.

This first version deliberately does **not** add a generic `maxEvents` limit. Production has legitimate large histories, so an arbitrary cap would trade the current memory problem for silent loss or broken replay semantics.

## Incident Evidence and Motivation

The production Team9 deployment showed the following state during the investigation:

- The suspicious task `01KRK8Y78MA3SV416YNAV3E3KJ` reached 636,966 durable events and was still `pending` with no TTL.
- PostgreSQL contained about 4.79 million Taskcast events across 4,725 tasks.
- Event-count distribution was approximately p50 156, p95 2,903, p99 14,754, with four tasks above 100,000 events.
- Several non-incident tasks legitimately contained tens of thousands of `agent.message_update` events.
- The target task's Redis task, event, index, and series keys all had no expiry.
- All 4,719 observed `agent.session` tasks in PostgreSQL were `pending` with `ttl = NULL`.
- Tasks that did receive a TTL eventually disappeared from Redis, proving Redis expiry itself works; their PostgreSQL task rows nevertheless remained non-terminal.

These observations lead to three distinct problems:

1. There is no reliable Redis release mechanism for long-lived tasks without an execution TTL.
2. Redis expiry is not equivalent to lifecycle timeout and leaves PostgreSQL state inconsistent.
3. A history cap cannot be chosen safely from the current distribution and would not solve lifecycle correctness.

## Goals

1. Release Redis event/state data only after Taskcast proves PostgreSQL has a durable, self-consistent archive through a known event watermark.
2. Make PostgreSQL authoritative for full history when configured.
3. Permit inactive pending tasks to become cold without making them terminal.
4. Rehydrate cold tasks before a later publish or status mutation, preserving monotonic event indexes and series semantics.
5. Make release safe under concurrent publishers, readers, process crashes, and PostgreSQL failures.
6. Make execution TTL produce a durable `timeout` transition for every non-terminal state.
7. Keep the TypeScript and Rust implementations behaviorally identical.

## Non-Goals

- A generic per-task `maxEvents` setting.
- Event-type-specific denoising or compaction beyond existing series semantics.
- Changing the public meaning of `keep-all`, `latest`, or `accumulate` series.
- Treating Redis eviction policies as a storage lifecycle mechanism.
- Automatically deciding whether a product-level owner, agent, or worker is inactive. The embedding service owns that decision.

## Terminology

- **Hot task:** Task state required for mutation and the configured replay window are present in the short-term store.
- **Cold task:** PostgreSQL contains the authoritative task/history/series state, while Redis task-specific storage has been released.
- **Durable watermark:** Highest event index for which Taskcast has transactionally persisted all required durable state through that index.
- **Release:** The guarded hot-to-cold transition.
- **Rehydration:** Reconstructing the Redis mutation state for a cold task before a write.
- **Execution TTL:** A task lifecycle deadline that causes a durable `timeout` transition.
- **Hot retention:** A storage policy deciding when an inactive task may be released from Redis.

## Required Invariants

1. An event index is never reused for the same task.
2. Redis data is never deleted before PostgreSQL has acknowledged the release watermark.
3. A failed release leaves the task hot and writable; it never leaves a partial cold state.
4. Only one release or rehydration operation may mutate a task's storage state at a time.
5. A publisher cannot write between the release barrier and Redis deletion.
6. History reads never silently return a partial Redis prefix or suffix as complete history.
7. A cold task can accept a later write without index collision or lost series accumulation.
8. Execution expiry is represented in durable task state, not inferred from missing Redis keys.
9. A PostgreSQL outage increases Redis retention; it must not cause data loss.

## Storage Metadata

Add durable storage metadata associated with each task. The exact migration may use columns on `taskcast_tasks` or a one-to-one metadata table, but both implementations must expose the same semantics:

| Field | Meaning |
|---|---|
| `storage_state` | `hot`, `releasing`, or `cold` |
| `storage_epoch` | Monotonic fencing generation used to reject writes racing release/rehydration |
| `active_release_generation` | Unique release/rehydration generation allowed to mutate storage state, nullable |
| `archive_watermark` | Highest event index covered by the last atomic archive barrier; `-1` means no events |
| `last_event_at` | Durable timestamp of the most recent event |
| `cold_at` | Timestamp of successful Redis release, nullable |
| `execution_deadline_at` | Absolute lifecycle deadline derived from task TTL, nullable |

`storage_state = releasing` is recoverable coordination state, not proof that Redis has been deleted. Recovery always checks the durable watermark and Redis presence before choosing whether to finish the release or revert to `hot`.

Add indexes for `(storage_state, last_event_at)` retention scans and a partial index on `execution_deadline_at` for non-terminal tasks. Migration queries must avoid full scans of the production event table.

## Long-Term Store Contract

The existing fire-and-forget `saveEvent` path is insufficient as a deletion barrier. Extend the core long-term store contract with explicit archive operations:

- `describeArchiveSource(taskId, watermark)` performs a bounded pass over the fenced short-term source and returns a manifest: prior durable watermark, target watermark, source-entry count, ordered source-index/ID digest, series-state digest, and expected batch ordinals.
- `beginArchive(taskId, watermark, storageEpoch, releaseGeneration, manifest)` opens or resumes an idempotent archive generation.
- `archiveBatch(generation, ordinal, previousBatchDigest, events)` durably upserts a bounded batch, records its coverage/digest receipt, and rejects conflicts.
- `finalizeArchive(generation, task, seriesLatest)` verifies a contiguous ordinal/digest chain against the source manifest, then transactionally writes final task/series state and advances the committed watermark.
- `getArchiveWatermark(taskId)` returns the committed watermark.
- `getTaskStorageMetadata(taskId)` returns storage state and timestamps.
- `setTaskStorageState(...)` participates in the release/rehydration protocol.
- `getLastEventIndex(taskId)` returns the durable maximum event index.

The archive barrier is idempotent and streaming. It must not materialize a 600,000-event task as a second in-memory array. A crashed generation has no committed watermark and may be resumed or replayed safely; a lost response after finalization is resolved by reading back the watermark. Existing event IDs/indexes must match on replay; conflicting content is an integrity error, not an overwrite.

The manifest is sealed from the fenced source before batch upload. Finalization rejects missing/duplicate ordinals, a broken digest chain, mismatched entry count, altered source index/ID coverage, or a series-state digest mismatch. This provides completeness proof even though compacted series make PostgreSQL row count intentionally different from source event count.

Series modes require special care:

- `keep-all` events are persisted individually.
- `latest` and `accumulate` may already be compacted in PostgreSQL. Their current durable event plus series-latest state must represent the watermark correctly.
- Release validation compares the committed archive watermark, not PostgreSQL row count, because compacted series intentionally produce sparse durable rows.

The implementation reuses the existing archive/export normalization rules through a bounded iterator. Release uses an in-process API rather than an HTTP export followed by import. PostgreSQL batches may commit incrementally, but the archive watermark advances only in the final transaction, so incomplete generations are never eligible for Redis deletion.

## Storage Lock

Use a per-task distributed lock in the short-term store for release and rehydration. Redis implementations use a tokenized lock with expiry and compare-and-delete renewal semantics; memory and SQLite implementations provide equivalent local serialization.

Every long-running storage operation also has a unique `releaseGeneration` fencing token recorded in both Redis and PostgreSQL. Lock renewal failure permanently invalidates that executor: it must stop before any further batch, deletion, reopen, or state change. All such mutations compare-and-set the current lock token, release generation, storage epoch, and expected state. Recovery acquires a new lock and generation, invalidating the old generation before it may reopen writes or finish deletion. A stale executor can therefore upload harmless uncommitted batches but can never finalize, delete Redis, reopen a fence, or mark a task cold.

The lock alone is not sufficient because taking it on every event would add avoidable contention, while checking storage state before a write leaves a time-of-check/time-of-use race. Each hot task therefore also has an atomic Redis write fence containing `acceptingWrites` and `storageEpoch`.

- Normal short-term mutation scripts assert `acceptingWrites = true` and the epoch observed by `ensureTaskHotForWrite` in the same atomic operation that writes the event/task state.
- Release atomically changes `acceptingWrites` to false before reading the Redis high watermark.
- A mutation that races the fence is rejected as retryable and runs `ensureTaskHotForWrite` again; it never writes into storage being deleted.
- Rehydration increments the epoch and atomically installs the restored state with `acceptingWrites = true`, so a publisher holding an old epoch cannot write after restoration.

Normal publish/status operations call an `ensureTaskHotForWrite` gate. The gate:

1. Reads durable storage state and the current hot-storage epoch.
2. Waits or returns a retryable conflict while another owner holds `releasing`.
3. Rehydrates when state is `cold`.
4. Proceeds when state is `hot`, passing the observed epoch into the atomic short-term mutation.

The storage lock is not held during arbitrary client work. It covers only Taskcast's barrier, Redis deletion, or rehydration transaction.

## Hot-to-Cold Release Protocol

Expose an idempotent administrative engine operation and server route:

`POST /tasks/:taskId/storage/release`

The route requires `task:manage`. It accepts preconditions supplied by the embedding service:

```json
{
  "expectedLastEventIndex": 636965,
  "inactiveSince": "2026-07-16T00:00:00.000Z"
}
```

The embedding service is responsible for first proving that it has released its session/worker ownership. Taskcast independently prevents concurrent writes through the storage lock and the expected-index check. `inactiveSince` is a cutoff precondition: release is rejected if durable `last_event_at` is later than that timestamp.

Release proceeds as follows:

1. Acquire the per-task storage lock and generate a unique release generation.
2. Atomically close the Redis write fence, install that generation, and capture the task's epoch and current high watermark. Writes that lost this race retry rather than entering Redis.
3. Reject with a retryable conflict and CAS-reopen the same generation if the expected index or `inactiveSince` precondition no longer matches.
4. CAS durable storage state to `releasing` with the fenced epoch and release generation.
5. Seal the bounded source manifest, then stream the canonical archive through the observed watermark in numbered, digest-chained batches.
6. Verify the complete manifest and finalize the synchronous PostgreSQL archive barrier for the same release generation.
7. Read back and verify `archive_watermark >= observed watermark` and the active release generation still matches.
8. In one Redis script, verify the current lock token, closed fence, epoch, and release generation, then delete all task-specific short-term keys including task, events, next-index, write-fence, and series keys. Broadcast/pub-sub infrastructure is not deleted.
9. CAS durable storage state from `releasing` to `cold` for that generation and set `cold_at`.
10. Release the lock and return the committed watermark.

If steps 5–8 fail while the executor still owns the lock/generation, Taskcast retains Redis data, CAS-reopens the fence with a new storage epoch, restores durable state to `hot`, emits a failure metric, and returns an error. After lock renewal loss, the executor performs no cleanup mutation and leaves recovery to the next generation. If the process dies after closing the fence, recovery first invalidates the old generation; it then either reopens with a new epoch or completes release from the verified watermark. If the process dies after Redis deletion but before step 9, recovery sees matching `releasing` metadata, Redis absence, and a valid watermark and finalizes `cold` under its new recovery generation.

Repeated release of an already cold task succeeds without changing the watermark.

## Rehydration Before Mutation

A write to a cold task must not recreate an empty Redis index. `ensureTaskHotForWrite` performs:

1. Acquire the per-task storage lock and install a unique rehydration generation, invalidating any stale storage executor.
2. Load the durable task, archive watermark, maximum event index, and series-latest entries from PostgreSQL.
3. Load a bounded recent replay window from PostgreSQL. This is a rehydration/SSE optimization, not a history-retention cap. The default is configurable and initially 1,000 events.
4. Increment `storage_epoch`, then atomically restore Redis task state, recent events, series-latest values, `nextIndex = durableMaxIndex + 1`, and an open write fence for the new epoch/generation.
5. CAS durable storage state to `hot` with the new epoch, clear `cold_at`, and clear the active generation.
6. Release the lock, then retry the original mutation through the normal optimistic-concurrency path.

Reads do not rehydrate a task. Rehydration is reserved for writes so that historical access cannot recreate Redis growth.

The short-term event representation must record that the hot list is a replay window rather than complete history. Engine code must never infer completeness merely from a non-empty Redis list.

## History and SSE Read Semantics

### REST history

When a long-term store is configured, `/events/history` always reads PostgreSQL as the authoritative baseline for all tasks, hot or cold. For a hot task it also reads the Redis replay/tail window and performs a canonical merge by event identity/index using the same normalization rules as archive export. This overlay provides read-after-write visibility while asynchronous long-term writes are still settling; it is never treated as complete history and therefore cannot hide older PostgreSQL rows.

The merge must handle `latest` and `accumulate` representations explicitly rather than concatenating arrays. It de-duplicates already-durable events, applies the requested `seriesFormat` only after the canonical history is assembled, and fails with an integrity error on conflicting identity/index content. A cold task needs no overlay because the release barrier proves PostgreSQL complete through the cold watermark.

Without a long-term store, existing short-term behavior remains unchanged.

### SSE replay

- Every non-terminal SSE path subscribes to live broadcast first, captures the subscription boundary, then obtains the canonical PostgreSQL/Redis snapshot and de-duplicates buffered/live events by global event index. A cursor wholly inside the proven hot window may use Redis for the snapshot, but it uses the same subscribe-before-snapshot protocol.
- A cold task replays PostgreSQL history according to existing collapse/filter/cursor semantics, then closes if terminal.
- A cold non-terminal task follows the same protocol, so an event published during rehydration is not missed.
- `since` cursors remain global task event indexes and therefore survive release/rehydration.

Series collapse must produce the same result from hot and cold paths. Parity tests compare complete SSE frames, not only event counts.

## Execution TTL

The existing `ttl` input retains its lifecycle meaning but is no longer implemented only as Redis expiry.

On task creation, Taskcast stores an absolute `execution_deadline_at`. PostgreSQL is the authoritative production scheduler source. Replicas select overdue non-terminal rows using PostgreSQL `CURRENT_TIMESTAMP` and short `FOR UPDATE SKIP LOCKED` batches, then persist a unique `ttl_claim_token` and database-time `ttl_claim_until`. Work occurs outside the scan transaction. A replica may act only while its claim token remains current; an expired claim is available to another replica.

Before terminalization, the claimant ensures a cold task is hot, then closes its write fence under a TTL generation and reserves the next event index. A PostgreSQL transaction compare-and-sets the non-terminal task version/status, writes the `timeout` event at that reserved index, clears the durable assignment, records an idempotent worker-capacity release/outbox item keyed by assignment ID, and clears the TTL claim. The outbox projects terminal state/event to Redis, broadcasts it, and releases worker capacity with idempotent scripts. If the process crashes after the PostgreSQL commit, another dispatcher completes those projections; if PostgreSQL fails before commit, durable state remains non-terminal and the claim is retried after expiry.

If the task-version CAS loses to completion/cancellation, the claimant discards the reserved timeout event, clears its claim, and projects the winning terminal state without reopening writes. If it loses to another non-terminal mutation, it CAS-reopens the fence at a new storage epoch and lets the deadline be claimed again. Every cleanup path is fenced by the TTL generation, so an expired claimant cannot reopen or project over its successor.

The state machine is updated so every non-terminal state (`pending`, `assigned`, `running`, `paused`, and `blocked`) can transition to `timeout`.

For assigned/running tasks, the timeout operation also clears the durable assignment/lease and releases worker capacity exactly once. Transition and assignment cleanup use an idempotent terminalization operation; a repair sweeper reconciles a terminal task that still has an assignment after a crash. Timeout is not considered operationally complete merely because the task status changed while the worker slot remained occupied.

The scheduler protocol must be:

- durable across restarts;
- safe with multiple server replicas through PostgreSQL claim-token and task-version CAS semantics;
- idempotent when racing completion/cancellation;
- observable through success, race-lost, and failure counters.

After the timeout transition and event have been durably archived, hot retention policy may release the task separately. Redis key expiry may remain as a cleanup backstop only after the durable timeout, never as the source of truth.

## Automatic Retention and Backfill

The explicit release endpoint is the correctness primitive. An optional sweeper may call it for eligible tasks.

Eligibility is deliberately conservative:

- terminal tasks after a configurable hot grace period; or
- non-terminal tasks whose embedding service has explicitly marked ownership released and whose last event is older than a configured idle threshold.

Taskcast does not infer product ownership from silence alone. Team9/agent-pi must revoke or release the session owner before requesting release of an idle pending session.

Historical backfill runs in small, resumable batches ordered by oldest `last_event_at`. Each candidate is rechecked immediately before release. The process pauses on elevated archive failures, Redis latency, or PostgreSQL replication/IO pressure.

## Configuration

Proposed settings, with identical environment/config-file behavior in TypeScript and Rust:

- `TASKCAST_HOT_RETENTION_ENABLED=false` initially
- `TASKCAST_HOT_RETENTION_TERMINAL_SECONDS`
- `TASKCAST_HOT_RETENTION_IDLE_SECONDS`
- `TASKCAST_REHYDRATE_REPLAY_EVENTS=1000`
- `TASKCAST_STORAGE_LOCK_TTL_SECONDS`
- `TASKCAST_TTL_SWEEP_INTERVAL_SECONDS`
- `TASKCAST_TTL_SWEEP_BATCH_SIZE`

Automatic retention remains off until the explicit release protocol, metrics, and production migration are verified.

## Observability

Emit structured logs and metrics for:

- hot, releasing, and cold task counts;
- Redis bytes/events released per task;
- archive barrier duration and failure reason;
- release precondition conflicts;
- rehydration count, duration, and replay size;
- durable/Redis watermark mismatch;
- history source and latency;
- overdue execution TTL count and timeout transition outcomes;
- tasks with unusually high Redis event count or age.

Logs include task ID and watermarks but never event payloads by default.

## Migration and Deployment

Production currently has `TASKCAST_AUTO_MIGRATE=FALSE`, so the PostgreSQL migration is a required manual deployment step.

Rollout order:

1. Deploy the PostgreSQL schema migration manually and verify columns/indexes.
2. Deploy TypeScript and Rust builds with metadata reads/writes and TTL scheduler/release endpoint disabled.
3. Drain every old writer replica or verify through a protocol-readiness endpoint/metric that every live writer enforces `acceptingWrites`, `storageEpoch`, and release-generation CAS. Any legacy writer keeps release disabled.
4. Enable PostgreSQL-authoritative history and verify hot/cold parity on sampled tasks.
5. Enable the explicit release endpoint for operators only.
6. Release the incident task after evidence archival and validate Redis/PostgreSQL watermarks.
7. Run a small historical batch and observe memory, DB load, and rehydration behavior.
8. Gradually enable automatic terminal-task release.
9. Enable non-terminal idle release only after the embedding service supplies reliable owner-release state.
10. Enable the TTL scheduler separately after its claim/outbox workers report ready.

No release may be attempted before the schema migration and archive barrier are active or while any writer without fence support remains live.

## Failure Handling

| Failure | Required behavior |
|---|---|
| PostgreSQL unavailable during release | Keep Redis data and task `hot`; retry later |
| Process crash while `releasing` | Recover by inspecting watermark and Redis presence |
| Release lock expires and stale executor resumes | Generation/lock CAS rejects every stale finalize/delete/reopen/state mutation |
| Publisher races release | Lock/precondition forces one side to retry; no event loss |
| Redis deletion partially fails | Atomic adapter operation or fail closed; do not mark `cold` |
| Rehydration fails | Leave durable state `cold`; reject mutation as retryable |
| Durable event conflict | Integrity alert; retain Redis and stop release |
| TTL races terminal transition | One optimistic transition wins; loser is harmless |

## Test Strategy

Every behavior is implemented and tested in both server stacks.

### Core/unit tests

- PostgreSQL history is selected even when Redis contains a non-empty partial window.
- Release is idempotent and preserves its watermark.
- Finalization rejects an omitted middle archive batch, reordered/duplicate ordinals, and manifest digest mismatch.
- PostgreSQL barrier failure prevents Redis deletion.
- Concurrent publish versus release yields a complete monotonic history.
- A publisher that passed the preflight gate before release is fenced at its atomic write and retries safely.
- Cold-task publish restores `nextIndex` and cannot reuse index zero.
- `latest` and `accumulate` series produce identical hot/cold results.
- Reads do not rehydrate.
- All non-terminal states can timeout; terminal races remain single-winner.
- Assigned/running timeout releases the assignment and worker capacity exactly once, including crash repair.
- Two replicas claiming one deadline produce one timeout event; restart-overdue, PostgreSQL failure, completion race, expired claim, and post-commit projection crash cases converge correctly.

### Adapter/integration tests

- Redis release deletes every task-specific key atomically.
- Tokenized locks expire safely and cannot be released by another holder.
- Recovery can reopen and accept a publish after an expired release lock; the original executor's later delete/finalize attempts are fenced out.
- PostgreSQL archive barrier is transactional and detects conflicts.
- Crash recovery covers both sides of the Redis-deletion boundary.
- Large-task release uses bounded memory and does not block unrelated tasks.

### HTTP/SSE parity tests

- TypeScript and Rust return identical history, cursors, status codes, and release responses.
- Cold and hot SSE replay have identical filter/series behavior.
- Subscribers racing a hot snapshot publish or cold rehydration neither miss nor duplicate events.
- Unauthorized release is rejected.

### Production regression fixture

Create a reduced fixture modeled on `01KRK8Y78MA3SV416YNAV3E3KJ`: many retry-cycle events, pending state, no execution TTL, multiple series, and a later write after release. Assert durable history completeness, Redis release, and safe rehydration.

## Acceptance Criteria

- Releasing a task is impossible unless PostgreSQL has acknowledged its archive watermark.
- Redis memory falls after release while full history remains queryable.
- A later write to the released task continues at the next global event index.
- History never changes merely because a task is hot or cold.
- TTL reliably produces a durable timeout instead of only missing Redis keys.
- TypeScript and Rust parity suites pass.
- No `maxEvents` or event-type denoising is introduced in this change.

## Deferred Decision: `maxEvents`

Revisit a per-task event cap only after hot/cold release metrics answer:

- hot replay sizes actually needed by SSE clients;
- rehydration frequency and cost;
- distribution by task type rather than global percentiles;
- whether any workload truly needs a hard safety ceiling in addition to cold release.

Any future cap must define overflow behavior explicitly—reject, compact, spill, or terminate—and must never silently truncate authoritative history.
