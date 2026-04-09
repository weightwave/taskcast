import { readFileSync, readdirSync, writeFileSync } from 'node:fs'
import { join, resolve, dirname } from 'node:path'
import { fileURLToPath } from 'node:url'

/**
 * Filename validation regex: 3-digit zero-padded version + underscore + description + .sql
 * Must match the Rust runtime's filename reconstruction assumption
 * (`format!("{:03}_{}.sql", version, description)`).
 */
const MIGRATION_FILENAME_RE = /^\d{3}_[a-zA-Z0-9_]+\.sql$/

/**
 * Read all *.sql files from migrations/postgres/ directory and generate
 * TypeScript code embedding them as string literals.
 *
 * Files are sorted by filename (numeric order) and embedded as-is with
 * byte-for-byte preservation of SQL content.
 *
 * Throws if:
 * - The migrations directory contains zero .sql files (prevents silent empty output)
 * - Any filename does not match the `NNN_description.sql` convention
 */
export function generateMigrations(migrationsDir) {
  // Read all .sql files and sort by filename
  const files = readdirSync(migrationsDir)
    .filter((f) => f.endsWith('.sql'))
    .sort()

  if (files.length === 0) {
    throw new Error(
      `No .sql migration files found in ${migrationsDir}. ` +
        `The embedded migrations file must not be empty.`,
    )
  }

  // Validate filenames against the NNN_description.sql convention
  for (const filename of files) {
    if (!MIGRATION_FILENAME_RE.test(filename)) {
      throw new Error(
        `Migration filename "${filename}" does not match the required ` +
          `3-digit zero-padded convention (expected pattern: ^\\d{3}_[a-zA-Z0-9_]+\\.sql$). ` +
          `Rename the file to match, e.g. "001_initial.sql".`,
      )
    }
  }

  const migrations = []

  for (const filename of files) {
    const filepath = join(migrationsDir, filename)
    const sql = readFileSync(filepath, 'utf8')
    migrations.push({ filename, sql })
  }

  // Generate TypeScript code
  const lines = []
  lines.push("/**")
  lines.push(" * Auto-generated migration embeddings.")
  lines.push(" * Do not edit manually — run: pnpm generate-migrations")
  lines.push(" */")
  lines.push("")
  lines.push("export interface EmbeddedMigration {")
  lines.push("  filename: string")
  lines.push("  sql: string")
  lines.push("}")
  lines.push("")
  lines.push("export const EMBEDDED_MIGRATIONS: readonly EmbeddedMigration[] = [")

  for (const migration of migrations) {
    lines.push("  {")
    lines.push(`    filename: ${JSON.stringify(migration.filename)},`)
    lines.push(`    sql: ${JSON.stringify(migration.sql)},`)
    lines.push("  },")
  }

  lines.push("]")
  lines.push("")

  return lines.join('\n')
}

// Main entry point (only runs when invoked directly, not when imported for tests)
const __filename = fileURLToPath(import.meta.url)
const isMainModule = process.argv[1] === __filename
if (isMainModule) {
  const __dirname = dirname(__filename)
  const repoRoot = resolve(__dirname, '..')
  const migrationsDir = join(repoRoot, 'migrations', 'postgres')
  const code = generateMigrations(migrationsDir)
  const outputPath = resolve(
    __dirname,
    '..',
    'packages',
    'cli',
    'src',
    'generated-migrations.ts',
  )
  writeFileSync(outputPath, code, 'utf8')
  console.log(`Generated ${outputPath}`)
}
