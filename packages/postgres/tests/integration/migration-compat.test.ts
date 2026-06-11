import { describe, it, expect, beforeAll, afterAll } from 'vitest'
import postgres from 'postgres'
import { GenericContainer, Wait, type StartedTestContainer } from 'testcontainers'
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
    .withWaitStrategy(Wait.forLogMessage(/ready to accept connections/, 2))
    .start()

  const connUri = `postgres://test:test@localhost:${container.getMappedPort(5432)}/testdb`
  sql = postgres(connUri)
}, 120000)

afterAll(async () => {
  await sql?.end()
  await container?.stop()
})

describe('cross-compatibility: TS runner with sqlx-style pre-applied migrations', () => {
  it('TS runner recognizes sqlx-style pre-applied migrations and applies only remaining', async () => {
    // Simulate what sqlx would do: manually create the _sqlx_migrations table
    // and insert a record for migration 001 with correct checksum, then execute the DDL
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

    // Compute the correct checksum for migration 001
    const file1Content = readFileSync(join(MIGRATIONS_DIR, '001_initial.sql'), 'utf8')
    const checksum1 = computeChecksum(file1Content)

    // Insert a sqlx-style record for migration 001
    await sql.unsafe(
      'INSERT INTO _sqlx_migrations (version, description, success, checksum, execution_time) VALUES ($1, $2, $3, $4, $5)',
      [1, 'initial', true, checksum1, 12345678],
    )

    // Execute the 001 DDL manually (as sqlx would have done)
    const migration001Sql = readFileSync(join(MIGRATIONS_DIR, '001_initial.sql'), 'utf8')
    await sql.unsafe(migration001Sql)

    // Now run the TS migration runner — it should skip 001 and apply only 002
    const result = await runMigrations(sql, MIGRATIONS_DIR)

    expect(result.skipped).toEqual(['001_initial.sql'])
    expect(result.applied).toEqual(['002_workers.sql', '003_client_seq.sql'])
  })

  it('TS-written records have correct sqlx field format', async () => {
    // After the previous test, version 2 was applied by the TS runner.
    // Verify its record matches the expected sqlx format.
    const rows = await sql`SELECT * FROM _sqlx_migrations WHERE version = 2`
    expect(rows).toHaveLength(1)

    const row = rows[0]!
    expect(row.description).toBe('workers')
    expect(row.success).toBe(true)
    expect(Number(row.execution_time)).toBeGreaterThanOrEqual(0)

    // Verify checksum matches SHA-384 of the file content
    const file2Content = readFileSync(join(MIGRATIONS_DIR, '002_workers.sql'), 'utf8')
    const expectedChecksum = computeChecksum(file2Content)
    expect(Buffer.from(row.checksum as Uint8Array).equals(expectedChecksum)).toBe(true)
  })
})
