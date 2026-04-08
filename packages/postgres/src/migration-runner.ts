import { createHash } from 'node:crypto'
import { readFileSync, readdirSync } from 'node:fs'
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

export interface EmbeddedMigration {
  filename: string
  sql: string
}

/**
 * Parse a migration filename into version and description.
 *
 * Format: `{version}_{description}.sql`
 * - version: numeric prefix (e.g. "001" → 1)
 * - description: rest of filename with underscores replaced by spaces
 *
 * Returns null if the filename doesn't match the expected format.
 */
export function parseMigrationFilename(filename: string): { version: number; description: string } | null {
  if (!filename.endsWith('.sql')) return null

  const withoutExt = filename.slice(0, -4) // remove ".sql"
  const underscoreIdx = withoutExt.indexOf('_')
  if (underscoreIdx === -1) return null // no underscore means no description

  const versionStr = withoutExt.slice(0, underscoreIdx)
  const descriptionRaw = withoutExt.slice(underscoreIdx + 1)

  if (!/^\d+$/.test(versionStr)) return null
  if (descriptionRaw.length === 0) return null

  const version = parseInt(versionStr, 10)
  const description = descriptionRaw.replace(/_/g, ' ')

  return { version, description }
}

/**
 * Compute a SHA-384 hash of SQL content, matching sqlx's checksum behavior.
 * Returns a 48-byte Buffer.
 */
export function computeChecksum(sql: string): Buffer {
  return createHash('sha384').update(sql).digest()
}

/**
 * Read and parse all *.sql migration files from a directory, sorted by version.
 */
export function loadMigrationFiles(migrationsDir: string): MigrationFile[] {
  const entries = readdirSync(migrationsDir).filter((f) => f.endsWith('.sql')).sort()
  const files: MigrationFile[] = []

  for (const filename of entries) {
    const parsed = parseMigrationFilename(filename)
    if (!parsed) continue

    const content = readFileSync(join(migrationsDir, filename), 'utf8')
    files.push({
      version: parsed.version,
      description: parsed.description,
      sql: content,
      checksum: computeChecksum(content),
      filename,
    })
  }

  files.sort((a, b) => a.version - b.version)
  return files
}

/**
 * Convert embedded migrations (code-generated SQL strings) to MigrationFile[] array.
 * Parses filenames, computes checksums, and sorts by version.
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

const CREATE_MIGRATIONS_TABLE = `
CREATE TABLE IF NOT EXISTS _sqlx_migrations (
    version BIGINT PRIMARY KEY,
    description TEXT NOT NULL,
    installed_on TIMESTAMPTZ NOT NULL DEFAULT now(),
    success BOOLEAN NOT NULL,
    checksum BYTEA NOT NULL,
    execution_time BIGINT NOT NULL
)
`

/**
 * Run pending migrations and verify checksums of already-applied ones.
 *
 * This is fully compatible with sqlx's _sqlx_migrations table — the Rust
 * server and the TS server can share the same database and track migrations
 * in the same table.
 *
 * @param sql - Postgres connection instance
 * @param migrationsOrFiles - Either a directory path (string) or array of MigrationFile objects
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
    : migrationsOrFiles
  const appliedRows = await sql.unsafe('SELECT version, checksum FROM _sqlx_migrations ORDER BY version')
  const appliedMap = new Map<number, Buffer>()
  for (const row of appliedRows) {
    appliedMap.set(Number(row['version']), Buffer.from(row['checksum'] as Uint8Array))
  }

  const result: MigrationResult = { applied: [], skipped: [] }

  // 4. Process each migration file
  for (const file of localFiles) {
    const appliedChecksum = appliedMap.get(file.version)

    if (appliedChecksum) {
      // Already applied — verify checksum
      if (!file.checksum.equals(appliedChecksum)) {
        throw new Error(
          `Checksum mismatch for migration ${file.filename} (version ${file.version}). ` +
          'The applied migration differs from the local file.',
        )
      }
      result.skipped.push(file.filename)
    } else {
      // Not yet applied — execute it
      const startTime = process.hrtime.bigint()

      await sql.begin(async (tx) => {
        // Insert the migration record first with execution_time = -1 (in-progress marker)
        await tx.unsafe(
          'INSERT INTO _sqlx_migrations (version, description, success, checksum, execution_time) VALUES ($1, $2, TRUE, $3, -1)',
          [file.version, file.description, file.checksum],
        )

        // Execute the migration SQL
        await tx.unsafe(file.sql)
      })

      // Update execution_time with actual nanoseconds (outside transaction, matching sqlx behavior)
      const elapsed = process.hrtime.bigint() - startTime
      await sql.unsafe(
        'UPDATE _sqlx_migrations SET execution_time = $1 WHERE version = $2',
        [elapsed.toString(), file.version],
      )

      result.applied.push(file.filename)
    }
  }

  return result
}
