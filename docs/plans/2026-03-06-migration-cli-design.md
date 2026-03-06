# Database Migration CLI Command — Design

**Date:** 2026-03-06
**Status:** Approved

## Problem

Currently, database migration handling is inconsistent across the project:

- **Rust Postgres** uses sqlx's built-in migration runner with `_sqlx_migrations` tracking table — the only one with proper version tracking
- **TS Postgres** has no migration runner; users must manually execute SQL files
- **SQLite (both sides)** runs full schema on startup via `CREATE TABLE IF NOT EXISTS` — acceptable for local single-process use
- Postgres migration SQL files are duplicated between `packages/postgres/migrations/` and `rust/taskcast-postgres/migrations/`
- Users may create tables with Rust CLI then run TS server (or vice versa), so migration tracking must be cross-compatible

## Design

### 1. Unified Migration File Location

Move Postgres migration files to a single location at the monorepo root:

```
migrations/
  postgres/
    001_initial.sql
    002_workers.sql
```

- **Rust** `taskcast-postgres`: `sqlx::migrate!("../../migrations/postgres")`
- **TS** `@taskcast/postgres`: runtime `readFileSync` resolving from package root to `../../migrations/postgres/`
- Delete `packages/postgres/migrations/` and `rust/taskcast-postgres/migrations/`
- SQLite migrations stay in their respective packages (no change)

### 2. TS sqlx-Compatible Migration Runner

New file: `packages/postgres/src/migration-runner.ts`

Implements a migration runner that reads/writes the same `_sqlx_migrations` table that sqlx uses, ensuring full cross-compatibility.

**`_sqlx_migrations` table structure (matching sqlx exactly):**

| Column | Type | Description |
|--------|------|-------------|
| `version` | `BIGINT PRIMARY KEY` | Extracted from filename prefix (`001` -> `1`) |
| `description` | `TEXT NOT NULL` | Filename without prefix (`initial`) |
| `installed_on` | `TIMESTAMPTZ NOT NULL` | Timestamp of execution |
| `success` | `BOOLEAN NOT NULL` | Whether migration succeeded |
| `checksum` | `BYTEA NOT NULL` | SHA-384 hash of file contents |
| `execution_time` | `BIGINT NOT NULL` | Execution time in nanoseconds |

**Execution logic:**

1. Scan `migrations/postgres/*.sql`, sort by filename
2. `CREATE TABLE IF NOT EXISTS _sqlx_migrations` (if first run)
3. Query existing records from `_sqlx_migrations`
4. For already-executed migrations: verify checksum matches — error if mismatch
5. For pending migrations: execute SQL, write tracking record
6. Return `{ applied: string[], skipped: string[] }`

Exported as `runMigrations(sql, migrationsDir)` for both CLI and programmatic use.

### 3. CLI `migrate` Subcommand

Both Rust and TS CLIs get a symmetric `migrate` command with identical behavior.

**Usage:**

```bash
taskcast migrate [options]
  --url <postgres://...>     # Direct database URL (highest priority)
  -c, --config <path>        # Config file for URL resolution
  -y, --yes                  # Skip confirmation prompt
```

**URL resolution priority:** `--url` > `TASKCAST_POSTGRES_URL` env var > config file

**Execution flow:**

1. Resolve postgres URL; error if none found
2. Connect to database, print target info (host/db name)
3. Scan pending migrations, list files to be applied
4. If no pending migrations: print "Database is up to date", exit
5. Without `-y`: prompt `Apply N migration(s) to <db>? (Y/n)`
6. Execute each migration, print progress: `Applied 001_initial (12ms)`
7. Print summary: `Applied N migration(s) successfully`

**Postgres only.** SQLite auto-migrates on startup. If user somehow targets SQLite, print a message explaining this.

### 4. Cross-Compatibility Tests

New file: `packages/postgres/tests/integration/migration-compat.test.ts`

Using testcontainers with real Postgres:

1. **Rust-first -> TS recognizes:** Simulate sqlx migration records in `_sqlx_migrations`, then run TS runner — verify it skips already-applied migrations and checksum validation passes
2. **TS-first -> Rust recognizes:** TS runner executes migrations, then verify the `_sqlx_migrations` records are field-level identical to what sqlx would produce (same checksum, same version, same description format)
3. **Checksum mismatch detection:** Tamper with checksum in `_sqlx_migrations`, verify runner refuses to proceed

### 5. File Changes Summary

| Change | Description |
|--------|-------------|
| Create `migrations/postgres/` | Single source of truth for Postgres SQL files |
| Delete `packages/postgres/migrations/` | Now references root |
| Delete `rust/taskcast-postgres/migrations/` | Now references root |
| Create `packages/postgres/src/migration-runner.ts` | sqlx-compatible runner |
| Modify `packages/postgres/src/index.ts` | Export `runMigrations` |
| Modify `packages/cli/src/index.ts` | Add `migrate` subcommand |
| Modify `rust/taskcast-cli/src/main.rs` | Add `Migrate` subcommand |
| Modify `rust/taskcast-postgres/src/store.rs` | Update `sqlx::migrate!` path |
| Create `packages/postgres/tests/integration/migration-compat.test.ts` | Cross-compatibility tests |

No changes to SQLite, Redis, core, server, client, react, or sentry packages.