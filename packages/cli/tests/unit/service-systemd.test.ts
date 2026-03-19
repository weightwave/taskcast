// packages/cli/tests/unit/service-systemd.test.ts
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

// Mock paths so SystemdServiceManager can be constructed on any platform
const MOCK_PATHS = {
  plistOrUnitPath: '/home/test/.config/systemd/user/taskcast.service',
  logDir: '',
  stdoutLog: '',
  stderrLog: '',
  defaultConfigPath: '/home/test/.taskcast/taskcast.config.yaml',
  defaultDbPath: '/home/test/.taskcast/taskcast.db',
}
vi.mock('../../src/service/paths.js', () => ({
  getServicePaths: vi.fn(() => MOCK_PATHS),
}))

import { existsSync, writeFileSync, mkdirSync, unlinkSync } from 'fs'
import { execFileSync } from 'child_process'
import { SystemdServiceManager } from '../../src/service/systemd.js'

describe('SystemdServiceManager', () => {
  let mgr: SystemdServiceManager

  beforeEach(() => {
    vi.clearAllMocks()
    mgr = new SystemdServiceManager()
  })

  describe('install', () => {
    it('creates unit directory and writes service file', async () => {
      vi.mocked(existsSync).mockReturnValue(false)
      vi.mocked(execFileSync).mockReturnValue(Buffer.from(''))

      await mgr.install({
        port: 3721,
        config: '/home/user/.taskcast/taskcast.config.yaml',
        nodePath: '/usr/bin/node',
        entryPoint: '/usr/lib/node_modules/@taskcast/cli/dist/index.js',
      })

      expect(mkdirSync).toHaveBeenCalled()
      expect(writeFileSync).toHaveBeenCalledOnce()
      const [path, content] = vi.mocked(writeFileSync).mock.calls[0] as [string, string]
      expect(path).toContain('taskcast.service')

      expect(content).toContain('[Unit]')
      expect(content).toContain('[Service]')
      expect(content).toContain('[Install]')
      expect(content).toContain('ExecStart=')
      expect(content).toContain('/usr/bin/node')
      expect(content).toContain('WantedBy=default.target')
    })

    it('runs daemon-reload and enable after writing', async () => {
      vi.mocked(existsSync).mockReturnValue(false)
      vi.mocked(execFileSync).mockReturnValue(Buffer.from(''))

      await mgr.install({
        port: 3721,
        nodePath: '/usr/bin/node',
        entryPoint: '/usr/lib/node_modules/@taskcast/cli/dist/index.js',
      })

      expect(execFileSync).toHaveBeenCalledWith(
        'systemctl', ['--user', 'daemon-reload'], expect.any(Object),
      )
      expect(execFileSync).toHaveBeenCalledWith(
        'systemctl', ['--user', 'enable', 'taskcast'], expect.any(Object),
      )
    })

    it('throws if already installed', async () => {
      vi.mocked(existsSync).mockReturnValue(true)

      await expect(mgr.install({
        port: 3721,
        nodePath: '/usr/bin/node',
        entryPoint: '/path/to/cli/dist/index.js',
      })).rejects.toThrow('already installed')
    })
  })

  describe('start', () => {
    it('calls systemctl --user start', async () => {
      vi.mocked(existsSync).mockReturnValue(true)
      vi.mocked(execFileSync).mockReturnValue(Buffer.from(''))

      await mgr.start()

      expect(execFileSync).toHaveBeenCalledWith(
        'systemctl', ['--user', 'start', 'taskcast'], expect.any(Object),
      )
    })

    it('throws if not installed', async () => {
      vi.mocked(existsSync).mockReturnValue(false)
      await expect(mgr.start()).rejects.toThrow('not installed')
    })
  })

  describe('stop', () => {
    it('calls systemctl --user stop', async () => {
      vi.mocked(execFileSync).mockReturnValue(Buffer.from(''))
      await mgr.stop()

      expect(execFileSync).toHaveBeenCalledWith(
        'systemctl', ['--user', 'stop', 'taskcast'], expect.any(Object),
      )
    })
  })

  describe('restart', () => {
    it('calls systemctl --user restart (single command)', async () => {
      vi.mocked(existsSync).mockReturnValue(true)
      vi.mocked(execFileSync).mockReturnValue(Buffer.from(''))
      await mgr.restart()

      expect(execFileSync).toHaveBeenCalledWith(
        'systemctl', ['--user', 'restart', 'taskcast'], expect.any(Object),
      )
    })
  })

  describe('status', () => {
    it('returns running with pid', async () => {
      vi.mocked(existsSync).mockReturnValue(true)
      vi.mocked(execFileSync).mockReturnValue(
        Buffer.from('ActiveState=active\nMainPID=9876\n'),
      )

      const result = await mgr.status()
      expect(result).toEqual({ state: 'running', pid: 9876 })
    })

    it('returns stopped when inactive', async () => {
      vi.mocked(existsSync).mockReturnValue(true)
      vi.mocked(execFileSync).mockReturnValue(
        Buffer.from('ActiveState=inactive\nMainPID=0\n'),
      )

      const result = await mgr.status()
      expect(result).toEqual({ state: 'stopped' })
    })

    it('returns not-installed when unit file missing', async () => {
      vi.mocked(existsSync).mockReturnValue(false)

      const result = await mgr.status()
      expect(result).toEqual({ state: 'not-installed' })
    })

    it('returns stopped when systemctl show throws', async () => {
      vi.mocked(existsSync).mockReturnValue(true)
      vi.mocked(execFileSync).mockImplementation(() => {
        throw new Error('Failed to connect to bus')
      })

      const result = await mgr.status()
      expect(result).toEqual({ state: 'stopped' })
    })
  })

  describe('uninstall', () => {
    it('does nothing when unit file does not exist', async () => {
      vi.mocked(existsSync).mockReturnValue(false)

      await mgr.uninstall()

      expect(unlinkSync).not.toHaveBeenCalled()
    })

    it('disables, daemon-reloads, and deletes unit file when stopped', async () => {
      vi.mocked(existsSync).mockReturnValue(true)
      vi.mocked(execFileSync).mockImplementation((_cmd, args) => {
        if ((args as string[]).includes('show')) {
          return Buffer.from('ActiveState=inactive\nMainPID=0\n')
        }
        return Buffer.from('')
      })

      await mgr.uninstall()

      expect(execFileSync).toHaveBeenCalledWith(
        'systemctl', ['--user', 'disable', 'taskcast'], expect.any(Object),
      )
      expect(execFileSync).toHaveBeenCalledWith(
        'systemctl', ['--user', 'daemon-reload'], expect.any(Object),
      )
      expect(unlinkSync).toHaveBeenCalledWith(MOCK_PATHS.plistOrUnitPath)
    })

    it('stops service before uninstalling if running', async () => {
      vi.mocked(existsSync).mockReturnValue(true)
      const calls: string[][] = []
      vi.mocked(execFileSync).mockImplementation((_cmd, args) => {
        calls.push(args as string[])
        if ((args as string[]).includes('show') && calls.filter(c => c.includes('show')).length === 1) {
          return Buffer.from('ActiveState=active\nMainPID=555\n')
        }
        return Buffer.from('')
      })

      await mgr.uninstall()

      expect(calls.some(c => c.includes('stop'))).toBe(true)
      expect(unlinkSync).toHaveBeenCalled()
    })
  })
})
