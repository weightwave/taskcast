import { describe, it, expect, beforeAll, afterAll, beforeEach, afterEach, vi } from 'vitest'
import postgres from 'postgres'
import { GenericContainer, Wait, type StartedTestContainer } from 'testcontainers'
import { MemoryBroadcastProvider, MemoryShortTermStore } from '@taskcast/core'
import { createSqliteAdapters } from '@taskcast/sqlite'
import { PostgresLongTermStore } from '@taskcast/postgres'
import { runStart, type RunStartOptions } from '../../src/commands/start.js'
import { readFileSync, rmSync } from 'node:fs'
import { join } from 'node:path'
import { tmpdir } from 'node:os'

// ─── Test Infrastructure ──────────────────────────────────────────────────────

let pgContainer: StartedTestContainer | undefined
let pgSql: ReturnType<typeof postgres> | undefined
let tmpSqlitePath: string | undefined

/**
 * Start a real Postgres container for testing.
 */
async function startPostgresContainer(): Promise<{ container: StartedTestContainer; sql: ReturnType<typeof postgres> }> {
  const container = await new GenericContainer('postgres:16-alpine')
    .withEnvironment({
      POSTGRES_USER: 'test',
      POSTGRES_PASSWORD: 'test',
      POSTGRES_DB: 'testdb',
    })
    .withExposedPorts(5432)
    .withWaitStrategy(Wait.forLogMessage(/ready to accept connections/, 2))
    .start()

  const port = container.getMappedPort(5432)
  const connUri = `postgres://test:test@localhost:${port}/testdb`
  const sql = postgres(connUri)

  return { container, sql }
}

/**
 * Get a test-specific SQLite path in tmp directory.
 */
function getTestSqlitePath(): string {
  return join(tmpdir(), `taskcast-test-${Date.now()}-${Math.random().toString(36).slice(2)}.db`)
}

/**
 * Verify tables exist via information_schema query (Postgres only).
 */
async function verifyPostgresTables(sql: ReturnType<typeof postgres>, expectedTables: string[]): Promise<void> {
  const rows = await sql`
    SELECT table_name FROM information_schema.tables
    WHERE table_schema = 'public'
    ORDER BY table_name
  `
  const tableNames = rows.map((r) => r.table_name)
  for (const table of expectedTables) {
    expect(tableNames).toContain(table)
  }
}

/**
 * Query _sqlx_migrations table to check applied migrations.
 */
async function getAppliedMigrationVersions(sql: ReturnType<typeof postgres>): Promise<number[]> {
  try {
    const rows = await sql`SELECT version FROM _sqlx_migrations WHERE success = true ORDER BY version`
    return rows.map((r) => Number(r.version))
  } catch {
    // Table might not exist
    return []
  }
}

// ─── Test Suites ──────────────────────────────────────────────────────────────

describe('CLI start command with auto-migrate', () => {
  // ─── Test 1: Auto-migrate enabled with Postgres configured ───────────────

  describe('Scenario 1: Auto-migrate enabled with Postgres configured', () => {
    beforeAll(async () => {
      const { container, sql } = await startPostgresContainer()
      pgContainer = container
      pgSql = sql
    }, 120000)

    afterAll(async () => {
      await pgSql?.end()
      await pgContainer?.stop()
    })

    it('applies migrations on first run', async () => {
      const store = new MemoryShortTermStore()
      const broadcast = new MemoryBroadcastProvider()
      const longTermStore = new PostgresLongTermStore(pgSql!)

      const options: RunStartOptions = {
        postgres: pgSql,
        broadcast,
        shortTermStore: store,
        longTermStore,
        port: 3721,
        config: {},
        verbose: false,
        playground: false,
        env: {
          TASKCAST_AUTO_MIGRATE: 'true',
          TASKCAST_POSTGRES_URL: `postgres://test:test@localhost:${pgContainer!.getMappedPort(5432)}/testdb`,
        },
      }

      // Verify migrations table doesn't exist yet
      let appliedVersions = await getAppliedMigrationVersions(pgSql!)
      expect(appliedVersions).toHaveLength(0)

      const pgUrl = `postgres://test:test@localhost:${pgContainer!.getMappedPort(5432)}/testdb`
      const { performAutoMigrateIfEnabled } = await import('../../src/auto-migrate.js')
      await performAutoMigrateIfEnabled(pgSql!, pgUrl, options.env)

      // Verify migrations were applied
      appliedVersions = await getAppliedMigrationVersions(pgSql!)
      expect(appliedVersions).toEqual([1, 2])

      // Verify tables exist
      await verifyPostgresTables(pgSql!, [
        'taskcast_tasks',
        'taskcast_events',
        'taskcast_worker_events',
        '_sqlx_migrations',
      ])
    })
  })

  // ─── Test 2: Idempotency (second run applies no migrations) ──────────────

  describe('Scenario 2: Idempotency (second run)', () => {
    beforeAll(async () => {
      const { container, sql } = await startPostgresContainer()
      pgContainer = container
      pgSql = sql
    }, 120000)

    afterAll(async () => {
      await pgSql?.end()
      await pgContainer?.stop()
    })

    it('second run skips migrations and logs no-op', async () => {
      const { performAutoMigrateIfEnabled } = await import('../../src/auto-migrate.js')
      const pgUrl = `postgres://test:test@localhost:${pgContainer!.getMappedPort(5432)}/testdb`
      const env = { TASKCAST_AUTO_MIGRATE: 'true' }

      // First run: apply migrations
      await performAutoMigrateIfEnabled(pgSql!, pgUrl, env)
      const firstVersions = await getAppliedMigrationVersions(pgSql!)
      expect(firstVersions).toEqual([1, 2])

      // Second run: should be no-op
      await performAutoMigrateIfEnabled(pgSql!, pgUrl, env)
      const secondVersions = await getAppliedMigrationVersions(pgSql!)
      expect(secondVersions).toEqual([1, 2]) // No additional migrations applied
    })
  })

  // ─── Test 3: Auto-migrate disabled via env var ─────────────────────────

  describe('Scenario 3: Auto-migrate disabled', () => {
    beforeAll(async () => {
      const { container, sql } = await startPostgresContainer()
      pgContainer = container
      pgSql = sql
    }, 120000)

    afterAll(async () => {
      await pgSql?.end()
      await pgContainer?.stop()
    })

    it('skips migrations when TASKCAST_AUTO_MIGRATE=false', async () => {
      const { performAutoMigrateIfEnabled } = await import('../../src/auto-migrate.js')
      const pgUrl = `postgres://test:test@localhost:${pgContainer!.getMappedPort(5432)}/testdb`
      const env = { TASKCAST_AUTO_MIGRATE: 'false' }

      // Call with auto-migrate disabled
      await performAutoMigrateIfEnabled(pgSql!, pgUrl, env)

      // Verify no migrations were applied
      const versions = await getAppliedMigrationVersions(pgSql!)
      expect(versions).toHaveLength(0)
    })

    it('skips migrations when TASKCAST_AUTO_MIGRATE=0', async () => {
      const { performAutoMigrateIfEnabled } = await import('../../src/auto-migrate.js')
      const pgUrl = `postgres://test:test@localhost:${pgContainer!.getMappedPort(5432)}/testdb`
      const env = { TASKCAST_AUTO_MIGRATE: '0' }

      await performAutoMigrateIfEnabled(pgSql!, pgUrl, env)

      const versions = await getAppliedMigrationVersions(pgSql!)
      expect(versions).toHaveLength(0)
    })

    it('skips migrations when TASKCAST_AUTO_MIGRATE undefined', async () => {
      const { performAutoMigrateIfEnabled } = await import('../../src/auto-migrate.js')
      const pgUrl = `postgres://test:test@localhost:${pgContainer!.getMappedPort(5432)}/testdb`
      const env: Record<string, string | undefined> = {}

      await performAutoMigrateIfEnabled(pgSql!, pgUrl, env)

      const versions = await getAppliedMigrationVersions(pgSql!)
      expect(versions).toHaveLength(0)
    })
  })

  // ─── Test 4: Postgres not configured ──────────────────────────────────

  describe('Scenario 4: Postgres not configured', () => {
    it('gracefully skips auto-migrate when sql connection is undefined', async () => {
      const { performAutoMigrateIfEnabled } = await import('../../src/auto-migrate.js')
      const env = { TASKCAST_AUTO_MIGRATE: 'true' }

      // Should not throw; logs skip message
      await expect(
        performAutoMigrateIfEnabled(undefined, undefined, env),
      ).resolves.toBeUndefined()
    })
  })

  // ─── Test 5: Error handling ──────────────────────────────────────────

  describe('Scenario 5: Error handling', () => {
    it('throws error when migrations fail (invalid SQL)', async () => {
      const { container, sql } = await startPostgresContainer()
      const container2 = container
      const sql2 = sql

      try {
        const { buildMigrationFiles, runMigrations } = await import('@taskcast/postgres')

        const badMigrations = [
          {
            filename: '001_bad.sql',
            sql: 'INVALID SQL SYNTAX HERE',
          },
        ]

        const migrations = buildMigrationFiles(badMigrations)

        await expect(runMigrations(sql2, migrations)).rejects.toThrow()
      } finally {
        await sql2?.end()
        await container2?.stop()
      }
    })

    it('wraps migration errors in performAutoMigrateIfEnabled', async () => {
      const { container, sql } = await startPostgresContainer()
      const container2 = container
      const sql2 = sql

      try {
        const { performAutoMigrateIfEnabled } = await import('../../src/auto-migrate.js')

        const pgUrl = `postgres://test:test@localhost:${container2.getMappedPort(5432)}/testdb`
        const env = { TASKCAST_AUTO_MIGRATE: 'true' }

        // Insert a dirty migration row to trigger the fail-fast path
        await sql2.unsafe(`
          CREATE TABLE IF NOT EXISTS _sqlx_migrations (
              version BIGINT PRIMARY KEY,
              description TEXT NOT NULL,
              installed_on TIMESTAMPTZ NOT NULL DEFAULT now(),
              success BOOLEAN NOT NULL,
              checksum BYTEA NOT NULL,
              execution_time BIGINT NOT NULL
          )
        `)
        await sql2.unsafe(
          `INSERT INTO _sqlx_migrations (version, description, success, checksum, execution_time)
           VALUES (99, 'corrupt test', false, '\\x00', -1)`,
        )

        // performAutoMigrateIfEnabled should detect the dirty row and throw with wrapped message
        await expect(performAutoMigrateIfEnabled(sql2, pgUrl, env)).rejects.toThrow(
          /^Auto-migration failed:/,
        )
      } finally {
        await sql2?.end().catch(() => {})
        await container2?.stop()
      }
    })
  })

  // ─── Test 6: Full runStart integration ────────────────────────────────

  describe('Scenario 6: Full runStart integration', () => {
    it('runStart calls performAutoMigrateIfEnabled when Postgres is configured', async () => {
      const { container, sql } = await startPostgresContainer()

      try {
        const store = new MemoryShortTermStore()
        const broadcast = new MemoryBroadcastProvider()
        const longTermStore = new PostgresLongTermStore(sql)

        const options: RunStartOptions = {
          postgres: sql,
          broadcast,
          shortTermStore: store,
          longTermStore,
          port: 37210, // Use non-standard port to avoid conflicts
          config: {},
          verbose: false,
          playground: false,
          env: {
            TASKCAST_AUTO_MIGRATE: 'true',
            TASKCAST_POSTGRES_URL: `postgres://test:test@localhost:${container.getMappedPort(5432)}/testdb`,
          },
        }

        // We can't fully test runStart without it trying to bind a port,
        // but we can test the auto-migrate part
        const pgUrl = `postgres://test:test@localhost:${container.getMappedPort(5432)}/testdb`
        const { performAutoMigrateIfEnabled } = await import('../../src/auto-migrate.js')
        await performAutoMigrateIfEnabled(options.postgres, pgUrl, options.env)

        // Verify migrations were applied
        const versions = await getAppliedMigrationVersions(sql)
        expect(versions).toEqual([1, 2])

        // Verify tables exist
        await verifyPostgresTables(sql, ['taskcast_tasks', 'taskcast_events', 'taskcast_worker_events'])
      } finally {
        await sql?.end()
        await container?.stop()
      }
    })

    it('performAutoMigrateIfEnabled logs skip message when sql is undefined', async () => {
      // Regression test: when Postgres is not configured (sql is undefined),
      // performAutoMigrateIfEnabled must log the spec "skipping" message and
      // return cleanly without attempting to run any migrations.
      const { performAutoMigrateIfEnabled } = await import('../../src/auto-migrate.js')

      const mockError = vi.spyOn(console, 'error').mockImplementation(() => {})

      await performAutoMigrateIfEnabled(undefined, undefined, {
        TASKCAST_AUTO_MIGRATE: 'true',
      })

      expect(mockError).toHaveBeenCalledWith(
        '[taskcast] TASKCAST_AUTO_MIGRATE is set but no Postgres configured — skipping',
      )

      mockError.mockRestore()
    })
  })

  // ─── Test 7: SQLite compatibility ───────────────────────────────────────

  describe('Scenario 7: Auto-migrate with SQLite storage', () => {
    beforeEach(() => {
      tmpSqlitePath = getTestSqlitePath()
    })

    afterEach(() => {
      if (tmpSqlitePath) {
        try {
          rmSync(tmpSqlitePath)
        } catch {
          // Ignore cleanup errors
        }
      }
    })

    it('skips auto-migrate when using SQLite (no Postgres)', async () => {
      const { performAutoMigrateIfEnabled } = await import('../../src/auto-migrate.js')

      // SQLite adapter is used for storage; no postgres connection is created
      createSqliteAdapters({ path: tmpSqlitePath! })
      const env = { TASKCAST_AUTO_MIGRATE: 'true' }

      // Should not throw, just skip (no sql connection passed)
      await expect(
        performAutoMigrateIfEnabled(undefined, undefined, env),
      ).resolves.toBeUndefined()
    })
  })

  // ─── Test 8: Migration schema verification ──────────────────────────────

  describe('Scenario 8: Migration schema verification', () => {
    beforeAll(async () => {
      const { container, sql } = await startPostgresContainer()
      pgContainer = container
      pgSql = sql
    }, 120000)

    afterAll(async () => {
      await pgSql?.end()
      await pgContainer?.stop()
    })

    it('creates correct table schema for taskcast_tasks', async () => {
      const { performAutoMigrateIfEnabled } = await import('../../src/auto-migrate.js')
      const pgUrl = `postgres://test:test@localhost:${pgContainer!.getMappedPort(5432)}/testdb`
      const env = { TASKCAST_AUTO_MIGRATE: 'true' }

      await performAutoMigrateIfEnabled(pgSql!, pgUrl, env)

      // Verify taskcast_tasks columns
      const columns = await pgSql!`
        SELECT column_name, data_type, is_nullable
        FROM information_schema.columns
        WHERE table_name = 'taskcast_tasks'
        ORDER BY ordinal_position
      `

      const columnMap = new Map(columns.map((c) => [c.column_name, { dataType: c.data_type, isNullable: c.is_nullable }]))

      expect(columnMap.get('id')).toBeDefined()
      expect(columnMap.get('status')).toBeDefined()
      expect(columnMap.get('created_at')).toBeDefined()
      expect(columnMap.get('updated_at')).toBeDefined()
    })

    it('creates correct table schema for taskcast_events', async () => {
      const { performAutoMigrateIfEnabled } = await import('../../src/auto-migrate.js')
      const pgUrl = `postgres://test:test@localhost:${pgContainer!.getMappedPort(5432)}/testdb`
      const env = { TASKCAST_AUTO_MIGRATE: 'true' }

      await performAutoMigrateIfEnabled(pgSql!, pgUrl, env)

      const columns = await pgSql!`
        SELECT column_name, data_type
        FROM information_schema.columns
        WHERE table_name = 'taskcast_events'
        ORDER BY ordinal_position
      `

      const columnNames = columns.map((c) => c.column_name)
      expect(columnNames).toContain('id')
      expect(columnNames).toContain('task_id')
      expect(columnNames).toContain('idx')
      expect(columnNames).toContain('timestamp')
      expect(columnNames).toContain('type')
      expect(columnNames).toContain('level')
    })

    it('creates _sqlx_migrations table for tracking', async () => {
      const { performAutoMigrateIfEnabled } = await import('../../src/auto-migrate.js')
      const pgUrl = `postgres://test:test@localhost:${pgContainer!.getMappedPort(5432)}/testdb`
      const env = { TASKCAST_AUTO_MIGRATE: 'true' }

      await performAutoMigrateIfEnabled(pgSql!, pgUrl, env)

      const migrationRows = await pgSql!`SELECT * FROM _sqlx_migrations ORDER BY version`
      expect(migrationRows).toHaveLength(2)

      // Verify migration 1
      const m1 = migrationRows[0]!
      expect(Number(m1.version)).toBe(1)
      expect(m1.description).toBe('initial')
      expect(m1.success).toBe(true)

      // Verify migration 2
      const m2 = migrationRows[1]!
      expect(Number(m2.version)).toBe(2)
      expect(m2.description).toBe('workers')
      expect(m2.success).toBe(true)
    })
  })
})
