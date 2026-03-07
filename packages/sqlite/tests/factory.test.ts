import { mkdtempSync, rmSync } from 'node:fs'
import { join } from 'node:path'
import { tmpdir } from 'node:os'
import { describe, it, expect, afterEach } from 'vitest'
import { createSqliteAdapters, SqliteShortTermStore, SqliteLongTermStore } from '../src/index.js'

describe('createSqliteAdapters', () => {
  let dir: string
  let cleanup: (() => void) | undefined

  afterEach(() => {
    cleanup?.()
    cleanup = undefined
  })

  function makeFactory(path: string) {
    const result = createSqliteAdapters({ path })
    cleanup = () => {
      result.db.close()
      rmSync(dir, { recursive: true, force: true })
    }
    return result
  }

  it('should return shortTermStore, longTermStore, and db', () => {
    dir = mkdtempSync(join(tmpdir(), 'taskcast-factory-'))
    const { shortTermStore, longTermStore, db } = makeFactory(join(dir, 'test.db'))

    expect(shortTermStore).toBeInstanceOf(SqliteShortTermStore)
    expect(longTermStore).toBeInstanceOf(SqliteLongTermStore)
    expect(db).toBeDefined()
    expect(db.open).toBe(true)
  })

  it('should apply WAL journal mode', () => {
    dir = mkdtempSync(join(tmpdir(), 'taskcast-factory-'))
    const { db } = makeFactory(join(dir, 'test.db'))

    const row = db.pragma('journal_mode') as { journal_mode: string }[]
    expect(row[0]!.journal_mode).toBe('wal')
  })

  it('should enable foreign keys', () => {
    dir = mkdtempSync(join(tmpdir(), 'taskcast-factory-'))
    const { db } = makeFactory(join(dir, 'test.db'))

    const row = db.pragma('foreign_keys') as { foreign_keys: number }[]
    expect(row[0]!.foreign_keys).toBe(1)
  })

  it('should run migration (tables exist)', () => {
    dir = mkdtempSync(join(tmpdir(), 'taskcast-factory-'))
    const { db } = makeFactory(join(dir, 'test.db'))

    const tables = db
      .prepare("SELECT name FROM sqlite_master WHERE type='table' ORDER BY name")
      .all() as { name: string }[]
    const names = tables.map((t) => t.name)

    expect(names).toContain('taskcast_tasks')
    expect(names).toContain('taskcast_events')
    expect(names).toContain('taskcast_series_latest')
    expect(names).toContain('taskcast_index_counters')
  })

  it('should produce working adapters (round-trip)', async () => {
    dir = mkdtempSync(join(tmpdir(), 'taskcast-factory-'))
    const { shortTermStore, longTermStore } = makeFactory(join(dir, 'test.db'))

    const task = { id: 'factory-1', status: 'pending' as const, createdAt: 1000, updatedAt: 1000 }

    await shortTermStore.saveTask(task)
    expect(await shortTermStore.getTask('factory-1')).toEqual(task)

    await longTermStore.saveTask(task)
    expect(await longTermStore.getTask('factory-1')).toEqual(task)
  })

  it('should use default path ./taskcast.db when no path or env var given', () => {
    // Ensure the env var is not set
    const original = process.env['TASKCAST_SQLITE_PATH']
    delete process.env['TASKCAST_SQLITE_PATH']

    // Use a temp dir as CWD so we don't pollute the repo
    dir = mkdtempSync(join(tmpdir(), 'taskcast-factory-default-'))
    const prevCwd = process.cwd()
    process.chdir(dir)

    try {
      const result = createSqliteAdapters()
      cleanup = () => {
        result.db.close()
        process.chdir(prevCwd)
        rmSync(dir, { recursive: true, force: true })
      }
      // The db name should be the default ./taskcast.db resolved from cwd
      expect(result.db.name).toContain('taskcast.db')
    } finally {
      if (original !== undefined) {
        process.env['TASKCAST_SQLITE_PATH'] = original
      }
    }
  })

  it('should use TASKCAST_SQLITE_PATH env var as default', () => {
    dir = mkdtempSync(join(tmpdir(), 'taskcast-factory-'))
    const envPath = join(dir, 'env-default.db')

    const original = process.env['TASKCAST_SQLITE_PATH']
    process.env['TASKCAST_SQLITE_PATH'] = envPath
    try {
      const result = createSqliteAdapters()
      cleanup = () => {
        result.db.close()
        rmSync(dir, { recursive: true, force: true })
      }
      expect(result.db.name).toBe(envPath)
    } finally {
      if (original === undefined) {
        delete process.env['TASKCAST_SQLITE_PATH']
      } else {
        process.env['TASKCAST_SQLITE_PATH'] = original
      }
    }
  })
})
