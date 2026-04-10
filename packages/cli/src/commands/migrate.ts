import { Command } from 'commander'
import postgres from 'postgres'
import { loadConfigFile } from '@taskcast/core'
import { buildMigrationFiles, runMigrations } from '@taskcast/postgres'
import { EMBEDDED_MIGRATIONS } from '../generated-migrations.js'
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

      const sql = postgres(pgUrl)
      try {
        // Build migration files from embedded migrations
        const allFiles = buildMigrationFiles(EMBEDDED_MIGRATIONS)

        // Query already-applied versions from the tracking table.
        // If the table does not exist yet, treat all migrations as pending.
        // (runMigrations() below will create the table idempotently — we do
        // not duplicate the CREATE TABLE schema here to avoid drift.)
        let appliedVersions = new Set<number>()
        try {
          const appliedRows = await sql.unsafe(
            'SELECT version FROM _sqlx_migrations WHERE success = true',
          )
          appliedVersions = new Set(appliedRows.map((r) => Number(r['version'])))
        } catch (err) {
          const code = (err as { code?: string }).code
          // 42P01 = undefined_table (table doesn't exist yet — first-time migration)
          if (code !== '42P01') {
            throw err
          }
        }
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

        const result = await runMigrations(sql, allFiles)

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
