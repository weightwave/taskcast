// packages/cli/tests/unit/service-launchd.test.ts
import { describe, it, expect, vi, beforeEach } from 'vitest'

vi.mock('fs', () => ({
  existsSync: vi.fn(),
  writeFileSync: vi.fn(),
  mkdirSync: vi.fn(),
  unlinkSync: vi.fn(),
}))

vi.mock('child_process', () => ({
  execFileSync: vi.fn(),
}))

// Mock paths so LaunchdServiceManager can be constructed on any platform
const MOCK_PATHS = {
  plistOrUnitPath: '/Users/test/Library/LaunchAgents/com.taskcast.daemon.plist',
  logDir: '/Users/test/Library/Application Support/taskcast',
  stdoutLog: '/Users/test/Library/Application Support/taskcast/taskcast.log',
  stderrLog: '/Users/test/Library/Application Support/taskcast/taskcast.err.log',
  defaultConfigPath: '/Users/test/.taskcast/taskcast.config.yaml',
  defaultDbPath: '/Users/test/.taskcast/taskcast.db',
}
vi.mock('../../src/service/paths.js', () => ({
  getServicePaths: vi.fn(() => MOCK_PATHS),
  LAUNCHD_LABEL: 'com.taskcast.daemon',
}))

import { existsSync, writeFileSync, mkdirSync, unlinkSync } from 'fs'
import { execFileSync } from 'child_process'
import { LaunchdServiceManager } from '../../src/service/launchd.js'

describe('LaunchdServiceManager', () => {
  let mgr: LaunchdServiceManager

  beforeEach(() => {
    vi.clearAllMocks()
    mgr = new LaunchdServiceManager()
  })

  describe('install', () => {
    it('creates log directory and writes plist file', async () => {
      vi.mocked(existsSync).mockReturnValue(false)

      await mgr.install({
        port: 3721,
        config: '/Users/test/.taskcast/taskcast.config.yaml',
        nodePath: '/usr/local/bin/node',
        entryPoint: '/usr/local/lib/node_modules/@taskcast/cli/dist/index.js',
      })

      expect(mkdirSync).toHaveBeenCalled()
      expect(writeFileSync).toHaveBeenCalledOnce()
      const [path, content] = vi.mocked(writeFileSync).mock.calls[0] as [string, string]
      expect(path).toContain('com.taskcast.daemon.plist')

      // Verify plist content
      expect(content).toContain('<key>Label</key>')
      expect(content).toContain('com.taskcast.daemon')
      expect(content).toContain('<key>RunAtLoad</key>')
      expect(content).toContain('<true/>')
      expect(content).toContain('<key>KeepAlive</key>')
      expect(content).toContain('<false/>')
      expect(content).toContain('/usr/local/bin/node')
      expect(content).toContain('start')
      expect(content).toContain('--config')
    })

    it('includes optional storage and dbPath args when provided', async () => {
      vi.mocked(existsSync).mockReturnValue(false)

      await mgr.install({
        port: 8080,
        nodePath: '/usr/local/bin/node',
        entryPoint: '/path/to/cli/dist/index.js',
        storage: 'sqlite',
        dbPath: '/home/user/data.db',
      })

      const [, content] = vi.mocked(writeFileSync).mock.calls[0] as [string, string]
      expect(content).toContain('--storage')
      expect(content).toContain('sqlite')
      expect(content).toContain('--db-path')
      expect(content).toContain('/home/user/data.db')
      expect(content).toContain('8080')
    })

    it('throws if already installed', async () => {
      vi.mocked(existsSync).mockReturnValue(true)

      await expect(mgr.install({
        port: 3721,
        nodePath: '/usr/local/bin/node',
        entryPoint: '/path/to/cli/dist/index.js',
      })).rejects.toThrow('already installed')
    })
  })

  describe('uninstall', () => {
    it('does nothing when plist does not exist', async () => {
      vi.mocked(existsSync).mockReturnValue(false)

      await mgr.uninstall()

      expect(unlinkSync).not.toHaveBeenCalled()
    })

    it('deletes plist when service is stopped', async () => {
      vi.mocked(existsSync).mockReturnValue(true)
      vi.mocked(execFileSync).mockImplementation((_cmd, args) => {
        if ((args as string[]).includes('list')) throw new Error('not found')
        return Buffer.from('')
      })

      await mgr.uninstall()

      expect(unlinkSync).toHaveBeenCalledWith(MOCK_PATHS.plistOrUnitPath)
    })

    it('stops service before deleting if running', async () => {
      vi.mocked(existsSync).mockReturnValue(true)
      const calls: string[][] = []
      vi.mocked(execFileSync).mockImplementation((_cmd, args) => {
        calls.push(args as string[])
        // First call is 'list' (status check) — return running PID
        if (calls.filter(c => c.includes('list')).length === 1) {
          return Buffer.from('"PID" = 99;')
        }
        return Buffer.from('')
      })

      await mgr.uninstall()

      expect(calls.some(c => c.includes('bootout'))).toBe(true)
      expect(unlinkSync).toHaveBeenCalled()
    })
  })

  describe('start', () => {
    it('calls launchctl bootstrap', async () => {
      vi.mocked(existsSync).mockReturnValue(true)
      vi.mocked(execFileSync).mockReturnValue(Buffer.from(''))

      await mgr.start()

      expect(execFileSync).toHaveBeenCalledWith(
        'launchctl',
        expect.arrayContaining(['bootstrap']),
        expect.any(Object),
      )
    })

    it('throws if not installed', async () => {
      vi.mocked(existsSync).mockReturnValue(false)

      await expect(mgr.start()).rejects.toThrow('not installed')
    })
  })

  describe('stop', () => {
    it('calls launchctl bootout', async () => {
      vi.mocked(execFileSync).mockReturnValue(Buffer.from(''))

      await mgr.stop()

      expect(execFileSync).toHaveBeenCalledWith(
        'launchctl',
        expect.arrayContaining(['bootout']),
        expect.any(Object),
      )
    })
  })

  describe('restart', () => {
    it('calls stop then start', async () => {
      vi.mocked(existsSync).mockReturnValue(true)
      const calls: string[][] = []
      vi.mocked(execFileSync).mockImplementation((_cmd, args) => {
        calls.push(args as string[])
        return Buffer.from('')
      })

      await mgr.restart()

      const bootoutIdx = calls.findIndex(c => c.includes('bootout'))
      const bootstrapIdx = calls.findIndex(c => c.includes('bootstrap'))
      expect(bootoutIdx).toBeGreaterThanOrEqual(0)
      expect(bootstrapIdx).toBeGreaterThanOrEqual(0)
      expect(bootoutIdx).toBeLessThan(bootstrapIdx)
    })
  })

  describe('status', () => {
    it('returns running with pid when service is loaded', async () => {
      vi.mocked(existsSync).mockReturnValue(true)
      vi.mocked(execFileSync).mockReturnValue(
        Buffer.from('"PID" = 12345;'),
      )

      const result = await mgr.status()
      expect(result).toEqual({ state: 'running', pid: 12345 })
    })

    it('returns stopped when launchctl list output has no PID', async () => {
      vi.mocked(existsSync).mockReturnValue(true)
      vi.mocked(execFileSync).mockReturnValue(Buffer.from('{}'))

      const result = await mgr.status()
      expect(result).toEqual({ state: 'stopped' })
    })

    it('returns stopped when launchctl list throws', async () => {
      vi.mocked(existsSync).mockReturnValue(true)
      vi.mocked(execFileSync).mockImplementation(() => {
        throw new Error('Could not find service')
      })

      const result = await mgr.status()
      expect(result).toEqual({ state: 'stopped' })
    })

    it('returns not-installed when plist does not exist', async () => {
      vi.mocked(existsSync).mockReturnValue(false)

      const result = await mgr.status()
      expect(result).toEqual({ state: 'not-installed' })
    })
  })
})
