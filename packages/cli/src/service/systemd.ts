// packages/cli/src/service/systemd.ts
import { existsSync, writeFileSync, mkdirSync, unlinkSync } from 'fs'
import { execFileSync } from 'child_process'
import { dirname } from 'path'
import type { ServiceManager, ServiceInstallOptions, ServiceStatus } from './interface.js'
import { getServicePaths } from './paths.js'

const SYSTEMD_UNIT = 'taskcast'

function generateUnitFile(opts: ServiceInstallOptions): string {
  const args = [opts.entryPoint, 'start', '--port', String(opts.port)]
  if (opts.config) args.push('--config', opts.config)
  if (opts.storage) args.push('--storage', opts.storage)
  if (opts.dbPath) args.push('--db-path', opts.dbPath)

  // Quote any argument containing spaces so systemd parses them correctly
  const quote = (s: string) => (s.includes(' ') ? `"${s}"` : s)
  const execStart = [opts.nodePath, ...args].map(quote).join(' ')

  return `[Unit]
Description=Taskcast — unified task tracking and streaming service
After=network.target

[Service]
Type=simple
ExecStart=${execStart}
Restart=no

[Install]
WantedBy=default.target
`
}

export class SystemdServiceManager implements ServiceManager {
  private paths = getServicePaths()

  async install(opts: ServiceInstallOptions): Promise<void> {
    if (existsSync(this.paths.plistOrUnitPath)) {
      throw new Error(`Taskcast service is already installed. Run \`taskcast service uninstall\` first.`)
    }

    mkdirSync(dirname(this.paths.plistOrUnitPath), { recursive: true })

    const unit = generateUnitFile(opts)
    writeFileSync(this.paths.plistOrUnitPath, unit)

    execFileSync('systemctl', ['--user', 'daemon-reload'], { stdio: 'pipe' })
    execFileSync('systemctl', ['--user', 'enable', SYSTEMD_UNIT], { stdio: 'pipe' })
  }

  async uninstall(): Promise<void> {
    if (!existsSync(this.paths.plistOrUnitPath)) return

    const st = await this.status()
    if (st.state === 'running') {
      await this.stop()
    }

    execFileSync('systemctl', ['--user', 'disable', SYSTEMD_UNIT], { stdio: 'pipe' })
    execFileSync('systemctl', ['--user', 'daemon-reload'], { stdio: 'pipe' })
    unlinkSync(this.paths.plistOrUnitPath)
  }

  async start(): Promise<void> {
    if (!existsSync(this.paths.plistOrUnitPath)) {
      throw new Error(`Taskcast service is not installed. Run \`taskcast service install\` first.`)
    }

    execFileSync('systemctl', ['--user', 'start', SYSTEMD_UNIT], { stdio: 'pipe' })
  }

  async stop(): Promise<void> {
    execFileSync('systemctl', ['--user', 'stop', SYSTEMD_UNIT], { stdio: 'pipe' })
  }

  async restart(): Promise<void> {
    execFileSync('systemctl', ['--user', 'restart', SYSTEMD_UNIT], { stdio: 'pipe' })
  }

  async status(): Promise<ServiceStatus> {
    if (!existsSync(this.paths.plistOrUnitPath)) {
      return { state: 'not-installed' }
    }

    try {
      const output = execFileSync(
        'systemctl',
        ['--user', 'show', SYSTEMD_UNIT, '--property=ActiveState,MainPID'],
        { stdio: 'pipe' },
      ).toString()

      const activeMatch = output.match(/ActiveState=(\w+)/)
      const pidMatch = output.match(/MainPID=(\d+)/)

      if (activeMatch?.[1] === 'active' && pidMatch) {
        const pid = Number(pidMatch[1])
        if (pid > 0) return { state: 'running', pid }
      }

      return { state: 'stopped' }
    } catch {
      return { state: 'stopped' }
    }
  }
}
