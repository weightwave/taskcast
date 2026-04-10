---
"@taskcast/core": minor
"@taskcast/server": minor
"@taskcast/server-sdk": minor
"@taskcast/client": minor
"@taskcast/react": minor
"@taskcast/cli": minor
"@taskcast/redis": minor
"@taskcast/postgres": minor
"@taskcast/sentry": minor
"@taskcast/sqlite": minor
---

feat: add automatic PostgreSQL migrations on server startup

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
