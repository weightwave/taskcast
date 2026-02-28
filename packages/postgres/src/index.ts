export { PostgresLongTermStore } from './long-term.js'

import postgres from 'postgres'
import { PostgresLongTermStore } from './long-term.js'

export interface PostgresAdapterOptions {
  url: string
  /**
   * Table name prefix. Defaults to 'taskcast' (tables: taskcast_tasks, taskcast_events).
   * Can also be set via TASKCAST_PG_PREFIX environment variable.
   */
  prefix?: string
  ssl?: boolean
}

export function createPostgresAdapter(options: PostgresAdapterOptions): PostgresLongTermStore {
  const sql = postgres(options.url, { ssl: options.ssl ? 'require' : false })
  return new PostgresLongTermStore(sql, { prefix: options.prefix })
}
