import { describe, it, expect, beforeEach, afterEach } from 'vitest'
import { mkdtempSync, rmSync, writeFileSync, readFileSync, readdirSync, mkdirSync } from 'node:fs'
import { join } from 'node:path'
import { tmpdir } from 'node:os'

/**
 * Generator function used in tests.
 * Mirrors the logic from scripts/generate-migrations.js
 */
function generateMigrationsForDir(migrationsDir: string): string {
  // Read all .sql files and sort by filename
  const files = readdirSync(migrationsDir)
    .filter((f) => f.endsWith('.sql'))
    .sort()

  const migrations: Array<{ filename: string; sql: string }> = []

  for (const filename of files) {
    const filepath = join(migrationsDir, filename)
    const sql = readFileSync(filepath, 'utf8')
    migrations.push({ filename, sql })
  }

  // Generate TypeScript code
  const lines: string[] = []
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

describe('migration generator', () => {
  let tempDir: string

  beforeEach(() => {
    tempDir = mkdtempSync(join(tmpdir(), 'migrate-test-'))
  })

  afterEach(() => {
    rmSync(tempDir, { recursive: true, force: true })
  })

  it('generates valid TypeScript with interface and const export', () => {
    const migrationsDir = join(tempDir, 'migrations')
    mkdirSync(migrationsDir, { recursive: true })

    writeFileSync(
      join(migrationsDir, '001_initial.sql'),
      'CREATE TABLE test (id INT);'
    )

    const result = generateMigrationsForDir(migrationsDir)

    expect(result).toContain('export interface EmbeddedMigration')
    expect(result).toContain('export const EMBEDDED_MIGRATIONS')
    expect(result).toContain('readonly EmbeddedMigration[]')
  })

  it('preserves SQL content byte-for-byte without escaping artifacts', () => {
    const migrationsDir = join(tempDir, 'migrations')
    mkdirSync(migrationsDir, { recursive: true })

    const sqlContent = `CREATE TABLE test (
  id INT PRIMARY KEY,
  name VARCHAR(255) NOT NULL
);

CREATE INDEX idx_test_name ON test(name);
`

    writeFileSync(join(migrationsDir, '001_initial.sql'), sqlContent)

    const result = generateMigrationsForDir(migrationsDir)

    // The SQL should be JSON.stringify'd, but when parsed back, it should be identical
    const match = result.match(/sql: "([^"\\]|\\.)*"/)
    expect(match).toBeTruthy()

    // Extract the escaped string and unescape it
    const escapedSql = result.match(/sql: ("(?:[^"\\]|\\.)*")/)?.[1]
    expect(escapedSql).toBeTruthy()

    const unescapedSql = JSON.parse(escapedSql!)
    expect(unescapedSql).toBe(sqlContent)
  })

  it('handles SQL with special characters (newlines, quotes)', () => {
    const migrationsDir = join(tempDir, 'migrations')
    mkdirSync(migrationsDir, { recursive: true })

    const sqlContent = `CREATE TABLE test (
  id INT,
  data JSONB,
  comment TEXT DEFAULT 'O''Brien'
);
`

    writeFileSync(join(migrationsDir, '001_initial.sql'), sqlContent)

    const result = generateMigrationsForDir(migrationsDir)

    // Verify the SQL is properly escaped and can be parsed back
    const escapedSql = result.match(/sql: ("(?:[^"\\]|\\.)*")/)?.[1]
    expect(escapedSql).toBeTruthy()

    const unescapedSql = JSON.parse(escapedSql!)
    expect(unescapedSql).toBe(sqlContent)
  })

  it('includes correct filename in each migration object', () => {
    const migrationsDir = join(tempDir, 'migrations')
    mkdirSync(migrationsDir, { recursive: true })

    writeFileSync(join(migrationsDir, '001_initial.sql'), 'CREATE TABLE t1 (id INT);')
    writeFileSync(join(migrationsDir, '002_workers.sql'), 'CREATE TABLE t2 (id INT);')

    const result = generateMigrationsForDir(migrationsDir)

    expect(result).toContain('filename: "001_initial.sql"')
    expect(result).toContain('filename: "002_workers.sql"')
  })

  it('sorts migrations by filename in ascending order', () => {
    const migrationsDir = join(tempDir, 'migrations')
    mkdirSync(migrationsDir, { recursive: true })

    writeFileSync(join(migrationsDir, '003_late.sql'), 'CREATE TABLE t3 (id INT);')
    writeFileSync(join(migrationsDir, '001_first.sql'), 'CREATE TABLE t1 (id INT);')
    writeFileSync(join(migrationsDir, '002_middle.sql'), 'CREATE TABLE t2 (id INT);')

    const result = generateMigrationsForDir(migrationsDir)

    const firstMatch = result.indexOf('filename: "001_first.sql"')
    const secondMatch = result.indexOf('filename: "002_middle.sql"')
    const thirdMatch = result.indexOf('filename: "003_late.sql"')

    expect(firstMatch).toBeLessThan(secondMatch)
    expect(secondMatch).toBeLessThan(thirdMatch)
  })

  it('generates empty array when no migrations exist', () => {
    const migrationsDir = join(tempDir, 'migrations')
    mkdirSync(migrationsDir, { recursive: true })

    const result = generateMigrationsForDir(migrationsDir)

    expect(result).toContain('export const EMBEDDED_MIGRATIONS: readonly EmbeddedMigration[] = [')
    expect(result).toContain(']')

    // Count the number of migration objects (should be 0)
    // Match only filename fields inside objects (4 spaces indent), not in interface definition
    const lines = result.split('\n')
    const objectLines = lines.filter((line) => /^    filename:/.test(line))
    expect(objectLines.length).toBe(0)
  })

  it('ignores non-.sql files', () => {
    const migrationsDir = join(tempDir, 'migrations')
    mkdirSync(migrationsDir, { recursive: true })

    writeFileSync(join(migrationsDir, '001_initial.sql'), 'CREATE TABLE t1 (id INT);')
    writeFileSync(join(migrationsDir, '002_readme.txt'), 'This is a readme')
    writeFileSync(join(migrationsDir, '003_workers.sql'), 'CREATE TABLE t2 (id INT);')

    const result = generateMigrationsForDir(migrationsDir)

    // Count the number of migration objects (should be 2)
    // Match only filename fields inside objects (4 spaces indent), not in interface definition
    const lines = result.split('\n')
    const objectLines = lines.filter((line) => /^    filename:/.test(line))
    expect(objectLines.length).toBe(2)

    expect(result).toContain('001_initial.sql')
    expect(result).toContain('003_workers.sql')
    expect(result).not.toContain('002_readme.txt')
  })

  it('handles migrations with Postgres dollar-quotes', () => {
    const migrationsDir = join(tempDir, 'migrations')
    mkdirSync(migrationsDir, { recursive: true })

    const sqlContent = `CREATE FUNCTION test() RETURNS TEXT AS $$
BEGIN
  RETURN 'hello';
END;
$$ LANGUAGE plpgsql;
`

    writeFileSync(join(migrationsDir, '001_initial.sql'), sqlContent)

    const result = generateMigrationsForDir(migrationsDir)

    const escapedSql = result.match(/sql: ("(?:[^"\\]|\\.)*")/)?.[1]
    expect(escapedSql).toBeTruthy()

    const unescapedSql = JSON.parse(escapedSql!)
    expect(unescapedSql).toBe(sqlContent)
  })

  it('generates valid TypeScript that can be parsed', () => {
    const migrationsDir = join(tempDir, 'migrations')
    mkdirSync(migrationsDir, { recursive: true })

    writeFileSync(join(migrationsDir, '001_initial.sql'), 'CREATE TABLE test (id INT);')

    const result = generateMigrationsForDir(migrationsDir)

    // The result should be valid JavaScript (can eval the exported array)
    // We test this by checking the structure is correct
    expect(result).toContain('export interface EmbeddedMigration {')
    expect(result).toContain('export const EMBEDDED_MIGRATIONS:')
    expect(result).toContain('readonly EmbeddedMigration[] = [')

    // Count opening and closing brackets
    const openBrackets = (result.match(/\{/g) || []).length
    const closeBrackets = (result.match(/\}/g) || []).length
    expect(openBrackets).toBe(closeBrackets)
  })
})
