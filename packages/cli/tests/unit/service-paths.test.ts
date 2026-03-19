import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest'
import { homedir } from 'os'
import { join } from 'path'

// We test getServicePaths by mocking process.platform
describe('getServicePaths', () => {
  beforeEach(() => {
    vi.resetModules()  // Clear ESM module cache between tests
  })

  afterEach(() => {
    vi.restoreAllMocks()
  })

  it('returns macOS paths on darwin', async () => {
    vi.stubGlobal('process', { ...process, platform: 'darwin' })
    const { getServicePaths } = await import('../../src/service/paths.js')
    const paths = getServicePaths()
    const home = homedir()

    expect(paths.plistOrUnitPath).toBe(join(home, 'Library/LaunchAgents/com.taskcast.daemon.plist'))
    expect(paths.logDir).toBe(join(home, 'Library/Application Support/taskcast'))
    expect(paths.stdoutLog).toBe(join(home, 'Library/Application Support/taskcast/taskcast.log'))
    expect(paths.stderrLog).toBe(join(home, 'Library/Application Support/taskcast/taskcast.err.log'))
    expect(paths.defaultConfigPath).toBe(join(home, '.taskcast/taskcast.config.yaml'))
    expect(paths.defaultDbPath).toBe(join(home, '.taskcast/taskcast.db'))
  })

  it('returns Linux paths on linux', async () => {
    vi.stubGlobal('process', { ...process, platform: 'linux' })
    const { getServicePaths } = await import('../../src/service/paths.js')
    const paths = getServicePaths()
    const home = homedir()

    expect(paths.plistOrUnitPath).toBe(join(home, '.config/systemd/user/taskcast.service'))
    expect(paths.defaultConfigPath).toBe(join(home, '.taskcast/taskcast.config.yaml'))
    expect(paths.defaultDbPath).toBe(join(home, '.taskcast/taskcast.db'))
    expect(paths.logDir).toBe('')
    expect(paths.stdoutLog).toBe('')
    expect(paths.stderrLog).toBe('')
  })

  it('throws on unsupported platform', async () => {
    vi.stubGlobal('process', { ...process, platform: 'win32' })
    const { getServicePaths } = await import('../../src/service/paths.js')
    expect(() => getServicePaths()).toThrow('Unsupported platform: win32')
  })
})
