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

Add worker assignment system with four modes: external, pull, ws-offer, ws-race

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
