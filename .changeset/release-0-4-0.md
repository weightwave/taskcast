---
"@taskcast/core": minor
"@taskcast/server": minor
"@taskcast/server-sdk": minor
"@taskcast/client": minor
"@taskcast/react": minor
"@taskcast/cli": minor
"@taskcast/redis": minor
"@taskcast/postgres": minor
"@taskcast/sqlite": minor
"@taskcast/sentry": minor
---

### Dashboard Web

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