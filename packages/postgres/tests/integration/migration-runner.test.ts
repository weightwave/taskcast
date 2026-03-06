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

    expect(result.applied).toEqual(['001_initial.sql', '002_workers.sql'])
    expect(result.skipped).toEqual([])

    // Verify tables were actually created
    const tables = await sql`
      SELECT table_name FROM information_schema.tables
      WHERE table_schema = 'public'
        AND table_name IN ('taskcast_tasks', 'taskcast_events', 'taskcast_worker_events')
      ORDER BY table_name
    `
    const tableNames = tables.map((r) => r.table_name)
    expect(tableNames).toContain('taskcast_tasks')
    expect(tableNames).toContain('taskcast_events')
    expect(tableNames).toContain('taskcast_worker_events')
  })

  it('skips already-applied on second run', async () => {
    const result = await runMigrations(sql, MIGRATIONS_DIR)

    expect(result.applied).toEqual([])
    expect(result.skipped).toEqual(['001_initial.sql', '002_workers.sql'])
  })

  it('writes _sqlx_migrations records with correct format', async () => {
    const rows = await sql`SELECT * FROM _sqlx_migrations ORDER BY version`

    expect(rows).toHaveLength(2)

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
