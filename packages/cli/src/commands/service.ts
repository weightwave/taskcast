// packages/cli/src/commands/service.ts
import { Command } from 'commander'
import { existsSync, mkdirSync, readFileSync, writeFileSync } from 'fs'
import { dirname } from 'path'
import { createServiceManager } from '../service/resolve.js'
import { getServicePaths } from '../service/paths.js'
import type { ServiceInstallOptions } from '../service/interface.js'

function resolveNodePath(): string {
  return process.execPath
}

// Note: resolveEntryPoint uses import.meta.url which cannot be mocked in Vitest ESM tests.
// The entryPoint value is verified structurally (mockInstall called) but not by exact path.
/* v8 ignore next 3 */
function resolveEntryPoint(): string {
  return new URL('../index.js', import.meta.url).pathname
}

function ensureConfigFile(paths: ReturnType<typeof getServicePaths>, configOpt?: string): string {
  if (configOpt) return configOpt

  // Check if any config already exists
  if (existsSync(paths.defaultConfigPath)) {
    return paths.defaultConfigPath
  }

  // Create default config with SQLite
  const dir = dirname(paths.defaultConfigPath)
  mkdirSync(dir, { recursive: true })

  const dbPath = paths.defaultDbPath
  const content = `# Taskcast service configuration
port: 3721

adapters:
  shortTermStore:
    provider: sqlite
    path: ${dbPath}
  longTermStore:
    provider: sqlite
    path: ${dbPath}
`
  writeFileSync(paths.defaultConfigPath, content)
  console.log(`[taskcast] Created default config at ${paths.defaultConfigPath}`)

  return paths.defaultConfigPath
}

// Exported action functions — used directly by alias commands in index.ts
// _healthTimeoutMs is injectable for testing (default 5000ms)
export async function runServiceStart(_healthTimeoutMs = 5000): Promise<void> {
  const mgr = createServiceManager()
  const st = await mgr.status()
  if (st.state === 'running') {
    console.log('[taskcast] Service is already running.')
    return
  }
  await mgr.start()
  const paths = getServicePaths()
  const port = await getPortFromConfig()
  const started = await pollHealth(port, _healthTimeoutMs)
  if (started) {
    console.log(`[taskcast] Service started on http://localhost:${port}`)
  } else {
    console.error('[taskcast] Service may have failed to start. Check logs:')
    if (paths.stdoutLog) {
      console.error(`  ${paths.stdoutLog}`)
    } else {
      console.error('  journalctl --user -u taskcast')
    }
  }
}

export async function runServiceStop(): Promise<void> {
  const mgr = createServiceManager()
  const st = await mgr.status()
  if (st.state !== 'running') {
    console.log('[taskcast] Service is not running.')
    return
  }
  await mgr.stop()
  console.log('[taskcast] Service stopped.')
}

export async function runServiceStatus(): Promise<void> {
  const mgr = createServiceManager()
  const st = await mgr.status()
  if (st.state === 'not-installed') {
    console.log('Service:   not installed')
    return
  }
  if (st.state === 'stopped') {
    console.log('Service:   stopped')
    return
  }
  console.log(`Service:   running (pid ${st.pid})`)
  try {
    const port = await getPortFromConfig()
    const res = await fetch(`http://localhost:${port}/health/detail`)
    if (res.ok) {
      const body = await res.json() as { uptime?: number; adapters?: Record<string, { provider: string }> }
      if (body.uptime != null) {
        const h = Math.floor(body.uptime / 3600)
        const m = Math.floor((body.uptime % 3600) / 60)
        console.log(`Uptime:    ${h}h ${m}m`)
      }
      if (body.adapters?.shortTermStore) {
        console.log(`Storage:   ${body.adapters.shortTermStore.provider}`)
      }
    }
  } catch {
    // Health endpoint not reachable — just show basic status
  }
}

// _healthTimeoutMs is injectable for testing (default 5000ms)
export async function runServiceRestart(_healthTimeoutMs = 5000): Promise<void> {
  const mgr = createServiceManager()
  await mgr.restart()
  const paths = getServicePaths()
  const port = await getPortFromConfig()
  const started = await pollHealth(port, _healthTimeoutMs)
  if (started) {
    console.log(`[taskcast] Service restarted on http://localhost:${port}`)
  } else {
    console.error('[taskcast] Service may have failed to restart. Check logs:')
    if (paths.stdoutLog) {
      console.error(`  ${paths.stdoutLog}`)
    } else {
      console.error('  journalctl --user -u taskcast')
    }
  }
}

export function registerServiceCommand(program: Command): void {
  const service = program
    .command('service')
    .description('Manage Taskcast as a system service')

  service
    .command('install')
    .description('Register Taskcast as a system service with auto-start on boot')
    .option('-c, --config <path>', 'config file path')
    .option('-p, --port <port>', 'port to listen on', '3721')
    .option('-s, --storage <type>', 'storage backend: memory | redis | sqlite')
    .option('--db-path <path>', 'SQLite database file path')
    .action(async (opts: { config?: string; port: string; storage?: string; dbPath?: string }) => {
      const mgr = createServiceManager()
      const paths = getServicePaths()

      const configPath = ensureConfigFile(paths, opts.config)

      const installOpts: ServiceInstallOptions = {
        port: Number(opts.port),
        config: configPath,
        storage: opts.storage,
        dbPath: opts.dbPath,
        nodePath: resolveNodePath(),
        entryPoint: resolveEntryPoint(),
      }

      await mgr.install(installOpts)

      // Write state file with the installed port so start/restart poll the right port
      const statePath = paths.serviceStatePath
      mkdirSync(dirname(statePath), { recursive: true })
      writeFileSync(statePath, JSON.stringify({ port: Number(opts.port) }))

      console.log('[taskcast] Service installed successfully.')
      console.log('[taskcast] Run `taskcast service start` to start the service.')
    })

  service
    .command('uninstall')
    .description('Remove Taskcast system service registration')
    .action(async () => {
      const mgr = createServiceManager()
      await mgr.uninstall()
      console.log('[taskcast] Service uninstalled successfully.')
    })

  service
    .command('start')
    .description('Start Taskcast via system service manager')
    .action(() => runServiceStart())

  service
    .command('stop')
    .description('Stop Taskcast system service')
    .action(() => runServiceStop())

  service
    .command('restart')
    .description('Restart Taskcast system service')
    .action(() => runServiceRestart())

  service
    .command('reload')
    .description('Restart Taskcast system service (alias for restart)')
    .action(() => runServiceRestart())

  service
    .command('status')
    .description('Show Taskcast service status')
    .action(runServiceStatus)
}

async function getPortFromConfig(): Promise<number> {
  try {
    const paths = getServicePaths()
    // Try state file first (written by install, has the exact installed port)
    try {
      const state = JSON.parse(readFileSync(paths.serviceStatePath, 'utf8')) as { port?: number }
      if (typeof state.port === 'number') return state.port
    } catch {
      // State file not present — fall through to config file
    }
    const text = readFileSync(paths.defaultConfigPath, 'utf8')
    const match = text.match(/^port:\s*(\d+)/m)
    if (match) return Number(match[1])
  } catch {
    // Config not readable — use default
  }
  return 3721
}

export async function pollHealth(port: number, timeoutMs = 5000): Promise<boolean> {
  const start = Date.now()
  while (Date.now() - start < timeoutMs) {
    try {
      const res = await fetch(`http://localhost:${port}/health`)
      if (res.ok) return true
    } catch {
      // Not ready yet
    }
    await new Promise(r => setTimeout(r, 500))
  }
  return false
}
