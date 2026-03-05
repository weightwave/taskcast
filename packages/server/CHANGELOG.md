# @taskcast/server

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

### Patch Changes

- d4a391c: Unified release workflow: npm publish, Rust binary builds (5 platforms), and Docker image push now share a single version number and run in one workflow.
- Updated dependencies [d4a391c]
  - @taskcast/core@0.2.0

## 0.1.2

### Patch Changes

- Updated dependencies [987c9df]
  - @taskcast/core@0.1.2

## 0.1.1

### Patch Changes

- 5085c69: fix: resolve workspace:\* references in published packages
- Updated dependencies [5085c69]
  - @taskcast/core@0.1.1
