import type postgres from 'postgres'
import { buildMigrationFiles, runMigrations } from '@taskcast/postgres'
import { parseBooleanEnv } from './helpers.js'
import { EMBEDDED_MIGRATIONS } from './generated-migrations.js'

/**
 * Automatically run database migrations if enabled.
 *
 * This function checks two conditions:
 * 1. TASKCAST_AUTO_MIGRATE env var is truthy (case-insensitive, parsed via parseBooleanEnv)
 * 2. Postgres URL is configured (TASKCAST_POSTGRES_URL env var)
 *
 * If both are true, runs migrations and logs the result:
 * - "Applied N migrations" if N > 0
 * - "Database schema up to date" if N = 0
 * - "Auto-migration failed: <error_message>" if an error occurs
 *
 * If auto-migrate is disabled, returns immediately (no-op).
 * If Postgres is not configured, logs info message and returns (no-op).
 *
 * @param sql - Postgres connection instance
 * @param env - Environment variables (defaults to process.env for testability)
 * @throws Error with message "Auto-migration failed: <original_error>" if migration fails
 */
export async function performAutoMigrateIfEnabled(
  sql: ReturnType<typeof postgres>,
  env: Record<string, string | undefined> = process.env,
): Promise<void> {
  // Check if auto-migrate is enabled
  const autoMigrateEnabled = parseBooleanEnv(env['TASKCAST_AUTO_MIGRATE'])
  if (!autoMigrateEnabled) {
    return
  }

  // Check if Postgres is configured
  const postgresUrl = env['TASKCAST_POSTGRES_URL']
  if (!postgresUrl) {
    console.log('[taskcast] Auto-migrate disabled: Postgres not configured')
    return
  }

  // Run migrations
  try {
    const result = await runMigrations(sql, buildMigrationFiles(EMBEDDED_MIGRATIONS))

    if (result.applied.length === 0) {
      console.log('[taskcast] Database schema up to date')
    } else {
      console.log(`[taskcast] Applied ${result.applied.length} migrations`)
    }
  } catch (err) {
    const errorMessage = err instanceof Error ? err.message : String(err)
    console.log(`[taskcast] Auto-migration failed: ${errorMessage}`)
    throw new Error(`Auto-migration failed: ${errorMessage}`)
  }
}
