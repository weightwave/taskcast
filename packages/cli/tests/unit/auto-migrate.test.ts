import { describe, it, expect, vi, beforeEach } from 'vitest'
import type postgres from 'postgres'
import { performAutoMigrateIfEnabled } from '../../src/auto-migrate.js'

// Mock the dependencies
vi.mock('@taskcast/postgres', () => ({
  runMigrations: vi.fn(),
  buildMigrationFiles: vi.fn(),
}))

vi.mock('../../src/generated-migrations.js', () => ({
  EMBEDDED_MIGRATIONS: [
    { filename: '001_initial.sql', sql: 'CREATE TABLE test;' },
    { filename: '002_workers.sql', sql: 'ALTER TABLE test ADD COLUMN x INT;' },
  ],
}))

import { runMigrations, buildMigrationFiles } from '@taskcast/postgres'

describe('performAutoMigrateIfEnabled', () => {
  const mockSql = {} as ReturnType<typeof postgres>
  const testUrl = 'postgres://localhost/taskcast'

  beforeEach(() => {
    vi.clearAllMocks()
    // Set up default mock returns
    vi.mocked(buildMigrationFiles).mockReturnValue([])
  })

  // ─── Disabled scenarios ────────────────────────────────────────────────

  it('returns immediately when TASKCAST_AUTO_MIGRATE is not set', async () => {
    const env = {}
    const mockError = vi.spyOn(console, 'error').mockImplementation(() => {})

    await performAutoMigrateIfEnabled(mockSql, testUrl, env)

    expect(runMigrations).not.toHaveBeenCalled()
    expect(mockError).not.toHaveBeenCalled()

    mockError.mockRestore()
  })

  it('returns immediately when TASKCAST_AUTO_MIGRATE is explicitly false', async () => {
    const env = { TASKCAST_AUTO_MIGRATE: 'false' }
    const mockError = vi.spyOn(console, 'error').mockImplementation(() => {})

    await performAutoMigrateIfEnabled(mockSql, testUrl, env)

    expect(runMigrations).not.toHaveBeenCalled()
    expect(mockError).not.toHaveBeenCalled()

    mockError.mockRestore()
  })

  // ─── Postgres not configured ───────────────────────────────────────────

  it('logs skip message when sql is undefined (no Postgres configured)', async () => {
    const env = { TASKCAST_AUTO_MIGRATE: 'true' }
    const mockError = vi.spyOn(console, 'error').mockImplementation(() => {})

    await performAutoMigrateIfEnabled(undefined, undefined, env)

    expect(mockError).toHaveBeenCalledWith(
      '[taskcast] TASKCAST_AUTO_MIGRATE is set but no Postgres configured — skipping',
    )
    expect(runMigrations).not.toHaveBeenCalled()

    mockError.mockRestore()
  })

  it('logs skip message when sql is undefined even if URL env var is set', async () => {
    // Even if the env var is set, if the caller did not pass a sql connection
    // (e.g., because storage mode is sqlite), we still skip.
    const env = {
      TASKCAST_AUTO_MIGRATE: 'true',
      TASKCAST_POSTGRES_URL: 'postgres://localhost/taskcast',
    }
    const mockError = vi.spyOn(console, 'error').mockImplementation(() => {})

    await performAutoMigrateIfEnabled(undefined, undefined, env)

    expect(mockError).toHaveBeenCalledWith(
      '[taskcast] TASKCAST_AUTO_MIGRATE is set but no Postgres configured — skipping',
    )
    expect(runMigrations).not.toHaveBeenCalled()

    mockError.mockRestore()
  })

  it('proceeds when sql is provided even if URL env var is unset (config-file case)', async () => {
    // Regression test for config-file silent bypass: auto-migrate must work
    // when Postgres is configured only via the YAML config file.
    const env = { TASKCAST_AUTO_MIGRATE: 'true' }
    const mockError = vi.spyOn(console, 'error').mockImplementation(() => {})
    vi.mocked(runMigrations).mockResolvedValueOnce({ applied: [], skipped: [] })

    await performAutoMigrateIfEnabled(mockSql, 'postgres://from-config-file', env)

    expect(runMigrations).toHaveBeenCalled()

    mockError.mockRestore()
  })

  // ─── Banner log (before running) ───────────────────────────────────────

  it('logs banner with display URL (credentials stripped) before running migrations', async () => {
    // Regression test: the banner must not print raw credentials. A URL with
    // user:password must be formatted into host:port/db form via
    // formatDisplayUrl() to avoid leaking secrets into stderr/log aggregators.
    const env = { TASKCAST_AUTO_MIGRATE: 'true' }
    const mockError = vi.spyOn(console, 'error').mockImplementation(() => {})
    vi.mocked(runMigrations).mockResolvedValueOnce({ applied: [], skipped: [] })

    await performAutoMigrateIfEnabled(
      mockSql,
      'postgres://user:secretpass@db.example.com:5432/taskcast',
      env,
    )

    // Assert the banner line was emitted and the password does NOT appear
    const bannerCalls = mockError.mock.calls.filter((call) =>
      String(call[0]).includes('TASKCAST_AUTO_MIGRATE enabled'),
    )
    expect(bannerCalls).toHaveLength(1)
    const bannerLine = String(bannerCalls[0]?.[0])
    expect(bannerLine).toBe(
      '[taskcast] TASKCAST_AUTO_MIGRATE enabled — running Postgres migrations on db.example.com:5432/taskcast',
    )
    expect(bannerLine).not.toContain('secretpass')
    expect(bannerLine).not.toContain('user:')

    mockError.mockRestore()
  })

  // ─── Success scenarios ─────────────────────────────────────────────────

  it('logs "Database schema up to date (N migration(s) already applied)" when nothing new applied', async () => {
    const env = { TASKCAST_AUTO_MIGRATE: 'true' }
    const mockError = vi.spyOn(console, 'error').mockImplementation(() => {})
    vi.mocked(runMigrations).mockResolvedValueOnce({
      applied: [],
      skipped: ['001_initial.sql', '002_workers.sql'],
    })

    await performAutoMigrateIfEnabled(mockSql, testUrl, env)

    expect(mockError).toHaveBeenCalledWith(
      '[taskcast] Database schema up to date (2 migration(s) already applied)',
    )
    expect(runMigrations).toHaveBeenCalled()

    mockError.mockRestore()
  })

  it('logs "Applied 1 new migration(s)" with filename when one migration applied', async () => {
    const env = { TASKCAST_AUTO_MIGRATE: 'true' }
    const mockError = vi.spyOn(console, 'error').mockImplementation(() => {})
    vi.mocked(runMigrations).mockResolvedValueOnce({
      applied: ['003_add_index.sql'],
      skipped: ['001_initial.sql', '002_workers.sql'],
    })

    await performAutoMigrateIfEnabled(mockSql, testUrl, env)

    expect(mockError).toHaveBeenCalledWith(
      '[taskcast] Applied 1 new migration(s): 003_add_index.sql',
    )

    mockError.mockRestore()
  })

  it('logs "Applied N new migration(s)" with comma-separated filenames when multiple applied', async () => {
    const env = { TASKCAST_AUTO_MIGRATE: 'true' }
    const mockError = vi.spyOn(console, 'error').mockImplementation(() => {})
    vi.mocked(runMigrations).mockResolvedValueOnce({
      applied: ['001_initial.sql', '002_workers.sql'],
      skipped: [],
    })

    await performAutoMigrateIfEnabled(mockSql, testUrl, env)

    expect(mockError).toHaveBeenCalledWith(
      '[taskcast] Applied 2 new migration(s): 001_initial.sql, 002_workers.sql',
    )

    mockError.mockRestore()
  })

  // ─── Error handling ────────────────────────────────────────────────────

  it('re-throws wrapped error without logging a failure line (caller logs)', async () => {
    // The helper must NOT log its own failure line. The caller is responsible
    // for the single "[taskcast] Auto-migration failed: ..." output. Logging
    // here would produce a duplicate when the error propagates up.
    const env = { TASKCAST_AUTO_MIGRATE: 'true' }
    const mockError = vi.spyOn(console, 'error').mockImplementation(() => {})
    const testError = new Error('Checksum mismatch detected')
    vi.mocked(runMigrations).mockRejectedValueOnce(testError)

    await expect(performAutoMigrateIfEnabled(mockSql, testUrl, env)).rejects.toThrow(
      'Auto-migration failed: Checksum mismatch detected',
    )

    // Exactly one call expected: the "banner" log before runMigrations ran.
    // No "Auto-migration failed" line should have been emitted by the helper.
    const failureCalls = mockError.mock.calls.filter((call) =>
      String(call[0]).includes('Auto-migration failed'),
    )
    expect(failureCalls).toHaveLength(0)

    mockError.mockRestore()
  })

  it('handles non-Error objects thrown by runMigrations (no failure log)', async () => {
    const env = { TASKCAST_AUTO_MIGRATE: 'true' }
    const mockError = vi.spyOn(console, 'error').mockImplementation(() => {})
    vi.mocked(runMigrations).mockRejectedValueOnce('String error')

    await expect(performAutoMigrateIfEnabled(mockSql, testUrl, env)).rejects.toThrow(
      'Auto-migration failed: String error',
    )

    const failureCalls = mockError.mock.calls.filter((call) =>
      String(call[0]).includes('Auto-migration failed'),
    )
    expect(failureCalls).toHaveLength(0)

    mockError.mockRestore()
  })

  // ─── Parsing & dispatch ────────────────────────────────────────────────

  it('recognizes case-insensitive truthy values for TASKCAST_AUTO_MIGRATE', async () => {
    const mockError = vi.spyOn(console, 'error').mockImplementation(() => {})
    vi.mocked(runMigrations).mockResolvedValue({ applied: [], skipped: [] })

    await performAutoMigrateIfEnabled(mockSql, testUrl, { TASKCAST_AUTO_MIGRATE: 'TRUE' })
    expect(runMigrations).toHaveBeenCalledTimes(1)

    vi.clearAllMocks()
    vi.mocked(runMigrations).mockResolvedValue({ applied: [], skipped: [] })

    await performAutoMigrateIfEnabled(mockSql, testUrl, { TASKCAST_AUTO_MIGRATE: 'Yes' })
    expect(runMigrations).toHaveBeenCalledTimes(1)

    vi.clearAllMocks()
    vi.mocked(runMigrations).mockResolvedValue({ applied: [], skipped: [] })

    await performAutoMigrateIfEnabled(mockSql, testUrl, { TASKCAST_AUTO_MIGRATE: '1' })
    expect(runMigrations).toHaveBeenCalledTimes(1)

    vi.clearAllMocks()
    vi.mocked(runMigrations).mockResolvedValue({ applied: [], skipped: [] })

    await performAutoMigrateIfEnabled(mockSql, testUrl, { TASKCAST_AUTO_MIGRATE: 'on' })
    expect(runMigrations).toHaveBeenCalledTimes(1)

    mockError.mockRestore()
  })

  it('calls buildMigrationFiles with EMBEDDED_MIGRATIONS', async () => {
    const env = { TASKCAST_AUTO_MIGRATE: 'true' }
    const mockError = vi.spyOn(console, 'error').mockImplementation(() => {})
    vi.mocked(runMigrations).mockResolvedValueOnce({ applied: [], skipped: [] })
    vi.mocked(buildMigrationFiles).mockReturnValueOnce([])

    await performAutoMigrateIfEnabled(mockSql, testUrl, env)

    expect(buildMigrationFiles).toHaveBeenCalled()

    mockError.mockRestore()
  })

  it('passes sql connection to runMigrations', async () => {
    const env = { TASKCAST_AUTO_MIGRATE: 'true' }
    const mockError = vi.spyOn(console, 'error').mockImplementation(() => {})
    vi.mocked(runMigrations).mockResolvedValueOnce({ applied: [], skipped: [] })

    await performAutoMigrateIfEnabled(mockSql, testUrl, env)

    expect(runMigrations).toHaveBeenCalledWith(mockSql, expect.anything())

    mockError.mockRestore()
  })

  it('includes [taskcast] prefix exactly once in each log message', async () => {
    const env = { TASKCAST_AUTO_MIGRATE: 'true' }
    const mockError = vi.spyOn(console, 'error').mockImplementation(() => {})
    vi.mocked(runMigrations).mockResolvedValueOnce({ applied: ['001.sql'], skipped: [] })

    await performAutoMigrateIfEnabled(mockSql, testUrl, env)

    for (const call of mockError.mock.calls) {
      const message = call[0] as string
      const prefixCount = (message.match(/\[taskcast\]/g) ?? []).length
      expect(prefixCount).toBe(1)
    }

    mockError.mockRestore()
  })

  it('uses <postgres> placeholder in banner if URL not provided', async () => {
    const env = { TASKCAST_AUTO_MIGRATE: 'true' }
    const mockError = vi.spyOn(console, 'error').mockImplementation(() => {})
    vi.mocked(runMigrations).mockResolvedValueOnce({ applied: [], skipped: [] })

    await performAutoMigrateIfEnabled(mockSql, undefined, env)

    expect(mockError).toHaveBeenCalledWith(
      '[taskcast] TASKCAST_AUTO_MIGRATE enabled — running Postgres migrations on <postgres>',
    )

    mockError.mockRestore()
  })
})
