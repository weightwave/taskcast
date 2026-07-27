# Hot and Cold Storage

[中文](hot-cold-storage.zh.md)

Taskcast can use Redis as a bounded active-task and replay tier while PostgreSQL
remains the durable source of truth. Hot storage is released only after a
fenced archive protocol proves that PostgreSQL covers the complete Redis event
range. A later mutation rehydrates only a bounded replay window; history reads
continue to use canonical durable history.

## Why the Previous Behavior Grew Without Bound

The former three-layer write path did not implement storage tiering:

- Redis was called a short-term store, but task and event keys had no safe
  lifecycle that released them after PostgreSQL caught up.
- PostgreSQL received long-term writes, but an asynchronous dual write was not
  a deletion barrier. There was no durable watermark or archive receipt proving
  that every Redis event was present.
- History preferred Redis whenever Redis returned any events. A partial Redis
  prefix or replay tail could therefore hide older, complete PostgreSQL
  history.
- Task `ttl` expired Redis keys. It did not durably transition the task to
  `timeout`, emit exactly one timeout event, or settle worker ownership.

Low request load therefore did not imply low Redis memory: a pending task that
kept receiving retries could retain an arbitrarily large event list.

## Storage Model

With Redis and a PostgreSQL adapter that advertises the hot/cold protocol:

- PostgreSQL is canonical for task history and durable lifecycle metadata.
- Redis stores active task state, the current write fence, series state, and a
  bounded replay window.
- Reads never rehydrate a cold task. They assemble history from PostgreSQL and
  any proven hot tail.
- A mutation of a cold task obtains a storage lease and restores the task,
  durable series state, next global index, and at most
  `rehydrateReplayEvents` recent events before committing the write.
- Event indexes remain monotonic across release and rehydration.

The protocol uses a per-task lease, storage epoch, closed write fence, archive
generation, batch receipts, manifest, and durable archive watermark. Redis
task keys are deleted only after the durable watermark equals the fenced Redis
high watermark. A failed or ambiguous archive leaves Redis intact and is
retried or recovered.

## Releasing Hot Storage

Release is explicit:

```http
POST /tasks/{taskId}/storage/release
Content-Type: application/json

{
  "expectedLastEventIndex": 1542,
  "inactiveSince": 1785168000000
}
```

Both values are integer preconditions, and `inactiveSince` is Unix time in
milliseconds. Read them from an authoritative task/history snapshot; do not
guess. The caller needs `task:manage`.

A successful response is:

```json
{
  "taskId": "01...",
  "storageState": "cold",
  "archiveWatermark": 1542,
  "released": true
}
```

Calling the route again for an already-cold task is idempotent and returns
`released: false`.

Important failures:

- `409 storage_precondition_failed`: the task received newer activity or its
  last index changed. Refresh the task/history snapshot and decide again.
- `409 storage_busy`: another release, rehydration, or conflicting writer owns
  the lifecycle fence. Retry with bounded backoff.
- `500 storage_integrity_error`: the source, manifest, receipt coverage, or
  watermark is inconsistent. Stop automated release and investigate.
- `503 storage_release_unsupported`: the configured adapters do not implement
  the protocol.
- `503 storage_unavailable`: PostgreSQL, Redis, or writer readiness is
  unavailable. Redis is retained; fix readiness and allow the persisted request
  to retry.

Only terminal tasks are eligible for automatic retention. Non-terminal tasks
must be released explicitly by the owning service after it has released
session/task ownership. Silence alone is not proof that a pending or running
task is safe to release.

## Configuration

The same settings are supported by the TypeScript and Rust servers. Environment
variables override `storageLifecycle` values in the config file.

| Environment variable | Config key | Default | Meaning |
| --- | --- | ---: | --- |
| `TASKCAST_HOT_RETENTION_ENABLED` | `hotRetentionEnabled` | `false` | Automatically release eligible terminal tasks only. |
| `TASKCAST_HOT_RETENTION_TERMINAL_SECONDS` | `hotRetentionTerminalSeconds` | `86400` | Terminal grace period before automatic release. |
| `TASKCAST_HOT_RETENTION_IDLE_SECONDS` | `hotRetentionIdleSeconds` | `3600` | Minimum age of a persisted release cutoff before worker retry. |
| `TASKCAST_REHYDRATE_REPLAY_EVENTS` | `rehydrateReplayEvents` | `1000` | Recent events restored to Redis before a later mutation. |
| `TASKCAST_STORAGE_LOCK_TTL_SECONDS` | `storageLockTtlSeconds` | `30` | Storage lease and claim duration. |
| `TASKCAST_TTL_SWEEP_INTERVAL_SECONDS` | `ttlSweepIntervalSeconds` | `5` | Lifecycle worker interval. |
| `TASKCAST_TTL_SWEEP_BATCH_SIZE` | `ttlSweepBatchSize` | `100` | Maximum tasks claimed by each sweep. |

Values other than `TASKCAST_HOT_RETENTION_ENABLED` must be positive integers.
The servers reject invalid values and second-to-millisecond overflow at
startup. A YAML example:

```yaml
storageLifecycle:
  hotRetentionEnabled: false
  hotRetentionTerminalSeconds: 86400
  hotRetentionIdleSeconds: 3600
  rehydrateReplayEvents: 1000
  storageLockTtlSeconds: 30
  ttlSweepIntervalSeconds: 5
  ttlSweepBatchSize: 100
```

Durable TTL sweeping starts only when the long-term adapter advertises durable
TTL support. Automatic retention remains off unless explicitly enabled.

## TTL and Recovery Workers

The lifecycle worker independently performs bounded:

- durable TTL claims and terminalization;
- terminal projection repair;
- incomplete release recovery;
- persisted release-request retry; and
- optional terminal retention.

TTL and release paths have separate exponential backoff. A PostgreSQL outage
does not trigger Redis deletion. The worker emits payload-free structured JSON:

- `storage_lifecycle_tick` contains duration and counters for TTL,
  projections, release requests, and retention;
- `storage_lifecycle_error` contains the failed operation, error, and a task ID
  when one is safely available.

Use `/health/detail` before enabling release. `storage.releaseReady` must be
true, `requiredStorageProtocolVersion` must be `2`, and
`incompatibleWriterIds` must be empty.

## Event Volume Is Not Capped

This change does not add `maxEvents`, event-type denoising, or silent
truncation. The `limit` history query parameter limits one response; it is not a
retention policy. `TASKCAST_REHYDRATE_REPLAY_EVENTS` bounds only the Redis
replay cache restored on mutation. Complete canonical history remains in
PostgreSQL.

For production migration and rollback gates, follow the
[hot/cold rollout runbook](../runbooks/2026-07-16-production-hot-cold-rollout.md).
