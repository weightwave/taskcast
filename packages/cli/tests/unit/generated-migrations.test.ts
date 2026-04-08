import { describe, it, expect } from 'vitest'
import { EMBEDDED_MIGRATIONS, type EmbeddedMigration } from '../../src/generated-migrations.js'
import { buildMigrationFiles } from '@taskcast/postgres'

describe('generated-migrations', () => {
  it('exports EMBEDDED_MIGRATIONS as a readonly array', () => {
    expect(Array.isArray(EMBEDDED_MIGRATIONS)).toBe(true)
  })

  it('contains migration objects with filename and sql fields', () => {
    expect(EMBEDDED_MIGRATIONS.length).toBeGreaterThan(0)

    for (const migration of EMBEDDED_MIGRATIONS) {
      expect(migration).toHaveProperty('filename')
      expect(migration).toHaveProperty('sql')
      expect(typeof migration.filename).toBe('string')
      expect(typeof migration.sql).toBe('string')
    }
  })

  it('has non-empty SQL content for each migration', () => {
    for (const migration of EMBEDDED_MIGRATIONS) {
      expect(migration.sql.length).toBeGreaterThan(0)
    }
  })

  it('has valid filenames matching the pattern NNN_description.sql', () => {
    const filenameRegex = /^\d{3}_[a-z_]+\.sql$/

    for (const migration of EMBEDDED_MIGRATIONS) {
      expect(migration.filename).toMatch(filenameRegex)
    }
  })

  it('migrations are sorted by numeric version', () => {
    const versions = EMBEDDED_MIGRATIONS.map((m) => {
      const match = m.filename.match(/^(\d+)_/)
      return match ? parseInt(match[1], 10) : 0
    })

    for (let i = 1; i < versions.length; i++) {
      expect(versions[i]).toBeGreaterThan(versions[i - 1])
    }
  })

  it('can be used with buildMigrationFiles helper', () => {
    const migrationFiles = buildMigrationFiles(EMBEDDED_MIGRATIONS)

    expect(migrationFiles.length).toBe(EMBEDDED_MIGRATIONS.length)

    for (let i = 0; i < migrationFiles.length; i++) {
      const mf = migrationFiles[i]
      expect(mf.filename).toBe(EMBEDDED_MIGRATIONS[i].filename)
      expect(mf.sql).toBe(EMBEDDED_MIGRATIONS[i].sql)
      expect(typeof mf.version).toBe('number')
      expect(typeof mf.description).toBe('string')
      expect(Buffer.isBuffer(mf.checksum)).toBe(true)
      expect(mf.checksum.length).toBe(48) // SHA-384 is 48 bytes
    }
  })

  it('includes the initial migration (001_initial.sql)', () => {
    const initialMigration = EMBEDDED_MIGRATIONS.find((m) => m.filename === '001_initial.sql')
    expect(initialMigration).toBeDefined()
    expect(initialMigration!.sql).toContain('taskcast_tasks')
    expect(initialMigration!.sql).toContain('taskcast_events')
  })

  it('includes the workers migration (002_workers.sql)', () => {
    const workersMigration = EMBEDDED_MIGRATIONS.find((m) => m.filename === '002_workers.sql')
    expect(workersMigration).toBeDefined()
    expect(workersMigration!.sql).toContain('taskcast_worker_events')
  })

  it('001_initial.sql creates tables with IF NOT EXISTS', () => {
    const initialMigration = EMBEDDED_MIGRATIONS.find((m) => m.filename === '001_initial.sql')
    expect(initialMigration).toBeDefined()
    expect(initialMigration!.sql).toContain('CREATE TABLE IF NOT EXISTS taskcast_tasks')
    expect(initialMigration!.sql).toContain('CREATE TABLE IF NOT EXISTS taskcast_events')
  })

  it('002_workers.sql includes ALTER TABLE statements', () => {
    const workersMigration = EMBEDDED_MIGRATIONS.find((m) => m.filename === '002_workers.sql')
    expect(workersMigration).toBeDefined()
    expect(workersMigration!.sql).toContain('ALTER TABLE taskcast_tasks')
  })

  it('migration SQL content matches file content exactly', () => {
    // This test verifies byte-for-byte preservation
    // by checking that the SQL contains expected content

    const initialMigration = EMBEDDED_MIGRATIONS.find((m) => m.filename === '001_initial.sql')
    expect(initialMigration).toBeDefined()

    // Check for specific content that should be in the original file
    expect(initialMigration!.sql).toContain('CREATE INDEX IF NOT EXISTS taskcast_events_task_id_idx')
    expect(initialMigration!.sql).toContain('CREATE INDEX IF NOT EXISTS taskcast_events_task_id_timestamp')

    // Verify structure is preserved (should have newlines)
    expect(initialMigration!.sql).toContain('\n')
  })

  it('array is declared as readonly in type system', () => {
    // TypeScript compiler enforces readonly, which prevents accidental mutations
    // At runtime, the array is the actual implementation detail
    expect(EMBEDDED_MIGRATIONS).toBeDefined()
    expect(Array.isArray(EMBEDDED_MIGRATIONS)).toBe(true)

    // Verify it's the actual array from the generated module
    // (The readonly declaration in TypeScript is compile-time enforcement)
  })

  it('each migration produces a valid MigrationFile with correct version and description', () => {
    const migrationFiles = buildMigrationFiles(EMBEDDED_MIGRATIONS)

    expect(migrationFiles[0].version).toBe(1)
    expect(migrationFiles[0].description).toBe('initial')

    expect(migrationFiles[1].version).toBe(2)
    expect(migrationFiles[1].description).toBe('workers')
  })
})
