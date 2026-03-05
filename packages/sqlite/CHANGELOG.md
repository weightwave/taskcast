# @taskcast/sqlite

## 0.3.0

### Minor Changes

- 907d943: Add worker assignment system with four modes: external, pull, ws-offer, ws-race

  - New `WorkerManager` for worker registration, capacity/cost model, tag matching, and audit events
  - Extend task lifecycle with `assigned` status: `pending → assigned → running → terminal`
  - Add worker REST endpoints (list, get, delete, pull, decline) and WebSocket protocol
  - Add `paused`/`blocked` ↔ `assigned` state transitions
  - Enforce `auth.workerId` identity in REST and WebSocket endpoints
  - Add configurable `timeout` query param to pull endpoint
  - Add `/health` endpoint to Rust server
  - Add passive cleanup of stale task IDs in Redis adapters
  - Fix SQLite `listTasks()` to apply limit after tag filtering
  - Storage adapters (Redis, PostgreSQL, SQLite) extended with worker CRUD, claim, and assignment operations

### Patch Changes

- Updated dependencies [907d943]
  - @taskcast/core@0.3.0

## 0.2.0

### Minor Changes

- ca5ec96: Add SQLite local storage adapter for zero-dependency development. Use `taskcast start --storage sqlite` to persist data locally without Redis or PostgreSQL.

### Patch Changes

- Updated dependencies [d4a391c]
  - @taskcast/core@0.2.0
