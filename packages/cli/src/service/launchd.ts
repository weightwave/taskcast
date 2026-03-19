// packages/cli/src/service/launchd.ts
import { existsSync, writeFileSync, mkdirSync, unlinkSync } from 'fs'
import { execFileSync } from 'child_process'
import { dirname } from 'path'
import type { ServiceManager, ServiceInstallOptions, ServiceStatus } from './interface.js'
import { getServicePaths, LAUNCHD_LABEL } from './paths.js'

function getUid(): number {
  return process.getuid?.() ?? 501
}

function generatePlist(opts: ServiceInstallOptions, paths: ReturnType<typeof getServicePaths>): string {
  const args = [opts.entryPoint, 'start', '--port', String(opts.port)]
  if (opts.config) args.push('--config', opts.config)
  if (opts.storage) args.push('--storage', opts.storage)
  if (opts.dbPath) args.push('--db-path', opts.dbPath)

  const programArgs = [opts.nodePath, ...args]
    .map(a => `      <string>${a}</string>`)
    .join('\n')

  return `<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>${LAUNCHD_LABEL}</string>
    <key>ProgramArguments</key>
    <array>
${programArgs}
    </array>
    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <false/>
    <key>StandardOutPath</key>
    <string>${paths.stdoutLog}</string>
    <key>StandardErrorPath</key>
    <string>${paths.stderrLog}</string>
</dict>
</plist>
`
}

export class LaunchdServiceManager implements ServiceManager {
  private paths = getServicePaths()

  async install(opts: ServiceInstallOptions): Promise<void> {
    if (existsSync(this.paths.plistOrUnitPath)) {
      throw new Error(`Taskcast service is already installed. Run \`taskcast service uninstall\` first.`)
    }

    // Ensure log directory exists
    mkdirSync(this.paths.logDir, { recursive: true })
    // Ensure plist parent directory exists
    mkdirSync(dirname(this.paths.plistOrUnitPath), { recursive: true })

    const plist = generatePlist(opts, this.paths)
    writeFileSync(this.paths.plistOrUnitPath, plist)
  }

  async uninstall(): Promise<void> {
    if (!existsSync(this.paths.plistOrUnitPath)) return

    // Stop if running
    const st = await this.status()
    if (st.state === 'running') {
      await this.stop()
    }

    unlinkSync(this.paths.plistOrUnitPath)
  }

  async start(): Promise<void> {
    if (!existsSync(this.paths.plistOrUnitPath)) {
      throw new Error(`Taskcast service is not installed. Run \`taskcast service install\` first.`)
    }

    const uid = getUid()
    execFileSync('launchctl', ['bootstrap', `gui/${uid}`, this.paths.plistOrUnitPath], { stdio: 'pipe' })
  }

  async stop(): Promise<void> {
    const uid = getUid()
    execFileSync('launchctl', ['bootout', `gui/${uid}/${LAUNCHD_LABEL}`], { stdio: 'pipe' })
  }

  async restart(): Promise<void> {
    await this.stop()
    await this.start()
  }

  async status(): Promise<ServiceStatus> {
    if (!existsSync(this.paths.plistOrUnitPath)) {
      return { state: 'not-installed' }
    }

    try {
      const output = execFileSync('launchctl', ['list', LAUNCHD_LABEL], { stdio: 'pipe' }).toString()
      const pidMatch = output.match(/"PID"\s*=\s*(\d+)/)
      if (pidMatch) {
        return { state: 'running', pid: Number(pidMatch[1]) }
      }
      return { state: 'stopped' }
    } catch {
      return { state: 'stopped' }
    }
  }
}
