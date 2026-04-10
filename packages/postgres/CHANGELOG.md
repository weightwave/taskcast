# @taskcast/postgres

## 1.5.0

### Minor Changes

- 8c1ead8: feat: add automatic PostgreSQL migrations on server startup

  Taskcast now supports automatic PostgreSQL migrations via the new
  `TASKCAST_AUTO_MIGRATE` environment variable. When enabled, migrations run
  automatically on server startup before accepting requests.

  **New features:**

  - Auto-migrate is **opt-in**: enabled only when `TASKCAST_AUTO_MIGRATE=true` (or `1`/`yes`/`on`)
  - Migrations embedded in CLI (both TypeScript and Rust) for zero external dependencies
  - Fail-fast on errors — server startup is blocked if migrations fail
  - Compatible with PostgreSQL configuration via `TASKCAST_POSTGRES_URL` or config file
  - Idempotent: safe to run repeatedly; already-applied migrations are skipped
  - **Concurrent startup:** the Rust CLI uses sqlx advisory locks and tolerates
    parallel startup across replicas. The Node.js CLI does **not** take an
    advisory lock and is unsafe under concurrent auto-migrate; see
    `docs/guide/auto-migrate.md` for recommended patterns (pre-deploy migrate
    step, Kubernetes initContainer, or use the Rust CLI)

  **New CLI command:**

  - `taskcast migrate` — manually run pending migrations with interactive confirmation

  **Behavior:**

  - When `TASKCAST_AUTO_MIGRATE` is truthy AND a Postgres connection is available,
    migrations run automatically before the server accepts requests
  - When auto-migrate is disabled or no Postgres is configured, a clear log line
    is emitted (`[taskcast] TASKCAST_AUTO_MIGRATE is set but no Postgres configured — skipping`)
    and the server continues to start normally
  - Migrations are tracked in `_sqlx_migrations` table (created automatically)
  - Each migration runs exactly once, idempotent and safe for retries
  - Symmetric log output between TypeScript and Rust CLIs

  **Documentation:**

  See `docs/guide/auto-migrate.md` for configuration, examples, and troubleshooting.

### Patch Changes

- Updated dependencies [8c1ead8]
  - @taskcast/core@1.5.0

## 1.4.1

### Patch Changes

- @taskcast/core@1.4.1

## 1.4.0

### Patch Changes

- @taskcast/core@1.4.0

## 1.3.0

### Patch Changes

- @taskcast/core@1.3.0

## 1.2.1

### Patch Changes

- Updated dependencies [3151b5b]
  - @taskcast/core@1.2.1

## 1.2.0

### Patch Changes

- Updated dependencies [5e2ceb6]
- Updated dependencies [13d321c]
  - @taskcast/core@1.2.0

## 1.1.0

### Patch Changes

- Updated dependencies [771f7de]
  - @taskcast/core@1.1.0

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
