// packages/cli/tests/unit/service-command.test.ts
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest'
import { Command } from 'commander'

// Mock service dependencies
const mockInstall = vi.fn()
const mockUninstall = vi.fn()
const mockStart = vi.fn()
const mockStop = vi.fn()
const mockRestart = vi.fn()
const mockStatus = vi.fn()

vi.mock('../../src/service/resolve.js', () => ({
  createServiceManager: vi.fn(() => ({
    install: mockInstall,
    uninstall: mockUninstall,
    start: mockStart,
    stop: mockStop,
    restart: mockRestart,
    status: mockStatus,
  })),
}))

vi.mock('../../src/service/paths.js', () => ({
  getServicePaths: vi.fn(() => ({
    defaultConfigPath: '/home/test/.taskcast/taskcast.config.yaml',
    defaultDbPath: '/home/test/.taskcast/taskcast.db',
    plistOrUnitPath: '/tmp/test.plist',
    logDir: '/tmp/logs',
    stdoutLog: '/tmp/logs/taskcast.log',
    stderrLog: '/tmp/logs/taskcast.err.log',
    serviceStatePath: '/home/test/.taskcast/service.state.json',
  })),
  LAUNCHD_LABEL: 'com.taskcast.daemon',
}))

vi.mock('fs', () => ({
  existsSync: vi.fn().mockReturnValue(true),
  mkdirSync: vi.fn(),
  writeFileSync: vi.fn(),
  readFileSync: vi.fn((path: string) => {
    if (String(path).endsWith('service.state.json')) return JSON.stringify({ port: 3721 })
    return 'port: 3721\n'
  }),
}))

import { registerServiceCommand, runServiceStart, runServiceRestart } from '../../src/commands/service.js'

// Mock fetch for health polling
const mockFetch = vi.fn()
vi.stubGlobal('fetch', mockFetch)

describe('registerServiceCommand', () => {
  let logSpy: ReturnType<typeof vi.spyOn>
  let errorSpy: ReturnType<typeof vi.spyOn>

  function makeProgram() {
    const program = new Command()
    program.exitOverride()
    registerServiceCommand(program)
    return program
  }

  beforeEach(() => {
    vi.clearAllMocks()
    logSpy = vi.spyOn(console, 'log').mockImplementation(() => {})
    errorSpy = vi.spyOn(console, 'error').mockImplementation(() => {})
    mockStatus.mockResolvedValue({ state: 'not-installed' })
    // Default fetch: health endpoint not available
    mockFetch.mockRejectedValue(new Error('ECONNREFUSED'))
  })

  afterEach(() => {
    logSpy.mockRestore()
    errorSpy.mockRestore()
  })

  it('registers service subcommand group with all subcommands', () => {
    const program = makeProgram()
    const serviceCmd = program.commands.find(c => c.name() === 'service')
    expect(serviceCmd).toBeDefined()

    const subNames = serviceCmd!.commands.map(c => c.name())
    expect(subNames).toContain('install')
    expect(subNames).toContain('uninstall')
    expect(subNames).toContain('start')
    expect(subNames).toContain('stop')
    expect(subNames).toContain('restart')
    expect(subNames).toContain('reload')
    expect(subNames).toContain('status')
  })

  describe('install', () => {
    it('calls ServiceManager.install and prints success', async () => {
      mockInstall.mockResolvedValue(undefined)
      await makeProgram().parseAsync(['node', 'test', 'service', 'install'])

      expect(mockInstall).toHaveBeenCalledOnce()
      expect(logSpy).toHaveBeenCalledWith(expect.stringContaining('installed successfully'))
    })

    it('creates default config when none exists', async () => {
      const { existsSync } = await import('fs')
      vi.mocked(existsSync).mockReturnValue(false)
      mockInstall.mockResolvedValue(undefined)

      await makeProgram().parseAsync(['node', 'test', 'service', 'install'])

      const { writeFileSync } = await import('fs')
      expect(writeFileSync).toHaveBeenCalled()
      expect(logSpy).toHaveBeenCalledWith(expect.stringContaining('Created default config'))
    })

    it('uses existing config without creating a new one', async () => {
      const { existsSync } = await import('fs')
      vi.mocked(existsSync).mockReturnValue(true)
      mockInstall.mockResolvedValue(undefined)

      await makeProgram().parseAsync(['node', 'test', 'service', 'install'])

      const { writeFileSync } = await import('fs')
      // Config file should not be written (it already exists), but state file should be
      expect(writeFileSync).not.toHaveBeenCalledWith(
        '/home/test/.taskcast/taskcast.config.yaml',
        expect.anything(),
      )
      expect(writeFileSync).toHaveBeenCalledWith(
        '/home/test/.taskcast/service.state.json',
        expect.any(String),
      )
    })

    it('passes --port option to ServiceManager.install', async () => {
      mockInstall.mockResolvedValue(undefined)

      await makeProgram().parseAsync(['node', 'test', 'service', 'install', '--port', '8080'])

      expect(mockInstall).toHaveBeenCalledWith(
        expect.objectContaining({ port: 8080 }),
      )
    })

    it('writes service state file during install', async () => {
      mockInstall.mockResolvedValue(undefined)

      await makeProgram().parseAsync(['node', 'test', 'service', 'install', '--port', '9000'])

      const { writeFileSync } = await import('fs')
      expect(writeFileSync).toHaveBeenCalledWith(
        '/home/test/.taskcast/service.state.json',
        JSON.stringify({ port: 9000 }),
      )
    })
  })

  describe('start', () => {
    it('calls ServiceManager.start when stopped', async () => {
      mockStatus.mockResolvedValue({ state: 'stopped' })
      mockStart.mockResolvedValue(undefined)
      mockFetch.mockResolvedValue({ ok: true })

      await makeProgram().parseAsync(['node', 'test', 'service', 'start'])

      expect(mockStart).toHaveBeenCalledOnce()
      expect(logSpy).toHaveBeenCalledWith(expect.stringContaining('started'))
    })

    it('prints already running when service is running', async () => {
      mockStatus.mockResolvedValue({ state: 'running', pid: 123 })

      await makeProgram().parseAsync(['node', 'test', 'service', 'start'])

      expect(mockStart).not.toHaveBeenCalled()
      expect(logSpy).toHaveBeenCalledWith(expect.stringContaining('already running'))
    })

    it('prints log path when health check times out (macOS)', async () => {
      mockStatus.mockResolvedValue({ state: 'stopped' })
      mockStart.mockResolvedValue(undefined)
      mockFetch.mockRejectedValue(new Error('ECONNREFUSED'))

      // Call exported function directly with short timeout to avoid 5s delay
      await runServiceStart(100)

      expect(errorSpy).toHaveBeenCalledWith(expect.stringContaining('failed to start'))
      expect(errorSpy).toHaveBeenCalledWith(expect.stringContaining('/tmp/logs/taskcast.log'))
    })
  })

  describe('stop', () => {
    it('calls ServiceManager.stop when running', async () => {
      mockStatus.mockResolvedValue({ state: 'running', pid: 42 })
      mockStop.mockResolvedValue(undefined)

      await makeProgram().parseAsync(['node', 'test', 'service', 'stop'])

      expect(mockStop).toHaveBeenCalledOnce()
      expect(logSpy).toHaveBeenCalledWith(expect.stringContaining('stopped'))
    })

    it('prints not running when service is not running', async () => {
      mockStatus.mockResolvedValue({ state: 'stopped' })

      await makeProgram().parseAsync(['node', 'test', 'service', 'stop'])

      expect(mockStop).not.toHaveBeenCalled()
      expect(logSpy).toHaveBeenCalledWith(expect.stringContaining('not running'))
    })
  })

  describe('restart', () => {
    it('calls ServiceManager.restart and prints success', async () => {
      mockRestart.mockResolvedValue(undefined)
      mockFetch.mockResolvedValue({ ok: true })

      await makeProgram().parseAsync(['node', 'test', 'service', 'restart'])

      expect(mockRestart).toHaveBeenCalledOnce()
      expect(logSpy).toHaveBeenCalledWith(expect.stringContaining('restarted'))
    })

    it('prints log path when health check times out after restart', async () => {
      mockRestart.mockResolvedValue(undefined)
      mockFetch.mockRejectedValue(new Error('ECONNREFUSED'))

      // Call exported function directly with short timeout to avoid 5s delay
      await runServiceRestart(100)

      expect(errorSpy).toHaveBeenCalledWith(expect.stringContaining('failed to restart'))
      expect(errorSpy).toHaveBeenCalledWith(expect.stringContaining('/tmp/logs/taskcast.log'))
    })
  })

  describe('reload', () => {
    it('calls ServiceManager.restart (same as restart)', async () => {
      mockRestart.mockResolvedValue(undefined)
      mockFetch.mockResolvedValue({ ok: true })

      await makeProgram().parseAsync(['node', 'test', 'service', 'reload'])

      expect(mockRestart).toHaveBeenCalledOnce()
    })
  })

  describe('status', () => {
    it('prints not installed when not installed', async () => {
      mockStatus.mockResolvedValue({ state: 'not-installed' })

      await makeProgram().parseAsync(['node', 'test', 'service', 'status'])

      expect(logSpy).toHaveBeenCalledWith(expect.stringContaining('not installed'))
    })

    it('prints stopped when stopped', async () => {
      mockStatus.mockResolvedValue({ state: 'stopped' })

      await makeProgram().parseAsync(['node', 'test', 'service', 'status'])

      expect(logSpy).toHaveBeenCalledWith(expect.stringContaining('stopped'))
    })

    it('prints running with PID when running', async () => {
      mockStatus.mockResolvedValue({ state: 'running', pid: 12345 })
      mockFetch.mockRejectedValue(new Error('not available'))

      await makeProgram().parseAsync(['node', 'test', 'service', 'status'])

      expect(logSpy).toHaveBeenCalledWith(expect.stringContaining('running'))
      expect(logSpy).toHaveBeenCalledWith(expect.stringContaining('12345'))
    })

    it('prints health details when health endpoint responds', async () => {
      mockStatus.mockResolvedValue({ state: 'running', pid: 1 })
      mockFetch.mockResolvedValue({
        ok: true,
        json: async () => ({
          uptime: 7500,
          adapters: { shortTermStore: { provider: 'sqlite' } },
        }),
      })

      await makeProgram().parseAsync(['node', 'test', 'service', 'status'])

      expect(logSpy).toHaveBeenCalledWith(expect.stringContaining('2h'))
      expect(logSpy).toHaveBeenCalledWith(expect.stringContaining('sqlite'))
    })
  })

  describe('uninstall', () => {
    it('calls ServiceManager.uninstall', async () => {
      mockUninstall.mockResolvedValue(undefined)

      await makeProgram().parseAsync(['node', 'test', 'service', 'uninstall'])

      expect(mockUninstall).toHaveBeenCalledOnce()
      expect(logSpy).toHaveBeenCalledWith(expect.stringContaining('uninstalled successfully'))
    })
  })
})
