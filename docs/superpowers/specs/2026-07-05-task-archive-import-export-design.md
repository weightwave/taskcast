# Task Archive Import/Export and Version Handshake Design

Date: 2026-07-05

## Summary

Taskcast will add native import/export for a single task as a protocol-level archive, plus public server version reporting. Agent-pi and the Claw Hive Dashboard can then build session import/export on top of this instead of inventing their own event-history backup format.

The first implementation target is Taskcast itself. The downstream Claw Hive session bundle will embed a Taskcast task archive together with agent, session, component config, storage entries, LLM calls, and component snapshots.

## Goals

- Export a complete Taskcast task history as a portable `TaskArchive`.
- Import that archive while preserving the original task and event identity:
  - task id and timestamps
  - event ids
  - event indexes
  - event timestamps
  - series metadata and accumulated state
- Avoid `createTask` and `publishEvent` for archive import because those APIs generate new ULIDs, timestamps, and event indexes.
- Do not broadcast imported historical events to SSE, WebSocket, or webhook subscribers.
- Reject task-id conflicts by default; require explicit `overwrite: true` to replace an existing task and all of its events.
- Return Taskcast server version from public endpoints so agent-pi can detect an old Taskcast server and tell the user to upgrade.
- Keep TypeScript and Rust server-side behavior in sync.

## Non-Goals

- Multi-task archives in the first Taskcast phase.
- Storage-engine dumps or raw database import/export.
- Broadcasting imported historical events.
- Automatically renaming task ids on import.
- Capability-based version negotiation. Agent-pi will use semver only.

## Archive Format

Taskcast core will define a first-class archive type:

```ts
export interface TaskArchive {
  schema: 'taskcast.taskArchive'
  version: 1
  exportedAt: number
  task: Task
  events: TaskEvent[]
}
```

The archive is the canonical portable representation of one task. Events are emitted in ascending `index` order.

Validation rules:

- `schema` must be `taskcast.taskArchive`.
- `version` must be supported by the importer.
- `task.id` must match every event `taskId`.
- Event `id` values must be unique.
- Event `index` values must be unique and contiguous from `0`.
- Event ordering in the archive is normalized by `index`; malformed or duplicate indexes are rejected.

## Core API

Taskcast core will expose archive operations from the engine:

```ts
exportTaskArchive(taskId: string): Promise<TaskArchive>
importTaskArchive(
  archive: TaskArchive,
  options?: { overwrite?: boolean }
): Promise<{ taskId: string; eventCount: number; overwritten: boolean }>
```

Export reads the task and full event history from the best available store path and returns a normalized archive.

Import writes task and events directly through an archive restore path, not through the live publish path. It must preserve original ids, indexes, timestamps, task state, and task timestamps.

After import, Taskcast must restore runtime bookkeeping so new events can continue safely:

- the next event index is `max(imported.index) + 1`
- series latest and accumulated state are rebuilt from imported events
- later `publishEvent` calls behave as if the task had always existed in this instance

## Conflict Handling

If the target already contains `archive.task.id`, import fails with a conflict unless `overwrite: true` is supplied.

With `overwrite: true`, import replaces the whole task archive atomically:

- old task state is replaced
- old events are replaced
- derived event counters and series state are rebuilt from the imported archive

Storage backends should provide all-or-nothing behavior. Where a backend cannot provide a native transaction, the implementation must use a tested replacement strategy that prevents partial success from leaving mixed old/new task history.

## Broadcast and Webhooks

Imported historical events are silent. Import does not publish to:

- per-task SSE
- global SSE
- worker WebSocket routes
- webhooks

Only future live events generated after import are broadcast normally.

This is intentional because subscribers are live connections, not persisted task properties. Even if a client is currently subscribed to the same task id during overwrite, archive import is a restore operation, not a live event stream.

## HTTP API

Taskcast server will expose archive endpoints:

```http
GET /tasks/:taskId/archive
POST /tasks/import
```

`GET /tasks/:taskId/archive` returns a `TaskArchive`.

`POST /tasks/import` accepts:

```json
{
  "archive": {
    "schema": "taskcast.taskArchive",
    "version": 1,
    "exportedAt": 1783180800000,
    "task": {},
    "events": []
  },
  "overwrite": false
}
```

Successful import returns:

```json
{
  "ok": true,
  "taskId": "task_...",
  "eventCount": 42,
  "overwritten": false
}
```

Error mapping:

- `400`: unsupported archive schema/version, malformed archive, invalid event sequence, or mismatched task id
- `404`: export requested for a missing task
- `409`: import target exists and `overwrite` was not true

The TypeScript Hono server and Rust Axum server must expose identical paths, methods, status codes, and JSON shapes.

## Version Handshake

Taskcast will expose its own server version on public endpoints.

Endpoints:

```http
GET /
GET /health
GET /health/detail
```

`GET /` is a new unauthenticated API root JSON endpoint:

```json
{
  "name": "taskcast",
  "version": "1.5.1",
  "apiVersion": "v1",
  "links": {
    "health": "/health",
    "healthDetail": "/health/detail",
    "openapi": "/openapi.json",
    "docs": "/docs"
  }
}
```

`GET /health` remains lightweight but adds version data:

```json
{
  "ok": true,
  "name": "taskcast",
  "version": "1.5.1",
  "apiVersion": "v1"
}
```

`GET /health/detail` keeps its existing `ok`, `uptime`, `auth`, and `adapters` fields and adds `name`, `version`, and `apiVersion`.

Agent-pi will use semver only. If Taskcast does not return a version, or the version is lower than agent-pi's minimum required version, agent-pi should tell the user to upgrade Taskcast.

The OpenAPI `info.version` should be sourced from the same server version instead of a stale hard-coded value. Rust can use `env!("CARGO_PKG_VERSION")`. TypeScript should use a build-safe package-version source.

## SDK API

`@taskcast/server-sdk` will add:

```ts
getServerInfo(): Promise<{ name: string; version: string; apiVersion: string }>
exportTaskArchive(taskId: string): Promise<TaskArchive>
importTaskArchive(
  archive: TaskArchive,
  options?: { overwrite?: boolean }
): Promise<{ ok: true; taskId: string; eventCount: number; overwritten: boolean }>
```

Existing health and task APIs remain compatible. Older servers that return only `{ ok: true }` from `/health` are treated as version-unknown by agent-pi.

## Storage Requirements

Short-term stores need an archive restore path that can:

- save the imported task exactly
- replace task events on overwrite
- append imported events exactly as provided
- reset the next-index counter to continue after the imported max index
- rebuild series helper state used by late join and future publish operations

Long-term stores need a matching restore path for task and event persistence. For accumulated series, the imported event data must preserve the same semantics that normal long-term writes use today.

Memory, Redis, SQLite, and Postgres implementations must pass the same archive round-trip tests where applicable.

## Downstream Claw Hive Session Bundle

The Claw Hive Dashboard will build session import/export on top of Taskcast archives.

Proposed bundle:

```ts
export interface ClawHiveSessionBundle {
  schema: 'clawHive.sessionBundle'
  version: 1
  exportedAt: number
  taskcast: {
    minVersion: string
    taskArchive: TaskArchive
  }
  agent: unknown
  session: unknown
  componentConfigs: unknown[]
  componentConfigOverrides: unknown[]
  sessionEntries: unknown[]
  llmCalls: unknown[]
  llmCallComponentSnapshots: unknown[]
}
```

Export preserves original ids and timestamps for agent, session, Taskcast task, session entries, LLM calls, and component snapshots.

Import flow:

1. Check Taskcast server version with semver. If missing or too old, prompt the user to upgrade Taskcast.
2. Check the referenced agent:
   - create it if missing
   - reuse it if identical
   - show a diff if it exists but differs, then let the user choose overwrite or use existing
3. Check the session:
   - preserve original `sessionId`
   - block by default if the same `sessionId` exists
   - require explicit overwrite to replace
4. Import the embedded Taskcast task archive with the same conflict policy.
5. Imported active sessions are displayed as runtime-inactive. Their lifecycle status may remain active, but they do not auto-acquire a worker or auto-run. They can be triggered again later.

## Testing

Taskcast core tests:

- archive export/import round-trip preserves task fields and all event identity fields
- import rejects invalid schema/version
- import rejects mismatched event task ids
- import rejects duplicate event ids and duplicate/non-contiguous indexes
- conflict without overwrite returns an error
- overwrite replaces the full old task history
- imported historical events do not hit broadcast or webhook paths
- publish after import uses the next expected index
- series latest/accumulated behavior remains correct after import

Taskcast server and SDK tests:

- `GET /`, `/health`, and `/health/detail` return version data without auth
- OpenAPI version matches the server version source
- archive export missing task returns `404`
- archive import malformed body returns `400`
- archive import conflict returns `409`
- archive import overwrite succeeds
- TypeScript and Rust HTTP behavior match

Agent-pi and Dashboard tests:

- old or version-missing Taskcast returns a user-facing upgrade prompt
- agent missing/identical/different import flows
- session id conflict block and explicit overwrite
- imported active session appears runtime-inactive
- imported session can be triggered again

## Implementation Phasing

1. Add Taskcast archive types, validation, and core memory-store behavior.
2. Add archive restore support to the persistent adapters.
3. Add TypeScript server routes, SDK methods, version root/health fields, and tests.
4. Add Rust core/server equivalent behavior and tests.
5. Add agent-pi version check and Dashboard session bundle import/export.

Each server-side Taskcast phase must keep TypeScript and Rust behavior aligned before merging.
