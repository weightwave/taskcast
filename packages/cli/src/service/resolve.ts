// packages/cli/src/service/resolve.ts
import type { ServiceManager } from './interface.js'
import { LaunchdServiceManager } from './launchd.js'
import { SystemdServiceManager } from './systemd.js'

export function createServiceManager(): ServiceManager {
  if (process.platform === 'darwin') return new LaunchdServiceManager()
  if (process.platform === 'linux') return new SystemdServiceManager()
  throw new Error(`Unsupported platform: ${process.platform}`)
}
