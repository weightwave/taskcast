// packages/cli/tests/unit/service-resolve.test.ts
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest'

vi.mock('../../src/service/launchd.js', () => ({
  LaunchdServiceManager: vi.fn(),
}))
vi.mock('../../src/service/systemd.js', () => ({
  SystemdServiceManager: vi.fn(),
}))

describe('createServiceManager', () => {
  beforeEach(() => {
    vi.resetModules()  // Clear ESM module cache so platform stub is picked up
  })

  afterEach(() => {
    vi.restoreAllMocks()
  })

  it('returns LaunchdServiceManager on darwin', async () => {
    vi.stubGlobal('process', { ...process, platform: 'darwin' })
    const { createServiceManager } = await import('../../src/service/resolve.js')
    const { LaunchdServiceManager } = await import('../../src/service/launchd.js')
    createServiceManager()
    expect(LaunchdServiceManager).toHaveBeenCalled()
  })

  it('returns SystemdServiceManager on linux', async () => {
    vi.stubGlobal('process', { ...process, platform: 'linux' })
    const { createServiceManager } = await import('../../src/service/resolve.js')
    const { SystemdServiceManager } = await import('../../src/service/systemd.js')
    createServiceManager()
    expect(SystemdServiceManager).toHaveBeenCalled()
  })

  it('throws on unsupported platform', async () => {
    vi.stubGlobal('process', { ...process, platform: 'win32' })
    const { createServiceManager } = await import('../../src/service/resolve.js')
    expect(() => createServiceManager()).toThrow('Unsupported platform: win32')
  })
})
