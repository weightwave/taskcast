import { describe, it, expect, beforeAll, afterAll } from 'vitest'
import postgres from 'postgres'
import { GenericContainer, Wait, type StartedTestContainer } from 'testcontainers'
import { join } from 'node:path'
import { readFileSync } from 'node:fs'
import { runMigrations, buildMigrationFiles, computeChecksum, type EmbeddedMigration } from '../../src/migration-runner.js'

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
    .withWaitStrategy(Wait.forLogMessage(/ready to accept connections/, 2))
    .start()

  const connUri = `postgres://test:test@localhost:${container.getMappedPort(5432)}/testdb`
  sql = postgres(connUri)
}, 120000)

afterAll(async () => {
  await sql?.end()
  await container?.stop()
})

describe('migration runner integration', () => {
  it('applies all migrations on fresh database', async () => {
    const result = await runMigrations(sql, MIGRATIONS_DIR)

    expect(result.applied).toEqual([
      '001_initial.sql',
      '002_workers.sql',
      '003_storage_lifecycle.sql',
      '004_archive_receipt_coverage.sql',
      '005_task_creation_claim.sql',
    ])
    expect(result.skipped).toEqual([])

    // Verify tables were actually created
    const tables = await sql`
      SELECT table_name FROM information_schema.tables
      WHERE table_schema = 'public'
        AND table_name IN (
          'taskcast_tasks',
          'taskcast_events',
          'taskcast_worker_events',
          'taskcast_archive_generations',
          'taskcast_archive_batches',
          'taskcast_series_state',
          'taskcast_durable_assignments',
          'taskcast_terminal_outbox'
        )
      ORDER BY table_name
    `
    const tableNames = tables.map((r) => r.table_name)
    expect(tableNames).toContain('taskcast_tasks')
    expect(tableNames).toContain('taskcast_events')
    expect(tableNames).toContain('taskcast_worker_events')
    expect(tableNames).toContain('taskcast_archive_generations')
    expect(tableNames).toContain('taskcast_archive_batches')
    expect(tableNames).toContain('taskcast_series_state')
    expect(tableNames).toContain('taskcast_durable_assignments')
    expect(tableNames).toContain('taskcast_terminal_outbox')

    const lifecycleColumns = await sql`
      SELECT column_name FROM information_schema.columns
      WHERE table_schema = 'public'
        AND table_name = 'taskcast_tasks'
        AND column_name IN (
          'storage_state',
          'storage_epoch',
          'archive_watermark',
          'execution_deadline_at',
          'task_version',
          'creation_token',
          'creation_claimed_at',
          'creation_claim_expires_at',
          'creation_completed_at'
        )
      ORDER BY column_name
    `
    expect(lifecycleColumns.map((row) => row.column_name)).toEqual([
      'archive_watermark',
      'creation_claim_expires_at',
      'creation_claimed_at',
      'creation_completed_at',
      'creation_token',
      'execution_deadline_at',
      'storage_epoch',
      'storage_state',
      'task_version',
    ])
  })

  it('skips already-applied on second run', async () => {
    const result = await runMigrations(sql, MIGRATIONS_DIR)

    expect(result.applied).toEqual([])
    expect(result.skipped).toEqual([
      '001_initial.sql',
      '002_workers.sql',
      '003_storage_lifecycle.sql',
      '004_archive_receipt_coverage.sql',
      '005_task_creation_claim.sql',
    ])
  })

  it('writes _sqlx_migrations records with correct format', async () => {
    const rows = await sql`SELECT * FROM _sqlx_migrations ORDER BY version`

    expect(rows).toHaveLength(5)

    // Verify migration 001
    const row1 = rows[0]!
    expect(Number(row1.version)).toBe(1)
    expect(row1.description).toBe('initial')
    expect(row1.success).toBe(true)
    expect(Number(row1.execution_time)).toBeGreaterThanOrEqual(0)

    const file1Content = readFileSync(join(MIGRATIONS_DIR, '001_initial.sql'), 'utf8')
    const expectedChecksum1 = computeChecksum(file1Content)
    expect(Buffer.from(row1.checksum as Uint8Array).equals(expectedChecksum1)).toBe(true)

    // Verify migration 002
    const row2 = rows[1]!
    expect(Number(row2.version)).toBe(2)
    expect(row2.description).toBe('workers')
    expect(row2.success).toBe(true)
    expect(Number(row2.execution_time)).toBeGreaterThanOrEqual(0)

    const file2Content = readFileSync(join(MIGRATIONS_DIR, '002_workers.sql'), 'utf8')
    const expectedChecksum2 = computeChecksum(file2Content)
    expect(Buffer.from(row2.checksum as Uint8Array).equals(expectedChecksum2)).toBe(true)

    // Verify migration 003
    const row3 = rows[2]!
    expect(Number(row3.version)).toBe(3)
    expect(row3.description).toBe('storage lifecycle')
    expect(row3.success).toBe(true)
    expect(Number(row3.execution_time)).toBeGreaterThanOrEqual(0)

    const file3Content = readFileSync(join(MIGRATIONS_DIR, '003_storage_lifecycle.sql'), 'utf8')
    const expectedChecksum3 = computeChecksum(file3Content)
    expect(Buffer.from(row3.checksum as Uint8Array).equals(expectedChecksum3)).toBe(true)

    const row4 = rows[3]!
    expect(Number(row4.version)).toBe(4)
    expect(row4.description).toBe('archive receipt coverage')
    expect(row4.success).toBe(true)

    const file4Content = readFileSync(
      join(MIGRATIONS_DIR, '004_archive_receipt_coverage.sql'),
      'utf8',
    )
    const expectedChecksum4 = computeChecksum(file4Content)
    expect(Buffer.from(row4.checksum as Uint8Array).equals(expectedChecksum4)).toBe(true)

    const row5 = rows[4]!
    expect(Number(row5.version)).toBe(5)
    expect(row5.description).toBe('task creation claim')
    expect(row5.success).toBe(true)

    const file5Content = readFileSync(
      join(MIGRATIONS_DIR, '005_task_creation_claim.sql'),
      'utf8',
    )
    const expectedChecksum5 = computeChecksum(file5Content)
    expect(Buffer.from(row5.checksum as Uint8Array).equals(expectedChecksum5)).toBe(true)
  })

  it('rejects tampered checksum', async () => {
    // Save the original checksum for restoration
    const original = await sql`SELECT version, checksum FROM _sqlx_migrations WHERE version = 1`
    const originalChecksum = original[0]!.checksum

    // Tamper the checksum with garbage bytes
    const garbage = Buffer.alloc(48, 0xff)
    await sql`UPDATE _sqlx_migrations SET checksum = ${garbage} WHERE version = 1`

    await expect(runMigrations(sql, MIGRATIONS_DIR)).rejects.toThrow(/checksum mismatch/i)

    // Restore the correct checksum so subsequent tests aren't affected
    await sql`UPDATE _sqlx_migrations SET checksum = ${originalChecksum} WHERE version = 1`
  })

  it('rejects dirty (failed) migration', async () => {
    // Mark migration 1 as failed
    await sql`UPDATE _sqlx_migrations SET success = false WHERE version = 1`

    await expect(runMigrations(sql, MIGRATIONS_DIR)).rejects.toThrow(/dirty migration/i)

    // Restore success state
    await sql`UPDATE _sqlx_migrations SET success = true WHERE version = 1`
  })
})

describe('migration runner with embedded migrations (array overload)', () => {
  it('applies embedded migrations from MigrationFile[] array', async () => {
    // Create fresh database for embedded migration tests
    const container2 = await new GenericContainer('postgres:16-alpine')
      .withEnvironment({
        POSTGRES_USER: 'test',
        POSTGRES_PASSWORD: 'test',
        POSTGRES_DB: 'testdb',
      })
      .withExposedPorts(5432)
      .withWaitStrategy(Wait.forLogMessage(/ready to accept connections/, 2))
      .start()

    const connUri2 = `postgres://test:test@localhost:${container2.getMappedPort(5432)}/testdb`
    const sql2 = postgres(connUri2)

    try {
      const embedded: EmbeddedMigration[] = [
        {
          filename: '001_create_test_table.sql',
          sql: 'CREATE TABLE test_table (id SERIAL PRIMARY KEY, name TEXT)',
        },
        {
          filename: '002_add_column.sql',
          sql: 'ALTER TABLE test_table ADD COLUMN created_at TIMESTAMPTZ DEFAULT now()',
        },
      ]

      const migrations = buildMigrationFiles(embedded)
      const result = await runMigrations(sql2, migrations)

      expect(result.applied).toHaveLength(2)
      expect(result.applied).toContain('001_create_test_table.sql')
      expect(result.applied).toContain('002_add_column.sql')
      expect(result.skipped).toHaveLength(0)

      // Verify table was created
      const tables = await sql2`
        SELECT table_name FROM information_schema.tables
        WHERE table_schema = 'public' AND table_name = 'test_table'
      `
      expect(tables).toHaveLength(1)
    } finally {
      await sql2?.end()
      await container2?.stop()
    }
  })

  it('skips already-applied embedded migrations on second run', async () => {
    const container2 = await new GenericContainer('postgres:16-alpine')
      .withEnvironment({
        POSTGRES_USER: 'test',
        POSTGRES_PASSWORD: 'test',
        POSTGRES_DB: 'testdb',
      })
      .withExposedPorts(5432)
      .withWaitStrategy(Wait.forLogMessage(/ready to accept connections/, 2))
      .start()

    const connUri2 = `postgres://test:test@localhost:${container2.getMappedPort(5432)}/testdb`
    const sql2 = postgres(connUri2)

    try {
      const embedded: EmbeddedMigration[] = [
        { filename: '001_first.sql', sql: 'CREATE TABLE t1 (id INT)' },
      ]

      const migrations = buildMigrationFiles(embedded)

      // First run
      const result1 = await runMigrations(sql2, migrations)
      expect(result1.applied).toContain('001_first.sql')

      // Second run
      const result2 = await runMigrations(sql2, migrations)
      expect(result2.skipped).toContain('001_first.sql')
      expect(result2.applied).toHaveLength(0)
    } finally {
      await sql2?.end()
      await container2?.stop()
    }
  })

  it('verifies checksums for embedded migrations', async () => {
    const container2 = await new GenericContainer('postgres:16-alpine')
      .withEnvironment({
        POSTGRES_USER: 'test',
        POSTGRES_PASSWORD: 'test',
        POSTGRES_DB: 'testdb',
      })
      .withExposedPorts(5432)
      .withWaitStrategy(Wait.forLogMessage(/ready to accept connections/, 2))
      .start()

    const connUri2 = `postgres://test:test@localhost:${container2.getMappedPort(5432)}/testdb`
    const sql2 = postgres(connUri2)

    try {
      const embedded: EmbeddedMigration[] = [
        { filename: '001_test.sql', sql: 'CREATE TABLE t1 (id INT)' },
      ]

      const migrations = buildMigrationFiles(embedded)

      // Apply the migrations
      await runMigrations(sql2, migrations)

      // Verify _sqlx_migrations has correct checksum
      const rows = await sql2`SELECT * FROM _sqlx_migrations WHERE version = 1`
      expect(rows).toHaveLength(1)

      const row = rows[0]!
      const expectedChecksum = computeChecksum('CREATE TABLE t1 (id INT)')
      expect(Buffer.from(row.checksum as Uint8Array).equals(expectedChecksum)).toBe(true)
    } finally {
      await sql2?.end()
      await container2?.stop()
    }
  })

  it('detects checksum mismatch for embedded migrations', async () => {
    const container2 = await new GenericContainer('postgres:16-alpine')
      .withEnvironment({
        POSTGRES_USER: 'test',
        POSTGRES_PASSWORD: 'test',
        POSTGRES_DB: 'testdb',
      })
      .withExposedPorts(5432)
      .withWaitStrategy(Wait.forLogMessage(/ready to accept connections/, 2))
      .start()

    const connUri2 = `postgres://test:test@localhost:${container2.getMappedPort(5432)}/testdb`
    const sql2 = postgres(connUri2)

    try {
      const embedded1: EmbeddedMigration[] = [
        { filename: '001_test.sql', sql: 'CREATE TABLE t1 (id INT)' },
      ]

      const migrations1 = buildMigrationFiles(embedded1)
      await runMigrations(sql2, migrations1)

      // Now try to apply a different SQL with the same filename
      const embedded2: EmbeddedMigration[] = [
        { filename: '001_test.sql', sql: 'CREATE TABLE t1 (id INT, name TEXT)' },
      ]

      const migrations2 = buildMigrationFiles(embedded2)

      await expect(runMigrations(sql2, migrations2)).rejects.toThrow(/checksum mismatch/i)
    } finally {
      await sql2?.end()
      await container2?.stop()
    }
  })
})
