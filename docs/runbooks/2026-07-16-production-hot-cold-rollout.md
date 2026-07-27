# Production Hot/Cold Storage Rollout

This runbook rolls out the protocol without ever deleting unverified Redis
data. Replace the example base URL, token, database URL, Redis URL, and prefix
through the deployment secret mechanism; do not paste credentials into logs or
shell history.

The incident task used by this rollout is:

```text
01KRK8Y78MA3SV416YNAV3E3KJ
```

## Stop Conditions

Pause before any new release if one of these is true:

- PostgreSQL backup or task archive verification fails.
- A migration is dirty, missing, or has a checksum mismatch.
- `/health/detail` reports `releaseReady: false`, a protocol version other than
  `2`, or any `incompatibleWriterIds`.
- PostgreSQL history differs from the Redis/hot sample.
- `storage_lifecycle_error`, archive failure, watermark mismatch, Redis latency,
  PostgreSQL latency, connection saturation, replication lag, or memory
  pressure rises materially.
- A release returns `500`, or repeated `409`/`503` cannot be explained.

Disabling retention is always safe. Deleting Redis keys manually is not a
rollback procedure.

## 1. Back Up and Verify

Export the target task before changing its status:

```bash
export TASKCAST_BASE_URL='https://taskcast.example'
export TASK_ID='01KRK8Y78MA3SV416YNAV3E3KJ'

curl --fail --silent --show-error \
  -H "Authorization: Bearer $TASKCAST_TOKEN" \
  "$TASKCAST_BASE_URL/tasks/$TASK_ID/archive" \
  > "$TASK_ID.before-release.json"
shasum -a 256 "$TASK_ID.before-release.json"
jq -e \
  --arg id "$TASK_ID" \
  '.schema == "taskcast.taskArchive" and .task.id == $id' \
  "$TASK_ID.before-release.json"
```

Take and verify a PostgreSQL backup according to the database recovery policy.
For a direct custom-format backup:

```bash
pg_dump --format=custom --file=taskcast-before-hot-cold.dump \
  "$TASKCAST_POSTGRES_URL"
pg_restore --list taskcast-before-hot-cold.dump >/dev/null
```

Record row counts and the incident watermark:

```bash
psql "$TASKCAST_POSTGRES_URL" -v ON_ERROR_STOP=1 -P pager=off <<'SQL'
SELECT id, status, updated_at
FROM taskcast_tasks
WHERE id = '01KRK8Y78MA3SV416YNAV3E3KJ';

SELECT COUNT(*) AS event_count,
       COALESCE(MIN(idx), -1) AS first_index,
       COALESCE(MAX(idx), -1) AS last_index,
       COALESCE(MAX(timestamp), 0) AS last_event_at
FROM taskcast_events
WHERE task_id = '01KRK8Y78MA3SV416YNAV3E3KJ';
SQL
```

Do not continue if the archive and database identity/index range disagree.

## 2. Apply Migrations Manually

Production keeps `TASKCAST_AUTO_MIGRATE=false`. Use one migration job, before
starting new application replicas:

```bash
TASKCAST_AUTO_MIGRATE=false \
TASKCAST_POSTGRES_URL="$TASKCAST_POSTGRES_URL" \
npx @taskcast/cli migrate --yes
```

Migration `003_storage_lifecycle.sql` is the required lifecycle migration.
This release also applies forward migrations `004_archive_receipt_coverage.sql`
and `005_task_creation_claim.sql`; they must be recorded in order in
`_sqlx_migrations`.

```bash
psql "$TASKCAST_POSTGRES_URL" -v ON_ERROR_STOP=1 -P pager=off <<'SQL'
SELECT version, description, success
FROM _sqlx_migrations
WHERE version BETWEEN 3 AND 5
ORDER BY version;

SELECT column_name, data_type, is_nullable
FROM information_schema.columns
WHERE table_schema = 'public'
  AND table_name = 'taskcast_tasks'
  AND column_name IN (
    'storage_state', 'storage_epoch', 'active_release_generation',
    'archive_watermark', 'last_event_at', 'cold_at',
    'execution_deadline_at', 'task_version', 'ttl_claim_token',
    'ttl_claim_until', 'release_requested_at',
    'release_expected_index', 'release_inactive_since',
    'creation_token', 'creation_claimed_at',
    'creation_claim_expires_at', 'creation_completed_at'
  )
ORDER BY column_name;

SELECT indexname
FROM pg_indexes
WHERE schemaname = 'public'
  AND indexname IN (
    'idx_taskcast_tasks_storage_activity',
    'idx_taskcast_tasks_due_execution_deadline',
    'idx_taskcast_tasks_release_requested',
    'idx_taskcast_archive_generations_incomplete',
    'idx_taskcast_durable_assignments_worker',
    'idx_taskcast_terminal_outbox_pending',
    'idx_taskcast_tasks_creation_token',
    'idx_taskcast_tasks_creation_claim_expiry'
  )
ORDER BY indexname;
SQL
```

Also verify these tables exist: `taskcast_archive_generations`,
`taskcast_archive_batches`, `taskcast_series_state`,
`taskcast_durable_assignments`, and `taskcast_terminal_outbox`.

## 3. Deploy New Readers and Writers With Release Disabled

Deploy all TypeScript and Rust replicas with:

```text
TASKCAST_AUTO_MIGRATE=false
TASKCAST_HOT_RETENTION_ENABLED=false
```

Keep operator release disabled at the gateway or operational policy. Do not
enable TTL sweeping as a rollout action yet; leave existing configured TTL
behavior unchanged until step 10.

Verify normal create, publish, status transition, history, and SSE paths. New
history readers use PostgreSQL as canonical history and merge a proven hot tail;
they do not rehydrate on reads.

## 4. Drain Old Writers and Verify Protocol 2

Drain every writer running code without storage protocol 2. Wait at least the
writer-registration TTL so stale heartbeats expire, then check every new
replica:

```bash
curl --fail --silent --show-error \
  "$TASKCAST_BASE_URL/health/detail" |
  jq '.storage'
```

The gate is:

```json
{
  "releaseReady": true,
  "requiredStorageProtocolVersion": 2,
  "incompatibleWriterIds": []
}
```

`activeWriterCount` must match the expected live replica count. Read-only Redis
verification, using the configured prefix (default `taskcast`):

```bash
export TASKCAST_REDIS_PREFIX="${TASKCAST_REDIS_PREFIX:-taskcast}"
redis-cli -u "$TASKCAST_REDIS_URL" --raw \
  SMEMBERS "$TASKCAST_REDIS_PREFIX:storageWriters"
redis-cli -u "$TASKCAST_REDIS_URL" --scan \
  --pattern "$TASKCAST_REDIS_PREFIX:storageWriter:*"
```

Inspect registration JSON if needed. Do not delete registrations or task keys.

## 5. Verify PostgreSQL-Canonical History

For a sample of small hot tasks and the incident task, compare:

- API history count and index range;
- PostgreSQL `taskcast_events` count and range;
- series output in both `seriesFormat=delta` and
  `seriesFormat=accumulated`; and
- an SSE reconnect with a `since.index` cursor.

The API must include PostgreSQL history even when Redis contains only a partial
tail. Reads must not create missing Redis task/event keys.

```bash
curl --fail --silent --show-error \
  -H "Authorization: Bearer $TASKCAST_TOKEN" \
  "$TASKCAST_BASE_URL/tasks/$TASK_ID/events/history?seriesFormat=delta" \
  > "$TASK_ID.history.json"

jq '{count:length, first:(first.index // -1), last:(last.index // -1)}' \
  "$TASK_ID.history.json"
```

Pause if history is sparse, truncated, duplicated, or differs by payload/index.

## 6. Enable Operator Release Only

Allow `POST /tasks/{taskId}/storage/release` only for the operations identity
with `task:manage`. Keep `TASKCAST_HOT_RETENTION_ENABLED=false`.

Canary a terminal, low-volume task. Obtain `expectedLastEventIndex` and
`inactiveSince` from PostgreSQL immediately before the request:

```bash
read -r LAST_INDEX LAST_EVENT_AT <<EOF
$(psql "$TASKCAST_POSTGRES_URL" -At -F ' ' -v id="$CANARY_TASK_ID" -c \
  "SELECT COALESCE(MAX(idx),-1), COALESCE(MAX(timestamp),0)
   FROM taskcast_events WHERE task_id = :'id'")
EOF

curl --fail --silent --show-error -X POST \
  -H "Authorization: Bearer $TASKCAST_TOKEN" \
  -H 'Content-Type: application/json' \
  "$TASKCAST_BASE_URL/tasks/$CANARY_TASK_ID/storage/release" \
  --data "{\"expectedLastEventIndex\":$LAST_INDEX,\"inactiveSince\":$LAST_EVENT_AT}"
```

After success, verify `storage_state='cold'`,
`archive_watermark=LAST_INDEX`, finalized receipt coverage, complete API
history, and absence of only that task's hot keys. A `409` requires a fresh
snapshot; do not loosen the precondition. A `503` requires readiness recovery;
do not delete Redis.

## 7. Resolve and Release the Incident Task

This step is allowed only after:

1. agent-pi has quarantined the poison messages or attack source; and
2. Team9 has released task/session ownership.

The allowlist belongs in Team9, not agent-pi. Taskcast does not decide whether
this non-terminal task has an owner.

Cancel the task through the normal API so the terminal state and status event
are durable:

```bash
curl --fail --silent --show-error -X PATCH \
  -H "Authorization: Bearer $TASKCAST_TOKEN" \
  -H 'Content-Type: application/json' \
  "$TASKCAST_BASE_URL/tasks/$TASK_ID/status" \
  --data '{"status":"cancelled","reason":"confirmed abusive task; owner released"}'
```

Re-export the archive and re-read the final event index after cancellation.
Then release using those exact values:

```bash
curl --fail --silent --show-error \
  -H "Authorization: Bearer $TASKCAST_TOKEN" \
  "$TASKCAST_BASE_URL/tasks/$TASK_ID/archive" \
  > "$TASK_ID.cancelled.json"
shasum -a 256 "$TASK_ID.cancelled.json"

read -r LAST_INDEX LAST_EVENT_AT <<EOF
$(psql "$TASKCAST_POSTGRES_URL" -At -F ' ' -v id="$TASK_ID" -c \
  "SELECT COALESCE(MAX(idx),-1), COALESCE(MAX(timestamp),0)
   FROM taskcast_events WHERE task_id = :'id'")
EOF

curl --fail --silent --show-error -X POST \
  -H "Authorization: Bearer $TASKCAST_TOKEN" \
  -H 'Content-Type: application/json' \
  "$TASKCAST_BASE_URL/tasks/$TASK_ID/storage/release" \
  --data "{\"expectedLastEventIndex\":$LAST_INDEX,\"inactiveSince\":$LAST_EVENT_AT}"
```

Verify PostgreSQL before inspecting Redis:

```bash
psql "$TASKCAST_POSTGRES_URL" -v ON_ERROR_STOP=1 -v id="$TASK_ID" \
  -v expected="$LAST_INDEX" -P pager=off <<'SQL'
SELECT id, status, storage_state, storage_epoch, archive_watermark,
       active_release_generation, cold_at, release_requested_at
FROM taskcast_tasks
WHERE id = :'id';

SELECT generation, target_watermark, status, finalized_at
FROM taskcast_archive_generations
WHERE task_id = :'id'
ORDER BY created_at DESC;

SELECT COUNT(*) AS event_count, COALESCE(MAX(idx), -1) AS last_index
FROM taskcast_events
WHERE task_id = :'id';
SQL
```

Expected: `cancelled`, `cold`, archive watermark equal to `LAST_INDEX`, a
finalized generation, complete events, no active generation, and no pending
release request.

Now perform read-only Redis checks:

```bash
for suffix in \
  "task:$TASK_ID" \
  "taskStatus:$TASK_ID" \
  "events:$TASK_ID" \
  "idx:$TASK_ID" \
  "seriesState:$TASK_ID" \
  "seriesListEntries:$TASK_ID" \
  "writeFence:$TASK_ID" \
  "hotWindow:$TASK_ID" \
  "assignment:$TASK_ID"
do
  redis-cli -u "$TASKCAST_REDIS_URL" EXISTS \
    "$TASKCAST_REDIS_PREFIX:$suffix"
done

redis-cli -u "$TASKCAST_REDIS_URL" --scan \
  --pattern "$TASKCAST_REDIS_PREFIX:series:$TASK_ID:*"
```

Expected: all task-local keys are absent. The task ID should also be absent from
`$TASKCAST_REDIS_PREFIX:tasks`. Do not use `DEL`, `UNLINK`, `FLUSHDB`, or broad
`SCAN | xargs` commands.

Finally, fetch API history again and compare its count, last index, and archive
hash semantics with the pre-release copy.

## 8. Release Small Oldest-First Batches

Select only terminal tasks, ordered by oldest `updated_at`, in a small bounded
batch. Recompute each task's exact preconditions immediately before release.
Start with one task, then a batch smaller than
`TASKCAST_TTL_SWEEP_BATCH_SIZE`.

Between batches, inspect:

- `storage_lifecycle_tick`, `storage_lifecycle_error`, `storage_release`,
  `storage_rehydrate`, `storage_history_read`, `storage_watermark_mismatch`,
  and `storage_hot_task`;
- Redis used memory, command latency, evictions, and blocked clients;
- PostgreSQL query latency, locks, connections, CPU, I/O, replication lag, and
  archive generation/receipt rows; and
- API history/SSE canaries.

Pause on any stop condition. Never compensate for a failed release by deleting
Redis manually.

## 9. Enable Terminal Retention

After several clean operator batches, set:

```text
TASKCAST_HOT_RETENTION_ENABLED=true
TASKCAST_HOT_RETENTION_TERMINAL_SECONDS=86400
```

Start with a long terminal grace and small sweep batch. Non-terminal release
remains explicit regardless of `TASKCAST_HOT_RETENTION_IDLE_SECONDS`; Team9 or
another owning service must prove ownership release and invoke the route.

## 10. Enable Durable TTL and Projection Sweeping

Treat TTL as a separate rollout. Confirm PostgreSQL owns
`execution_deadline_at`, all replicas use the new transition protocol, and
terminal projection repair is healthy. Then enable normal lifecycle worker
interval/batch settings:

```text
TASKCAST_TTL_SWEEP_INTERVAL_SECONDS=5
TASKCAST_TTL_SWEEP_BATCH_SIZE=100
TASKCAST_STORAGE_LOCK_TTL_SECONDS=30
```

Canary a short-TTL task. Verify exactly one durable `taskcast:status` timeout
event, one terminal task transition, and exactly-once worker assignment/capacity
settlement.

## Rollback and Recovery

For rising pressure or unexpected errors:

1. Set `TASKCAST_HOT_RETENTION_ENABLED=false`.
2. Remove operator access to the release route.
3. Keep protocol-2 readers/writers running if they are healthy; do not roll
   back to legacy history preference while tasks may be cold.
4. Preserve lifecycle workers when they are safely repairing a known
   `releasing` task. If PostgreSQL is unhealthy, stop new release attempts and
   restore PostgreSQL first; Redis remains the safety copy.
5. Inspect the persisted task metadata and archive generation before choosing
   recovery. Re-run the same guarded release only with freshly verified
   preconditions. A later legitimate write can rehydrate a verified cold task.
6. Restore from the verified PostgreSQL backup/task archive only through a
   reviewed recovery procedure.

Never:

- mark a task cold by hand;
- advance `archive_watermark` by hand;
- clear `active_release_generation` without reconciling the write fence and
  archive generation;
- delete Redis because PostgreSQL “probably” has the events; or
- downgrade all readers while any task has `storage_state='cold'`.

Useful recovery queries:

```sql
SELECT id, storage_state, storage_epoch, active_release_generation,
       archive_watermark, release_requested_at,
       release_expected_index, release_inactive_since
FROM taskcast_tasks
WHERE storage_state <> 'hot'
   OR release_requested_at IS NOT NULL
ORDER BY updated_at;

SELECT task_id, generation, storage_epoch, target_watermark, status,
       created_at, updated_at, finalized_at
FROM taskcast_archive_generations
WHERE status <> 'finalized'
ORDER BY updated_at;

SELECT COUNT(*) AS pending_terminal_projections
FROM taskcast_terminal_outbox
WHERE projected_at IS NULL;

SELECT COUNT(*) AS overdue_execution_deadlines
FROM taskcast_tasks
WHERE execution_deadline_at IS NOT NULL
  AND execution_deadline_at <=
      FLOOR(EXTRACT(EPOCH FROM clock_timestamp()) * 1000)::BIGINT
  AND status NOT IN ('completed', 'failed', 'timeout', 'cancelled');
```
