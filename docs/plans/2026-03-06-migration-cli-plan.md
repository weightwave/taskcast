# Migration CLI Command — Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Add a `taskcast migrate` CLI subcommand (both Rust and TS) with unified Postgres migration files and a TS migration runner that is cross-compatible with sqlx's `_sqlx_migrations` table.

**Architecture:** Postgres migration SQL files live in a single location (`migrations/postgres/`). The TS `@taskcast/postgres` package gains a `runMigrations()` function that reads/writes the same `_sqlx_migrations` tracking table as sqlx. Both CLIs add a `migrate` subcommand with confirmation prompt, `--url`/config/env URL resolution, and `-y` to skip confirmation.

**Tech Stack:** TypeScript (postgres.js, node:crypto SHA-384), Rust (clap, sqlx), vitest + testcontainers for integration tests.

---

## Task 1: Move Postgres migration files to shared location

**Files:**
- Create: `migrations/postgres/001_initial.sql` (move from `packages/postgres/migrations/`)
- Create: `migrations/postgres/002_workers.sql` (move from `packages/postgres/migrations/`)
- Delete: `packages/postgres/migrations/001_initial.sql`
- Delete: `packages/postgres/migrations/002_workers.sql`
- Delete: `rust/taskcast-postgres/migrations/001_initial.sql`
- Delete: `rust/taskcast-postgres/migrations/002_workers.sql`

**Step 1: Create shared directory and move files**

```bash
mkdir -p migrations/postgres
cp packages/postgres/migrations/001_initial.sql migrations/postgres/
cp packages/postgres/migrations/002_workers.sql migrations/postgres/
```

**Step 2: Remove old directories**

```bash
rm -rf packages/postgres/migrations
rm -rf rust/taskcast-postgres/migrations
```

**Step 3: Commit**

```bash
git add -A migrations/postgres
git add packages/postgres/migrations rust/taskcast-postgres/migrations
git commit -m "refactor: move Postgres migrations to shared location at repo root"
```

---

## Task 2: Update Rust sqlx::migrate! path

**Files:**
- Modify: `rust/taskcast-postgres/src/store.rs:38` — change `sqlx::migrate!("./migrations")` to `sqlx::migrate!("../../migrations/postgres")`

**Step 1: Update path**

In `rust/taskcast-postgres/src/store.rs`, line 38, change:

```rust
// Before:
sqlx::migrate!("./migrations").run(&self.pool).await?;

// After:
sqlx::migrate!("../../migrations/postgres").run(&self.pool).await?;
```

**Step 2: Verify Rust builds**

```bash
cd rust && cargo check -p taskcast-postgres
```

Expected: compiles successfully (sqlx::migrate! validates at compile time that the directory exists and contains valid migrations).

**Step 3: Commit**

```bash
git add rust/taskcast-postgres/src/store.rs
git commit -m "fix: update sqlx migrate path to shared migrations/postgres"
```

---

## Task 3: Update TS Postgres test migration paths

**Files:**
- Modify: `packages/postgres/tests/long-term.test.ts:27-36` — update `readFileSync` paths to `../../migrations/postgres/`

**Step 1: Update test paths**

In `packages/postgres/tests/long-term.test.ts`, change the migration loading (around lines 27-36):

```typescript
// Before:
const migration001 = readFileSync(
  join(import.meta.dirname, '../migrations/001_initial.sql'),
  'utf8',
)
await sql.unsafe(migration001)
const migration002 = readFileSync(
  join(import.meta.dirname, '../migrations/002_workers.sql'),
  'utf8',
)
await sql.unsafe(migration002)

// After:
const migration001 = readFileSync(
  join(import.meta.dirname, '../../migrations/postgres/001_initial.sql'),
  'utf8',
)
await sql.unsafe(migration001)
const migration002 = readFileSync(
  join(import.meta.dirname, '../../migrations/postgres/002_workers.sql'),
  'utf8',
)
await sql.unsafe(migration002)
```

Note: `import.meta.dirname` resolves to `packages/postgres/tests/`, so `../../migrations/postgres/` reaches the repo root.

**Step 2: Run TS Postgres tests**

```bash
cd packages/postgres && pnpm test
```

Expected: all tests pass.

**Step 3: Commit**

```bash
git add packages/postgres/tests/long-term.test.ts
git commit -m "fix: update TS Postgres test migration paths to shared location"
```

---

## Task 4: Implement TS migration runner — core logic

**Files:**
- Create: `packages/postgres/src/migration-runner.ts`
- Modify: `packages/postgres/src/index.ts` — add export

The runner must produce `_sqlx_migrations` records identical to sqlx. Key details from sqlx source code:

- **Table**: `_sqlx_migrations (version BIGINT PK, description TEXT, installed_on TIMESTAMPTZ DEFAULT now(), success BOOLEAN, checksum BYTEA, execution_time BIGINT)`
- **Checksum**: `SHA-384` hash of the raw SQL file content (bytes), stored as `BYTEA`
- **Version**: parsed from filename prefix as integer (`001` → `1`)
- **Description**: filename part after first `_`, remove `.sql` suffix, replace remaining `_` with space. Example: `001_initial.sql` → `"initial"`, `002_workers.sql` → `"workers"`
- **execution_time**: inserted as `-1`, then updated to actual nanoseconds after execution
- **Insert SQL**: `INSERT INTO _sqlx_migrations (version, description, success, checksum, execution_time) VALUES ($1, $2, TRUE, $3, -1)`
- **Update SQL**: `UPDATE _sqlx_migrations SET execution_time = $1 WHERE version = $2`

**Step 1: Write unit tests for helper functions**

Create `packages/postgres/tests/unit/migration-runner.test.ts`:

```typescript
import { describe, it, expect } from 'vitest'
import { parseMigrationFilename, computeChecksum } from '../src/migration-runner.js'

describe('parseMigrationFilename', () => {
  it('parses standard migration filename', () => {
    const result = parseMigrationFilename('001_initial.sql')
    expect(result).toEqual({ version: 1, description: 'initial' })
  })

  it('parses multi-word description with underscores', () => {
    const result = parseMigrationFilename('002_add_worker_tables.sql')
    expect(result).toEqual({ version: 2, description: 'add worker tables' })
  })

  it('returns null for non-sql files', () => {
    expect(parseMigrationFilename('README.md')).toBeNull()
  })

  it('returns null for files without version prefix', () => {
    expect(parseMigrationFilename('initial.sql')).toBeNull()
  })

  it('returns null for files without description', () => {
    expect(parseMigrationFilename('001.sql')).toBeNull()
  })
})

describe('computeChecksum', () => {
  it('returns SHA-384 hash as Buffer', () => {
    const checksum = computeChecksum('SELECT 1;')
    expect(checksum).toBeInstanceOf(Buffer)
    expect(checksum.length).toBe(48) // SHA-384 = 384 bits = 48 bytes
  })

  it('produces consistent results', () => {
    const a = computeChecksum('CREATE TABLE foo (id INT);')
    const b = computeChecksum('CREATE TABLE foo (id INT);')
    expect(a).toEqual(b)
  })

  it('produces different results for different input', () => {
    const a = computeChecksum('SELECT 1;')
    const b = computeChecksum('SELECT 2;')
    expect(a).not.toEqual(b)
  })
})
```

**Step 2: Run tests, verify they fail**

```bash
cd packages/postgres && pnpm test -- tests/unit/migration-runner.test.ts
```

Expected: FAIL — module not found.

**Step 3: Implement migration-runner.ts**

Create `packages/postgres/src/migration-runner.ts`:

```typescript
import { createHash } from 'node:crypto'
import { readdirSync, readFileSync } from 'node:fs'
import { join } from 'node:path'
import type postgres from 'postgres'

export interface MigrationFile {
  version: number
  description: string
  sql: string
  checksum: Buffer
  filename: string
}

export interface MigrationResult {
  applied: string[]
  skipped: string[]
}

export function parseMigrationFilename(
  filename: string,
): { version: number; description: string } | null {
  const parts = filename.split(/_(.*)/s) // split on first underscore
  if (parts.length < 2 || !parts[1]?.endsWith('.sql')) return null

  const version = parseInt(parts[0]!, 10)
  if (isNaN(version)) return null

  const description = parts[1]!.replace(/\.sql$/, '').replaceAll('_', ' ')
  if (!description) return null

  return { version, description }
}

export function computeChecksum(sql: string): Buffer {
  return createHash('sha384').update(sql).digest()
}

export function loadMigrationFiles(migrationsDir: string): MigrationFile[] {
  const files = readdirSync(migrationsDir).filter((f) => f.endsWith('.sql')).sort()
  const migrations: MigrationFile[] = []

  for (const filename of files) {
    const parsed = parseMigrationFilename(filename)
    if (!parsed) continue

    const sql = readFileSync(join(migrationsDir, filename), 'utf8')
    migrations.push({
      ...parsed,
      sql,
      checksum: computeChecksum(sql),
      filename,
    })
  }

  return migrations
}

export async function runMigrations(
  sql: ReturnType<typeof postgres>,
  migrationsDir: string,
): Promise<MigrationResult> {
  // Ensure tracking table exists (exact sqlx schema)
  await sql.unsafe(`
CREATE TABLE IF NOT EXISTS _sqlx_migrations (
    version BIGINT PRIMARY KEY,
    description TEXT NOT NULL,
    installed_on TIMESTAMPTZ NOT NULL DEFAULT now(),
    success BOOLEAN NOT NULL,
    checksum BYTEA NOT NULL,
    execution_time BIGINT NOT NULL
)`)

  // Load local migration files
  const migrations = loadMigrationFiles(migrationsDir)

  // Check for dirty (failed) migrations
  const dirty = await sql`
    SELECT version FROM _sqlx_migrations WHERE success = false ORDER BY version LIMIT 1
  `
  if (dirty.length > 0) {
    throw new Error(
      `Migration version ${dirty[0]!.version} was previously applied but failed. ` +
        `Resolve the issue and remove the row from _sqlx_migrations before retrying.`,
    )
  }

  // Load applied migrations
  const applied = await sql<{ version: string; checksum: Buffer }[]>`
    SELECT version, checksum FROM _sqlx_migrations ORDER BY version
  `
  const appliedMap = new Map(applied.map((r) => [Number(r.version), r.checksum]))

  const result: MigrationResult = { applied: [], skipped: [] }

  for (const migration of migrations) {
    const existingChecksum = appliedMap.get(migration.version)

    if (existingChecksum) {
      // Verify checksum matches
      if (!Buffer.from(existingChecksum).equals(migration.checksum)) {
        throw new Error(
          `Checksum mismatch for migration ${migration.filename} (version ${migration.version}). ` +
            `The migration file has been modified after it was applied.`,
        )
      }
      result.skipped.push(migration.filename)
      continue
    }

    // Apply migration
    const start = performance.now()

    await sql.begin(async (tx) => {
      await tx.unsafe(migration.sql)
      await tx.unsafe(
        `INSERT INTO _sqlx_migrations (version, description, success, checksum, execution_time)
         VALUES ($1, $2, TRUE, $3, -1)`,
        [migration.version, migration.description, migration.checksum],
      )
    })

    const elapsedNs = Math.round((performance.now() - start) * 1_000_000)
    await sql.unsafe(
      `UPDATE _sqlx_migrations SET execution_time = $1 WHERE version = $2`,
      [elapsedNs, migration.version],
    )

    result.applied.push(migration.filename)
  }

  return result
}
```

**Step 4: Run unit tests, verify they pass**

```bash
cd packages/postgres && pnpm test -- tests/unit/migration-runner.test.ts
```

Expected: PASS.

**Step 5: Export from index.ts**

In `packages/postgres/src/index.ts`, add:

```typescript
export { runMigrations } from './migration-runner.js'
```

**Step 6: Commit**

```bash
git add packages/postgres/src/migration-runner.ts packages/postgres/src/index.ts packages/postgres/tests/unit/migration-runner.test.ts
git commit -m "feat(postgres): add sqlx-compatible migration runner"
```

---

## Task 5: Integration test — TS migration runner against real Postgres

**Files:**
- Create: `packages/postgres/tests/integration/migration-runner.test.ts`

**Step 1: Write integration test**

```typescript
import { describe, it, expect, beforeAll, afterAll } from 'vitest'
import postgres from 'postgres'
import { GenericContainer, type StartedTestContainer } from 'testcontainers'
import { join } from 'node:path'
import { runMigrations, computeChecksum } from '../../src/migration-runner.js'
import { readFileSync } from 'node:fs'

const MIGRATIONS_DIR = join(import.meta.dirname, '../../../../migrations/postgres')

let container: StartedTestContainer
let sql: ReturnType<typeof postgres>

beforeAll(async () => {
  container = await new GenericContainer('postgres:16-alpine')
    .withEnvironment({
      POSTGRES_USER: 'test',
      POSTGRES_PASSWORD: 'test',
      POSTGRES_DB: 'testdb',
    })
    .withExposedPorts(5432)
    .start()
  sql = postgres(
    `postgres://test:test@localhost:${container.getMappedPort(5432)}/testdb`,
  )
}, 120000)

afterAll(async () => {
  await sql.end()
  await container?.stop()
})

describe('runMigrations', () => {
  it('applies all migrations on fresh database', async () => {
    const result = await runMigrations(sql, MIGRATIONS_DIR)
    expect(result.applied).toEqual(['001_initial.sql', '002_workers.sql'])
    expect(result.skipped).toEqual([])

    // Verify tables exist
    const tables = await sql`
      SELECT tablename FROM pg_tables
      WHERE schemaname = 'public' AND tablename LIKE 'taskcast_%'
      ORDER BY tablename
    `
    expect(tables.map((r) => r.tablename)).toContain('taskcast_tasks')
    expect(tables.map((r) => r.tablename)).toContain('taskcast_events')
  })

  it('skips already-applied migrations on second run', async () => {
    const result = await runMigrations(sql, MIGRATIONS_DIR)
    expect(result.applied).toEqual([])
    expect(result.skipped).toEqual(['001_initial.sql', '002_workers.sql'])
  })

  it('writes _sqlx_migrations records with correct format', async () => {
    const rows = await sql`
      SELECT version, description, success, checksum, execution_time
      FROM _sqlx_migrations ORDER BY version
    `
    expect(rows.length).toBe(2)

    // Version 1: 001_initial.sql
    expect(Number(rows[0]!.version)).toBe(1)
    expect(rows[0]!.description).toBe('initial')
    expect(rows[0]!.success).toBe(true)
    expect(rows[0]!.execution_time).toBeGreaterThanOrEqual(0)

    // Verify checksum matches SHA-384 of file content
    const sql001 = readFileSync(join(MIGRATIONS_DIR, '001_initial.sql'), 'utf8')
    const expected001 = computeChecksum(sql001)
    expect(Buffer.from(rows[0]!.checksum)).toEqual(expected001)

    // Version 2: 002_workers.sql
    expect(Number(rows[1]!.version)).toBe(2)
    expect(rows[1]!.description).toBe('workers')
  })

  it('rejects tampered migration checksum', async () => {
    // Tamper with checksum in _sqlx_migrations
    await sql`UPDATE _sqlx_migrations SET checksum = '\\xdeadbeef' WHERE version = 1`

    await expect(runMigrations(sql, MIGRATIONS_DIR)).rejects.toThrow(
      /checksum mismatch/i,
    )

    // Restore correct checksum for subsequent tests
    const sql001 = readFileSync(join(MIGRATIONS_DIR, '001_initial.sql'), 'utf8')
    const correct = computeChecksum(sql001)
    await sql`UPDATE _sqlx_migrations SET checksum = ${correct} WHERE version = 1`
  })

  it('rejects dirty (failed) migration', async () => {
    await sql`UPDATE _sqlx_migrations SET success = false WHERE version = 1`

    await expect(runMigrations(sql, MIGRATIONS_DIR)).rejects.toThrow(
      /previously applied but failed/i,
    )

    // Restore
    await sql`UPDATE _sqlx_migrations SET success = true WHERE version = 1`
  })
})
```

**Step 2: Run integration test**

```bash
cd packages/postgres && pnpm test -- tests/integration/migration-runner.test.ts
```

Expected: PASS (requires Docker running for testcontainers).

**Step 3: Commit**

```bash
git add packages/postgres/tests/integration/migration-runner.test.ts
git commit -m "test(postgres): integration tests for sqlx-compatible migration runner"
```

---

## Task 6: Add `migrate` subcommand to TS CLI

**Files:**
- Modify: `packages/cli/src/index.ts` — add `migrate` command

**Step 1: Add the `migrate` command**

In `packages/cli/src/index.ts`, after the `status` command (around line 194), add:

```typescript
program
  .command('migrate')
  .description('Run Postgres database migrations')
  .option('--url <url>', 'Postgres connection URL (highest priority)')
  .option('-c, --config <path>', 'config file path')
  .option('-y, --yes', 'skip confirmation prompt', false)
  .action(async (options: { url?: string; config?: string; yes: boolean }) => {
    const { loadConfigFile } = await import('@taskcast/core')
    const { runMigrations } = await import('@taskcast/postgres')
    const { join, dirname } = await import('path')
    const { fileURLToPath } = await import('url')

    // 1. Resolve Postgres URL: --url > env > config
    let postgresUrl = options.url
    if (!postgresUrl) {
      postgresUrl = process.env['TASKCAST_POSTGRES_URL']
    }
    if (!postgresUrl) {
      const { config: fileConfig } = await loadConfigFile(options.config)
      postgresUrl = fileConfig.adapters?.longTermStore?.url
    }
    if (!postgresUrl) {
      console.error(
        '[taskcast] No Postgres URL found. Provide --url, set TASKCAST_POSTGRES_URL, or configure in config file.',
      )
      process.exit(1)
    }

    // 2. Parse and display target info
    let displayUrl: string
    try {
      const parsed = new URL(postgresUrl)
      displayUrl = `${parsed.hostname}:${parsed.port || '5432'}${parsed.pathname}`
    } catch {
      displayUrl = postgresUrl
    }
    console.log(`[taskcast] Target database: ${displayUrl}`)

    // 3. Resolve migrations directory
    const __dirname = dirname(fileURLToPath(import.meta.url))
    const migrationsDir = join(__dirname, '../../migrations/postgres')

    // 4. Connect and check pending migrations
    const pgSql = postgres(postgresUrl)
    try {
      const { loadMigrationFiles } = await import('@taskcast/postgres/migration-runner')

      // We need to check what's pending before running.
      // For simplicity, we do a dry-run check by loading files and querying applied.
      await pgSql.unsafe(`
CREATE TABLE IF NOT EXISTS _sqlx_migrations (
    version BIGINT PRIMARY KEY,
    description TEXT NOT NULL,
    installed_on TIMESTAMPTZ NOT NULL DEFAULT now(),
    success BOOLEAN NOT NULL,
    checksum BYTEA NOT NULL,
    execution_time BIGINT NOT NULL
)`)
      const applied = await pgSql`SELECT version FROM _sqlx_migrations ORDER BY version`
      const appliedVersions = new Set(applied.map((r) => Number(r.version)))
      const allFiles = (await import('@taskcast/postgres')).loadMigrationFiles
        ? loadMigrationFiles(migrationsDir)
        : []
      const pending = allFiles.filter((m) => !appliedVersions.has(m.version))

      if (pending.length === 0) {
        console.log('[taskcast] Database is up to date.')
        await pgSql.end()
        return
      }

      console.log(`[taskcast] Pending migrations:`)
      for (const m of pending) {
        console.log(`  - ${m.filename}`)
      }

      // 5. Confirmation
      if (!options.yes) {
        const ok = await promptConfirm(
          `Apply ${pending.length} migration(s) to ${displayUrl}? (Y/n) `,
        )
        if (!ok) {
          console.log('[taskcast] Migration cancelled.')
          await pgSql.end()
          return
        }
      }

      // 6. Run migrations
      const result = await runMigrations(pgSql, migrationsDir)
      for (const name of result.applied) {
        console.log(`  Applied ${name}`)
      }
      console.log(
        `[taskcast] Applied ${result.applied.length} migration(s) successfully.`,
      )
    } catch (err) {
      console.error(`[taskcast] Migration failed: ${(err as Error).message}`)
      process.exit(1)
    } finally {
      await pgSql.end()
    }
  })

function promptConfirm(message: string): Promise<boolean> {
  if (!process.stdin.isTTY) return Promise.resolve(false)
  return new Promise((resolve) => {
    const rl = createInterface({ input: process.stdin, output: process.stdout })
    rl.on('close', () => resolve(false))
    rl.question(message, (answer) => {
      const trimmed = answer.trim().toLowerCase()
      resolve(trimmed === '' || trimmed === 'y' || trimmed === 'yes')
      rl.close()
    })
  })
}
```

Note: The `promptConfirm` function is similar to the existing `promptCreateGlobalConfig` — consider refactoring both to share the helper. The `loadMigrationFiles` function must be exported from `@taskcast/postgres`.

**Step 2: Update @taskcast/postgres exports**

In `packages/postgres/src/index.ts`, ensure `loadMigrationFiles` is also exported:

```typescript
export { runMigrations, loadMigrationFiles } from './migration-runner.js'
```

**Step 3: Update @taskcast/postgres package.json exports**

The CLI needs to import `loadMigrationFiles` from the package. Since both `runMigrations` and `loadMigrationFiles` are exported from the main entry point, no additional `exports` map entry is needed.

**Step 4: Ensure `migrations/postgres/` is included in CLI's published files**

In `packages/cli/package.json`, the `files` field currently only includes `dist`. Since the CLI reads migrations at runtime, the migration files must be accessible. The CLI uses a path relative to `__dirname` (`../../migrations/postgres`), which works in the monorepo but NOT when published to npm.

For npm publishing, add the migrations to the CLI package:

```json
{
  "files": [
    "dist",
    "migrations",
    "LICENSE",
    "README.md"
  ]
}
```

And add a build step or postinstall that copies `migrations/postgres/` into `packages/cli/migrations/postgres/`. Alternatively, adjust the runtime path to resolve correctly in both monorepo and published contexts. The simplest approach: have the migration runner accept the directory as a parameter (it already does), and at CLI build time, copy migrations into the dist.

For now (monorepo use), the relative path `../../migrations/postgres` works. Add a TODO comment for npm publish support.

**Step 5: Build and verify**

```bash
pnpm build
```

Expected: builds without errors.

**Step 6: Commit**

```bash
git add packages/cli/src/index.ts packages/postgres/src/index.ts
git commit -m "feat(cli): add migrate subcommand to TS CLI"
```

---

## Task 7: Add `migrate` subcommand to Rust CLI

**Files:**
- Modify: `rust/taskcast-cli/src/main.rs` — add `Migrate` variant to `Commands` enum and handler

**Step 1: Add Migrate command definition**

In `rust/taskcast-cli/src/main.rs`, add to the `Commands` enum (after `Status`):

```rust
/// Run Postgres database migrations
Migrate {
    /// Postgres connection URL (highest priority)
    #[arg(long)]
    url: Option<String>,
    /// Config file path
    #[arg(short, long)]
    config: Option<String>,
    /// Skip confirmation prompt
    #[arg(short, long)]
    yes: bool,
},
```

**Step 2: Add handler in match block**

In the `match cmd { ... }` block (before the closing `}`), add:

```rust
Commands::Migrate { url, config, yes } => {
    // 1. Resolve Postgres URL: --url > env > config
    let postgres_url = url
        .or_else(|| std::env::var("TASKCAST_POSTGRES_URL").ok())
        .or_else(|| {
            let file_config = taskcast_core::config::load_config_file(config.as_deref())
                .unwrap_or_default();
            file_config.adapters?.long_term_store?.url
        });

    let postgres_url = match postgres_url {
        Some(u) => u,
        None => {
            eprintln!("[taskcast] No Postgres URL found. Provide --url, set TASKCAST_POSTGRES_URL, or configure in config file.");
            std::process::exit(1);
        }
    };

    // 2. Display target info
    let display_url = if let Ok(parsed) = url::Url::parse(&postgres_url) {
        format!(
            "{}:{}{}",
            parsed.host_str().unwrap_or("localhost"),
            parsed.port().unwrap_or(5432),
            parsed.path()
        )
    } else {
        postgres_url.clone()
    };
    eprintln!("[taskcast] Target database: {display_url}");

    // 3. Connect
    let pool = sqlx::PgPool::connect(&postgres_url).await?;
    let store = taskcast_postgres::PostgresLongTermStore::new(pool.clone());

    // 4. Check pending (sqlx handles this internally, but we need to show info)
    // Run ensure_migrations_table + list applied to show pending count
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS _sqlx_migrations (
            version BIGINT PRIMARY KEY,
            description TEXT NOT NULL,
            installed_on TIMESTAMPTZ NOT NULL DEFAULT now(),
            success BOOLEAN NOT NULL,
            checksum BYTEA NOT NULL,
            execution_time BIGINT NOT NULL
        )",
    )
    .execute(&pool)
    .await?;

    let applied: Vec<(i64,)> =
        sqlx::query_as("SELECT version FROM _sqlx_migrations ORDER BY version")
            .fetch_all(&pool)
            .await?;
    let applied_versions: std::collections::HashSet<i64> =
        applied.iter().map(|r| r.0).collect();

    // sqlx::migrate! embeds files at compile time, so we can inspect them
    let migrator = sqlx::migrate!("../../migrations/postgres");
    let pending: Vec<_> = migrator
        .iter()
        .filter(|m| !applied_versions.contains(&m.version))
        .collect();

    if pending.is_empty() {
        eprintln!("[taskcast] Database is up to date.");
        pool.close().await;
        return Ok(());
    }

    eprintln!("[taskcast] Pending migrations:");
    for m in &pending {
        eprintln!("  - {:03}_{}.sql", m.version, m.description);
    }

    // 5. Confirmation
    if !yes {
        eprint!(
            "Apply {} migration(s) to {}? (Y/n) ",
            pending.len(),
            display_url
        );
        let mut input = String::new();
        std::io::stdin().read_line(&mut input)?;
        let trimmed = input.trim().to_lowercase();
        if !(trimmed.is_empty() || trimmed == "y" || trimmed == "yes") {
            eprintln!("[taskcast] Migration cancelled.");
            pool.close().await;
            return Ok(());
        }
    }

    // 6. Run migrations
    store.migrate().await.map_err(|e| {
        format!("Migration failed: {e}")
    })?;

    eprintln!(
        "[taskcast] Applied {} migration(s) successfully.",
        pending.len()
    );
    pool.close().await;
}
```

**Step 3: Add `url` crate dependency**

In `rust/taskcast-cli/Cargo.toml`, add:

```toml
url = "2"
```

**Step 4: Add tests for CLI parsing**

In the `#[cfg(test)] mod tests` block, add:

```rust
#[test]
fn cli_migrate_subcommand_parses() {
    let cli = Cli::parse_from(["taskcast", "migrate", "--url", "postgres://localhost/db"]);
    match cli.command.unwrap() {
        Commands::Migrate { url, config, yes } => {
            assert_eq!(url, Some("postgres://localhost/db".to_string()));
            assert!(config.is_none());
            assert!(!yes);
        }
        _ => panic!("expected Migrate command"),
    }
}

#[test]
fn cli_migrate_with_yes_flag() {
    let cli = Cli::parse_from(["taskcast", "migrate", "-y", "--url", "postgres://localhost/db"]);
    match cli.command.unwrap() {
        Commands::Migrate { yes, .. } => {
            assert!(yes);
        }
        _ => panic!("expected Migrate command"),
    }
}

#[test]
fn cli_migrate_with_config_flag() {
    let cli = Cli::parse_from(["taskcast", "migrate", "-c", "/etc/taskcast.yaml"]);
    match cli.command.unwrap() {
        Commands::Migrate { config, url, .. } => {
            assert_eq!(config, Some("/etc/taskcast.yaml".to_string()));
            assert!(url.is_none());
        }
        _ => panic!("expected Migrate command"),
    }
}
```

**Step 5: Build and test**

```bash
cd rust && cargo test -p taskcast-cli && cargo check -p taskcast-cli
```

Expected: all tests pass, builds successfully.

**Step 6: Commit**

```bash
git add rust/taskcast-cli/src/main.rs rust/taskcast-cli/Cargo.toml
git commit -m "feat(cli): add migrate subcommand to Rust CLI"
```

---

## Task 8: Cross-compatibility integration test

**Files:**
- Create: `packages/postgres/tests/integration/migration-compat.test.ts`

This test verifies that TS migration runner writes `_sqlx_migrations` records that sqlx would accept, and vice versa.

**Step 1: Write cross-compat test**

```typescript
import { describe, it, expect, beforeAll, afterAll } from 'vitest'
import postgres from 'postgres'
import { GenericContainer, type StartedTestContainer } from 'testcontainers'
import { join } from 'node:path'
import { readFileSync } from 'node:fs'
import { runMigrations, computeChecksum } from '../../src/migration-runner.js'

const MIGRATIONS_DIR = join(import.meta.dirname, '../../../../migrations/postgres')

let container: StartedTestContainer
let sql: ReturnType<typeof postgres>

beforeAll(async () => {
  container = await new GenericContainer('postgres:16-alpine')
    .withEnvironment({
      POSTGRES_USER: 'test',
      POSTGRES_PASSWORD: 'test',
      POSTGRES_DB: 'testdb',
    })
    .withExposedPorts(5432)
    .start()
  sql = postgres(
    `postgres://test:test@localhost:${container.getMappedPort(5432)}/testdb`,
  )
}, 120000)

afterAll(async () => {
  await sql.end()
  await container?.stop()
})

describe('sqlx cross-compatibility', () => {
  it('TS runner recognizes sqlx-style pre-applied migrations', async () => {
    // Simulate what sqlx would have written
    await sql.unsafe(`
      CREATE TABLE IF NOT EXISTS _sqlx_migrations (
        version BIGINT PRIMARY KEY,
        description TEXT NOT NULL,
        installed_on TIMESTAMPTZ NOT NULL DEFAULT now(),
        success BOOLEAN NOT NULL,
        checksum BYTEA NOT NULL,
        execution_time BIGINT NOT NULL
      )
    `)

    // Apply migration 001 "as sqlx would"
    const sql001 = readFileSync(join(MIGRATIONS_DIR, '001_initial.sql'), 'utf8')
    await sql.unsafe(sql001) // execute the DDL
    const checksum001 = computeChecksum(sql001)
    await sql.unsafe(
      `INSERT INTO _sqlx_migrations (version, description, success, checksum, execution_time)
       VALUES ($1, $2, TRUE, $3, 12345)`,
      [1, 'initial', checksum001],
    )

    // Now run TS runner — it should skip 001 and apply 002
    const result = await runMigrations(sql, MIGRATIONS_DIR)
    expect(result.skipped).toEqual(['001_initial.sql'])
    expect(result.applied).toEqual(['002_workers.sql'])
  })

  it('TS-written records have correct sqlx field format', async () => {
    // The previous test applied 002 via TS runner. Verify the record format.
    const rows = await sql`
      SELECT version, description, success, checksum, execution_time
      FROM _sqlx_migrations
      WHERE version = 2
    `
    expect(rows.length).toBe(1)
    const row = rows[0]!

    // version is integer
    expect(Number(row.version)).toBe(2)
    // description: filename "002_workers.sql" -> "workers" (strip prefix, strip .sql, replace _ with space)
    expect(row.description).toBe('workers')
    // success is true
    expect(row.success).toBe(true)
    // checksum matches SHA-384 of file content
    const sql002 = readFileSync(join(MIGRATIONS_DIR, '002_workers.sql'), 'utf8')
    expect(Buffer.from(row.checksum)).toEqual(computeChecksum(sql002))
    // execution_time is non-negative (was updated from -1)
    expect(Number(row.execution_time)).toBeGreaterThanOrEqual(0)
  })
})
```

**Step 2: Run test**

```bash
cd packages/postgres && pnpm test -- tests/integration/migration-compat.test.ts
```

Expected: PASS.

**Step 3: Commit**

```bash
git add packages/postgres/tests/integration/migration-compat.test.ts
git commit -m "test(postgres): cross-compatibility test for TS/sqlx migration records"
```

---

## Task 9: Update existing test to use migration runner

**Files:**
- Modify: `packages/postgres/tests/long-term.test.ts:26-36` — use `runMigrations` instead of raw SQL

**Step 1: Refactor test setup**

In `packages/postgres/tests/long-term.test.ts`, replace the manual migration execution with the new runner:

```typescript
// Before (lines 26-36):
const migration001 = readFileSync(...)
await sql.unsafe(migration001)
const migration002 = readFileSync(...)
await sql.unsafe(migration002)

// After:
import { runMigrations } from '../src/migration-runner.js'
const migrationsDir = join(import.meta.dirname, '../../../migrations/postgres')
await runMigrations(sql, migrationsDir)
```

Remove the now-unused `readFileSync` and `join` imports if they're not used elsewhere in the file.

**Step 2: Run tests**

```bash
cd packages/postgres && pnpm test
```

Expected: all tests pass.

**Step 3: Commit**

```bash
git add packages/postgres/tests/long-term.test.ts
git commit -m "refactor(postgres): use migration runner in long-term store tests"
```

---

## Summary

| Task | Description | Estimated |
|------|-------------|-----------|
| 1 | Move migration files to shared location | 2 min |
| 2 | Update Rust sqlx::migrate! path | 2 min |
| 3 | Update TS test migration paths | 3 min |
| 4 | TS migration runner — core logic + unit tests | 15 min |
| 5 | TS migration runner — integration test | 5 min |
| 6 | TS CLI `migrate` subcommand | 10 min |
| 7 | Rust CLI `migrate` subcommand | 10 min |
| 8 | Cross-compatibility integration test | 5 min |
| 9 | Update existing test to use runner | 3 min |

Tasks 1-3 must be sequential (file moves before path updates). Tasks 4-5 depend on 1. Tasks 6-7 depend on 4. Task 8 depends on 4. Task 9 depends on 4.
