export { SqliteShortTermStore } from './short-term.js'
export { SqliteLongTermStore } from './long-term.js'

import Database, { type Database as DatabaseType } from 'better-sqlite3'
import { readFileSync } from 'node:fs'
import { join, dirname } from 'node:path'
import { fileURLToPath } from 'node:url'
import { SqliteShortTermStore } from './short-term.js'
import { SqliteLongTermStore } from './long-term.js'

export interface SqliteAdapterOptions {
  path?: string
}

export function createSqliteAdapters(options: SqliteAdapterOptions = {}): {
  shortTermStore: SqliteShortTermStore
  longTermStore: SqliteLongTermStore
  db: DatabaseType
} {
  const dbPath = options.path ?? process.env['TASKCAST_SQLITE_PATH'] ?? './taskcast.db'
  const db = new Database(dbPath)
  db.pragma('journal_mode = WAL')
  db.pragma('foreign_keys = ON')

  const __dirname = dirname(fileURLToPath(import.meta.url))
  const migration = readFileSync(join(__dirname, '../migrations/001_initial.sql'), 'utf8')
  db.exec(migration)
  runStorageLifecycleMigration(db, __dirname)

  return {
    shortTermStore: new SqliteShortTermStore(db),
    longTermStore: new SqliteLongTermStore(db, true),
    db,
  }
}

const LIFECYCLE_COLUMNS: ReadonlyArray<readonly [string, string]> = [
  ['storage_state', "TEXT NOT NULL DEFAULT 'hot'"],
  ['storage_epoch', 'INTEGER NOT NULL DEFAULT 1'],
  ['active_release_generation', 'TEXT'],
  ['archive_watermark', 'INTEGER NOT NULL DEFAULT -1'],
  ['last_event_at', 'INTEGER'],
  ['cold_at', 'INTEGER'],
  ['execution_deadline_at', 'INTEGER'],
  ['task_version', 'INTEGER NOT NULL DEFAULT 0'],
  ['ttl_claim_token', 'TEXT'],
  ['ttl_claim_until', 'INTEGER'],
]

function runStorageLifecycleMigration(db: DatabaseType, baseDir: string): void {
  const migrate = db.transaction(() => {
    const existing = new Set(
      (db.pragma('table_info(taskcast_tasks)') as { name: string }[]).map((column) => column.name),
    )
    for (const [name, definition] of LIFECYCLE_COLUMNS) {
      if (!existing.has(name)) {
        db.exec(`ALTER TABLE taskcast_tasks ADD COLUMN ${name} ${definition}`)
      }
    }

    const migration = readFileSync(
      join(baseDir, '../migrations/002_storage_lifecycle.sql'),
      'utf8',
    )
    db.exec(migration)
  })

  migrate()
}
