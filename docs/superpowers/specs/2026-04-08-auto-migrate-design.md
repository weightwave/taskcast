# Automatic PostgreSQL Migration on Startup

**Date:** 2026-04-08
**Status:** Approved (pending spec review)

## Overview

Add an opt-in capability to automatically apply pending PostgreSQL migrations
when the Taskcast server starts. Controlled by the environment variable
`TASKCAST_AUTO_MIGRATE`. Implemented symmetrically in both the Node.js CLI
(`@taskcast/cli`) and the Rust CLI (`taskcast-cli`), sharing the same
`_sqlx_migrations` tracking table that the existing manual `taskcast migrate`
subcommand already uses.

The feature is designed for convenience in development, single-instance
production deployments, and simple containerized setups. For multi-replica
rolling deployments, the existing manual `taskcast migrate` deploy step
remains the recommended approach.

## Motivation

Today, PostgreSQL migrations must be applied explicitly via the `taskcast migrate`
subcommand before the server can serve any write requests. If a user forgets
this step, the server starts successfully but every write hits
`relation "taskcast_tasks" does not exist`, producing confusing runtime errors
only on the request path.

A common user request — especially for simple Docker or docker-compose
deployments — is "just make the server handle migrations on its own." This
design provides that capability without removing the manual option (which
remains safer for staged rollouts) and without silently changing behavior
for existing users (the feature is opt-in via an explicit env var).

As a side benefit, this work fixes a pre-existing TODO in the manual
`taskcast migrate` subcommand: it currently uses a monorepo-relative path
(`../../../../migrations/postgres`) to locate SQL files, which breaks when
the CLI is installed via `npm install -g @taskcast/cli`. The fix (embedding
SQL files at build time) benefits both the new auto-migrate path and the
existing manual path.

## Scope

**In scope:**

- New `TASKCAST_AUTO_MIGRATE` environment variable (Node + Rust CLI)
- Shared boolean parsing helper in both runtimes (recognized truthy values:
  `1`, `true`, `yes`, `on`, case-insensitive; all other values treated as false)
- Auto-migrate execution inside the `start` command, before the Postgres
  long-term store adapter is created
- Fail-fast on migration errors (server exits with status 1; HTTP server
  never binds)
- No-op (with informational log) when Postgres isn't configured
- Idempotency via reuse of `runMigrations()` (TS) and `sqlx::migrate!` (Rust);
  both share the `_sqlx_migrations` tracking table
- **Fix:** embed `migrations/postgres/*.sql` into the published `@taskcast/cli`
  package via build-time TS codegen, so both `taskcast migrate` and auto-migrate
  work when the CLI is installed via npm
- Testability refactor: extract auto-migrate logic into a dedicated
  `performAutoMigrateIfEnabled` (TS) / `run_auto_migrate` (Rust) function
- Extract the TS `start` action body into an exported `runStart()` function
  to enable in-process end-to-end testing
- Comprehensive tests: unit (boolean parsing, codegen), integration (real
  Postgres via testcontainers), end-to-end in-process (start flow with env
  vars set)
- Documentation updates in `deployment.md`/`deployment.zh.md` and the CLI
  README
- Changeset for the release pipeline

**Out of scope:**

- Changing the default behavior (auto-migrate remains opt-in)
- Auto-migrate for SQLite (SQLite adapter already applies its single schema
  file unconditionally at `createSqliteAdapters()` time; no change needed)
- Migration rollback/downgrade commands
- Automatic migration for Redis (Redis has no schema)
- Coordinating migrations across multi-replica rolling deployments (advisory
  locks or leader election) — users with such topologies should continue to
  use the manual `taskcast migrate` deploy step
- Exposing auto-migrate behavior through `createTaskcastApp()` or via the
  `@taskcast/postgres` adapter factory — the feature lives in the CLI layer
  only, to keep the adapter pure

## Design Decisions

The following decisions were made during brainstorming and are now fixed
for the implementation:

| Decision | Choice | Rationale |
|---|---|---|
| Env var name | `TASKCAST_AUTO_MIGRATE` | Matches existing `TASKCAST_*` prefix convention (`TASKCAST_POSTGRES_URL`, `TASKCAST_REDIS_URL`, etc.). Avoids collisions with generic `AUTO_MIGRATE` names used by other frameworks. |
| Truthy values | `1`, `true`, `yes`, `on` (case-insensitive) | Common boolean conventions; easy to document. All other values (including empty string, `0`, `false`, `no`, `off`) are treated as false. |
| Where auto-migrate runs | CLI `start` command, before adapter creation | Keeps `@taskcast/postgres` / `taskcast-postgres` adapters pure (no env var or filesystem dependencies). CLI is already the "startup orchestration" layer. |
| Failure policy | Fail-fast (exit 1, server does not start) | A broken schema produces worse user experience than a startup failure. Consistent with the existing `runMigrations()` behavior for dirty migrations. |
| No Postgres configured + flag enabled | Info log + skip, normal startup | Common dev scenario: shell-wide `TASKCAST_AUTO_MIGRATE=1`, but running against memory/sqlite locally. Fail-fast here would be annoying; a silent skip would be confusing. |
| Idempotency | Delegate to existing `runMigrations()` (TS) and `sqlx::migrate!` (Rust) | Both already track applied versions via `_sqlx_migrations`, verify checksums, and skip already-applied migrations. No new code needed. |
| Migration file distribution (TS) | Build-time codegen of `src/generated/postgres-migrations.ts` with embedded SQL strings, committed to git | Eliminates stale-file risk from copy-based approaches, eliminates runtime filesystem dependency, works in bundlers and read-only filesystems, parallels Rust's compile-time `include_str!` semantics. |
| Migration file distribution (Rust) | No change — `sqlx::migrate!("../../migrations/postgres")` already embeds SQL at compile time | Free. |
| Testing breadth | Unit + integration + in-process end-to-end, both runtimes | Matches CLAUDE.md's 100% coverage requirement and the existing test infrastructure (testcontainers for Postgres, in-process `tokio::spawn` for Rust start tests, in-process `createTaskcastApp` for TS CLI tests). |
| E2E style | In-process, not subprocess spawn | Matches existing test infrastructure on both sides. Faster, easier to assert, no subprocess lifecycle management. |

## Architecture

### High-level flow (TS and Rust, identical)

```
taskcast start
  │
  ├─ 1. Load config file
  ├─ 2. Resolve TASKCAST_POSTGRES_URL (env > config > none)
  ├─ 3. Resolve TASKCAST_AUTO_MIGRATE (env, parseBooleanEnv/parse_boolean_env)
  │
  ├─ 4. [NEW] performAutoMigrateIfEnabled / run_auto_migrate:
  │      ├─ If auto_migrate == false → no-op, return
  │      ├─ If auto_migrate == true && (storage == sqlite || !postgres_url)
  │      │     → info log "skipping", return
  │      ├─ Else (auto_migrate && postgres_url):
  │      │     ├─ Connect to Postgres (temporary pool/connection)
  │      │     ├─ Run runMigrations(POSTGRES_MIGRATIONS) / store.migrate()
  │      │     ├─ Log result (applied N / up to date)
  │      │     ├─ Close temporary pool/connection
  │      │     ├─ On success → return
  │      │     └─ On failure → log error, exit 1 / Err(_) up the call stack
  │      │
  ├─ 5. Build storage adapters (PostgresLongTermStore, etc.) — unchanged
  ├─ 6. Build TaskEngine — unchanged
  ├─ 7. Start HTTP server — unchanged
```

### Key invariant

**The Postgres long-term store adapter is never created while the schema is
in an unknown state.** Step 5 strictly follows step 4 in the code path; if
step 4 fails, the process exits before step 5 runs. This guarantees that no
request handler can ever issue SQL against an unmigrated schema.

### Why a temporary connection for the migration

The production adapter pool (created in step 5) is sized for the long-lived
HTTP request path and has different lifetime semantics. Using a separate
temporary connection for the one-shot DDL operation:

1. Preserves the ordering invariant (pool creation strictly after migration
   success).
2. Matches the pattern already used by the manual `taskcast migrate`
   subcommand ([packages/cli/src/commands/migrate.ts:37](../../../packages/cli/src/commands/migrate.ts#L37)),
   for consistency.
3. Avoids any interaction between the DDL transaction and the shared pool's
   connection state.

The temporary connection is explicitly closed after migration, regardless
of success or failure.

## TS Implementation

### File changes

| Path | Change |
|---|---|
| `packages/postgres/src/migration-runner.ts` | Extend `runMigrations()` signature to accept `string \| MigrationFile[]`. Export new helper `buildMigrationFiles(embedded: EmbeddedMigration[]): MigrationFile[]`. |
| `packages/postgres/src/index.ts` | Export new helper `buildMigrationFiles` and `EmbeddedMigration` type. |
| `packages/cli/scripts/generate-migrations.mjs` | **NEW** — build-time codegen script. |
| `packages/cli/src/generated/postgres-migrations.ts` | **NEW — committed to git.** Contains `export const POSTGRES_MIGRATIONS: readonly EmbeddedMigration[]` with inline SQL strings. |
| `packages/cli/package.json` | `"build"` script becomes `"node scripts/generate-migrations.mjs ../../migrations/postgres src/generated/postgres-migrations.ts && tsc"`. |
| `packages/cli/src/utils.ts` | Add exported `parseBooleanEnv(value: string \| undefined): boolean`. |
| `packages/cli/src/auto-migrate.ts` | **NEW** — `performAutoMigrateIfEnabled(options, deps?)` helper. |
| `packages/cli/src/commands/start.ts` | Extract action body into exported `runStart(options): Promise<{ stop: () => Promise<void> }>`. Call `performAutoMigrateIfEnabled()` before adapter creation. |
| `packages/cli/src/commands/migrate.ts` | Replace monorepo-relative path with `POSTGRES_MIGRATIONS` + `buildMigrationFiles()`. Delete the "works in monorepo only" TODO. |
| `packages/cli/package.json` | Add `testcontainers@^10.13.0` to devDependencies. |
| `.github/workflows/ci.yml` | Add staleness-check step: regenerate the embedded-migrations TS file and `git diff --exit-code` on it. |

### `runMigrations` API extension

```ts
// packages/postgres/src/migration-runner.ts

export interface EmbeddedMigration {
  filename: string
  sql: string
}

/**
 * Convert an array of embedded migrations (filename + raw SQL string) into
 * the internal MigrationFile representation used by runMigrations. Parses
 * filenames for version/description and computes checksums.
 */
export function buildMigrationFiles(embedded: readonly EmbeddedMigration[]): MigrationFile[] {
  const files: MigrationFile[] = []
  for (const m of embedded) {
    const parsed = parseMigrationFilename(m.filename)
    if (!parsed) continue
    files.push({
      version: parsed.version,
      description: parsed.description,
      sql: m.sql,
      checksum: computeChecksum(m.sql),
      filename: m.filename,
    })
  }
  files.sort((a, b) => a.version - b.version)
  return files
}

export async function runMigrations(
  sql: ReturnType<typeof postgres>,
  migrationsOrFiles: string | MigrationFile[],
): Promise<MigrationResult> {
  const localFiles = typeof migrationsOrFiles === 'string'
    ? loadMigrationFiles(migrationsOrFiles)
    : [...migrationsOrFiles].sort((a, b) => a.version - b.version)
  // ... rest of the existing implementation unchanged
}
```

The overload is purely additive; existing callers passing a directory path
continue to work.

### Build-time code generation

`packages/cli/scripts/generate-migrations.mjs` is a Node ESM script that:

1. Accepts two positional arguments: source directory and output file path
2. Reads all `*.sql` files from the source directory, sorted alphabetically
3. Throws if zero files are found (prevents silent empty output)
4. **Validates each filename against `^\d{3}_[a-zA-Z0-9_]+\.sql$`** — enforces
   the 3-digit zero-padded convention required for the Rust runtime's
   filename reconstruction to work correctly
5. Writes the output TS file **atomically** (write to `.tmp`, then `renameSync`)
6. Uses `JSON.stringify()` for SQL string escaping (safe for any content
   including backticks, `${}`, and backslashes)

Generated output structure:

```ts
// AUTO-GENERATED by scripts/generate-migrations.mjs — do not edit.
// Source: migrations/postgres/
// Regenerated on every `pnpm build`.

export interface EmbeddedMigration {
  filename: string
  sql: string
}

export const POSTGRES_MIGRATIONS: readonly EmbeddedMigration[] = [
  { filename: "001_initial.sql", sql: "..." },
  { filename: "002_workers.sql", sql: "..." },
] as const
```

The generated file **is committed to git** (not gitignored) because:

- `pnpm lint` uses `tsc -b`, which does not invoke package `build` scripts.
  On a fresh clone, `tsc -b` would fail if the generated file didn't exist.
- Committing it lets reviewers see migration content changes in diffs.
- Staleness is enforced by a CI step: run the generator and
  `git diff --exit-code` on the output file. If a PR adds/modifies SQL
  without regenerating, CI fails at this gate.

### `parseBooleanEnv` helper

Added to `packages/cli/src/utils.ts`:

```ts
/**
 * Parse a boolean-like environment variable.
 * Recognized truthy values (case-insensitive): "1", "true", "yes", "on".
 * All other values (including undefined, empty string, "0", "false",
 * "no", "off") are treated as false.
 */
export function parseBooleanEnv(value: string | undefined): boolean {
  if (value === undefined || value === '') return false
  const normalized = value.trim().toLowerCase()
  return normalized === '1' || normalized === 'true' || normalized === 'yes' || normalized === 'on'
}
```

### `performAutoMigrateIfEnabled` helper

New file `packages/cli/src/auto-migrate.ts`:

```ts
import postgres from 'postgres'
import { runMigrations, buildMigrationFiles } from '@taskcast/postgres'
import { POSTGRES_MIGRATIONS } from './generated/postgres-migrations.js'

export interface AutoMigrateOptions {
  enabled: boolean
  postgresUrl: string | undefined
  storageMode: string  // "memory" | "redis" | "sqlite"
}

export interface AutoMigrateDeps {
  // Injected for testability; default to real implementations.
  createSql?: (url: string) => ReturnType<typeof postgres>
  logger?: {
    info: (msg: string) => void
    error: (msg: string) => void
  }
}

/**
 * Run Postgres migrations if TASKCAST_AUTO_MIGRATE is enabled and Postgres
 * is configured. Throws on failure — caller should fail-fast (exit the process).
 * No-op with an info log if Postgres isn't configured.
 */
export async function performAutoMigrateIfEnabled(
  options: AutoMigrateOptions,
  deps: AutoMigrateDeps = {},
): Promise<void> {
  const logger = deps.logger ?? {
    info: (msg) => console.log(msg),
    error: (msg) => console.error(msg),
  }
  const createSql = deps.createSql ?? ((url) => postgres(url))

  if (!options.enabled) return

  if (options.storageMode === 'sqlite' || !options.postgresUrl) {
    logger.info('[taskcast] TASKCAST_AUTO_MIGRATE is set but no Postgres configured — skipping')
    return
  }

  logger.info(`[taskcast] TASKCAST_AUTO_MIGRATE enabled — running Postgres migrations on ${options.postgresUrl}`)

  let migrateSql: ReturnType<typeof postgres> | undefined
  try {
    migrateSql = createSql(options.postgresUrl)
    const files = buildMigrationFiles(POSTGRES_MIGRATIONS)
    const result = await runMigrations(migrateSql, files)
    if (result.applied.length > 0) {
      logger.info(`[taskcast] Applied ${result.applied.length} new migration(s): ${result.applied.join(', ')}`)
    } else {
      logger.info(`[taskcast] Database schema up to date (${result.skipped.length} migration(s) already applied)`)
    }
    await migrateSql.end()
  } catch (err) {
    logger.error(`[taskcast] Auto-migration failed: ${(err as Error).message}`)
    if (migrateSql) {
      await migrateSql.end().catch(() => { /* best-effort */ })
    }
    throw err
  }
}
```

Callers (i.e. `runStart`) wrap invocation in a `try/catch` and call
`process.exit(1)` on failure, so the helper itself just throws.

### `start.ts` refactor: extract `runStart`

The current `registerStartCommand` wraps the entire server startup logic in
an anonymous arrow function passed to `commander`'s `.action()`, which
cannot be tested without booting the full CLI. Refactor to:

```ts
// packages/cli/src/commands/start.ts

export interface RunStartOptions {
  config?: string
  port: number
  storage?: string
  dbPath?: string
  playground?: boolean
  verbose?: boolean
}

export interface RunStartHandle {
  stop: () => Promise<void>
  port: number
}

/**
 * Programmatic entry point for `taskcast start`. Exported for integration
 * tests that need to start the full stack in-process (with env vars, real
 * adapters, and a listening HTTP server) and then stop it cleanly.
 */
export async function runStart(options: RunStartOptions): Promise<RunStartHandle> {
  // ... all logic currently in the arrow function body, plus:
  //     `await performAutoMigrateIfEnabled({ ... })` inserted after URL
  //     resolution and before adapter creation
  // ... returns { stop, port }
}

export function registerStartCommand(program: Command): void {
  program
    .command('start', { isDefault: true })
    .description('Start the taskcast server in foreground (default)')
    // ... options ...
    .action(async (options) => {
      try {
        const { stop } = await runStart({
          config: options.config,
          port: Number(options.port ?? 3721),
          storage: options.storage,
          dbPath: options.dbPath,
          playground: options.playground,
          verbose: options.verbose,
        })
        process.on('SIGTERM', () => { void stop() })
        process.on('SIGINT', () => { void stop() })
      } catch (err) {
        console.error(`[taskcast] ${(err as Error).message}`)
        process.exit(1)
      }
    })
}
```

Key points:

- `runStart` returns a `{ stop }` handle so tests can clean up (it calls
  the existing `stop()` from `createTaskcastApp` and `server.close()`)
- `runStart` may throw on auto-migrate failure; the `.action()` wrapper
  translates that into `process.exit(1)`
- Signal handlers are installed by the action wrapper, not inside `runStart`
  — tests don't want stray signal handlers registered

### Fixing the manual `taskcast migrate` subcommand

`packages/cli/src/commands/migrate.ts` currently uses:

```ts
const migrationsDir = join(dirname(fileURLToPath(import.meta.url)), '../../../../migrations/postgres')
const allFiles = loadMigrationFiles(migrationsDir)
// ...
const result = await runMigrations(sql, migrationsDir)
```

Replace with:

```ts
import { POSTGRES_MIGRATIONS } from '../generated/postgres-migrations.js'
import { buildMigrationFiles } from '@taskcast/postgres'

const allFiles = buildMigrationFiles(POSTGRES_MIGRATIONS)
// ...
const result = await runMigrations(sql, allFiles)
```

Delete the `// TODO: This path works in the monorepo only ...` comment.
The manual subcommand now works identically in monorepo dev and
npm-installed contexts.

## Rust Implementation

The Rust side is structurally simpler because:

1. `sqlx::migrate!("../../migrations/postgres")` is a **compile-time macro**
   that embeds SQL files into the binary via `include_str!`. No runtime
   filesystem dependency, no distribution problem, no codegen needed.
2. `taskcast_postgres::PostgresLongTermStore::migrate()` already exists and
   already tracks migrations in the shared `_sqlx_migrations` table
   ([rust/taskcast-postgres/src/store.rs:35-38](../../../rust/taskcast-postgres/src/store.rs#L35)).

So the Rust changes are only:

- A boolean env parser (`parse_boolean_env`)
- An auto-migrate helper (`run_auto_migrate`)
- Wiring into `start::run` before the Postgres adapter is created

### File changes

| Path | Change |
|---|---|
| `rust/taskcast-cli/src/helpers.rs` | Add `parse_boolean_env(Option<&str>) -> bool` and its unit tests |
| `rust/taskcast-cli/src/commands/start.rs` | Read `TASKCAST_AUTO_MIGRATE` via parser; add `run_auto_migrate(url, storage_mode) -> Result<(), BoxError>` helper; call it before building the Postgres adapter in both `"redis"` and memory-fallback branches; (minor refactor) extract the Postgres adapter construction into a shared helper `build_postgres_long_term_store` to eliminate the existing duplication between the two branches |
| `rust/taskcast-cli/tests/start_env_tests.rs` | Add tests for auto-migrate happy path, idempotency, fail-fast, and skip cases using the existing `EnvGuard` pattern |
| `rust/taskcast-postgres/src/store.rs` | **Unchanged** — `migrate()` already does the right thing |
| `rust/taskcast-cli/src/commands/migrate.rs` | **Unchanged** — already uses the compile-time macro |
| `migrations/postgres/*.sql` | **Unchanged** |

### `parse_boolean_env`

```rust
// rust/taskcast-cli/src/helpers.rs

/// Parse a boolean-like environment variable value.
///
/// Recognized truthy values (case-insensitive): "1", "true", "yes", "on".
/// All other values (including empty string, "0", "false", "no", "off")
/// are treated as false.
pub fn parse_boolean_env(value: Option<&str>) -> bool {
    match value {
        None => false,
        Some(v) => {
            let trimmed = v.trim().to_ascii_lowercase();
            matches!(trimmed.as_str(), "1" | "true" | "yes" | "on")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_truthy_values() {
        for v in &["1", "true", "True", "TRUE", "yes", "YES", "on", "ON"] {
            assert!(parse_boolean_env(Some(v)), "expected {v:?} to be truthy");
        }
    }

    #[test]
    fn rejects_falsy_values() {
        for v in &["", "0", "false", "False", "no", "off", "maybe", "2"] {
            assert!(!parse_boolean_env(Some(v)), "expected {v:?} to be falsy");
        }
        assert!(!parse_boolean_env(None));
    }

    #[test]
    fn handles_whitespace() {
        assert!(parse_boolean_env(Some(" 1 ")));
        assert!(parse_boolean_env(Some("\ttrue\n")));
    }
}
```

### `run_auto_migrate` and `build_postgres_long_term_store`

```rust
// rust/taskcast-cli/src/commands/start.rs

type BoxError = Box<dyn std::error::Error>;

async fn run_auto_migrate(url: &str) -> Result<(), BoxError> {
    eprintln!("[taskcast] TASKCAST_AUTO_MIGRATE enabled — running Postgres migrations on {url}");
    let migrate_pool = sqlx::PgPool::connect(url).await?;

    // Snapshot applied versions before migration so we can compute the
    // delta and emit log output symmetric with the TS implementation.
    // If the table doesn't exist yet, we get an empty set (sqlx will
    // create it during migrate).
    let before: std::collections::HashSet<i64> = sqlx::query_scalar::<_, i64>(
        "SELECT version FROM _sqlx_migrations"
    )
    .fetch_all(&migrate_pool)
    .await
    .unwrap_or_default()  // table may not exist yet
    .into_iter()
    .collect();

    let store = taskcast_postgres::PostgresLongTermStore::new(migrate_pool.clone());
    let migrate_result = store.migrate().await;

    // On success, query the full applied set and compute newly-applied
    // filenames (description is the filename without version prefix;
    // sqlx stores it in the `description` column).
    let log_line: Option<String> = if migrate_result.is_ok() {
        let rows: Vec<(i64, String)> = sqlx::query_as(
            "SELECT version, description FROM _sqlx_migrations ORDER BY version"
        )
        .fetch_all(&migrate_pool)
        .await
        .unwrap_or_default();

        let newly_applied: Vec<String> = rows
            .iter()
            .filter(|(v, _)| !before.contains(v))
            .map(|(v, d)| format!("{v:03}_{}.sql", d.replace(' ', "_")))
            .collect();

        if newly_applied.is_empty() {
            Some(format!(
                "[taskcast] Database schema up to date ({} migration(s) already applied)",
                rows.len()
            ))
        } else {
            Some(format!(
                "[taskcast] Applied {} new migration(s): {}",
                newly_applied.len(),
                newly_applied.join(", ")
            ))
        }
    } else {
        None
    };

    migrate_pool.close().await;

    migrate_result.map_err(|e| -> BoxError {
        format!("[taskcast] Auto-migration failed: {e}").into()
    })?;

    if let Some(line) = log_line {
        eprintln!("{line}");
    }
    Ok(())
}
```

**Filename reconstruction caveat:** sqlx stores migration metadata as
`(version, description)` without the original filename. Rust reconstructs
filenames for log output using the project convention
`{version:03}_{description_with_underscores}.sql` (where `description`
comes from sqlx with underscores replaced by spaces, and Rust inverts that
replacement). This requires that all migration filenames follow the
**3-digit zero-padded** convention (`001_...`, `002_...`, etc.). The
codegen script in `packages/cli/scripts/generate-migrations.mjs` validates
this convention at build time: any filename not matching
`^\d{3}_[a-zA-Z0-9_]+\.sql$` causes the generator to fail, which in turn
fails the build. Existing migrations already follow the convention.

async fn build_postgres_long_term_store(
    postgres_url: Option<&str>,
    auto_migrate: bool,
    storage_mode: &str,
) -> Result<Option<Arc<dyn taskcast_core::LongTermStore>>, BoxError> {
    // 1. Auto-migrate decision
    if auto_migrate {
        match (storage_mode, postgres_url) {
            ("sqlite", _) | (_, None) => {
                eprintln!("[taskcast] TASKCAST_AUTO_MIGRATE is set but no Postgres configured — skipping");
            }
            (_, Some(url)) => {
                run_auto_migrate(url).await?;
            }
        }
    }

    // 2. Build production store (independent pool)
    if storage_mode != "sqlite" {
        if let Some(url) = postgres_url {
            let pool = sqlx::PgPool::connect(url).await?;
            let store = taskcast_postgres::PostgresLongTermStore::new(pool);
            return Ok(Some(Arc::new(store) as Arc<dyn taskcast_core::LongTermStore>));
        }
    }
    Ok(None)
}
```

In `start::run`, the two existing duplicated "if postgres_url then build
store" blocks (one in the `"redis"` match arm, one in the memory-fallback
arm) collapse into a single call:

```rust
let auto_migrate = parse_boolean_env(
    std::env::var("TASKCAST_AUTO_MIGRATE").ok().as_deref(),
);

// ... existing storage_mode resolution ...

let long_term_store = build_postgres_long_term_store(
    postgres_url.as_deref(),
    auto_migrate,
    storage_mode,
).await?;
```

The `?` operator at the call site ensures that migration failure propagates
up to `main::run_cli` which already prints and exits with a non-zero status
— fail-fast is preserved without any explicit `exit(1)` call.

### Behavioral parity table (TS ↔ Rust)

| Behavior | TS | Rust |
|---|---|---|
| Env var name | `TASKCAST_AUTO_MIGRATE` | `TASKCAST_AUTO_MIGRATE` |
| Truthy values `1`/`true`/`yes`/`on`, case-insensitive | `parseBooleanEnv` | `parse_boolean_env` |
| Enabled + no Postgres → info log, skip | ✓ | ✓ |
| Enabled + Postgres → run migrations | `runMigrations(POSTGRES_MIGRATIONS)` | `store.migrate()` (sqlx embedded) |
| Success → server starts | ✓ | ✓ |
| Failure → fail-fast exit | `process.exit(1)` | `Err(_)` bubbles to `main` |
| Temporary connection for DDL | `postgres()` + `.end()` | `PgPool` + `.close()` |
| Idempotency via `_sqlx_migrations` | ✓ (native) | ✓ (native) |
| Checksum mismatch → error | ✓ | ✓ |
| Shares `_sqlx_migrations` with the other runtime | ✓ | ✓ |

## Error Handling & Log Messages

### State matrix

| # | `TASKCAST_AUTO_MIGRATE` | Postgres configured | Migration state | Behavior | Log output | Exit |
|---|---|---|---|---|---|---|
| 1 | unset / empty / falsy | (any) | (not triggered) | Skip auto-migrate entirely | none | normal start |
| 2 | truthy | not configured / sqlite mode | (not triggered) | Skip with info log | `[taskcast] TASKCAST_AUTO_MIGRATE is set but no Postgres configured — skipping` | normal start |
| 3 | truthy | configured | all already applied | Run migrate, log "up to date" | banner + `[taskcast] Database schema up to date (N migration(s) already applied)` | normal start |
| 4 | truthy | configured | pending migrations, all succeed | Run migrate, log applied | banner + `[taskcast] Applied N new migration(s): 003_foo.sql, 004_bar.sql` | normal start |
| 5 | truthy | configured | connection error | Fail-fast | banner + `[taskcast] Auto-migration failed: <error>` | exit 1 |
| 6 | truthy | configured | checksum mismatch | Fail-fast | banner + `[taskcast] Auto-migration failed: <error>` | exit 1 |
| 7 | truthy | configured | dirty migration (`success=false` in `_sqlx_migrations`) | Fail-fast | banner + `[taskcast] Auto-migration failed: <error>` | exit 1 |
| 8 | truthy | configured | SQL execution error | Fail-fast | banner + `[taskcast] Auto-migration failed: <error>` | exit 1 |

Where `<banner>` is: `[taskcast] TASKCAST_AUTO_MIGRATE enabled — running Postgres migrations on <url>`

### Exact log strings (fixed for test assertions)

- Banner: `[taskcast] TASKCAST_AUTO_MIGRATE enabled — running Postgres migrations on <url>`
- Skip: `[taskcast] TASKCAST_AUTO_MIGRATE is set but no Postgres configured — skipping`
- Up to date: `[taskcast] Database schema up to date (<N> migration(s) already applied)`
- Applied: `[taskcast] Applied <N> new migration(s): <comma-separated filenames>`
- Failed: `[taskcast] Auto-migration failed: <error message>`

The `migration(s)` form is used in both "applied" and "up to date" to keep
plural logic out of both implementations (TS and Rust can avoid any
pluralization code).

### Output streams

- **TS:** info → `console.log` (stdout), errors → `console.error` (stderr),
  matching the existing `start.ts` convention
  ([packages/cli/src/commands/start.ts:93-95](../../../packages/cli/src/commands/start.ts#L93))
- **Rust:** all auto-migrate logs → `eprintln!` (stderr), matching the
  existing `start.rs` convention
  ([rust/taskcast-cli/src/commands/start.rs:86](../../../rust/taskcast-cli/src/commands/start.rs#L86))

This stream asymmetry is intentional — each runtime follows its own existing
convention. Unifying the stream choice across runtimes is out of scope.

### Error prefix strategy

The underlying error message from `runMigrations()` or `sqlx::migrate!` is
preserved verbatim and wrapped with the `[taskcast] Auto-migration failed:`
prefix. This keeps full diagnostic context (e.g., "Dirty migration found:
version 3 (add indexes). A previous migration failed. Please fix it manually
before running migrations.") while making it easy to grep logs for
auto-migrate failures specifically, distinguishing them from manual
`taskcast migrate` invocations.

### Temporary connection cleanup

**TS:** Because `process.exit(1)` is synchronous and skips `finally` blocks,
explicit cleanup is done in both success and failure paths without a
`finally` clause:

```ts
let migrateSql: ReturnType<typeof postgres> | undefined
try {
  migrateSql = createSql(url)
  // ... migrate + log ...
  await migrateSql.end()  // success path
} catch (err) {
  logger.error(...)
  if (migrateSql) await migrateSql.end().catch(() => {})  // best-effort
  throw err
}
```

**Rust:** The pool close must happen before the `?` operator to avoid
skipping cleanup on the error path:

```rust
let migrate_result = store.migrate().await;
migrate_pool.close().await;
migrate_result.map_err(|e| -> BoxError {
    format!("[taskcast] Auto-migration failed: {e}").into()
})?;
```

## Testing Strategy

### Unit tests

| File | What it tests |
|---|---|
| `packages/cli/tests/unit/parse-boolean-env.test.ts` | `parseBooleanEnv` across all truthy/falsy variants, whitespace, edge cases. Target: 100% branch coverage. |
| `packages/cli/tests/unit/generate-migrations.test.ts` | Code generator: happy path, sort order, byte-for-byte SQL content preservation, error on empty dir, **stale-file regression**: overwriting a pre-existing generated file produces the current SQL set (no stale entries). Target: 100%. |
| `packages/cli/tests/unit/generated-migrations.test.ts` | `POSTGRES_MIGRATIONS` contains every `.sql` file in `migrations/postgres/` and each embedded SQL matches the source byte-for-byte. (Monorepo-only test — reads from the repo's SQL directory.) |
| `packages/postgres/tests/unit/migration-runner.test.ts` | Existing tests continue to cover `parseMigrationFilename`, `computeChecksum`, `loadMigrationFiles`. Add tests for `buildMigrationFiles` (transforms `EmbeddedMigration[]` → `MigrationFile[]`, sorts, computes checksums). |
| `rust/taskcast-cli/src/helpers.rs` (inline `#[cfg(test)] mod tests`) | `parse_boolean_env` across all variants. Target: 100%. |

### Integration tests (testcontainers Postgres)

| File | What it tests |
|---|---|
| `packages/cli/tests/integration/auto-migrate.test.ts` (**NEW**) | Direct unit tests of `performAutoMigrateIfEnabled()` against a real Postgres container. 8 cases covering the full state matrix: happy path (new DB), idempotency (run twice), disabled, no Postgres URL, sqlite mode, connection failure, checksum mismatch, dirty migration. |
| `packages/postgres/tests/integration/migration-runner.test.ts` | Existing suite. Add coverage for the new `runMigrations(sql, MigrationFile[])` overload to ensure parity with the directory-based path. |
| `rust/taskcast-cli/tests/start_env_tests.rs` (**EXTEND**) | Add tests for the Rust auto-migrate path using existing `EnvGuard` + testcontainers pattern. Covers: happy path, idempotency, fail-fast on connection error, skip when no Postgres URL. |

### End-to-end tests (in-process)

| File | What it tests |
|---|---|
| `packages/cli/tests/integration/auto-migrate.test.ts` | After the direct helper tests above, add one case that calls the new exported `runStart({...})`, passes through env vars and a real Postgres testcontainer, asserts the `_sqlx_migrations` table is populated, hits `/health` → 200, then calls `stop()` for clean teardown. |
| `rust/taskcast-cli/tests/start_env_tests.rs` | Add one `#[tokio::test]` using `EnvGuard` + `tokio::spawn(start::run(StartArgs {...}))` + `find_available_port()` + a testcontainer Postgres, asserts the table is populated and `/health` responds, then `handle.abort()`. |

**Note:** Neither runtime spawns subprocess-based tests. Both use in-process
testing patterns already established in the codebase.

### Build/packaging regression tests

| Test | Location | What it catches |
|---|---|---|
| CI staleness check | `.github/workflows/ci.yml` | A PR modifies `migrations/postgres/*.sql` but forgets to regenerate `packages/cli/src/generated/postgres-migrations.ts`. Step: regenerate and `git diff --exit-code` on the file. |
| Coverage of the generator itself | `packages/cli/tests/unit/generate-migrations.test.ts` (above) | Breakage in the codegen script. |
| `POSTGRES_MIGRATIONS` matches source | `packages/cli/tests/unit/generated-migrations.test.ts` (above) | Any drift between the source SQL files and the embedded TS at dev time (CI-only, because the test reads from the monorepo layout). |

### Coverage targets

Per CLAUDE.md's 100% target:

| Module | Target |
|---|---|
| `packages/cli/src/utils.ts::parseBooleanEnv` | 100% |
| `packages/cli/src/auto-migrate.ts` | 100% |
| `packages/cli/scripts/generate-migrations.mjs` | 100% |
| `packages/cli/src/commands/start.ts` (new `runStart` function) | 100% (minus the HTTP `serve()` callback which is already a coverage-excluded boundary) |
| `packages/cli/src/commands/migrate.ts` (modified imports) | Existing tests continue to pass; add one smoke test exercising the new path |
| `packages/postgres/src/migration-runner.ts::buildMigrationFiles` | 100% |
| `packages/postgres/src/migration-runner.ts::runMigrations` (new array branch) | 100% (integration layer) |
| `rust/taskcast-cli/src/helpers.rs::parse_boolean_env` | 100% |
| `rust/taskcast-cli/src/commands/start.rs::run_auto_migrate` | 100% |
| `rust/taskcast-cli/src/commands/start.rs::build_postgres_long_term_store` | 100% |

### Coverage exclusion (user-approved)

- `packages/cli/src/generated/postgres-migrations.ts` — added to vitest
  `exclude` list. This file is pure data export with no executable logic;
  coverage metrics would be misleading. The file is validated by the
  staleness CI check and by `generated-migrations.test.ts` instead.

### New dependencies

| Package | Dev dependency added | Version |
|---|---|---|
| `@taskcast/cli` | `testcontainers` | `^10.13.0` (match `@taskcast/postgres`) |

Rust needs no new crates — `testcontainers-modules` is already in the
workspace via `taskcast-postgres`.

## Documentation Updates

| File | Change |
|---|---|
| `docs/guide/deployment.md` | (a) Add `TASKCAST_AUTO_MIGRATE` row to the environment variable table. (b) Add a new "Database migrations" section with manual and automatic subsections, including the warning about multi-instance rolling deploys. |
| `docs/guide/deployment.zh.md` | Chinese version of the same changes. |
| `packages/cli/README.md` | Add `TASKCAST_AUTO_MIGRATE` row to the env var table. (Short — no full migrations section; `deployment.md` is the canonical reference.) |
| `.changeset/auto-migrate.md` (**NEW**) | Release changeset. Marks `@taskcast/cli` and `@taskcast/postgres` as `minor`; fixed versioning will bump all other packages together. |

The design doc itself lives at
`docs/superpowers/specs/2026-04-08-auto-migrate-design.md` (this file).

Out of scope for doc updates: root `README.md` (high-level overview, no
env var details), `docs/api/` (HTTP API reference, unrelated),
`docs/skill/taskcast.md` (external users' skill, not deployment-focused),
and `docs/plans/` (historical plans, not where new specs live).

## Risks & Mitigations

| Risk | Likelihood | Impact | Mitigation |
|---|---|---|---|
| Generated TS file becomes stale vs. SQL sources | Medium | Migrations don't apply at runtime or wrong SQL runs | CI staleness check (`git diff --exit-code`) after re-running the generator. Also unit-tested. |
| Two replicas start simultaneously and race on the migration | Low in single-instance deploys | Migration locks row in `_sqlx_migrations`; second replica would see "already applied" on retry | Document recommendation to use manual `taskcast migrate` for multi-replica deploys. `runMigrations()` already wraps each migration in a transaction, so the race is safe but one replica may transiently fail. |
| Temporary connection pool not cleaned up on fail-fast | Low | Process is exiting anyway, connection cleanup by OS | Explicit `end()`/`close()` on both success and failure paths; best-effort on failure. |
| `parseBooleanEnv` misinterprets a user value | Very Low | User flag doesn't take effect as expected | Comprehensive unit tests cover all documented truthy/falsy values. Documentation specifies exactly which values are recognized. |
| `runMigrations()` API change breaks existing callers | None | — | The new signature is `string \| MigrationFile[]`; existing `string` callers continue to work. |
| CLI `build` script changes break existing developer workflows | Low | `pnpm build` behaves differently | The change is additive (`node scripts/... && tsc` instead of `tsc`); `pnpm lint`'s `tsc -b` is unchanged because the generated file is committed. Fresh clones work without a pre-build step. |
| Committing `src/generated/postgres-migrations.ts` causes merge conflicts | Low | Occasional conflict when two PRs add different migrations | Migrations are serialized by version number; simultaneous conflicting adds are rare and trivially resolved by choosing non-overlapping versions. |
| Auto-migrate silently succeeds against wrong database | Low | Wrong schema in wrong place | The user explicitly sets `TASKCAST_POSTGRES_URL`; auto-migrate uses the same URL as the production adapter. Log line prints the URL, making it visible in startup output. |

## Open Questions

None. All design decisions are resolved. This spec is ready for
implementation planning.

## Implementation Sequence (preview)

High-level ordering of work units; a detailed plan will be produced by the
`writing-plans` skill in the next step.

1. **Foundation — TS side:** extend `runMigrations` to accept
   `MigrationFile[]`, export `buildMigrationFiles`. Tests for both.
2. **Codegen — TS side:** create generator script, generate the initial
   `postgres-migrations.ts` file, commit it. Update `packages/cli/package.json`
   build script. Unit tests for generator.
3. **Helpers — both sides:** add `parseBooleanEnv` (TS) and
   `parse_boolean_env` (Rust) with unit tests.
4. **Auto-migrate helpers — both sides:** create `performAutoMigrateIfEnabled`
   (TS) and `run_auto_migrate`/`build_postgres_long_term_store` (Rust).
5. **Wire into `start` — both sides:** refactor TS `start.ts` to extract
   `runStart`; call `performAutoMigrateIfEnabled` before adapter creation.
   Rust: call `build_postgres_long_term_store` from `start::run`.
6. **Fix the manual `taskcast migrate` TS subcommand:** switch to embedded
   migrations, delete the monorepo-path TODO.
7. **Integration tests:** testcontainers-based tests for both runtimes,
   covering the state matrix.
8. **E2E tests:** in-process `runStart` / `start::run` tests with real
   Postgres containers.
9. **CI:** add the staleness check step.
10. **Documentation:** update `deployment.md`/`deployment.zh.md` and
    `packages/cli/README.md`.
11. **Changeset:** create `.changeset/auto-migrate.md`.

## References

- Existing migration runner: [packages/postgres/src/migration-runner.ts](../../../packages/postgres/src/migration-runner.ts)
- Existing manual migrate subcommand (with the TODO this fixes): [packages/cli/src/commands/migrate.ts](../../../packages/cli/src/commands/migrate.ts)
- TS start command: [packages/cli/src/commands/start.ts](../../../packages/cli/src/commands/start.ts)
- Rust postgres store with `migrate()`: [rust/taskcast-postgres/src/store.rs](../../../rust/taskcast-postgres/src/store.rs)
- Rust start command: [rust/taskcast-cli/src/commands/start.rs](../../../rust/taskcast-cli/src/commands/start.rs)
- Rust env-based test pattern (blueprint for new tests): [rust/taskcast-cli/tests/start_env_tests.rs](../../../rust/taskcast-cli/tests/start_env_tests.rs)
- Existing migration CLI design: [docs/plans/2026-03-06-migration-cli-plan.md](../../plans/2026-03-06-migration-cli-plan.md)

