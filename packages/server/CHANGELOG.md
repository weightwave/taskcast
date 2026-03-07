# @taskcast/server

## 1.0.0

### Minor Changes

- af1d289: ### Dashboard Web

  - Add full-featured management dashboard with overview, tasks, events, and workers pages
  - Docker support with nginx for standalone deployment
  - `taskcast ui` / `taskcast dashboard` commands to serve the dashboard

  ### Playground

  - Embed interactive API playground via rust-embed in Rust CLI
  - `taskcast playground` standalone command
  - Backend, Browser SSE, Worker Pull, and Worker WS panels with per-panel auth

  ### Migration CLI

  - `taskcast migrate` subcommand for both TS and Rust CLIs
  - sqlx-compatible migration runner for PostgreSQL

  ### Server

  - `GET /tasks` list endpoint with filters
  - `PATCH /workers/:id/status` for drain control
  - `POST /admin/token` route for dashboard authentication
  - Return 409 Conflict for duplicate task IDs and invalid state transitions
  - CORS support
  - Add `hot`/`subscriberCount` to task responses
  - Per-instance subscriber tracking (no more global state)

  ### Core

  - Admin token config with auto-generation
  - Sync Rust validation rules with TypeScript implementation

  ### Testing

  - 100% line/function coverage across all packages
  - E2E test infrastructure with Playwright
  - Integration tests for Redis, CLI, server-sdk, client SSE, webhooks, auth, concurrency
  - `startTestServer` helper exported from `@taskcast/server`

### Patch Changes

- Updated dependencies [af1d289]
  - @taskcast/core@1.0.0

## 0.3.1

### Patch Changes

- @taskcast/core@0.3.1

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
