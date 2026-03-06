export { PostgresLongTermStore } from './long-term.js'
export { runMigrations, loadMigrationFiles } from './migration-runner.js'

import postgres from 'postgres'
import { PostgresLongTermStore } from './long-term.js'

export interface PostgresAdapterOptions {
  url: string
  ssl?: boolean
}

export function createPostgresAdapter(options: PostgresAdapterOptions): PostgresLongTermStore {
  const sql = postgres(options.url, { ssl: options.ssl ? 'require' : false })
  return new PostgresLongTermStore(sql)
}
