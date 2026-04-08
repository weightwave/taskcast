import { readFileSync, readdirSync, writeFileSync } from 'node:fs'
import { join, resolve, dirname } from 'node:path'
import { fileURLToPath } from 'node:url'

/**
 * Read all *.sql files from migrations/postgres/ directory and generate
 * TypeScript code embedding them as string literals.
 *
 * Files are sorted by filename (numeric order) and embedded as-is with
 * byte-for-byte preservation of SQL content.
 */
function generateMigrations() {
  const __dirname = dirname(fileURLToPath(import.meta.url))
  const repoRoot = resolve(__dirname, '..')
  const migrationsDir = join(repoRoot, 'migrations', 'postgres')

  // Read all .sql files and sort by filename
  const files = readdirSync(migrationsDir)
    .filter((f) => f.endsWith('.sql'))
    .sort()

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

// Main entry point
const code = generateMigrations()
const __dirname = dirname(fileURLToPath(import.meta.url))
const outputPath = resolve(__dirname, '..', 'packages', 'cli', 'src', 'generated-migrations.ts')
writeFileSync(outputPath, code, 'utf8')
console.log(`Generated ${outputPath}`)
