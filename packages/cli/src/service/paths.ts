import { homedir } from 'os'
import { join } from 'path'

export interface ServicePaths {
  plistOrUnitPath: string
  logDir: string
  stdoutLog: string
  stderrLog: string
  defaultConfigPath: string
  defaultDbPath: string
  serviceStatePath: string
}

export const LAUNCHD_LABEL = 'com.taskcast.daemon'

export function getServicePaths(): ServicePaths {
  const home = homedir()
  const platform = process.platform

  const defaultConfigPath = join(home, '.taskcast', 'taskcast.config.yaml')
  const defaultDbPath = join(home, '.taskcast', 'taskcast.db')
  const serviceStatePath = join(home, '.taskcast', 'service.state.json')

  if (platform === 'darwin') {
    const logDir = join(home, 'Library/Application Support/taskcast')
    return {
      plistOrUnitPath: join(home, 'Library/LaunchAgents', `${LAUNCHD_LABEL}.plist`),
      logDir,
      stdoutLog: join(logDir, 'taskcast.log'),
      stderrLog: join(logDir, 'taskcast.err.log'),
      defaultConfigPath,
      defaultDbPath,
      serviceStatePath,
    }
  }

  if (platform === 'linux') {
    return {
      plistOrUnitPath: join(home, '.config/systemd/user/taskcast.service'),
      logDir: '', // systemd uses journalctl
      stdoutLog: '', // journalctl --user -u taskcast
      stderrLog: '', // journalctl --user -u taskcast
      defaultConfigPath,
      defaultDbPath,
      serviceStatePath,
    }
  }

  throw new Error(`Unsupported platform: ${platform}`)
}
