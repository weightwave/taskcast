# Terminal history retention

Permanent cleanup is opt-in (`cleanup.enabled: false` by default). It requires Redis and PostgreSQL adapters that support protocol 3, including every live writer. Memory adapters support deterministic tests; SQLite and incomplete custom adapters reject enabled cleanup at startup.

Redis hot retention is separate: it archives and releases Redis storage without deleting durable history. Terminal history retention deletes PostgreSQL history, or the whole task. Disabling cleanup stops future cleanup ticks; it does not restore deleted history.

## Policies

An enabled server snapshots the matched global `cleanup.rules` when a task is created. A task's `cleanup.rules` replaces the entire global policy; `rules: []` opts out. Changing defaults never changes existing task snapshots. Existing tasks, legacy cleanup JSON and imported archives are not automatically enrolled.

Only `completed`, `failed`, `cancelled` and `timeout` can become eligible. Deadlines are relative to `completedAt`, not task creation or the latest event. Tasks without a valid completion timestamp are skipped. `pending`, `assigned`, `running`, `paused` and `blocked` are never deleted.

- `target: events`: delete the complete history, retaining task status, result, error and metadata.
- `target: all`: delete history and the task record. When both match, the earliest events deadline applies and all is the final task lifetime cap.
- `trigger.afterMs`: non-negative milliseconds from completion; omitted means immediately eligible.
- `match.taskTypes` and `match.status`: optional filters. The initial version rejects `eventFilter` and `target: task` when cleanup is enabled.

Example for **WonderAgent only**, not a Taskcast default:

```yaml
cleanup:
  enabled: true
  rules:
    - name: search-success-events
      match:
        taskTypes: [influagent.search.youtube]
        status: [completed, cancelled]
      trigger: { afterMs: 86400000 }
      target: events
    - name: search-failure-events
      match:
        taskTypes: [influagent.search.youtube]
        status: [failed]
      trigger: { afterMs: 604800000 }
      target: events
```

This keeps completed/cancelled history for 24 hours and failed history for 7 days. Timeout history and task records are retained. `TASKCAST_CLEANUP_ENABLED=true|false` overrides the file switch; other values fail startup.

## Clients and archives

Once deletion starts, `GET /tasks/:id` includes optional `historyExpiredAt` (Unix milliseconds). History returns `[]` with `X-Taskcast-History-Expired: true`, exposed by configured CORS. Even while physical deletion is incomplete, no residual history is replayed.

SSE sends `taskcast.history_expired` with `{taskId, expiredAt}`, then `taskcast.done` with the original terminal reason. Old cursors do not bypass this signal. The browser client optionally accepts `onHistoryExpired(info)`; existing consumers can ignore the new event. Global SSE remains a live subscription for newly created tasks; it does not replay historical tasks.

Exporting expired history returns HTTP 409 with `code: TASKCAST_HISTORY_EXPIRED`. A fully deleted task returns 404. Normal archive import still conflicts with an existing task. Explicit `overwrite: true` requires the existing task-management permission and valid archive. It restores a fresh creation generation under the storage lease, rejecting writes captured from the previous generation. Server cleanup markers are stripped; the archive's cleanup JSON is preserved as legacy configuration without automatic enrollment.

## Operation and rollout

The lifecycle worker checks every 5 seconds by default, claims up to 100 tasks per tick, and deletes up to 1,000 event rows per task per tick. Existing `storageLifecycle.ttlSweepIntervalSeconds` and `ttlSweepBatchSize` tune the interval and task bound. A busy or failed task is deferred so later tasks can progress. Durable claims resume cold tasks after restart.

Cleanup first resolves archival/release state and releases Redis, then checks the task generation and completion again. Unsettled assignments, terminal projections and unfinished archive uploads defer cleanup. The logical expiry marker commits before bounded physical deletion. Related series bodies and obsolete archive receipts are cleaned too; the highest historical event index remains on an events-only task. Structured logs contain aggregate counts and identifiers, not event payloads.

1. Back up PostgreSQL and verify that required business results/evidence live in the business system or a retained task record.
2. Apply the additive migrations, including `006_terminal_retention.sql`.
3. Upgrade all API/worker/embedded writers to storage protocol 3. Older live writers block permanent cleanup; ordinary Redis release still supports protocol 2.
4. Validate a new disposable task in a non-production environment, including expiry, restart and archive restore. Review policies by task type before enabling the switch.
5. Enable only the approved environment and watch lifecycle summaries. CI success is not evidence that a deployed instance has enabled cleanup.

Legacy adoption is separate operational work: produce a read-only inventory by task type, status and age; check business references and recovery requirements; obtain a backup and an explicit task allowlist before any enrollment. This release provides no automatic adopt-all switch. Migration does not delete or enroll existing rows. A policy change or this implementation alone does not authorize production cleanup.
