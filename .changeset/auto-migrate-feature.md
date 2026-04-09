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

- Auto-migrate enabled via `TASKCAST_AUTO_MIGRATE=true`
- Migrations embedded in CLI (both TypeScript and Rust) for zero external dependencies
- Synchronous failure — server startup is blocked if migrations fail
- Compatible with PostgreSQL configuration via `TASKCAST_POSTGRES_URL` or config file
- Can be disabled/overridden at runtime via environment variables
- Parallel-safe — multiple instances can safely start simultaneously

**New CLI command:**

- `taskcast migrate` — manually run pending migrations with interactive confirmation

**Behavior:**

- When auto-migrate is enabled and PostgreSQL is configured, migrations run automatically
- If disabled or no Postgres URL, auto-migrate skips silently
- Migrations are tracked in `_sqlx_migrations` table (created automatically)
- Each migration runs exactly once, idempotent and safe for retries
- Symmetric implementation in both TypeScript and Rust

**Documentation:**

See `docs/guide/auto-migrate.md` for configuration, examples, and troubleshooting.
