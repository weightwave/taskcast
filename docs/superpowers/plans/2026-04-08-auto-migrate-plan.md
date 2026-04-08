# Auto-Migrate Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers-extended-cc:subagent-driven-development (recommended) or superpowers-extended-cc:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add opt-in automatic PostgreSQL migration on server startup via `TASKCAST_AUTO_MIGRATE` env var, symmetric across the Node.js and Rust CLIs, and fix the pre-existing npm-packaging TODO in `taskcast migrate` as a prerequisite.

**Architecture:** TS CLI uses build-time TS codegen to embed `migrations/postgres/*.sql` into the published package (eliminates stale-file risk and runtime FS dependencies). Rust CLI leverages `sqlx::migrate!` compile-time embedding (already in place). Both runtimes share the `_sqlx_migrations` tracking table. Auto-migrate logic is extracted into a dedicated helper so it is unit-testable, integration-testable, and callable from an in-process E2E test that boots the full `start` flow.

**Tech Stack:** TypeScript (ESM), Rust (tokio/axum/sqlx), vitest + testcontainers@10 (TS), cargo test + testcontainers-rs (Rust), commander (TS CLI), clap (Rust CLI), PostgreSQL 15+.

**Spec:** [docs/superpowers/specs/2026-04-08-auto-migrate-design.md](../specs/2026-04-08-auto-migrate-design.md)

---

## File Structure Overview

**Created (new files):**

| Path | Purpose | Lang |
|---|---|---|
| `packages/cli/scripts/generate-migrations.mjs` | Build-time codegen; reads `migrations/postgres/*.sql`, writes an embedded TS module | Node ESM |
| `packages/cli/src/generated/postgres-migrations.ts` | **Committed to git.** Generated data module — array of `{filename, sql}` for each migration | TS |
| `packages/cli/src/auto-migrate.ts` | `performAutoMigrateIfEnabled(options, deps?)` — DI-friendly helper | TS |
| `packages/cli/tests/unit/parse-boolean-env.test.ts` | Unit tests for the boolean env parser | TS |
| `packages/cli/tests/unit/generate-migrations.test.ts` | Unit tests for the codegen script (incl. stale-file regression) | TS |
| `packages/cli/tests/unit/generated-migrations.test.ts` | Monorepo-only test: generated TS matches source SQL byte-for-byte | TS |
| `packages/cli/tests/integration/auto-migrate.test.ts` | testcontainers Postgres integration tests + in-process E2E | TS |
| `.changeset/auto-migrate.md` | Release changeset marking `@taskcast/cli` and `@taskcast/postgres` minor | Markdown |

**Modified:**

| Path | Change |
|---|---|
| `packages/postgres/src/migration-runner.ts` | Add `EmbeddedMigration` type + `buildMigrationFiles` helper; extend `runMigrations` to accept `string \| MigrationFile[]` |
| `packages/postgres/src/index.ts` | Re-export `buildMigrationFiles`, `EmbeddedMigration` |
| `packages/postgres/tests/unit/migration-runner.test.ts` | Add unit tests for `buildMigrationFiles` |
| `packages/postgres/tests/integration/migration-runner.test.ts` | Add integration test for `runMigrations(sql, MigrationFile[])` overload |
| `packages/cli/package.json` | Add `testcontainers` devDep; `build` script prepends codegen step |
| `packages/cli/src/utils.ts` | Add exported `parseBooleanEnv(value): boolean` |
| `packages/cli/src/commands/start.ts` | Extract action body into exported `runStart(options)`; call `performAutoMigrateIfEnabled` before adapter creation |
| `packages/cli/src/commands/migrate.ts` | Replace monorepo-relative path with `POSTGRES_MIGRATIONS`; delete the "works in monorepo only" TODO |
| `packages/cli/vitest.config.ts` | Add `src/generated/**` to coverage exclude |
| `rust/taskcast-cli/src/helpers.rs` | Add `parse_boolean_env(Option<&str>) -> bool` + inline tests |
| `rust/taskcast-cli/src/commands/start.rs` | Read `TASKCAST_AUTO_MIGRATE` via parser; add `run_auto_migrate` and `build_postgres_long_term_store` helpers; collapse duplicate Postgres-adapter blocks |
| `rust/taskcast-cli/tests/start_env_tests.rs` | Add auto-migrate tests using existing `EnvGuard` + testcontainers pattern |
| `docs/guide/deployment.md` | Env var table row + new "Database migrations" section |
| `docs/guide/deployment.zh.md` | Chinese mirror of deployment.md changes |
| `packages/cli/README.md` | Env var table row |
| `.github/workflows/ci.yml` | New step: regenerate migrations file, `git diff --exit-code` |

## Task Dependency Graph

```
T0 (runMigrations overload + buildMigrationFiles)   ── root
T1 (parseBooleanEnv TS + parse_boolean_env Rust)    ── root
T2 (codegen + generated file)                       ── root

T0 ─┐
T1 ─┤
T2 ─┴→ T3 (auto-migrate helper — uses all three)
       └→ T4 (runStart extraction + wiring)
           └→ T7 (TS integration tests)

T0 ─┐
T2 ─┴→ T5 (fix migrate subcommand — uses buildMigrationFiles + POSTGRES_MIGRATIONS)

T1 ──→ T6 (Rust run_auto_migrate + build_postgres_long_term_store)
       └→ T8 (Rust integration tests)

T9  (CI staleness check) — after T2, T4, T6
T10 (docs + changeset)   — after all code tasks
```

T0, T1, T2 are all roots with no inter-dependencies and can be worked in parallel.
T6 (Rust) can be done in parallel with the entire TS chain (T3 → T4 → T7) as long as T1 is done.

---

## Task 0: Extend `@taskcast/postgres` — array overload + `buildMigrationFiles`

**Goal:** Expose a way to call `runMigrations()` with pre-loaded migration content (needed by the CLI's embedded-migrations path) while keeping the existing directory-based API intact for backward compatibility.

**Files:**
- Modify: `packages/postgres/src/migration-runner.ts`
- Modify: `packages/postgres/src/index.ts`
- Modify: `packages/postgres/tests/unit/migration-runner.test.ts`
- Test: `packages/postgres/tests/integration/migration-runner.test.ts` (add new case)

**Acceptance Criteria:**
- [ ] New `EmbeddedMigration` interface exported from `@taskcast/postgres` root (`{ filename: string; sql: string }`)
- [ ] New `buildMigrationFiles(embedded)` helper exported; parses version/description via existing `parseMigrationFilename`, computes `checksum` via existing `computeChecksum`, sorts by version
- [ ] `runMigrations(sql, migrationsOrFiles)` accepts both `string` and `MigrationFile[]`; array input is defensively re-sorted (no in-place mutation)
- [ ] All existing `runMigrations` tests still pass (directory path path unchanged)
- [ ] New unit tests cover `buildMigrationFiles`: sort order, filename validation, checksum consistency with `loadMigrationFiles`
- [ ] New integration test verifies `runMigrations(sql, files)` against a real Postgres container produces the same result as `runMigrations(sql, dir)` for the same input

**Verify:**
```sh
cd packages/postgres && pnpm test
# All unit and integration tests pass
```

**Steps:**

- [ ] **Step 1: Add `EmbeddedMigration` + `buildMigrationFiles` to `migration-runner.ts`**

In `packages/postgres/src/migration-runner.ts`, add after `MigrationResult` interface:

```ts
/**
 * A migration entry whose SQL content has already been loaded into memory
 * (e.g., by a build-time codegen step). Used as an alternative to
 * `loadMigrationFiles()` for runtimes that don't have filesystem access.
 */
export interface EmbeddedMigration {
  filename: string
  sql: string
}

/**
 * Convert embedded migrations (filename + raw SQL) into the internal
 * MigrationFile representation. Parses each filename for version/description,
 * computes SHA-384 checksums, and returns the result sorted by version.
 *
 * Filenames that don't match `{version}_{description}.sql` are skipped
 * silently (matching `loadMigrationFiles` behavior).
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
```

- [ ] **Step 2: Extend `runMigrations` signature**

Replace the signature and first few lines of the existing `runMigrations` in `packages/postgres/src/migration-runner.ts`:

```ts
/**
 * Run pending migrations and verify checksums of already-applied ones.
 *
 * Accepts either a filesystem directory path (for monorepo/dev use) or a
 * pre-loaded array of MigrationFile entries (for runtimes with no
 * filesystem access, e.g., bundled CLIs with embedded migrations).
 *
 * This is fully compatible with sqlx's _sqlx_migrations table — the Rust
 * server and the TS server can share the same database and track migrations
 * in the same table.
 */
export async function runMigrations(
  sql: ReturnType<typeof postgres>,
  migrationsOrFiles: string | MigrationFile[],
): Promise<MigrationResult> {
  // 1. Ensure the migrations table exists
  await sql.unsafe(CREATE_MIGRATIONS_TABLE)

  // 2. Check for dirty migrations (success = false)
  const dirty = await sql.unsafe(
    'SELECT version, description FROM _sqlx_migrations WHERE success = false',
  )
  if (dirty.length > 0) {
    const row = dirty[0]!
    throw new Error(
      `Dirty migration found: version ${row['version']} (${row['description']}). ` +
      'A previous migration failed. Please fix it manually before running migrations.',
    )
  }

  // 3. Load local files and query applied migrations
  const localFiles = typeof migrationsOrFiles === 'string'
    ? loadMigrationFiles(migrationsOrFiles)
    : [...migrationsOrFiles].sort((a, b) => a.version - b.version)
  const appliedRows = await sql.unsafe('SELECT version, checksum FROM _sqlx_migrations ORDER BY version')
  // ... rest unchanged
```

**Do not change** any code after the `appliedRows` line — the migration-processing loop continues to work identically for both input types.

- [ ] **Step 3: Re-export from package index**

In `packages/postgres/src/index.ts`, update the existing re-export line:

```ts
export { PostgresLongTermStore } from './long-term.js'
export { runMigrations, loadMigrationFiles, buildMigrationFiles } from './migration-runner.js'
export type { EmbeddedMigration, MigrationFile, MigrationResult } from './migration-runner.js'
```

(Add `buildMigrationFiles` to the runtime export line and add a new type-only export line for `EmbeddedMigration`, `MigrationFile`, `MigrationResult`.)

- [ ] **Step 4: Write unit tests for `buildMigrationFiles` (red)**

Append to `packages/postgres/tests/unit/migration-runner.test.ts`:

```ts
import { buildMigrationFiles, loadMigrationFiles } from '../../src/migration-runner.js'

describe('buildMigrationFiles', () => {
  it('converts embedded entries to MigrationFile[], sorted by version', () => {
    const result = buildMigrationFiles([
      { filename: '003_foo.sql', sql: 'CREATE TABLE a (id INT);' },
      { filename: '001_initial.sql', sql: 'CREATE TABLE b (id INT);' },
      { filename: '002_bar.sql', sql: 'CREATE TABLE c (id INT);' },
    ])

    expect(result.map((f) => f.version)).toEqual([1, 2, 3])
    expect(result[0]!.filename).toBe('001_initial.sql')
    expect(result[0]!.description).toBe('initial')
    expect(result[1]!.description).toBe('bar')
    expect(result[2]!.description).toBe('foo')
  })

  it('skips entries that do not match the filename format', () => {
    const result = buildMigrationFiles([
      { filename: '001_ok.sql', sql: 'SELECT 1;' },
      { filename: 'readme.md', sql: 'not sql' },
      { filename: 'nodot', sql: 'also not sql' },
    ])
    expect(result).toHaveLength(1)
    expect(result[0]!.filename).toBe('001_ok.sql')
  })

  it('computes the same checksum as loadMigrationFiles for identical content', () => {
    const { mkdtempSync, writeFileSync } = require('node:fs') as typeof import('node:fs')
    const { join } = require('node:path') as typeof import('node:path')
    const { tmpdir } = require('node:os') as typeof import('node:os')

    const dir = mkdtempSync(join(tmpdir(), 'mig-'))
    writeFileSync(join(dir, '001_initial.sql'), 'CREATE TABLE t (id INT);')
    const fromDir = loadMigrationFiles(dir)

    const fromEmbedded = buildMigrationFiles([
      { filename: '001_initial.sql', sql: 'CREATE TABLE t (id INT);' },
    ])

    expect(fromEmbedded[0]!.checksum.equals(fromDir[0]!.checksum)).toBe(true)
  })

  it('does not mutate the input array', () => {
    const input = [
      { filename: '002_bar.sql', sql: 'SELECT 2;' },
      { filename: '001_foo.sql', sql: 'SELECT 1;' },
    ]
    const snapshot = [...input]
    buildMigrationFiles(input)
    expect(input).toEqual(snapshot)
  })
})
```

- [ ] **Step 5: Run unit tests — should fail with "buildMigrationFiles is not a function"**

```sh
cd packages/postgres && pnpm test tests/unit/migration-runner.test.ts
```

Expected: FAIL — `buildMigrationFiles` not found. This confirms the test is reaching the unimplemented code.

- [ ] **Step 6: Implement (Steps 1–3 above). Re-run unit tests — should pass**

```sh
cd packages/postgres && pnpm test tests/unit/migration-runner.test.ts
```

Expected: PASS — all `buildMigrationFiles` tests plus the existing `parseMigrationFilename` / `loadMigrationFiles` tests.

- [ ] **Step 7: Add integration test for the array overload of `runMigrations`**

Append to `packages/postgres/tests/integration/migration-runner.test.ts`:

```ts
describe('runMigrations with pre-loaded MigrationFile[]', () => {
  it('produces the same result as the directory-based call on a fresh DB', async () => {
    // Assumes testcontainers setup from existing suite — reuse the `sql` fixture
    const files = buildMigrationFiles([
      { filename: '001_initial.sql', sql: readFileSync(join(MIGRATIONS_DIR, '001_initial.sql'), 'utf8') },
      { filename: '002_workers.sql', sql: readFileSync(join(MIGRATIONS_DIR, '002_workers.sql'), 'utf8') },
    ])

    const result = await runMigrations(sql, files)
    expect(result.applied).toEqual(['001_initial.sql', '002_workers.sql'])
    expect(result.skipped).toEqual([])

    // Second call: idempotent
    const second = await runMigrations(sql, files)
    expect(second.applied).toEqual([])
    expect(second.skipped).toEqual(['001_initial.sql', '002_workers.sql'])
  })
})
```

(Place this `describe` inside the existing integration suite so it shares the container setup. Adjust the `buildMigrationFiles` input list to match whatever files are actually in `migrations/postgres/` at implementation time — use `readdirSync` if you want to auto-discover.)

- [ ] **Step 8: Run integration tests**

```sh
cd packages/postgres && pnpm test tests/integration/migration-runner.test.ts
```

Expected: PASS — including both existing directory-based cases and the new array-based case. This requires Docker/testcontainers.

- [ ] **Step 9: Commit**

```bash
git add packages/postgres/src/migration-runner.ts \
        packages/postgres/src/index.ts \
        packages/postgres/tests/unit/migration-runner.test.ts \
        packages/postgres/tests/integration/migration-runner.test.ts
git commit -m "$(cat <<'EOF'
feat(postgres): accept pre-loaded MigrationFile[] in runMigrations

Add buildMigrationFiles() helper and extend runMigrations() to accept
either a directory path or a pre-loaded array. Enables the CLI's
embedded-migrations distribution path without breaking existing callers.

Co-Authored-By: Claude Opus 4.6 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 1: Boolean env var parser (TS + Rust)

**Goal:** Add a shared-spirit `parseBooleanEnv` helper in both runtimes that recognizes `1` / `true` / `yes` / `on` (case-insensitive, trimmed) as truthy. Fully unit-tested. This helper is the foundation that both the TS auto-migrate (T3) and Rust auto-migrate (T6) wiring depend on.

**Files:**
- Modify: `packages/cli/src/utils.ts`
- Create: `packages/cli/tests/unit/parse-boolean-env.test.ts`
- Modify: `rust/taskcast-cli/src/helpers.rs`

**Acceptance Criteria:**
- [ ] TS `parseBooleanEnv(undefined)` returns `false`
- [ ] TS `parseBooleanEnv('')` returns `false`
- [ ] TS `parseBooleanEnv('1')`, `'true'`, `'TRUE'`, `'True'`, `'yes'`, `'YES'`, `'on'`, `'ON'` all return `true`
- [ ] TS `parseBooleanEnv('0')`, `'false'`, `'no'`, `'off'`, `'maybe'`, `'2'` all return `false`
- [ ] TS handles whitespace: `' 1 '`, `'\ttrue\n'` both return `true`
- [ ] Rust `parse_boolean_env` behaves identically for the same inputs (as `Option<&str>`)
- [ ] 100% branch coverage on both helpers

**Verify:**
```sh
cd packages/cli && pnpm test tests/unit/parse-boolean-env.test.ts
cd rust/taskcast-cli && cargo test helpers::tests::boolean
```

**Steps:**

- [ ] **Step 1: Write TS unit tests (red)**

Create `packages/cli/tests/unit/parse-boolean-env.test.ts`:

```ts
import { describe, it, expect } from 'vitest'
import { parseBooleanEnv } from '../../src/utils.js'

describe('parseBooleanEnv', () => {
  it('returns false for undefined', () => {
    expect(parseBooleanEnv(undefined)).toBe(false)
  })

  it('returns false for empty string', () => {
    expect(parseBooleanEnv('')).toBe(false)
  })

  it.each([
    '1',
    'true', 'TRUE', 'True', 'tRuE',
    'yes', 'YES', 'Yes',
    'on', 'ON', 'On',
  ])('returns true for truthy value %s', (value) => {
    expect(parseBooleanEnv(value)).toBe(true)
  })

  it.each([
    '0',
    'false', 'FALSE', 'False',
    'no', 'NO',
    'off', 'OFF',
    'maybe',
    '2',
    'truee',  // typo
    'ye',     // prefix only
  ])('returns false for non-truthy value %s', (value) => {
    expect(parseBooleanEnv(value)).toBe(false)
  })

  it('trims leading and trailing whitespace', () => {
    expect(parseBooleanEnv(' 1 ')).toBe(true)
    expect(parseBooleanEnv(' true ')).toBe(true)
    expect(parseBooleanEnv('\ttrue\n')).toBe(true)
    expect(parseBooleanEnv(' 0 ')).toBe(false)
  })
})
```

- [ ] **Step 2: Run TS test — should fail with "parseBooleanEnv is not a function"**

```sh
cd packages/cli && pnpm test tests/unit/parse-boolean-env.test.ts
```

Expected: FAIL with import error.

- [ ] **Step 3: Implement TS helper in `packages/cli/src/utils.ts`**

Append to `packages/cli/src/utils.ts` (after the existing functions):

```ts
/**
 * Parse a boolean-like environment variable value.
 *
 * Recognized truthy values (case-insensitive, trimmed): "1", "true", "yes", "on".
 * All other values (including undefined, empty string, "0", "false", "no",
 * "off", or any unrecognized text) are treated as false.
 */
export function parseBooleanEnv(value: string | undefined): boolean {
  if (value === undefined) return false
  const normalized = value.trim().toLowerCase()
  if (normalized === '') return false
  return normalized === '1' || normalized === 'true' || normalized === 'yes' || normalized === 'on'
}
```

- [ ] **Step 4: Re-run TS test — should pass**

```sh
cd packages/cli && pnpm test tests/unit/parse-boolean-env.test.ts
```

Expected: PASS all cases.

- [ ] **Step 5: Add Rust helper + inline tests**

In `rust/taskcast-cli/src/helpers.rs`, insert BEFORE the `// ─── Tests ───` comment line (line 85 in the current file):

```rust
/// Parse a boolean-like environment variable value.
///
/// Recognized truthy values (case-insensitive, trimmed): "1", "true", "yes", "on".
/// All other values (including None, empty string, "0", "false", "no", "off",
/// or any unrecognized text) are treated as false.
pub fn parse_boolean_env(value: Option<&str>) -> bool {
    let Some(v) = value else { return false };
    let trimmed = v.trim().to_ascii_lowercase();
    matches!(trimmed.as_str(), "1" | "true" | "yes" | "on")
}
```

Then inside the existing `#[cfg(test)] mod tests` block (after the last test `display_url_normal_host_unchanged`), add:

```rust
    // ─── parse_boolean_env ───────────────────────────────────────────────────

    #[test]
    fn boolean_none_is_false() {
        assert!(!parse_boolean_env(None));
    }

    #[test]
    fn boolean_empty_string_is_false() {
        assert!(!parse_boolean_env(Some("")));
    }

    #[test]
    fn boolean_truthy_values() {
        for v in &[
            "1",
            "true", "TRUE", "True", "tRuE",
            "yes", "YES", "Yes",
            "on", "ON", "On",
        ] {
            assert!(parse_boolean_env(Some(v)), "expected {v:?} to be truthy");
        }
    }

    #[test]
    fn boolean_falsy_values() {
        for v in &[
            "0",
            "false", "FALSE", "False",
            "no", "NO",
            "off", "OFF",
            "maybe",
            "2",
            "truee",
            "ye",
        ] {
            assert!(!parse_boolean_env(Some(v)), "expected {v:?} to be falsy");
        }
    }

    #[test]
    fn boolean_trims_whitespace() {
        assert!(parse_boolean_env(Some(" 1 ")));
        assert!(parse_boolean_env(Some(" true ")));
        assert!(parse_boolean_env(Some("\ttrue\n")));
        assert!(!parse_boolean_env(Some(" 0 ")));
    }
}
```

(The closing brace `}` above is the brace of the existing `mod tests` — don't add an extra one.)

- [ ] **Step 6: Run Rust tests**

```sh
cd rust/taskcast-cli && cargo test helpers::tests::boolean
```

Expected: all 5 `boolean_*` tests PASS.

- [ ] **Step 7: Commit**

```bash
git add packages/cli/src/utils.ts \
        packages/cli/tests/unit/parse-boolean-env.test.ts \
        rust/taskcast-cli/src/helpers.rs
git commit -m "$(cat <<'EOF'
feat(cli): add parseBooleanEnv / parse_boolean_env helper

Add a boolean env var parser in both runtimes that recognizes
1/true/yes/on (case-insensitive, trimmed) as truthy. Foundation for
the upcoming TASKCAST_AUTO_MIGRATE feature.

Co-Authored-By: Claude Opus 4.6 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 2: CLI migration codegen + committed generated file

**Goal:** Create a build-time codegen script that embeds `migrations/postgres/*.sql` into a TS module (`packages/cli/src/generated/postgres-migrations.ts`), commit the initial output, and wire it into the package `build` script. This eliminates the pre-existing "monorepo-only path" TODO and gives the CLI a runtime-FS-free way to access migrations.

**Files:**
- Create: `packages/cli/scripts/generate-migrations.mjs`
- Create: `packages/cli/src/generated/postgres-migrations.ts` (generated, committed)
- Modify: `packages/cli/package.json` (build script only; no devDep changes yet — `testcontainers` is added later in Task 7)
- Modify: `packages/cli/vitest.config.ts` (coverage exclude)
- Create: `packages/cli/tests/unit/generate-migrations.test.ts`
- Create: `packages/cli/tests/unit/generated-migrations.test.ts`

**Acceptance Criteria:**
- [ ] `node packages/cli/scripts/generate-migrations.mjs <srcDir> <outFile>` regenerates the target TS file atomically (write .tmp, rename)
- [ ] Generator rejects any filename that doesn't match `^\d{3}_[a-zA-Z0-9_]+\.sql$` (3-digit zero-padded convention)
- [ ] Generator throws on an empty source directory
- [ ] Generated TS file exports `EmbeddedMigration` interface + `POSTGRES_MIGRATIONS: readonly EmbeddedMigration[]` sorted by filename
- [ ] SQL content is preserved byte-for-byte (via `JSON.stringify` escaping)
- [ ] `pnpm --filter @taskcast/cli build` runs the generator before `tsc` and produces a valid `dist/generated/postgres-migrations.js`
- [ ] `pnpm lint` (root) succeeds without requiring a prior build (because the generated file is committed)
- [ ] Unit tests cover: happy path (3 files), sort order, byte-for-byte SQL preservation, rejection of invalid filenames, empty-dir error, stale-file regression (pre-existing .ts with extra entry is overwritten cleanly)
- [ ] Second unit test suite asserts `POSTGRES_MIGRATIONS` matches `migrations/postgres/*.sql` (monorepo-only)

**Verify:**
```sh
cd packages/cli && pnpm build && pnpm test tests/unit/generate-migrations.test.ts tests/unit/generated-migrations.test.ts
# All tests pass; no "POSTGRES_MIGRATIONS not found" errors
```

**Steps:**

- [ ] **Step 1: Create the generator script**

Create `packages/cli/scripts/generate-migrations.mjs`:

```js
#!/usr/bin/env node
// Build-time codegen: read migrations/postgres/*.sql and emit a TS module
// with embedded SQL strings. Run from package.json "build" script.
//
// Usage:
//   node scripts/generate-migrations.mjs <sourceDir> <outFile>

import { readFileSync, readdirSync, writeFileSync, renameSync, mkdirSync } from 'node:fs'
import { dirname, join, resolve } from 'node:path'
import { argv, exit } from 'node:process'

const [, , sourceArg, outArg] = argv
if (!sourceArg || !outArg) {
  console.error('Usage: node generate-migrations.mjs <sourceDir> <outFile>')
  exit(1)
}

const sourceDir = resolve(sourceArg)
const outFile = resolve(outArg)

const FILENAME_RE = /^\d{3}_[a-zA-Z0-9_]+\.sql$/

// 1. Read and validate all SQL files
let entries
try {
  entries = readdirSync(sourceDir)
} catch (err) {
  console.error(`[generate-migrations] Cannot read ${sourceDir}: ${err.message}`)
  exit(1)
}

const sqlFiles = entries.filter((f) => f.endsWith('.sql')).sort()

if (sqlFiles.length === 0) {
  console.error(`[generate-migrations] No SQL files found in ${sourceDir}`)
  exit(1)
}

for (const filename of sqlFiles) {
  if (!FILENAME_RE.test(filename)) {
    console.error(
      `[generate-migrations] Filename '${filename}' does not match the required ` +
      `3-digit zero-padded convention (^\\d{3}_[a-zA-Z0-9_]+\\.sql$). ` +
      `This is required so the Rust runtime can reconstruct filenames from sqlx metadata.`,
    )
    exit(1)
  }
}

// 2. Build the TS file contents
const fileEntries = sqlFiles.map((filename) => {
  const sql = readFileSync(join(sourceDir, filename), 'utf8')
  return `  { filename: ${JSON.stringify(filename)}, sql: ${JSON.stringify(sql)} },`
}).join('\n')

const content = `// AUTO-GENERATED by scripts/generate-migrations.mjs — do not edit.
// Source: migrations/postgres/
// Regenerated on every \`pnpm build\`.
import type { EmbeddedMigration } from '@taskcast/postgres'

export const POSTGRES_MIGRATIONS: readonly EmbeddedMigration[] = [
${fileEntries}
] as const
`

// 3. Write atomically (tmp + rename)
mkdirSync(dirname(outFile), { recursive: true })
const tmpFile = outFile + '.tmp'
writeFileSync(tmpFile, content, 'utf8')
renameSync(tmpFile, outFile)

console.log(`[generate-migrations] Wrote ${sqlFiles.length} migration(s) to ${outFile}`)
```

- [ ] **Step 2: Run the generator once to produce the initial committed TS file**

```sh
cd packages/cli && node scripts/generate-migrations.mjs ../../migrations/postgres src/generated/postgres-migrations.ts
```

Expected output: `[generate-migrations] Wrote 2 migration(s) to .../src/generated/postgres-migrations.ts`

Verify the file exists and has two entries:

```sh
cat packages/cli/src/generated/postgres-migrations.ts | head -20
```

Expected: contains `export const POSTGRES_MIGRATIONS` with `001_initial.sql` and `002_workers.sql` entries.

- [ ] **Step 3: Update `packages/cli/package.json` build script**

Change the `"build"` script value from `"tsc"` to:

```json
"build": "node scripts/generate-migrations.mjs ../../migrations/postgres src/generated/postgres-migrations.ts && tsc",
```

The script MUST run before `tsc` so that the generated file is up to date when TypeScript compiles it. Do NOT add `testcontainers` to devDependencies in this task — that happens in T7.

- [ ] **Step 4: Update `packages/cli/vitest.config.ts` to exclude the generated file from coverage**

Read the current `vitest.config.ts`:

```sh
cat packages/cli/vitest.config.ts
```

If it already has a `coverage.exclude` array, append `'src/generated/**'`. If not, add a `test.coverage.exclude` block. Example form to add:

```ts
// packages/cli/vitest.config.ts
import { defineConfig } from 'vitest/config'

export default defineConfig({
  test: {
    coverage: {
      // ...existing config...
      exclude: [
        // ...existing excludes...
        'src/generated/**',
      ],
    },
  },
})
```

- [ ] **Step 5: Write unit tests for the generator (red)**

Create `packages/cli/tests/unit/generate-migrations.test.ts`:

```ts
import { describe, it, expect, beforeEach, afterEach } from 'vitest'
import { execSync } from 'node:child_process'
import { mkdtempSync, mkdirSync, rmSync, writeFileSync, readFileSync, existsSync } from 'node:fs'
import { join, dirname } from 'node:path'
import { tmpdir } from 'node:os'
import { fileURLToPath } from 'node:url'

const __dirname = dirname(fileURLToPath(import.meta.url))
const SCRIPT = join(__dirname, '../../scripts/generate-migrations.mjs')

function runGenerator(srcDir: string, outFile: string): { code: number; stdout: string; stderr: string } {
  try {
    const stdout = execSync(`node ${SCRIPT} ${srcDir} ${outFile}`, { stdio: 'pipe' }).toString()
    return { code: 0, stdout, stderr: '' }
  } catch (err) {
    const e = err as { status?: number; stdout?: Buffer; stderr?: Buffer }
    return {
      code: e.status ?? 1,
      stdout: e.stdout?.toString() ?? '',
      stderr: e.stderr?.toString() ?? '',
    }
  }
}

describe('generate-migrations.mjs', () => {
  let tmpSrc: string
  let tmpOut: string

  beforeEach(() => {
    const base = mkdtempSync(join(tmpdir(), 'genmig-'))
    tmpSrc = join(base, 'src')
    tmpOut = join(base, 'out', 'postgres-migrations.ts')
    mkdirSync(tmpSrc, { recursive: true })
  })

  afterEach(() => {
    // Cleanup temp dir (base is parent of both tmpSrc and tmpOut)
    rmSync(join(tmpSrc, '..'), { recursive: true, force: true })
  })

  it('generates a TS module with all SQL files sorted by filename', () => {
    writeFileSync(join(tmpSrc, '002_bar.sql'), 'CREATE TABLE b (id INT);')
    writeFileSync(join(tmpSrc, '001_foo.sql'), 'CREATE TABLE a (id INT);')
    writeFileSync(join(tmpSrc, '003_baz.sql'), 'CREATE TABLE c (id INT);')

    const result = runGenerator(tmpSrc, tmpOut)
    expect(result.code).toBe(0)

    const content = readFileSync(tmpOut, 'utf8')
    // Sorted order: 001, 002, 003
    const idx1 = content.indexOf('001_foo.sql')
    const idx2 = content.indexOf('002_bar.sql')
    const idx3 = content.indexOf('003_baz.sql')
    expect(idx1).toBeGreaterThan(0)
    expect(idx2).toBeGreaterThan(idx1)
    expect(idx3).toBeGreaterThan(idx2)
  })

  it('preserves SQL content byte-for-byte including special chars', () => {
    // Intentionally include backtick, ${}, and backslash
    const sql = "CREATE TABLE t (name TEXT DEFAULT '\\n`${x}`');"
    writeFileSync(join(tmpSrc, '001_specials.sql'), sql)

    const result = runGenerator(tmpSrc, tmpOut)
    expect(result.code).toBe(0)

    const content = readFileSync(tmpOut, 'utf8')
    // JSON.stringify escapes backslash and quotes — the generated file
    // should contain the escaped form, which at JS parse time yields
    // the original string
    expect(content).toContain(JSON.stringify(sql))
  })
})
```

(Continued in Step 6.)

- [ ] **Step 6: Finish the generator unit tests**

Append more `it()` blocks to the same `describe('generate-migrations.mjs', ...)` in `packages/cli/tests/unit/generate-migrations.test.ts`:

```ts
  it('rejects filenames that do not match the 3-digit zero-padded convention', () => {
    writeFileSync(join(tmpSrc, '1_foo.sql'), 'SELECT 1;')  // 1 digit, not 3
    const result = runGenerator(tmpSrc, tmpOut)
    expect(result.code).toBe(1)
    expect(result.stderr).toContain('does not match')
    expect(existsSync(tmpOut)).toBe(false)
  })

  it('rejects 4-digit version prefixes', () => {
    writeFileSync(join(tmpSrc, '0001_foo.sql'), 'SELECT 1;')
    const result = runGenerator(tmpSrc, tmpOut)
    expect(result.code).toBe(1)
    expect(result.stderr).toContain('does not match')
  })

  it('errors on empty source directory', () => {
    // tmpSrc has no .sql files
    const result = runGenerator(tmpSrc, tmpOut)
    expect(result.code).toBe(1)
    expect(result.stderr).toContain('No SQL files found')
    expect(existsSync(tmpOut)).toBe(false)
  })

  it('errors when source directory does not exist', () => {
    const result = runGenerator(join(tmpSrc, 'nonexistent'), tmpOut)
    expect(result.code).toBe(1)
    expect(result.stderr).toContain('Cannot read')
  })

  // This is the stale-file regression test the user explicitly asked for.
  it('overwrites a pre-existing generated file cleanly (no stale entries)', () => {
    // First run: two files
    writeFileSync(join(tmpSrc, '001_keep.sql'), 'SELECT 1;')
    writeFileSync(join(tmpSrc, '002_stale.sql'), 'SELECT 2;')
    expect(runGenerator(tmpSrc, tmpOut).code).toBe(0)
    let content = readFileSync(tmpOut, 'utf8')
    expect(content).toContain('001_keep.sql')
    expect(content).toContain('002_stale.sql')

    // Simulate: 002_stale.sql was removed from the source directory
    rmSync(join(tmpSrc, '002_stale.sql'))

    // Second run: only one file
    expect(runGenerator(tmpSrc, tmpOut).code).toBe(0)
    content = readFileSync(tmpOut, 'utf8')
    expect(content).toContain('001_keep.sql')
    expect(content).not.toContain('002_stale.sql')
  })
```

- [ ] **Step 7: Run generator unit tests — should all pass**

```sh
cd packages/cli && pnpm test tests/unit/generate-migrations.test.ts
```

Expected: PASS on all cases. If any fail, fix the generator.

- [ ] **Step 8: Write the monorepo-only "generated matches source" test**

Create `packages/cli/tests/unit/generated-migrations.test.ts`:

```ts
import { describe, it, expect } from 'vitest'
import { readdirSync, readFileSync } from 'node:fs'
import { join, dirname } from 'node:path'
import { fileURLToPath } from 'node:url'
import { POSTGRES_MIGRATIONS } from '../../src/generated/postgres-migrations.js'

const __dirname = dirname(fileURLToPath(import.meta.url))
// packages/cli/tests/unit → ../../../../migrations/postgres
const SOURCE_DIR = join(__dirname, '../../../../migrations/postgres')

describe('POSTGRES_MIGRATIONS (generated)', () => {
  it('contains every .sql file from migrations/postgres/', () => {
    const sqlFiles = readdirSync(SOURCE_DIR).filter((f) => f.endsWith('.sql')).sort()
    const embeddedNames = POSTGRES_MIGRATIONS.map((m) => m.filename)
    expect(embeddedNames).toEqual(sqlFiles)
  })

  it('embedded SQL matches source files byte-for-byte', () => {
    for (const m of POSTGRES_MIGRATIONS) {
      const source = readFileSync(join(SOURCE_DIR, m.filename), 'utf8')
      expect(m.sql).toBe(source)
    }
  })

  it('is sorted by filename ascending', () => {
    const names = POSTGRES_MIGRATIONS.map((m) => m.filename)
    const sorted = [...names].sort()
    expect(names).toEqual(sorted)
  })
})
```

- [ ] **Step 9: Run full CLI tests — the generated test must pass**

```sh
cd packages/cli && pnpm test tests/unit/generated-migrations.test.ts
```

Expected: PASS — confirms the committed TS file is in sync with the source SQL files.

- [ ] **Step 10: Run a fresh root build to verify the build script works end-to-end**

```sh
cd /Users/winrey/Projects/taskcast && pnpm --filter @taskcast/cli build
```

Expected output includes: `[generate-migrations] Wrote 2 migration(s) to .../src/generated/postgres-migrations.ts` before `tsc` completes.

Then confirm `dist/generated/postgres-migrations.js` exists:

```sh
ls packages/cli/dist/generated/postgres-migrations.js
```

- [ ] **Step 11: Confirm `pnpm lint` works on a fresh clone scenario**

Simulate: remove generated file, then run lint (should fail), then restore and re-run (should pass):

```sh
cd /Users/winrey/Projects/taskcast
mv packages/cli/src/generated/postgres-migrations.ts /tmp/_backup_generated.ts
pnpm lint 2>&1 | tail -5
# Expected: FAIL — "Cannot find module './generated/postgres-migrations.js'"

mv /tmp/_backup_generated.ts packages/cli/src/generated/postgres-migrations.ts
pnpm lint 2>&1 | tail -5
# Expected: PASS — no errors
```

This confirms committing the generated file is necessary (lint doesn't auto-regenerate).

- [ ] **Step 12: Commit**

```bash
git add packages/cli/scripts/generate-migrations.mjs \
        packages/cli/src/generated/postgres-migrations.ts \
        packages/cli/package.json \
        packages/cli/vitest.config.ts \
        packages/cli/tests/unit/generate-migrations.test.ts \
        packages/cli/tests/unit/generated-migrations.test.ts
git commit -m "$(cat <<'EOF'
feat(cli): embed Postgres migrations via build-time codegen

Add scripts/generate-migrations.mjs that reads migrations/postgres/*.sql
and emits src/generated/postgres-migrations.ts as a committed, inline TS
module. Eliminates the "monorepo-only path" TODO in taskcast migrate and
unblocks the upcoming auto-migrate feature for npm-installed CLIs.

The generated file is committed so pnpm lint (tsc -b) works on fresh
clones without a prior build. CI will gate with a staleness check in a
follow-up commit.

Co-Authored-By: Claude Opus 4.6 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 3: `performAutoMigrateIfEnabled` helper (TS)

**Goal:** Create a dedicated, dependency-injectable helper that encapsulates the auto-migrate decision + execution logic. This is the single function that `start.ts` (T4) will call, and that integration tests (T7) will directly exercise without booting an HTTP server.

**Depends on:** T0 (`buildMigrationFiles`), T1 (`parseBooleanEnv`), T2 (`POSTGRES_MIGRATIONS`).

**Files:**
- Create: `packages/cli/src/auto-migrate.ts`

**Acceptance Criteria:**
- [ ] New module exports `AutoMigrateOptions`, `AutoMigrateDeps`, and `performAutoMigrateIfEnabled`
- [ ] `options.enabled === false` → returns without any side effects (no logs, no SQL)
- [ ] `options.enabled === true && (options.storageMode === 'sqlite' || !options.postgresUrl)` → info log `[taskcast] TASKCAST_AUTO_MIGRATE is set but no Postgres configured — skipping`, returns
- [ ] `options.enabled === true && options.postgresUrl` → opens a temporary connection via `deps.createSql ?? postgres(url)`, calls `runMigrations(sql, buildMigrationFiles(POSTGRES_MIGRATIONS))`, closes connection on both success and failure paths, logs success as `Applied N new migration(s): ...` or `Database schema up to date (N migration(s) already applied)`, throws on failure with message prefixed `[taskcast] Auto-migration failed: ...`
- [ ] `deps.logger` is injectable; default logger uses `console.log` (info) and `console.error` (error)
- [ ] `deps.createSql` is injectable (for tests to inject a mock connection if they want to avoid real Postgres)
- [ ] The helper does NOT call `process.exit` — it throws, and the caller decides what to do

**Verify:**
```sh
cd packages/cli && pnpm typecheck && pnpm test tests/unit/
# Type check passes; no regression in existing tests
```

**Steps:**

- [ ] **Step 1: Create `packages/cli/src/auto-migrate.ts`**

```ts
import postgres from 'postgres'
import { runMigrations, buildMigrationFiles } from '@taskcast/postgres'
import { POSTGRES_MIGRATIONS } from './generated/postgres-migrations.js'

export interface AutoMigrateOptions {
  /**
   * Whether auto-migrate is enabled. Caller is responsible for resolving
   * this from `TASKCAST_AUTO_MIGRATE` via `parseBooleanEnv` (see utils.ts).
   */
  enabled: boolean
  /** Resolved Postgres connection URL, if any. */
  postgresUrl: string | undefined
  /** Resolved storage mode string: "memory" | "redis" | "sqlite". */
  storageMode: string
}

export interface AutoMigrateLogger {
  info: (msg: string) => void
  error: (msg: string) => void
}

export interface AutoMigrateDeps {
  /**
   * Factory for the temporary Postgres connection used for migrations.
   * Injected for tests that want to supply a mock. Default: `postgres(url)`.
   */
  createSql?: (url: string) => ReturnType<typeof postgres>
  /**
   * Logger implementation. Default writes info to stdout and error to stderr
   * via console.log / console.error.
   */
  logger?: AutoMigrateLogger
}

const DEFAULT_LOGGER: AutoMigrateLogger = {
  info: (msg) => console.log(msg),
  error: (msg) => console.error(msg),
}

/**
 * Run Postgres migrations if TASKCAST_AUTO_MIGRATE is enabled AND a Postgres
 * URL is configured. No-op (with an informational log) if Postgres isn't
 * configured. Throws on migration failure — the caller is responsible for
 * deciding whether to fail-fast (process.exit) or propagate the error.
 *
 * The migration uses a temporary connection that is closed before the function
 * returns, regardless of outcome.
 */
export async function performAutoMigrateIfEnabled(
  options: AutoMigrateOptions,
  deps: AutoMigrateDeps = {},
): Promise<void> {
  const logger = deps.logger ?? DEFAULT_LOGGER
  const createSql = deps.createSql ?? ((url: string) => postgres(url))

  if (!options.enabled) return

  if (options.storageMode === 'sqlite' || !options.postgresUrl) {
    logger.info('[taskcast] TASKCAST_AUTO_MIGRATE is set but no Postgres configured — skipping')
    return
  }

  logger.info(
    `[taskcast] TASKCAST_AUTO_MIGRATE enabled — running Postgres migrations on ${options.postgresUrl}`,
  )

  let migrateSql: ReturnType<typeof postgres> | undefined
  try {
    migrateSql = createSql(options.postgresUrl)
    const files = buildMigrationFiles(POSTGRES_MIGRATIONS)
    const result = await runMigrations(migrateSql, files)

    if (result.applied.length > 0) {
      logger.info(
        `[taskcast] Applied ${result.applied.length} new migration(s): ${result.applied.join(', ')}`,
      )
    } else {
      logger.info(
        `[taskcast] Database schema up to date (${result.skipped.length} migration(s) already applied)`,
      )
    }

    await migrateSql.end()
  } catch (err) {
    // Best-effort cleanup — we may be mid-failure, and we don't want to mask
    // the real error with a secondary cleanup failure.
    if (migrateSql) {
      try {
        await migrateSql.end()
      } catch {
        // ignore
      }
    }
    // Wrap the error so the caller's single log line is already the fully
    // prefixed form. The caller (runStart's .action wrapper) does
    // `console.error("[taskcast] " + err.message)`, producing exactly:
    //   [taskcast] Auto-migration failed: <original message>
    const original = err as Error
    throw new Error(`Auto-migration failed: ${original.message}`)
  }
}
```

- [ ] **Step 2: Type-check to catch any import / type errors**

```sh
cd /Users/winrey/Projects/taskcast && pnpm lint
```

Expected: no TypeScript errors. If there are errors about `runMigrations` argument types, recheck T0 Step 2 — the signature must be `string | MigrationFile[]`.

- [ ] **Step 3: No unit tests in this task**

Unit-level mock-based tests for `performAutoMigrateIfEnabled` are possible but offer weak value compared to the integration tests in Task 7 that use a real Postgres container via testcontainers. We skip unit tests for this helper and rely on Task 7 to cover every branch of the decision tree (enabled/disabled × postgresUrl yes/no × storageMode × migrate success/failure).

Rationale: the logic here is mostly branching on inputs + delegating to `runMigrations`. The real risk is mis-wiring `runMigrations` / connection lifecycle, which only a real-DB test can catch.

- [ ] **Step 4: Commit**

```bash
git add packages/cli/src/auto-migrate.ts
git commit -m "$(cat <<'EOF'
feat(cli): add performAutoMigrateIfEnabled helper

New module that encapsulates the auto-migrate decision/execution logic
with DI-friendly options (logger, createSql). Skips with an info log
when Postgres isn't configured; throws on migration failure so the
caller can fail-fast. Integration tests land in a follow-up commit.

Co-Authored-By: Claude Opus 4.6 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 4: Extract `runStart` and wire auto-migrate (TS `start.ts`)

**Goal:** Refactor `packages/cli/src/commands/start.ts` so that the action-body code becomes an exported `runStart(options)` function returning a `{ stop, port }` handle. Then call `performAutoMigrateIfEnabled` immediately after URL resolution and before Postgres adapter creation. The existing `.action()` wrapper becomes a thin shim that parses CLI flags, calls `runStart`, wires signal handlers, and translates thrown errors into `process.exit(1)`.

**Depends on:** T3 (`performAutoMigrateIfEnabled`), T1 (`parseBooleanEnv`).

**Files:**
- Modify: `packages/cli/src/commands/start.ts`

**Acceptance Criteria:**
- [ ] `runStart(options): Promise<{ stop, port }>` is exported from `start.ts`
- [ ] `runStart` performs config loading, URL resolution, auto-migrate (if enabled), adapter construction, engine creation, server boot, and returns a `{ stop, port }` handle that cleanly shuts down scheduler + server
- [ ] `runStart` calls `performAutoMigrateIfEnabled({ enabled: parseBooleanEnv(process.env.TASKCAST_AUTO_MIGRATE), postgresUrl, storageMode })` **after** URL resolution and **before** `PostgresLongTermStore` is created
- [ ] On auto-migrate failure (thrown error), `runStart` does NOT create any adapters or start the HTTP server — the error bubbles up
- [ ] The existing `registerStartCommand` `.action()` wrapper catches errors from `runStart` and calls `process.exit(1)` with the error printed to stderr
- [ ] Signal handlers (`SIGTERM`, `SIGINT`) are registered by the `.action()` wrapper, not by `runStart` — tests calling `runStart` directly must not leak handlers
- [ ] All existing CLI-level tests continue to pass

**Verify:**
```sh
cd packages/cli && pnpm build && pnpm test
# Full TS test suite passes; lint shows no new errors
```

**Steps:**

- [ ] **Step 1: Read the current `start.ts` to locate insertion points**

```sh
cat packages/cli/src/commands/start.ts
```

Note key lines:
- Imports (top)
- `registerStartCommand` (exported function)
- The `.action(async (options) => { ... })` block with URL resolution, storage decision, adapter creation, engine, server boot, and signal handler registration

- [ ] **Step 2: Add new imports at the top of `start.ts`**

Add these imports near the top (after the existing imports):

```ts
import { parseBooleanEnv } from '../utils.js'
import { performAutoMigrateIfEnabled } from '../auto-migrate.js'
```

- [ ] **Step 3: Add exported types and `runStart` wrapper function**

**Above** `export function registerStartCommand(program: Command): void {`, add:

```ts
export interface RunStartOptions {
  config?: string
  port: number
  storage?: string
  dbPath?: string
  playground?: boolean
  verbose?: boolean
}

export interface RunStartHandle {
  /**
   * Stops the scheduler + HTTP server. Safe to call multiple times.
   * Callers that need to wait for graceful shutdown should await the
   * returned promise.
   */
  stop: () => Promise<void>
  /** The port the server is listening on. */
  port: number
}

/**
 * Programmatic entry point for `taskcast start`. Exported so integration
 * tests can boot the full stack in-process (with env vars, real adapters,
 * a listening HTTP server) and cleanly stop it via the returned handle.
 *
 * Throws on auto-migrate failure — callers decide whether to `process.exit`
 * or propagate the error. Does NOT register signal handlers; the CLI
 * `.action()` wrapper in registerStartCommand takes care of that.
 */
export async function runStart(options: RunStartOptions): Promise<RunStartHandle> {
  // BODY: move the existing `.action` body here (Step 4), with adjustments
  //       for receiving options directly instead of via commander.
  throw new Error('runStart not yet implemented — see Step 4')
}
```

- [ ] **Step 4: Move the existing `.action()` body into `runStart`**

Copy the entire body of the current `.action(async (options) => { ... })` function into `runStart`, with these adjustments:

1. Remove the outermost `async (options: {...}) =>` wrapper — `runStart` already is an async function taking `RunStartOptions`
2. Remove the `process.on('SIGTERM', ...)` and `process.on('SIGINT', ...)` calls — these move to the `.action()` wrapper (Step 6)
3. At the point where the file does `const port = Number(options.port ?? fileConfig.port ?? 3721)`, this becomes `const port = options.port ?? fileConfig.port ?? 3721` (options.port is already a number in `RunStartOptions`)
4. **Insert the auto-migrate call immediately after `postgresUrl` is resolved**, and **before** the `if (storage === 'sqlite' ...)` storage decision block. Specifically, after this existing line:

   ```ts
   const postgresUrl = process.env['TASKCAST_POSTGRES_URL'] ?? fileConfig.adapters?.longTermStore?.url
   ```

   The cleanest rewrite: move the existing `const storage = ...` line **up** so it's resolved before auto-migrate, then call the helper with it. Replace this existing snippet:

   ```ts
   // OLD:
   let shortTermStore: ShortTermStore
   let broadcast: BroadcastProvider
   let longTermStore: LongTermStore | undefined

   const storage = options.storage ?? process.env['TASKCAST_STORAGE'] ?? (redisUrl ? 'redis' : 'memory')
   ```

   With:

   ```ts
   // NEW:
   const storage = options.storage ?? process.env['TASKCAST_STORAGE'] ?? (redisUrl ? 'redis' : 'memory')

   await performAutoMigrateIfEnabled({
     enabled: parseBooleanEnv(process.env['TASKCAST_AUTO_MIGRATE']),
     postgresUrl,
     storageMode: storage,
   })

   let shortTermStore: ShortTermStore
   let broadcast: BroadcastProvider
   let longTermStore: LongTermStore | undefined
   ```

   The `storage` variable keeps its existing name; all downstream code that
   references it continues to work unchanged.

5. At the end of the body (where the existing code does `const server = serve({ fetch: app.fetch, port }, ...)`), replace the block that registers signal handlers with a return statement. Current:

   ```ts
   const { serve } = await import('@hono/node-server')
   const server = serve({ fetch: app.fetch, port }, () => { ... })

   process.on('SIGTERM', () => { stop(); (server as { close?: () => void }).close?.() })
   process.on('SIGINT', () => { stop(); (server as { close?: () => void }).close?.() })
   ```

   New:

   ```ts
   const { serve } = await import('@hono/node-server')
   const server = serve({ fetch: app.fetch, port }, () => {
     console.log(`[taskcast] Server started on http://localhost:${port}`)
     if (options.playground) {
       console.log(`[taskcast] Playground UI at http://localhost:${port}/_playground/`)
     }
   })

   return {
     port,
     stop: async () => {
       stop()
       const serverLike = server as { close?: () => void }
       serverLike.close?.()
     },
   }
   ```

6. Delete the throw stub placeholder from Step 3.

- [ ] **Step 5: Replace the `.action()` wrapper with a thin shim**

The `.action(async (options) => { ... huge body ... })` block becomes:

```ts
.action(async (options: { config?: string; port?: string; storage?: string; dbPath?: string; playground?: boolean; verbose?: boolean }) => {
  try {
    const handle = await runStart({
      config: options.config,
      port: Number(options.port ?? 3721),
      storage: options.storage,
      dbPath: options.dbPath,
      playground: options.playground,
      verbose: options.verbose,
    })
    process.on('SIGTERM', () => { void handle.stop() })
    process.on('SIGINT', () => { void handle.stop() })
  } catch (err) {
    console.error(`[taskcast] ${(err as Error).message}`)
    process.exit(1)
  }
})
```

**Note on error formatting:** `performAutoMigrateIfEnabled` (Task 3) throws
an `Error` whose message is already `"Auto-migration failed: <original>"`
(without the `[taskcast]` prefix). The action wrapper's
`console.error(`[taskcast] ${err.message}`)` then produces the final form:
`[taskcast] Auto-migration failed: <original>`, which matches the spec's
state matrix exactly. Do not add the `[taskcast]` prefix inside the helper
— it would produce double-prefixed output.

- [ ] **Step 6: Build + type-check**

```sh
cd /Users/winrey/Projects/taskcast && pnpm --filter @taskcast/cli build && pnpm lint
```

Expected: no type errors. If there are, most likely causes:
- Forgot to change `options.port` from string to number in `RunStartOptions`
- Forgot to remove the outer `async (options: {...})` wrapper
- Missing `Promise<void>` return type on `stop`

- [ ] **Step 7: Run all existing CLI tests to confirm no regression**

```sh
cd packages/cli && pnpm test
```

Expected: all existing unit and integration tests pass. If `startup.test.ts` or similar breaks because it was spying on the `.action` body, update the test to call `runStart` directly or to stub `process.exit`.

- [ ] **Step 8: Smoke test — manually boot the server with auto-migrate disabled**

```sh
cd /Users/winrey/Projects/taskcast && pnpm --filter @taskcast/cli start -- --port 9999 &
sleep 2
curl -sS http://localhost:9999/health
# Expected: {"ok":true}
kill %1
wait
```

Verifies the refactor didn't break the default startup flow.

- [ ] **Step 9: Commit**

```bash
git add packages/cli/src/commands/start.ts
git commit -m "$(cat <<'EOF'
feat(cli): extract runStart + wire TASKCAST_AUTO_MIGRATE

Refactor start.ts so the action body is an exported runStart() function
returning a {stop, port} handle. The commander .action wrapper becomes a
thin shim that parses flags, calls runStart, registers signal handlers,
and translates thrown errors into process.exit(1). This enables
in-process E2E testing of the full startup flow.

Call performAutoMigrateIfEnabled immediately after URL resolution, before
any Postgres adapter is built — guarantees no code ever issues SQL against
an unmigrated schema.

Co-Authored-By: Claude Opus 4.6 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 5: Fix `taskcast migrate` subcommand to use embedded migrations

**Goal:** Switch the manual `taskcast migrate` subcommand from the monorepo-relative filesystem path to the embedded `POSTGRES_MIGRATIONS` module. Delete the `// TODO: This path works in the monorepo only ...` comment. After this task, both auto-migrate AND manual migrate work identically when the CLI is installed via npm.

**Depends on:** T0 (`buildMigrationFiles`), T2 (`POSTGRES_MIGRATIONS`).

**Files:**
- Modify: `packages/cli/src/commands/migrate.ts`

**Acceptance Criteria:**
- [ ] `migrate.ts` no longer imports `fileURLToPath` or uses `import.meta.url` for path resolution (unless used elsewhere)
- [ ] `migrate.ts` imports `POSTGRES_MIGRATIONS` from `../generated/postgres-migrations.js` and `buildMigrationFiles` from `@taskcast/postgres`
- [ ] The "works in monorepo only" TODO comment is removed
- [ ] `runMigrations(sql, files)` is called with the `MigrationFile[]` variant
- [ ] Existing `packages/cli/tests/unit/migrate-command.test.ts` still passes (may need minor update if it was stubbing the directory path)
- [ ] Running `taskcast migrate -y --url postgres://...` against a fresh DB applies both migrations successfully

**Verify:**
```sh
cd packages/cli && pnpm test tests/unit/migrate-command.test.ts && pnpm build
# Unit tests pass; build succeeds
```

**Steps:**

- [ ] **Step 1: Read the current `migrate.ts`**

```sh
cat packages/cli/src/commands/migrate.ts
```

Locate:
- The `import { loadMigrationFiles, runMigrations } from '@taskcast/postgres'` line (line 6 in current code)
- The TODO comment and `migrationsDir` assignment (lines 33–35)
- The `const allFiles = loadMigrationFiles(migrationsDir)` line
- The `await runMigrations(sql, migrationsDir)` line near the end

- [ ] **Step 2: Update imports**

Change the import line to:

```ts
import { buildMigrationFiles, runMigrations } from '@taskcast/postgres'
import { POSTGRES_MIGRATIONS } from '../generated/postgres-migrations.js'
```

Remove any now-unused imports (`fileURLToPath`, `dirname`, `join` from path/url if they're only used for `migrationsDir`).

- [ ] **Step 3: Replace the path-based migration loading**

Delete these lines (from [packages/cli/src/commands/migrate.ts:33-35](../../../packages/cli/src/commands/migrate.ts#L33)):

```ts
// TODO: This path works in the monorepo only. For npm publishing,
// migrations would need to be bundled with the package.
const migrationsDir = join(dirname(fileURLToPath(import.meta.url)), '../../../../migrations/postgres')
```

And replace the `const allFiles = loadMigrationFiles(migrationsDir)` line with:

```ts
const allFiles = buildMigrationFiles(POSTGRES_MIGRATIONS)
```

- [ ] **Step 4: Update the `runMigrations` call**

Find the line near line 80 of the original file:

```ts
const result = await runMigrations(sql, migrationsDir)
```

Replace with:

```ts
const result = await runMigrations(sql, allFiles)
```

- [ ] **Step 5: Type-check**

```sh
cd /Users/winrey/Projects/taskcast && pnpm lint
```

Expected: no errors.

- [ ] **Step 6: Run migrate-command unit tests**

```sh
cd packages/cli && pnpm test tests/unit/migrate-command.test.ts
```

Expected: PASS. If the test was mocking `loadMigrationFiles`, it may need to be updated to mock `buildMigrationFiles` instead — or just rely on the real `POSTGRES_MIGRATIONS` constant (preferred; mocks are fragile).

- [ ] **Step 7: Smoke test against a real Postgres (optional but recommended)**

```sh
# In one terminal: start a throwaway Postgres
docker run --rm -it -e POSTGRES_PASSWORD=test -p 15432:5432 postgres:15

# In another terminal, from the repo root:
cd /Users/winrey/Projects/taskcast
pnpm --filter @taskcast/cli build
node packages/cli/dist/index.js migrate -y --url "postgres://postgres:test@localhost:15432/postgres"
# Expected:
#   [taskcast] Target: localhost:15432/postgres
#   [taskcast] Pending migrations:
#     001_initial.sql
#     002_workers.sql
#   Applied 001_initial.sql
#   Applied 002_workers.sql
#   [taskcast] Applied 2 migration(s) successfully.
```

Then re-run — should report "Database is up to date."

- [ ] **Step 8: Commit**

```bash
git add packages/cli/src/commands/migrate.ts
git commit -m "$(cat <<'EOF'
fix(cli): use embedded migrations in taskcast migrate

Switch the manual migrate subcommand from the monorepo-relative SQL
directory path to the embedded POSTGRES_MIGRATIONS module generated at
build time. The command now works identically in monorepo dev and in
npm-installed CLIs, resolving the long-standing TODO.

Co-Authored-By: Claude Opus 4.6 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 6: Rust `run_auto_migrate` + `build_postgres_long_term_store`

**Goal:** Mirror the TS auto-migrate behavior in the Rust CLI. Extract a `run_auto_migrate(url) -> Result<_>` helper that runs migrations with log output symmetric to TS, and a `build_postgres_long_term_store(postgres_url, auto_migrate, storage_mode) -> Result<Option<Arc<dyn LongTermStore>>>` helper that encapsulates the decision tree and adapter creation. Collapse the two existing duplicate blocks in `start::run`.

**Depends on:** T1 (`parse_boolean_env` in Rust helpers).

**Files:**
- Modify: `rust/taskcast-cli/src/commands/start.rs`

**Acceptance Criteria:**
- [ ] `start.rs` reads `TASKCAST_AUTO_MIGRATE` via `parse_boolean_env(std::env::var("TASKCAST_AUTO_MIGRATE").ok().as_deref())`
- [ ] New `run_auto_migrate(url: &str) -> Result<(), Box<dyn Error>>` helper runs migrations via a temporary pool, then closes the pool
- [ ] On success, the helper queries `_sqlx_migrations` before and after to compute the newly-applied set and logs one of:
  - `[taskcast] Applied N new migration(s): 003_foo.sql, 004_bar.sql` (when pending migrations were applied)
  - `[taskcast] Database schema up to date (N migration(s) already applied)` (when no pending)
- [ ] Filename reconstruction uses `{version:03}_{description_with_underscores}.sql` (3-digit zero-padded convention — enforced at build time by the TS generator in T2)
- [ ] On failure, the helper wraps the error with `"[taskcast] Auto-migration failed: "` prefix
- [ ] New `build_postgres_long_term_store(postgres_url, auto_migrate, storage_mode)` helper replaces the two duplicate "if postgres_url then build store" blocks in `start::run`
- [ ] When `auto_migrate` is true but `storage_mode == "sqlite"` or `postgres_url is None`, helper emits `[taskcast] TASKCAST_AUTO_MIGRATE is set but no Postgres configured — skipping` and proceeds with normal startup (no error)
- [ ] All existing Rust CLI tests continue to pass

**Verify:**
```sh
cd rust/taskcast-cli && cargo test --test start_tests --test start_env_tests
cd rust && cargo build
```

**Steps:**

- [ ] **Step 1: Add the `run_auto_migrate` helper to `start.rs`**

In `rust/taskcast-cli/src/commands/start.rs`, near the bottom but **above** `async fn shutdown_signal()`, add:

```rust
type StartError = Box<dyn std::error::Error + Send + Sync>;

/// Run Postgres migrations via a temporary pool. Emits log output symmetric
/// with the TS implementation by querying `_sqlx_migrations` before and
/// after to compute newly-applied filenames.
///
/// Filename reconstruction uses `{version:03}_{description}.sql` — the
/// build-time TS generator enforces this 3-digit zero-padded convention.
async fn run_auto_migrate(url: &str) -> Result<(), StartError> {
    use std::collections::HashSet;

    eprintln!(
        "[taskcast] TASKCAST_AUTO_MIGRATE enabled — running Postgres migrations on {url}"
    );

    let migrate_pool = sqlx::PgPool::connect(url)
        .await
        .map_err(|e| format!("[taskcast] Auto-migration failed: {e}"))?;

    // Snapshot applied versions BEFORE migration. If the table doesn't exist
    // yet (first run against a blank DB), this returns an empty set.
    let before: HashSet<i64> = sqlx::query_scalar::<_, i64>(
        "SELECT version FROM _sqlx_migrations",
    )
    .fetch_all(&migrate_pool)
    .await
    .unwrap_or_default()
    .into_iter()
    .collect();

    let store = taskcast_postgres::PostgresLongTermStore::new(migrate_pool.clone());
    let migrate_result = store.migrate().await;

    // On success, query the full applied set and compute the delta.
    let log_line: Option<String> = if migrate_result.is_ok() {
        let rows: Vec<(i64, String)> = sqlx::query_as::<_, (i64, String)>(
            "SELECT version, description FROM _sqlx_migrations ORDER BY version",
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

    migrate_result.map_err(|e| -> StartError {
        format!("[taskcast] Auto-migration failed: {e}").into()
    })?;

    if let Some(line) = log_line {
        eprintln!("{line}");
    }
    Ok(())
}
```

- [ ] **Step 2: Add the `build_postgres_long_term_store` helper**

Immediately after `run_auto_migrate`, add:

```rust
/// Decide whether to run auto-migrate and build the Postgres long-term store
/// adapter. Returns `None` when Postgres isn't configured (memory/sqlite
/// storage modes) — callers continue without a long-term store.
///
/// Replaces the two duplicate "if postgres_url then build store" blocks in
/// `start::run` and inserts the auto-migrate decision in front of them.
async fn build_postgres_long_term_store(
    postgres_url: Option<&str>,
    auto_migrate: bool,
    storage_mode: &str,
) -> Result<Option<Arc<dyn taskcast_core::LongTermStore>>, StartError> {
    // 1. Auto-migrate decision
    if auto_migrate {
        match (storage_mode, postgres_url) {
            ("sqlite", _) | (_, None) => {
                eprintln!(
                    "[taskcast] TASKCAST_AUTO_MIGRATE is set but no Postgres configured — skipping"
                );
            }
            (_, Some(url)) => {
                run_auto_migrate(url).await?;
            }
        }
    }

    // 2. Build production store (independent pool). Sqlite storage mode
    //    means the long-term store is provided by the sqlite adapter path,
    //    not by Postgres — return None.
    if storage_mode == "sqlite" {
        return Ok(None);
    }

    if let Some(url) = postgres_url {
        let pool = sqlx::PgPool::connect(url).await?;
        let store = taskcast_postgres::PostgresLongTermStore::new(pool);
        return Ok(Some(Arc::new(store) as Arc<dyn taskcast_core::LongTermStore>));
    }

    Ok(None)
}
```

- [ ] **Step 3: Wire `build_postgres_long_term_store` into `start::run`**

In `start::run`, near where `env_storage` and `storage_mode` are resolved (current lines 74–75), add:

```rust
let auto_migrate = crate::helpers::parse_boolean_env(
    std::env::var("TASKCAST_AUTO_MIGRATE").ok().as_deref(),
);
```

Then replace the two existing duplicated blocks that build `PostgresLongTermStore` — one in the `"redis"` arm (current lines 105–112), one in the memory-fallback arm (current lines 125–132) — with a single call **before** the `match storage_mode` block:

```rust
// 5. Build adapters
let long_term_store = build_postgres_long_term_store(
    postgres_url.as_deref(),
    auto_migrate,
    storage_mode,
).await?;
```

In the `match storage_mode` block, for the `"redis"` and memory arms, **remove** the per-arm `long_term_store` derivation that referenced `postgres_url`, and keep only the `broadcast` + `short_term_store` construction. The outer `long_term_store` computed above is used directly.

For the `"sqlite"` arm, the existing sqlite adapter already produces its own long-term store — keep that as-is, but ensure it's assigned to a different variable name (e.g., `sqlite_long_term_store`) and then the construction of the `StorageAdapters` tuple for the sqlite arm uses `sqlite_long_term_store` instead of the outer `long_term_store`.

Concretely, adjust the existing `match storage_mode { ... }` so it becomes:

```rust
type StorageAdapters = (
    Arc<dyn taskcast_core::BroadcastProvider>,
    Arc<dyn taskcast_core::ShortTermStore>,
    Option<Arc<dyn taskcast_core::LongTermStore>>,
);

let (broadcast, short_term_store, long_term_store): StorageAdapters = match storage_mode {
    "sqlite" => {
        let adapters = taskcast_sqlite::create_sqlite_adapters(&db_path).await?;
        eprintln!("[taskcast] Using SQLite storage at {db_path}");
        (
            Arc::new(taskcast_core::MemoryBroadcastProvider::new()),
            Arc::new(adapters.short_term_store),
            Some(Arc::new(adapters.long_term_store) as Arc<dyn taskcast_core::LongTermStore>),
        )
    }
    "redis" => {
        let url = redis_url
            .as_deref()
            .ok_or("--storage redis requires TASKCAST_REDIS_URL")?;
        let client = redis::Client::open(url)?;
        let pub_conn = client.get_multiplexed_async_connection().await?;
        let sub_conn = client.get_async_pubsub().await?;
        let store_conn = client.get_multiplexed_async_connection().await?;

        let adapters =
            taskcast_redis::create_redis_adapters(pub_conn, sub_conn, store_conn, None);

        (
            Arc::new(adapters.broadcast),
            Arc::new(adapters.short_term_store),
            long_term_store.clone(),
        )
    }
    _ => {
        eprintln!(
            "[taskcast] No TASKCAST_REDIS_URL configured \u{2014} using in-memory adapters"
        );
        (
            Arc::new(taskcast_core::MemoryBroadcastProvider::new()),
            Arc::new(taskcast_core::MemoryShortTermStore::new()),
            long_term_store.clone(),
        )
    }
};
```

**Important:** because `long_term_store` is moved into the tuple in the redis/memory arms, and into a different place in the sqlite arm, the outer `long_term_store` variable must be `Option<Arc<dyn LongTermStore>>` with `Clone` semantics — `Arc<dyn _>` does implement `Clone`, and `Option<Arc<_>>::clone()` is fine. The `.clone()` calls in the redis and memory arms are intentional.

Alternative (simpler) if the borrow checker objects: move `long_term_store` into a `let` binding before the match and use it exactly once by consuming it in a helper. If you hit ownership pain, convert the above to:

```rust
let effective_long_term = long_term_store;
let (broadcast, short_term_store, final_long_term) = match storage_mode {
    "sqlite" => { /* as above, uses sqlite adapter's long-term store */ }
    "redis"  => { /* (broadcast, short, effective_long_term) */ }
    _        => { /* (memory broadcast, memory short, effective_long_term) */ }
};
```

Then rename `final_long_term` → `long_term_store` for the rest of the function.

- [ ] **Step 4: Remove the now-unused `postgres_url` references in the old per-arm adapter code**

After the refactor, `postgres_url` is only used in the call to `build_postgres_long_term_store`. Double-check there are no remaining references in the match arms. If compilation fails with "unused variable", prefix with underscore or remove entirely.

- [ ] **Step 5: Build and verify**

```sh
cd /Users/winrey/Projects/taskcast && cd rust && cargo build
```

Expected: clean build. If there are borrow errors on `long_term_store`, try the Alternative in Step 3.

- [ ] **Step 6: Run existing Rust tests to confirm no regression**

```sh
cd rust && cargo test -p taskcast-cli
```

Expected: all existing tests pass (start_tests, start_env_tests, migrate_tests, etc.). The new auto-migrate tests in `start_env_tests.rs` land in Task 8 — they don't exist yet.

- [ ] **Step 7: Smoke test manually**

With a throwaway Postgres container:

```sh
docker run --rm -d --name taskcast-pg-smoke -e POSTGRES_PASSWORD=test -p 15433:5432 postgres:15
sleep 3
cd /Users/winrey/Projects/taskcast/rust
TASKCAST_POSTGRES_URL="postgres://postgres:test@localhost:15433/postgres" \
TASKCAST_AUTO_MIGRATE=1 \
cargo run -p taskcast-cli -- start --port 9998 &
CLI_PID=$!
sleep 3
curl -sS http://localhost:9998/health
kill $CLI_PID
wait
docker stop taskcast-pg-smoke
```

Expected stderr from the CLI includes:
- `[taskcast] TASKCAST_AUTO_MIGRATE enabled — running Postgres migrations on ...`
- `[taskcast] Applied 2 new migration(s): 001_initial.sql, 002_workers.sql`

- [ ] **Step 8: Commit**

```bash
git add rust/taskcast-cli/src/commands/start.rs
git commit -m "$(cat <<'EOF'
feat(rust-cli): wire TASKCAST_AUTO_MIGRATE into start

Add run_auto_migrate and build_postgres_long_term_store helpers that
encapsulate the auto-migrate decision and adapter construction. On
success, query _sqlx_migrations before and after the migration run to
compute newly-applied filenames, so log output is symmetric with the
TS implementation. Reconstruction assumes the 3-digit zero-padded
filename convention (enforced at build time by the TS codegen).

Collapses the two existing duplicate "if postgres_url then build store"
blocks in start::run into a single helper call, eliminating drift risk.

Co-Authored-By: Claude Opus 4.6 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 7: TS integration + in-process E2E tests (testcontainers Postgres)

**Goal:** Add comprehensive integration tests covering every branch of `performAutoMigrateIfEnabled`, plus one in-process E2E test that calls `runStart` with auto-migrate enabled against a real Postgres container and verifies the `_sqlx_migrations` table + `/health` endpoint.

**Depends on:** T3 (`performAutoMigrateIfEnabled`), T4 (`runStart` exported).

**Files:**
- Modify: `packages/cli/package.json` (add `testcontainers` devDependency)
- Create: `packages/cli/tests/integration/auto-migrate.test.ts`

**Acceptance Criteria:**
- [ ] `packages/cli/package.json` lists `testcontainers@^10.13.0` under `devDependencies`
- [ ] `pnpm install` at repo root succeeds
- [ ] Integration test uses a single shared Postgres container for the whole suite (start in `beforeAll`, stop in `afterAll`)
- [ ] Test covers all 8 cases from the spec's state matrix:
  - Happy path: enabled + new DB → applies both migrations, returns resolved; `_sqlx_migrations` populated
  - Idempotency: run twice → second run is a no-op with "up to date" log
  - Disabled: `enabled: false` → no SQL executed, `_sqlx_migrations` does not exist
  - No Postgres URL: `enabled: true, postgresUrl: undefined` → info log "skipping", no SQL executed
  - Sqlite storage mode: `storageMode: 'sqlite'` → info log "skipping"
  - Connection failure: bad URL → throws with "Auto-migration failed:" prefix
  - Checksum mismatch: pre-populate `_sqlx_migrations` with a bad checksum → throws
  - Dirty migration: pre-populate `_sqlx_migrations` with `success=false` → throws
- [ ] One in-process E2E test calls `runStart({ port: 0, ... })` with env vars set, verifies `_sqlx_migrations` is populated, hits `/health` → 200, calls `stop()` for clean teardown
- [ ] Test file uses a logger-capture helper to assert on log strings (not `console.log` spying)

**Verify:**
```sh
cd packages/cli && pnpm test tests/integration/auto-migrate.test.ts
# All 9+ cases pass. Requires Docker for testcontainers.
```

**Steps:**

- [ ] **Step 1: Add `testcontainers` devDependency**

In `packages/cli/package.json`, add to `devDependencies`:

```json
"testcontainers": "^10.13.0"
```

Then:

```sh
cd /Users/winrey/Projects/taskcast && pnpm install
```

Verify no errors.

- [ ] **Step 2: Create the integration test file**

Create `packages/cli/tests/integration/auto-migrate.test.ts`:

```ts
import { describe, it, expect, beforeAll, afterAll, beforeEach } from 'vitest'
import { GenericContainer, Wait, type StartedTestContainer } from 'testcontainers'
import postgres from 'postgres'
import { performAutoMigrateIfEnabled } from '../../src/auto-migrate.js'
import type { AutoMigrateLogger } from '../../src/auto-migrate.js'

// ─── Shared Postgres container ───────────────────────────────────────────────

let container: StartedTestContainer
let baseUrl: string
let sql: ReturnType<typeof postgres>

beforeAll(async () => {
  container = await new GenericContainer('postgres:15')
    .withExposedPorts(5432)
    .withEnvironment({ POSTGRES_PASSWORD: 'test' })
    .withWaitStrategy(Wait.forLogMessage(/database system is ready to accept connections/, 2))
    .start()

  const host = container.getHost()
  const port = container.getMappedPort(5432)
  baseUrl = `postgres://postgres:test@${host}:${port}/postgres`
  sql = postgres(baseUrl)
}, 60_000)

afterAll(async () => {
  await sql?.end()
  await container?.stop()
}, 30_000)

// ─── Per-test DB reset (drop migration + tables) ────────────────────────────

beforeEach(async () => {
  await sql.unsafe('DROP TABLE IF EXISTS taskcast_events CASCADE').catch(() => {})
  await sql.unsafe('DROP TABLE IF EXISTS taskcast_tasks CASCADE').catch(() => {})
  await sql.unsafe('DROP TABLE IF EXISTS taskcast_workers CASCADE').catch(() => {})
  await sql.unsafe('DROP TABLE IF EXISTS taskcast_worker_events CASCADE').catch(() => {})
  await sql.unsafe('DROP TABLE IF EXISTS _sqlx_migrations').catch(() => {})
})

// ─── Capture logger ──────────────────────────────────────────────────────────

function captureLogger(): { logger: AutoMigrateLogger; info: string[]; error: string[] } {
  const info: string[] = []
  const error: string[] = []
  return {
    logger: {
      info: (msg) => info.push(msg),
      error: (msg) => error.push(msg),
    },
    info,
    error,
  }
}

async function countMigrationsTable(): Promise<number | null> {
  try {
    const rows = await sql.unsafe('SELECT COUNT(*)::int AS c FROM _sqlx_migrations')
    return Number(rows[0]!['c'])
  } catch {
    return null  // table doesn't exist
  }
}

// ─── Tests ───────────────────────────────────────────────────────────────────

describe('performAutoMigrateIfEnabled (integration)', () => {
  it('happy path: applies all migrations on a fresh DB', async () => {
    const { logger, info, error } = captureLogger()

    await performAutoMigrateIfEnabled(
      { enabled: true, postgresUrl: baseUrl, storageMode: 'memory' },
      { logger },
    )

    expect(error).toHaveLength(0)
    const banner = info.find((m) => m.includes('TASKCAST_AUTO_MIGRATE enabled'))
    expect(banner).toBeTruthy()
    const applied = info.find((m) => m.includes('Applied'))
    expect(applied).toMatch(/Applied 2 new migration\(s\): 001_initial\.sql, 002_workers\.sql/)

    const count = await countMigrationsTable()
    expect(count).toBe(2)
  })

  it('idempotency: second run reports up-to-date', async () => {
    const first = captureLogger()
    await performAutoMigrateIfEnabled(
      { enabled: true, postgresUrl: baseUrl, storageMode: 'memory' },
      { logger: first.logger },
    )
    expect(first.info.some((m) => m.includes('Applied 2 new'))).toBe(true)

    const second = captureLogger()
    await performAutoMigrateIfEnabled(
      { enabled: true, postgresUrl: baseUrl, storageMode: 'memory' },
      { logger: second.logger },
    )
    expect(second.error).toHaveLength(0)
    expect(second.info.some((m) => m.includes('Database schema up to date'))).toBe(true)
    expect(second.info.some((m) => m.includes('Applied'))).toBe(false)

    expect(await countMigrationsTable()).toBe(2)
  })

  it('disabled: no-op', async () => {
    const { logger, info, error } = captureLogger()
    await performAutoMigrateIfEnabled(
      { enabled: false, postgresUrl: baseUrl, storageMode: 'memory' },
      { logger },
    )
    expect(info).toHaveLength(0)
    expect(error).toHaveLength(0)
    expect(await countMigrationsTable()).toBeNull()
  })

  it('skip: enabled but no postgresUrl', async () => {
    const { logger, info, error } = captureLogger()
    await performAutoMigrateIfEnabled(
      { enabled: true, postgresUrl: undefined, storageMode: 'memory' },
      { logger },
    )
    expect(info).toHaveLength(1)
    expect(info[0]).toContain('no Postgres configured')
    expect(error).toHaveLength(0)
    expect(await countMigrationsTable()).toBeNull()
  })

  it('skip: sqlite storage mode', async () => {
    const { logger, info, error } = captureLogger()
    await performAutoMigrateIfEnabled(
      { enabled: true, postgresUrl: baseUrl, storageMode: 'sqlite' },
      { logger },
    )
    expect(info).toHaveLength(1)
    expect(info[0]).toContain('no Postgres configured')
    expect(error).toHaveLength(0)
    // Note: baseUrl is a real Postgres, but storageMode says sqlite, so we skip
    expect(await countMigrationsTable()).toBeNull()
  })

  it('fails: connection error', async () => {
    const { logger } = captureLogger()
    const badUrl = 'postgres://postgres:test@127.0.0.1:1/nonexistent'

    await expect(
      performAutoMigrateIfEnabled(
        { enabled: true, postgresUrl: badUrl, storageMode: 'memory' },
        { logger },
      ),
    ).rejects.toThrow(/^Auto-migration failed:/)
  })
})
```

(Checksum mismatch and dirty migration tests continued in Step 3.)

- [ ] **Step 3: Add checksum mismatch and dirty migration tests**

Append inside the same `describe('performAutoMigrateIfEnabled (integration)', ...)` block:

```ts
  it('fails: checksum mismatch for already-applied migration', async () => {
    // First: apply the migrations cleanly
    await performAutoMigrateIfEnabled(
      { enabled: true, postgresUrl: baseUrl, storageMode: 'memory' },
      { logger: captureLogger().logger },
    )

    // Corrupt the checksum of version 1 so it won't match the embedded SQL
    await sql.unsafe(
      "UPDATE _sqlx_migrations SET checksum = '\\x00000000'::bytea WHERE version = 1",
    )

    const { logger } = captureLogger()
    await expect(
      performAutoMigrateIfEnabled(
        { enabled: true, postgresUrl: baseUrl, storageMode: 'memory' },
        { logger },
      ),
    ).rejects.toThrow(/^Auto-migration failed: .*checksum/i)
  })

  it('fails: dirty migration marker', async () => {
    // Create the _sqlx_migrations table manually and insert a dirty row
    await sql.unsafe(`
      CREATE TABLE _sqlx_migrations (
          version BIGINT PRIMARY KEY,
          description TEXT NOT NULL,
          installed_on TIMESTAMPTZ NOT NULL DEFAULT now(),
          success BOOLEAN NOT NULL,
          checksum BYTEA NOT NULL,
          execution_time BIGINT NOT NULL
      )
    `)
    await sql.unsafe(
      "INSERT INTO _sqlx_migrations (version, description, success, checksum, execution_time) VALUES (1, 'initial', false, '\\x00'::bytea, -1)",
    )

    const { logger } = captureLogger()
    await expect(
      performAutoMigrateIfEnabled(
        { enabled: true, postgresUrl: baseUrl, storageMode: 'memory' },
        { logger },
      ),
    ).rejects.toThrow(/^Auto-migration failed: .*[Dd]irty migration/)
  })
})
```

- [ ] **Step 4: Add the in-process E2E test (full `runStart` flow)**

Append a second `describe` block in the same file:

```ts
describe('runStart with TASKCAST_AUTO_MIGRATE=1 (in-process E2E)', () => {
  const ORIGINAL_ENV = { ...process.env }

  afterEach(() => {
    process.env = { ...ORIGINAL_ENV }
  })

  it('boots the server, runs migrations, and stops cleanly', async () => {
    const { runStart } = await import('../../src/commands/start.js')

    process.env['TASKCAST_AUTO_MIGRATE'] = '1'
    process.env['TASKCAST_POSTGRES_URL'] = baseUrl

    // Pick a free port (0 = OS-assigned) by trying a sensible range;
    // runStart passes the value straight to serve()
    const handle = await runStart({
      port: 0,
      storage: 'memory',
      verbose: false,
    })

    try {
      // Verify migrations applied
      expect(await countMigrationsTable()).toBe(2)

      // Verify HTTP server is alive
      const res = await fetch(`http://127.0.0.1:${handle.port}/health`)
      expect(res.status).toBe(200)
    } finally {
      await handle.stop()
    }
  }, 30_000)
})
```

**Note on `port: 0`:** check if `@hono/node-server`'s `serve()` accepts port 0 for "any available port" and populates it in the callback. If not, `runStart` needs to accept `port: 0` as an input and use `find_available_port()`-style logic to pick an unused port explicitly. Verify during execution; if broken, use a fixed high port (e.g., `12345 + Math.floor(Math.random() * 10000)`) as a fallback.

- [ ] **Step 5: Run the integration test suite**

```sh
cd packages/cli && pnpm test tests/integration/auto-migrate.test.ts
```

Expected: all 9 tests pass (8 performAutoMigrateIfEnabled cases + 1 runStart E2E). Docker must be running.

- [ ] **Step 6: Verify coverage target**

```sh
cd packages/cli && pnpm test:coverage tests/integration/auto-migrate.test.ts
```

Check the coverage report for `src/auto-migrate.ts` — should be 100%. If any branch is uncovered, add a test.

- [ ] **Step 7: Commit**

```bash
git add packages/cli/package.json \
        packages/cli/tests/integration/auto-migrate.test.ts
git commit -m "$(cat <<'EOF'
test(cli): integration + in-process E2E for auto-migrate

Add testcontainers-based integration tests covering all 8 cases of
performAutoMigrateIfEnabled (happy path, idempotency, disabled, skip
with no Postgres, skip with sqlite mode, connection failure, checksum
mismatch, dirty migration) plus one in-process E2E that boots the full
runStart flow and verifies both _sqlx_migrations population and
/health endpoint responsiveness.

Co-Authored-By: Claude Opus 4.6 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 8: Rust integration + in-process E2E tests

**Goal:** Extend `rust/taskcast-cli/tests/start_env_tests.rs` with auto-migrate test cases. Reuse the existing `EnvGuard` + testcontainers pattern. Tests call `start::run(StartArgs)` in-process via `tokio::spawn`, not subprocess spawn.

**Depends on:** T6 (`run_auto_migrate`, `build_postgres_long_term_store` in start.rs).

**Files:**
- Modify: `rust/taskcast-cli/tests/start_env_tests.rs`

**Acceptance Criteria:**
- [ ] New test cases use the existing `EnvGuard` and `lock_env()` pattern
- [ ] Tests spin up a Postgres testcontainer (similar to `rust/taskcast-postgres/tests/store_tests.rs` setup helper — copy the pattern into the CLI test file; no new shared helper crate)
- [ ] Happy path: `TASKCAST_AUTO_MIGRATE=1` + `TASKCAST_POSTGRES_URL=<pg>` + `find_available_port()` → `tokio::spawn(start::run)` → poll `/health` → query `_sqlx_migrations` table → expect 2 rows → `handle.abort()`
- [ ] Idempotency: run start twice back-to-back (with abort between), verify table still has 2 rows after second
- [ ] Skip path: `TASKCAST_AUTO_MIGRATE=1` without `TASKCAST_POSTGRES_URL` and with `TASKCAST_STORAGE=memory` → server starts normally, no Postgres connection attempted
- [ ] Fail-fast: `TASKCAST_AUTO_MIGRATE=1` + bad `TASKCAST_POSTGRES_URL` → `start::run` returns `Err`
- [ ] All existing tests in `start_env_tests.rs` continue to pass

**Verify:**
```sh
cd rust/taskcast-cli && cargo test --test start_env_tests
```

**Steps:**

- [ ] **Step 1: Add imports and shared helpers at the top of `start_env_tests.rs`**

Add to the existing import list:

```rust
use sqlx::postgres::PgPoolOptions;
use testcontainers_modules::postgres::Postgres as PostgresContainer;
```

Add a helper near the existing `find_available_port`:

```rust
async fn start_postgres_container() -> (
    testcontainers::ContainerAsync<PostgresContainer>,
    String,
) {
    let container = PostgresContainer::default().start().await.unwrap();
    let host_port = container.get_host_port_ipv4(5432).await.unwrap();
    let database_url = format!(
        "postgres://postgres:postgres@127.0.0.1:{}/postgres",
        host_port
    );
    (container, database_url)
}

async fn count_sqlx_migrations(url: &str) -> Option<i64> {
    let pool = match PgPoolOptions::new().max_connections(1).connect(url).await {
        Ok(p) => p,
        Err(_) => return None,
    };
    let row: Result<(i64,), _> = sqlx::query_as("SELECT COUNT(*)::BIGINT FROM _sqlx_migrations")
        .fetch_one(&pool)
        .await;
    pool.close().await;
    row.ok().map(|(n,)| n)
}
```

- [ ] **Step 2: Add the happy-path test**

Append to the existing tests:

```rust
#[tokio::test]
async fn run_auto_migrate_applies_migrations_on_fresh_db() {
    let (container, database_url) = start_postgres_container().await;

    let _env = EnvGuard::new(&[
        ("TASKCAST_AUTO_MIGRATE", "1"),
        ("TASKCAST_POSTGRES_URL", database_url.as_str()),
    ]);

    let port = find_available_port().await;
    let handle = tokio::spawn(async move {
        let _ = taskcast_cli::commands::start::run(StartArgs {
            port,
            ..Default::default()
        })
        .await;
    });

    // Give the server time to boot + run migrations
    tokio::time::sleep(std::time::Duration::from_millis(1500)).await;

    // Verify _sqlx_migrations populated (2 rows for current set of migrations)
    let count = count_sqlx_migrations(&database_url).await;
    assert_eq!(count, Some(2), "expected 2 applied migrations");

    // Verify health endpoint works
    let res = reqwest::get(&format!("http://127.0.0.1:{port}/health"))
        .await
        .unwrap();
    assert!(res.status().is_success());

    handle.abort();
    drop(container);
}
```

- [ ] **Step 3: Add the idempotency test**

```rust
#[tokio::test]
async fn run_auto_migrate_is_idempotent() {
    let (container, database_url) = start_postgres_container().await;

    let _env = EnvGuard::new(&[
        ("TASKCAST_AUTO_MIGRATE", "1"),
        ("TASKCAST_POSTGRES_URL", database_url.as_str()),
    ]);

    // First boot
    let port1 = find_available_port().await;
    let handle1 = tokio::spawn(async move {
        let _ = taskcast_cli::commands::start::run(StartArgs {
            port: port1,
            ..Default::default()
        })
        .await;
    });
    tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
    assert_eq!(count_sqlx_migrations(&database_url).await, Some(2));
    handle1.abort();
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;

    // Second boot — should be no-op, count unchanged
    let port2 = find_available_port().await;
    let handle2 = tokio::spawn(async move {
        let _ = taskcast_cli::commands::start::run(StartArgs {
            port: port2,
            ..Default::default()
        })
        .await;
    });
    tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
    assert_eq!(count_sqlx_migrations(&database_url).await, Some(2));
    handle2.abort();

    drop(container);
}
```

- [ ] **Step 4: Add the skip test (memory storage, auto-migrate enabled)**

```rust
#[tokio::test]
async fn run_auto_migrate_skips_when_memory_storage() {
    // TASKCAST_AUTO_MIGRATE=1 but no TASKCAST_POSTGRES_URL → should skip
    // gracefully, not fail
    let _env = EnvGuard::new(&[
        ("TASKCAST_AUTO_MIGRATE", "1"),
        ("TASKCAST_STORAGE", "memory"),
    ]);

    let port = find_available_port().await;
    let handle = tokio::spawn(async move {
        let _ = taskcast_cli::commands::start::run(StartArgs {
            port,
            storage: "memory".to_string(),
            ..Default::default()
        })
        .await;
    });

    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    // Server should be up despite the enabled flag
    let res = reqwest::get(&format!("http://127.0.0.1:{port}/health"))
        .await
        .unwrap();
    assert!(res.status().is_success());

    handle.abort();
}
```

- [ ] **Step 5: Add the fail-fast test**

```rust
#[tokio::test]
async fn run_auto_migrate_fails_fast_on_bad_url() {
    let _env = EnvGuard::new(&[
        ("TASKCAST_AUTO_MIGRATE", "1"),
        ("TASKCAST_POSTGRES_URL", "postgres://nouser:nopass@127.0.0.1:1/nodb"),
    ]);

    let port = find_available_port().await;
    // We await run() directly this time — we expect it to return Err quickly,
    // not run an HTTP server
    let result = taskcast_cli::commands::start::run(StartArgs {
        port,
        ..Default::default()
    })
    .await;

    assert!(result.is_err(), "expected Err, got Ok");
    let err_msg = format!("{}", result.unwrap_err());
    assert!(
        err_msg.contains("Auto-migration failed:"),
        "expected 'Auto-migration failed:' in error, got: {err_msg}"
    );
}
```

- [ ] **Step 6: Run the Rust integration tests**

```sh
cd rust/taskcast-cli && cargo test --test start_env_tests
```

Expected: all 4 new tests pass + all existing tests pass. Docker required.

- [ ] **Step 7: Commit**

```bash
git add rust/taskcast-cli/tests/start_env_tests.rs
git commit -m "$(cat <<'EOF'
test(rust-cli): auto-migrate integration + in-process E2E tests

Extend start_env_tests.rs with four new cases for TASKCAST_AUTO_MIGRATE:
happy path (migrations applied on fresh DB), idempotency (second boot
is a no-op), skip path (memory storage + flag enabled does not try to
connect), and fail-fast on bad URL (start::run returns Err with the
[taskcast] Auto-migration failed: prefix).

Uses existing EnvGuard pattern + testcontainers-modules for Postgres.

Co-Authored-By: Claude Opus 4.6 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 9: CI staleness check for the generated migrations file

**Goal:** Add a CI step that catches PRs which modify `migrations/postgres/*.sql` but forget to regenerate `packages/cli/src/generated/postgres-migrations.ts`. Fails CI before the broken state can merge.

**Depends on:** T2 (codegen script exists).

**Files:**
- Modify: `.github/workflows/ci.yml`

**Acceptance Criteria:**
- [ ] New CI job step runs after `pnpm install` and before `pnpm test`
- [ ] Step runs the generator and then `git diff --exit-code` on the generated file
- [ ] If the file is stale, CI fails with an actionable error message
- [ ] Existing CI jobs continue to pass on the current committed state

**Verify:**
```sh
# Locally simulate the CI check:
cd /Users/winrey/Projects/taskcast
node packages/cli/scripts/generate-migrations.mjs migrations/postgres packages/cli/src/generated/postgres-migrations.ts
git diff --exit-code packages/cli/src/generated/postgres-migrations.ts
# Expected exit code 0 (no diff)
```

**Steps:**

- [ ] **Step 1: Read the current CI workflow**

```sh
cat .github/workflows/ci.yml
```

Identify the main TS test job — likely named `test` or `build-and-test` — and locate where `pnpm install` and `pnpm test` run.

- [ ] **Step 2: Insert the staleness check step**

Add a new step between `pnpm install` and `pnpm build` / `pnpm test` in the main job. Example (adjust indentation to match the existing file):

```yaml
      - name: Verify generated Postgres migrations file is up to date
        run: |
          node packages/cli/scripts/generate-migrations.mjs \
            migrations/postgres \
            packages/cli/src/generated/postgres-migrations.ts
          if ! git diff --exit-code packages/cli/src/generated/postgres-migrations.ts; then
            echo ""
            echo "::error::packages/cli/src/generated/postgres-migrations.ts is out of sync with migrations/postgres/*.sql."
            echo "::error::Run 'pnpm --filter @taskcast/cli build' locally and commit the updated file."
            exit 1
          fi
```

- [ ] **Step 3: Verify locally**

```sh
cd /Users/winrey/Projects/taskcast
node packages/cli/scripts/generate-migrations.mjs migrations/postgres packages/cli/src/generated/postgres-migrations.ts
git diff --exit-code packages/cli/src/generated/postgres-migrations.ts
echo "Exit code: $?"
# Expected: Exit code: 0
```

- [ ] **Step 4: Optionally validate the YAML**

```sh
# If you have yamllint installed:
yamllint .github/workflows/ci.yml
```

Or just rely on GitHub's schema check when the PR is pushed.

- [ ] **Step 5: Commit**

```bash
git add .github/workflows/ci.yml
git commit -m "$(cat <<'EOF'
ci: add staleness check for generated Postgres migrations file

Runs the codegen script and git diff --exit-code against the committed
src/generated/postgres-migrations.ts to catch PRs that modify SQL without
regenerating the embedded TS module.

Co-Authored-By: Claude Opus 4.6 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 10: Documentation + changeset

**Goal:** Document the new `TASKCAST_AUTO_MIGRATE` env var in user-facing docs (deployment.md EN/ZH + CLI README) and create a changeset entry for the release pipeline.

**Depends on:** All code tasks complete (T0–T9).

**Files:**
- Modify: `docs/guide/deployment.md`
- Modify: `docs/guide/deployment.zh.md`
- Modify: `packages/cli/README.md`
- Create: `.changeset/auto-migrate.md`

**Acceptance Criteria:**
- [ ] `deployment.md` env var table includes a row for `TASKCAST_AUTO_MIGRATE`
- [ ] `deployment.md` has a new "Database migrations" section covering both manual (`taskcast migrate`) and automatic (`TASKCAST_AUTO_MIGRATE=1`) modes, including the multi-replica rolling-deploy warning
- [ ] `deployment.zh.md` has the same additions translated into Chinese
- [ ] `packages/cli/README.md` env var table includes the row
- [ ] `.changeset/auto-migrate.md` marks `@taskcast/cli` and `@taskcast/postgres` as `minor`
- [ ] No other docs need updates (root README, api docs, skill docs — out of scope per spec)

**Verify:**
```sh
# Visual review. No command output to verify.
cat .changeset/auto-migrate.md
grep -A1 TASKCAST_AUTO_MIGRATE docs/guide/deployment.md docs/guide/deployment.zh.md packages/cli/README.md
```

**Steps:**

- [ ] **Step 1: Update `docs/guide/deployment.md` env var table**

Locate the existing env var table (contains `TASKCAST_POSTGRES_URL`). After the `TASKCAST_POSTGRES_URL` row, insert:

```markdown
| `TASKCAST_AUTO_MIGRATE` | Auto-apply Postgres migrations on startup: `1` / `true` / `yes` / `on` | `false` |
```

- [ ] **Step 2: Add the "Database migrations" section to `deployment.md`**

Find a sensible spot after the storage/configuration sections (before "Troubleshooting" if present). Insert:

````markdown
## Database migrations

Taskcast ships with PostgreSQL schema migrations that must be applied before
the server can write tasks/events to the database. You have two options:

### Manual (recommended for production)

Run migrations explicitly as a deploy step:

```sh
taskcast migrate --url postgres://user:pass@host/db
# or via config file:
taskcast migrate -c /etc/taskcast.yaml
# or via env var:
TASKCAST_POSTGRES_URL=postgres://... taskcast migrate -y
```

This gives you a clear deploy gate and an explicit audit trail.

### Automatic (convenient for dev, single-instance, and simple deployments)

Set `TASKCAST_AUTO_MIGRATE=1` (accepted values: `1`, `true`, `yes`, `on`,
case-insensitive) and any pending migrations will run before the HTTP server starts:

```sh
TASKCAST_AUTO_MIGRATE=1 \
TASKCAST_POSTGRES_URL=postgres://... \
taskcast start
```

Behavior:

- **Idempotent** — already-applied migrations are detected via the shared
  `_sqlx_migrations` tracking table and skipped.
- **Fail-fast** — if a migration fails (connection error, checksum mismatch,
  SQL error, or dirty migration), the server exits with status 1 and does
  **not** start. Fix the issue, then restart.
- **Safe across instances** — migrations run inside a transaction; if two
  replicas start simultaneously, only one will apply each migration.
- **No-op when Postgres isn't configured** — if you set `TASKCAST_AUTO_MIGRATE=1`
  but use memory/sqlite storage, an informational log is printed and startup
  proceeds normally.

> ⚠️ **Not recommended for multi-instance rolling deployments.** If you're
> running many replicas behind a load balancer, prefer running `taskcast migrate`
> as a separate deploy step so schema changes are a deliberate, observable
> operation — not a race between replicas.

The Node.js CLI and the Rust CLI share the same `_sqlx_migrations` tracking
table and the same SQL file set, so you can mix-and-match between them freely.
````

- [ ] **Step 3: Mirror the changes into `docs/guide/deployment.zh.md`**

Find the Chinese env var table and insert:

```markdown
| `TASKCAST_AUTO_MIGRATE` | 启动时自动应用 Postgres 迁移: `1` / `true` / `yes` / `on` | `false` |
```

Then add a Chinese version of the "Database migrations" section. Translation:

````markdown
## 数据库迁移

Taskcast 提供了一组 PostgreSQL schema 迁移,必须在服务器开始向数据库写入任务/事件之前应用。你有两种选择:

### 手动(生产环境推荐)

作为部署步骤显式运行迁移:

```sh
taskcast migrate --url postgres://user:pass@host/db
# 或通过配置文件:
taskcast migrate -c /etc/taskcast.yaml
# 或通过环境变量:
TASKCAST_POSTGRES_URL=postgres://... taskcast migrate -y
```

这种方式让 schema 变更成为一个有明确门禁、有审计轨迹的部署动作。

### 自动(开发、单实例、以及简单部署场景下方便)

设置 `TASKCAST_AUTO_MIGRATE=1`(接受的值: `1`、`true`、`yes`、`on`,大小写不敏感),服务器会在 HTTP 服务启动之前应用所有 pending 迁移:

```sh
TASKCAST_AUTO_MIGRATE=1 \
TASKCAST_POSTGRES_URL=postgres://... \
taskcast start
```

行为细节:

- **幂等** —— 已应用的迁移通过共享的 `_sqlx_migrations` 跟踪表识别并跳过。
- **失败即退出** —— 如果迁移失败(连接错误、checksum mismatch、SQL 错误、dirty migration),服务器以退出码 1 退出,**不会**启动。修复问题后重启。
- **多实例安全** —— 迁移在事务内运行;如果两个副本同时启动,只有一个会真正应用每条迁移。
- **未配置 Postgres 时无操作** —— 如果设了 `TASKCAST_AUTO_MIGRATE=1` 但使用 memory/sqlite 存储,会打印一条信息日志后正常启动。

> ⚠️ **多副本滚动部署不推荐使用。** 如果你在负载均衡后运行多个副本,建议把 `taskcast migrate` 作为独立的部署步骤执行,让 schema 变更成为一个有意识的、可观察的操作 —— 而不是副本之间的竞态。

Node.js CLI 和 Rust CLI 共享同一张 `_sqlx_migrations` 跟踪表和同一组 SQL 文件,所以你可以在两者之间自由切换。
````

- [ ] **Step 4: Update `packages/cli/README.md` env var table**

Find the existing env var table in `packages/cli/README.md` (contains `TASKCAST_POSTGRES_URL`). After that row, add:

```markdown
| `TASKCAST_AUTO_MIGRATE` | Auto-apply Postgres migrations on startup (`1`/`true`/`yes`/`on`) | `false` |
```

- [ ] **Step 5: Create the changeset file**

Create `.changeset/auto-migrate.md`:

```markdown
---
'@taskcast/cli': minor
'@taskcast/postgres': minor
---

Add automatic PostgreSQL migration on server startup

Set `TASKCAST_AUTO_MIGRATE=1` (or `true`/`yes`/`on`, case-insensitive) to
automatically apply pending migrations before the HTTP server starts.
Fail-fast on migration errors; no-op when Postgres isn't configured.
Works identically in the Node.js and Rust CLIs.

The `@taskcast/postgres` package's `runMigrations()` now also accepts a
pre-loaded `MigrationFile[]` in addition to a directory path, enabling
embedded migrations in the bundled CLI. Fixes the pre-existing "monorepo
only" TODO in `taskcast migrate`.
```

- [ ] **Step 6: Verify docs**

```sh
cd /Users/winrey/Projects/taskcast
grep -A1 TASKCAST_AUTO_MIGRATE docs/guide/deployment.md
grep -A1 TASKCAST_AUTO_MIGRATE docs/guide/deployment.zh.md
grep -A1 TASKCAST_AUTO_MIGRATE packages/cli/README.md
cat .changeset/auto-migrate.md
```

Expected: each command produces output containing the new row / section / changeset.

- [ ] **Step 7: Commit**

```bash
git add docs/guide/deployment.md \
        docs/guide/deployment.zh.md \
        packages/cli/README.md \
        .changeset/auto-migrate.md
git commit -m "$(cat <<'EOF'
docs: add TASKCAST_AUTO_MIGRATE + release changeset

Document the new env var in deployment.md (EN + ZH) and the CLI
README, including the full "Database migrations" section with manual
and automatic modes and the multi-replica rolling-deploy warning.
Add the release changeset marking @taskcast/cli and @taskcast/postgres
as minor (fixed versioning bumps all packages together).

Co-Authored-By: Claude Opus 4.6 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Final verification

After all tasks are complete, run the full quality gate before opening a PR:

```sh
cd /Users/winrey/Projects/taskcast

# 1. Clean build of everything
pnpm install
pnpm build

# 2. Type check
pnpm lint

# 3. Full test suite
pnpm test

# 4. Coverage check
pnpm test:coverage
# Confirm coverage is ≥ 90% on modified packages (per CLAUDE.md target: 100% where practical)

# 5. Rust tests
cd rust && cargo test && cargo build --release
cd ..

# 6. Staleness check (what CI will run)
node packages/cli/scripts/generate-migrations.mjs migrations/postgres packages/cli/src/generated/postgres-migrations.ts
git diff --exit-code packages/cli/src/generated/postgres-migrations.ts
```

Expected: all commands exit 0, no regressions in existing tests, new tests pass, generated file is in sync.

If everything passes, the feature is ready for code review.

