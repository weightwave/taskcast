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

  return {
    shortTermStore: new SqliteShortTermStore(db),
    longTermStore: new SqliteLongTermStore(db),
    db,
  }
}
