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

  beforeEach(() => {
    vi.clearAllMocks()
    // Set up default mock returns
    vi.mocked(buildMigrationFiles).mockReturnValue([])
  })

  // Test 1: Auto-migrate disabled (TASKCAST_AUTO_MIGRATE not set)
  it('returns immediately when auto-migrate is disabled (TASKCAST_AUTO_MIGRATE not set)', async () => {
    const env = {}
    const mockLog = vi.spyOn(console, 'log').mockImplementation(() => {})

    await performAutoMigrateIfEnabled(mockSql, env)

    expect(runMigrations).not.toHaveBeenCalled()
    expect(mockLog).not.toHaveBeenCalled()

    mockLog.mockRestore()
  })

  // Test 2: Auto-migrate disabled (TASKCAST_AUTO_MIGRATE = 'false')
  it('returns immediately when auto-migrate is explicitly disabled', async () => {
    const env = { TASKCAST_AUTO_MIGRATE: 'false' }
    const mockLog = vi.spyOn(console, 'log').mockImplementation(() => {})

    await performAutoMigrateIfEnabled(mockSql, env)

    expect(runMigrations).not.toHaveBeenCalled()
    expect(mockLog).not.toHaveBeenCalled()

    mockLog.mockRestore()
  })

  // Test 3: Auto-migrate enabled, Postgres not configured
  it('logs info and returns when Postgres is not configured', async () => {
    const env = { TASKCAST_AUTO_MIGRATE: 'true' }
    const mockLog = vi.spyOn(console, 'log').mockImplementation(() => {})

    await performAutoMigrateIfEnabled(mockSql, env)

    expect(mockLog).toHaveBeenCalledWith('[taskcast] Auto-migrate disabled: Postgres not configured')
    expect(runMigrations).not.toHaveBeenCalled()

    mockLog.mockRestore()
  })

  // Test 4: Auto-migrate enabled, Postgres configured, no migrations needed
  it('logs "Database schema up to date" when no migrations are applied', async () => {
    const env = {
      TASKCAST_AUTO_MIGRATE: 'true',
      TASKCAST_POSTGRES_URL: 'postgres://localhost/taskcast',
    }
    const mockLog = vi.spyOn(console, 'log').mockImplementation(() => {})
    vi.mocked(runMigrations).mockResolvedValueOnce({ applied: [], skipped: ['001_initial.sql', '002_workers.sql'] })

    await performAutoMigrateIfEnabled(mockSql, env)

    expect(mockLog).toHaveBeenCalledWith('[taskcast] Database schema up to date')
    expect(runMigrations).toHaveBeenCalled()

    mockLog.mockRestore()
  })

  // Test 5: Auto-migrate enabled, Postgres configured, 1 migration applied
  it('logs "Applied 1 migrations" when one migration is applied', async () => {
    const env = {
      TASKCAST_AUTO_MIGRATE: 'true',
      TASKCAST_POSTGRES_URL: 'postgres://localhost/taskcast',
    }
    const mockLog = vi.spyOn(console, 'log').mockImplementation(() => {})
    vi.mocked(runMigrations).mockResolvedValueOnce({ applied: ['001_initial.sql'], skipped: ['002_workers.sql'] })

    await performAutoMigrateIfEnabled(mockSql, env)

    expect(mockLog).toHaveBeenCalledWith('[taskcast] Applied 1 migrations')

    mockLog.mockRestore()
  })

  // Test 6: Auto-migrate enabled, Postgres configured, 2+ migrations applied
  it('logs "Applied N migrations" when multiple migrations are applied', async () => {
    const env = {
      TASKCAST_AUTO_MIGRATE: 'true',
      TASKCAST_POSTGRES_URL: 'postgres://localhost/taskcast',
    }
    const mockLog = vi.spyOn(console, 'log').mockImplementation(() => {})
    vi.mocked(runMigrations).mockResolvedValueOnce({
      applied: ['001_initial.sql', '002_workers.sql'],
      skipped: [],
    })

    await performAutoMigrateIfEnabled(mockSql, env)

    expect(mockLog).toHaveBeenCalledWith('[taskcast] Applied 2 migrations')

    mockLog.mockRestore()
  })

  // Test 7: Auto-migrate enabled, Postgres configured, runMigrations throws
  it('logs error and re-throws when migration fails', async () => {
    const env = {
      TASKCAST_AUTO_MIGRATE: 'true',
      TASKCAST_POSTGRES_URL: 'postgres://localhost/taskcast',
    }
    const mockLog = vi.spyOn(console, 'log').mockImplementation(() => {})
    const testError = new Error('Checksum mismatch detected')
    vi.mocked(runMigrations).mockRejectedValueOnce(testError)

    await expect(performAutoMigrateIfEnabled(mockSql, env)).rejects.toThrow(
      'Auto-migration failed: Checksum mismatch detected',
    )

    expect(mockLog).toHaveBeenCalledWith('[taskcast] Auto-migration failed: Checksum mismatch detected')

    mockLog.mockRestore()
  })

  // Test 8: Case-insensitive TASKCAST_AUTO_MIGRATE parsing
  it('recognizes case-insensitive truthy values for TASKCAST_AUTO_MIGRATE', async () => {
    const mockLog = vi.spyOn(console, 'log').mockImplementation(() => {})
    vi.mocked(runMigrations).mockResolvedValue({ applied: [], skipped: [] })

    // Test "TRUE"
    await performAutoMigrateIfEnabled(mockSql, {
      TASKCAST_AUTO_MIGRATE: 'TRUE',
      TASKCAST_POSTGRES_URL: 'postgres://localhost/taskcast',
    })

    expect(runMigrations).toHaveBeenCalledTimes(1)

    // Clear mocks and test "Yes"
    vi.clearAllMocks()
    vi.mocked(runMigrations).mockResolvedValue({ applied: [], skipped: [] })

    await performAutoMigrateIfEnabled(mockSql, {
      TASKCAST_AUTO_MIGRATE: 'Yes',
      TASKCAST_POSTGRES_URL: 'postgres://localhost/taskcast',
    })

    expect(runMigrations).toHaveBeenCalledTimes(1)

    mockLog.mockRestore()
  })

  // Test 9: Verify log messages don't have duplicate prefixes
  it('includes [taskcast] prefix exactly once in log messages', async () => {
    const env = {
      TASKCAST_AUTO_MIGRATE: 'true',
      TASKCAST_POSTGRES_URL: 'postgres://localhost/taskcast',
    }
    const mockLog = vi.spyOn(console, 'log').mockImplementation(() => {})
    vi.mocked(runMigrations).mockResolvedValueOnce({ applied: ['001.sql'], skipped: [] })

    await performAutoMigrateIfEnabled(mockSql, env)

    const logCall = mockLog.mock.calls[0]?.[0] as string
    const prefixCount = (logCall.match(/\[taskcast\]/g) ?? []).length
    expect(prefixCount).toBe(1)

    mockLog.mockRestore()
  })

  // Test 10: Verify buildMigrationFiles is called with EMBEDDED_MIGRATIONS
  it('calls buildMigrationFiles with EMBEDDED_MIGRATIONS', async () => {
    const env = {
      TASKCAST_AUTO_MIGRATE: 'true',
      TASKCAST_POSTGRES_URL: 'postgres://localhost/taskcast',
    }
    const mockLog = vi.spyOn(console, 'log').mockImplementation(() => {})
    vi.mocked(runMigrations).mockResolvedValueOnce({ applied: [], skipped: [] })
    vi.mocked(buildMigrationFiles).mockReturnValueOnce([])

    await performAutoMigrateIfEnabled(mockSql, env)

    expect(buildMigrationFiles).toHaveBeenCalled()

    mockLog.mockRestore()
  })

  // Test 11: Verify sql connection is passed to runMigrations
  it('passes sql connection to runMigrations', async () => {
    const env = {
      TASKCAST_AUTO_MIGRATE: 'true',
      TASKCAST_POSTGRES_URL: 'postgres://localhost/taskcast',
    }
    const mockLog = vi.spyOn(console, 'log').mockImplementation(() => {})
    vi.mocked(runMigrations).mockResolvedValueOnce({ applied: [], skipped: [] })

    await performAutoMigrateIfEnabled(mockSql, env)

    expect(runMigrations).toHaveBeenCalledWith(mockSql, expect.anything())

    mockLog.mockRestore()
  })

  // Test 12: Non-Error objects are converted to string
  it('handles non-Error objects thrown by runMigrations', async () => {
    const env = {
      TASKCAST_AUTO_MIGRATE: 'true',
      TASKCAST_POSTGRES_URL: 'postgres://localhost/taskcast',
    }
    const mockLog = vi.spyOn(console, 'log').mockImplementation(() => {})
    vi.mocked(runMigrations).mockRejectedValueOnce('String error')

    await expect(performAutoMigrateIfEnabled(mockSql, env)).rejects.toThrow(
      'Auto-migration failed: String error',
    )

    expect(mockLog).toHaveBeenCalledWith('[taskcast] Auto-migration failed: String error')

    mockLog.mockRestore()
  })
})
