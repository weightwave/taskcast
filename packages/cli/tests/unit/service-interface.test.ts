// packages/cli/tests/unit/service-interface.test.ts
import { describe, it, expect } from 'vitest'
import type { ServiceManager, ServiceInstallOptions, ServiceStatus } from '../../src/service/interface.js'

describe('ServiceManager interface', () => {
  it('ServiceStatus covers all states', () => {
    const running: ServiceStatus = { state: 'running', pid: 123, port: 3721 }
    const stopped: ServiceStatus = { state: 'stopped' }
    const notInstalled: ServiceStatus = { state: 'not-installed' }

    expect(running.state).toBe('running')
    expect(stopped.state).toBe('stopped')
    expect(notInstalled.state).toBe('not-installed')
  })

  it('ServiceInstallOptions accepts all fields', () => {
    const opts: ServiceInstallOptions = {
      port: 3721,
      config: '/home/user/.taskcast/taskcast.config.yaml',
      storage: 'sqlite',
      dbPath: '/home/user/.taskcast/taskcast.db',
      nodePath: '/usr/local/bin/node',
      entryPoint: '/usr/local/lib/node_modules/@taskcast/cli/dist/index.js',
    }
    expect(opts.port).toBe(3721)
  })
})
