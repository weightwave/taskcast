import type postgres from 'postgres'
import { buildMigrationFiles, runMigrations } from '@taskcast/postgres'
import { parseBooleanEnv } from './helpers.js'
import { EMBEDDED_MIGRATIONS } from './generated-migrations.js'

/**
 * Automatically run database migrations if enabled.
 *
 * This function checks two conditions:
 * 1. TASKCAST_AUTO_MIGRATE env var is truthy (case-insensitive, parsed via parseBooleanEnv)
 * 2. A Postgres connection was provided (`sql` is not undefined)
 *
 * If both are true, runs migrations and logs the result.
 *
 * Log messages (spec §Error Handling & Log Messages):
 * - Banner (before running): `[taskcast] TASKCAST_AUTO_MIGRATE enabled — running Postgres migrations on <url>`
 * - Skip (no Postgres):     `[taskcast] TASKCAST_AUTO_MIGRATE is set but no Postgres configured — skipping`
 * - Up to date:             `[taskcast] Database schema up to date (<N> migration(s) already applied)`
 * - Applied:                `[taskcast] Applied <N> new migration(s): <filename1>, <filename2>, ...`
 * - Failure:                `[taskcast] Auto-migration failed: <error_message>`
 *
 * All messages go to stderr (console.error / eprintln!) to match the Rust
 * implementation's convention and keep stdout clean for machine-readable output.
 *
 * @param sql - Postgres connection instance, or undefined if Postgres is not configured
 * @param postgresUrl - Resolved Postgres URL (for log banner), or undefined
 * @param env - Environment variables (defaults to process.env for testability)
 * @throws Error with message "Auto-migration failed: <original_error>" if migration fails
 */
export async function performAutoMigrateIfEnabled(
  sql: ReturnType<typeof postgres> | undefined,
  postgresUrl: string | undefined,
  env: Record<string, string | undefined> = process.env,
): Promise<void> {
  // Check if auto-migrate is enabled
  const autoMigrateEnabled = parseBooleanEnv(env['TASKCAST_AUTO_MIGRATE'])
  if (!autoMigrateEnabled) {
    return
  }

  // Check if Postgres is actually configured (by the presence of an sql connection).
  // The env var TASKCAST_POSTGRES_URL alone is insufficient because users may
  // configure Postgres via the YAML config file only.
  if (!sql) {
    console.error('[taskcast] TASKCAST_AUTO_MIGRATE is set but no Postgres configured — skipping')
    return
  }

  // Log banner with URL (if available)
  const urlDisplay = postgresUrl ?? '<postgres>'
  console.error(`[taskcast] TASKCAST_AUTO_MIGRATE enabled — running Postgres migrations on ${urlDisplay}`)

  // Run migrations
  try {
    const result = await runMigrations(sql, buildMigrationFiles(EMBEDDED_MIGRATIONS))

    if (result.applied.length === 0) {
      console.error(
        `[taskcast] Database schema up to date (${result.skipped.length} migration(s) already applied)`,
      )
    } else {
      console.error(
        `[taskcast] Applied ${result.applied.length} new migration(s): ${result.applied.join(', ')}`,
      )
    }
  } catch (err) {
    const errorMessage = err instanceof Error ? err.message : String(err)
    console.error(`[taskcast] Auto-migration failed: ${errorMessage}`)
    throw new Error(`Auto-migration failed: ${errorMessage}`)
  }
}
