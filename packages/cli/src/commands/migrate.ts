import { Command } from 'commander'
import postgres from 'postgres'
import { join, dirname } from 'path'
import { fileURLToPath } from 'url'
import { loadConfigFile } from '@taskcast/core'
import { loadMigrationFiles, runMigrations } from '@taskcast/postgres'
import { resolvePostgresUrl, formatDisplayUrl } from '../migrate-helpers.js'
import { promptConfirm } from '../utils.js'

export function registerMigrateCommand(program: Command): void {
  program
    .command('migrate')
    .description('Run pending PostgreSQL migrations')
    .option('--url <url>', 'Postgres connection URL (highest priority)')
    .option('-c, --config <path>', 'config file path')
    .option('-y, --yes', 'skip confirmation prompt')
    .action(async (options: { url?: string; config?: string; yes?: boolean }) => {
      // URL resolution priority: --url flag > env var > config file
      const { config: fileConfig } = await loadConfigFile(options.config)
      const pgUrl = resolvePostgresUrl({
        url: options.url,
        envUrl: process.env['TASKCAST_POSTGRES_URL'],
        configUrl: fileConfig.adapters?.longTermStore?.url,
      })

      if (!pgUrl) {
        console.error('[taskcast] No Postgres URL found. Provide --url, set TASKCAST_POSTGRES_URL, or configure adapters.longTermStore.url in config.')
        process.exit(1)
      }

      const target = formatDisplayUrl(pgUrl)

      // TODO: This path works in the monorepo only. For npm publishing,
      // migrations would need to be bundled with the package.
      const migrationsDir = join(dirname(fileURLToPath(import.meta.url)), '../../../../migrations/postgres')

      const sql = postgres(pgUrl)
      try {
        // Load migration files and check what's pending
        const allFiles = loadMigrationFiles(migrationsDir)

        // Ensure the migrations table exists so we can query applied versions
        await sql.unsafe(`
          CREATE TABLE IF NOT EXISTS _sqlx_migrations (
              version BIGINT PRIMARY KEY,
              description TEXT NOT NULL,
              installed_on TIMESTAMPTZ NOT NULL DEFAULT now(),
              success BOOLEAN NOT NULL,
              checksum BYTEA NOT NULL,
              execution_time BIGINT NOT NULL
          )
        `)
        const appliedRows = await sql.unsafe('SELECT version FROM _sqlx_migrations WHERE success = true')
        const appliedVersions = new Set(appliedRows.map((r) => Number(r['version'])))
        const pending = allFiles.filter((f) => !appliedVersions.has(f.version))

        if (pending.length === 0) {
          console.log('[taskcast] Database is up to date.')
          return
        }

        console.log(`[taskcast] Target: ${target}`)
        console.log(`[taskcast] Pending migrations:`)
        for (const file of pending) {
          console.log(`  ${file.filename}`)
        }

        if (!options.yes) {
          if (!process.stdin.isTTY) {
            console.error('[taskcast] No TTY detected. Re-run with --yes (-y) to skip confirmation.')
            process.exit(1)
          }
          const confirmed = await promptConfirm(`Apply ${pending.length} migration(s) to ${target}? (Y/n) `)
          if (!confirmed) {
            console.log('[taskcast] Migration cancelled.')
            return
          }
        }

        const result = await runMigrations(sql, migrationsDir)

        for (const filename of result.applied) {
          console.log(`  Applied ${filename}`)
        }
        console.log(`[taskcast] Applied ${result.applied.length} migration(s) successfully.`)
      } catch (err) {
        console.error(`[taskcast] Migration failed: ${(err as Error).message}`)
        process.exit(1)
      } finally {
        await sql.end()
      }
    })
}
