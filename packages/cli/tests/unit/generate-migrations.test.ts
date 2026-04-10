import { describe, it, expect, beforeEach, afterEach } from 'vitest'
import { mkdtempSync, rmSync, writeFileSync, mkdirSync } from 'node:fs'
import { join, resolve } from 'node:path'
import { tmpdir } from 'node:os'

// Import the REAL generator function from the script, so tests exercise
// the actual implementation (not a local mirror that can drift out of sync).
// Use .mjs extension so Node.js treats the script as ESM without relying on
// a `"type": "module"` declaration in the repo-root package.json.
const scriptPath = resolve(
  __dirname,
  '..',
  '..',
  '..',
  '..',
  'scripts',
  'generate-migrations.mjs',
)
// Dynamic import ESM module — vitest supports this
const { generateMigrations } = (await import(scriptPath)) as {
  generateMigrations: (dir: string) => string
}

describe('migration generator (scripts/generate-migrations.mjs)', () => {
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
      'CREATE TABLE test (id INT);',
    )

    const result = generateMigrations(migrationsDir)

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

    const result = generateMigrations(migrationsDir)

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

    const result = generateMigrations(migrationsDir)

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

    const result = generateMigrations(migrationsDir)

    expect(result).toContain('filename: "001_initial.sql"')
    expect(result).toContain('filename: "002_workers.sql"')
  })

  it('sorts migrations by filename in ascending order', () => {
    const migrationsDir = join(tempDir, 'migrations')
    mkdirSync(migrationsDir, { recursive: true })

    writeFileSync(join(migrationsDir, '003_late.sql'), 'CREATE TABLE t3 (id INT);')
    writeFileSync(join(migrationsDir, '001_first.sql'), 'CREATE TABLE t1 (id INT);')
    writeFileSync(join(migrationsDir, '002_middle.sql'), 'CREATE TABLE t2 (id INT);')

    const result = generateMigrations(migrationsDir)

    const firstMatch = result.indexOf('filename: "001_first.sql"')
    const secondMatch = result.indexOf('filename: "002_middle.sql"')
    const thirdMatch = result.indexOf('filename: "003_late.sql"')

    expect(firstMatch).toBeLessThan(secondMatch)
    expect(secondMatch).toBeLessThan(thirdMatch)
  })

  it('throws when migrations directory is empty (prevents silent empty output)', () => {
    const migrationsDir = join(tempDir, 'migrations')
    mkdirSync(migrationsDir, { recursive: true })

    // No .sql files at all — must throw, not silently produce empty output.
    expect(() => generateMigrations(migrationsDir)).toThrow(/No \.sql migration files found/)
  })

  it('throws when directory contains only non-.sql files', () => {
    const migrationsDir = join(tempDir, 'migrations')
    mkdirSync(migrationsDir, { recursive: true })

    writeFileSync(join(migrationsDir, 'readme.txt'), 'not a migration')

    expect(() => generateMigrations(migrationsDir)).toThrow(/No \.sql migration files found/)
  })

  it('throws when a filename does not match the 3-digit zero-padded convention', () => {
    const migrationsDir = join(tempDir, 'migrations')
    mkdirSync(migrationsDir, { recursive: true })

    // Two-digit version — must be rejected
    writeFileSync(join(migrationsDir, '10_bad.sql'), 'CREATE TABLE t (id INT);')

    expect(() => generateMigrations(migrationsDir)).toThrow(/does not match the required/)
  })

  it('throws on filename without version prefix', () => {
    const migrationsDir = join(tempDir, 'migrations')
    mkdirSync(migrationsDir, { recursive: true })

    writeFileSync(join(migrationsDir, 'bad.sql'), 'CREATE TABLE t (id INT);')

    expect(() => generateMigrations(migrationsDir)).toThrow(/does not match the required/)
  })

  it('ignores non-.sql files alongside valid ones', () => {
    const migrationsDir = join(tempDir, 'migrations')
    mkdirSync(migrationsDir, { recursive: true })

    writeFileSync(join(migrationsDir, '001_initial.sql'), 'CREATE TABLE t1 (id INT);')
    writeFileSync(join(migrationsDir, 'readme.txt'), 'This is a readme')
    writeFileSync(join(migrationsDir, '002_workers.sql'), 'CREATE TABLE t2 (id INT);')

    const result = generateMigrations(migrationsDir)

    const lines = result.split('\n')
    const objectLines = lines.filter((line) => /^    filename:/.test(line))
    expect(objectLines.length).toBe(2)

    expect(result).toContain('001_initial.sql')
    expect(result).toContain('002_workers.sql')
    expect(result).not.toContain('readme.txt')
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

    const result = generateMigrations(migrationsDir)

    const escapedSql = result.match(/sql: ("(?:[^"\\]|\\.)*")/)?.[1]
    expect(escapedSql).toBeTruthy()

    const unescapedSql = JSON.parse(escapedSql!)
    expect(unescapedSql).toBe(sqlContent)
  })

  it('generates valid TypeScript structure with matching brackets', () => {
    const migrationsDir = join(tempDir, 'migrations')
    mkdirSync(migrationsDir, { recursive: true })

    writeFileSync(join(migrationsDir, '001_initial.sql'), 'CREATE TABLE test (id INT);')

    const result = generateMigrations(migrationsDir)

    expect(result).toContain('export interface EmbeddedMigration {')
    expect(result).toContain('export const EMBEDDED_MIGRATIONS:')
    expect(result).toContain('readonly EmbeddedMigration[] = [')

    const openBrackets = (result.match(/\{/g) || []).length
    const closeBrackets = (result.match(/\}/g) || []).length
    expect(openBrackets).toBe(closeBrackets)
  })
})
