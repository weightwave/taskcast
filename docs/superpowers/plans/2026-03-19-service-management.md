# Service Management Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add `taskcast service install/uninstall/start/stop/restart/reload/status` commands with macOS launchd and Linux systemd backends, plus backward-compatible aliases for the old `daemon/stop/status` placeholders.

**Architecture:** Strategy pattern with a `ServiceManager` interface and two implementations (`LaunchdServiceManager`, `SystemdServiceManager`). The command layer (`commands/service.ts`) delegates all platform-specific logic through this interface. Platform is detected at runtime via `process.platform`.

**Tech Stack:** TypeScript, Commander.js (subcommand groups), Node.js `child_process.execFile` for launchctl/systemctl calls, `fs` for plist/unit file generation.

**Spec:** `docs/plans/2026-03-19-service-management-design.md`

---

## File Structure

| Action | Path | Responsibility |
|--------|------|----------------|
| Create | `packages/cli/src/service/interface.ts` | `ServiceManager` interface, `ServiceInstallOptions`, `ServiceStatus` types |
| Create | `packages/cli/src/service/paths.ts` | Platform-specific path resolution (log dir, plist path, unit file path, default config/db paths) |
| Create | `packages/cli/src/service/launchd.ts` | `LaunchdServiceManager` — plist generation, launchctl commands |
| Create | `packages/cli/src/service/systemd.ts` | `SystemdServiceManager` — unit file generation, systemctl commands |
| Create | `packages/cli/src/service/resolve.ts` | `createServiceManager()` factory |
| Create | `packages/cli/src/commands/service.ts` | `registerServiceCommand()` — `service` subcommand group with install/uninstall/start/stop/restart/reload/status |
| Modify | `packages/cli/src/index.ts` | Replace placeholder commands with `registerServiceCommand`, add aliases |
| Create | `packages/cli/tests/unit/service-paths.test.ts` | Tests for path resolution |
| Create | `packages/cli/tests/unit/service-launchd.test.ts` | Tests for LaunchdServiceManager |
| Create | `packages/cli/tests/unit/service-systemd.test.ts` | Tests for SystemdServiceManager |
| Create | `packages/cli/tests/unit/service-resolve.test.ts` | Tests for createServiceManager factory |
| Create | `packages/cli/tests/unit/service-command.test.ts` | Tests for registerServiceCommand |

---

### Task 1: ServiceManager Interface & Types

**Files:**
- Create: `packages/cli/src/service/interface.ts`
- Test: `packages/cli/tests/unit/service-interface.test.ts`

- [ ] **Step 1: Write the type definitions**

```typescript
// packages/cli/src/service/interface.ts

export interface ServiceInstallOptions {
  port: number
  config?: string      // Absolute path to config file
  storage?: string
  dbPath?: string
  nodePath: string     // Absolute path to node executable
  entryPoint: string   // Absolute path to taskcast CLI entry
}

export type ServiceStatus =
  | { state: 'running'; pid: number; port?: number }
  | { state: 'stopped' }
  | { state: 'not-installed' }

export interface ServiceManager {
  install(opts: ServiceInstallOptions): Promise<void>
  uninstall(): Promise<void>
  start(): Promise<void>
  stop(): Promise<void>
  restart(): Promise<void>
  status(): Promise<ServiceStatus>
}
```

- [ ] **Step 2: Write a type-level test to verify the interface is importable and usable**

```typescript
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
```

- [ ] **Step 3: Run tests to verify they pass**

Run: `cd packages/cli && pnpm test -- tests/unit/service-interface.test.ts`
Expected: PASS

- [ ] **Step 4: Commit**

```bash
git add packages/cli/src/service/interface.ts packages/cli/tests/unit/service-interface.test.ts
git commit -m "feat(cli): add ServiceManager interface and types"
```

---

### Task 2: Platform Path Resolution

**Files:**
- Create: `packages/cli/src/service/paths.ts`
- Test: `packages/cli/tests/unit/service-paths.test.ts`

- [ ] **Step 1: Write failing tests for path resolution**

```typescript
// packages/cli/tests/unit/service-paths.test.ts
import { describe, it, expect, vi, afterEach } from 'vitest'
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
  })

  it('throws on unsupported platform', async () => {
    vi.stubGlobal('process', { ...process, platform: 'win32' })
    const { getServicePaths } = await import('../../src/service/paths.js')
    expect(() => getServicePaths()).toThrow('Unsupported platform: win32')
  })
})
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cd packages/cli && pnpm test -- tests/unit/service-paths.test.ts`
Expected: FAIL — module not found

- [ ] **Step 3: Implement getServicePaths**

```typescript
// packages/cli/src/service/paths.ts
import { homedir } from 'os'
import { join } from 'path'

export interface ServicePaths {
  plistOrUnitPath: string
  logDir: string
  stdoutLog: string
  stderrLog: string
  defaultConfigPath: string
  defaultDbPath: string
}

export const LAUNCHD_LABEL = 'com.taskcast.daemon'

export function getServicePaths(): ServicePaths {
  const home = homedir()
  const platform = process.platform

  const defaultConfigPath = join(home, '.taskcast', 'taskcast.config.yaml')
  const defaultDbPath = join(home, '.taskcast', 'taskcast.db')

  if (platform === 'darwin') {
    const logDir = join(home, 'Library/Application Support/taskcast')
    return {
      plistOrUnitPath: join(home, 'Library/LaunchAgents', `${LAUNCHD_LABEL}.plist`),
      logDir,
      stdoutLog: join(logDir, 'taskcast.log'),
      stderrLog: join(logDir, 'taskcast.err.log'),
      defaultConfigPath,
      defaultDbPath,
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
    }
  }

  throw new Error(`Unsupported platform: ${platform}`)
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cd packages/cli && pnpm test -- tests/unit/service-paths.test.ts`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add packages/cli/src/service/paths.ts packages/cli/tests/unit/service-paths.test.ts
git commit -m "feat(cli): add platform-specific service path resolution"
```

---

### Task 3: LaunchdServiceManager

**Files:**
- Create: `packages/cli/src/service/launchd.ts`
- Test: `packages/cli/tests/unit/service-launchd.test.ts`

- [ ] **Step 1: Write failing tests for plist generation**

Test that `install()` generates a valid plist XML with correct entries (RunAtLoad, KeepAlive, ProgramArguments, StandardOutPath, StandardErrorPath, Label). Mock `fs.writeFileSync`, `fs.mkdirSync`, `fs.existsSync`, and `child_process.execFileSync`.

```typescript
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
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cd packages/cli && pnpm test -- tests/unit/service-launchd.test.ts`
Expected: FAIL — module not found

- [ ] **Step 3: Implement LaunchdServiceManager**

```typescript
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
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cd packages/cli && pnpm test -- tests/unit/service-launchd.test.ts`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add packages/cli/src/service/launchd.ts packages/cli/tests/unit/service-launchd.test.ts
git commit -m "feat(cli): implement LaunchdServiceManager for macOS"
```

---

### Task 4: SystemdServiceManager

**Files:**
- Create: `packages/cli/src/service/systemd.ts`
- Test: `packages/cli/tests/unit/service-systemd.test.ts`

- [ ] **Step 1: Write failing tests for unit file generation**

Same test structure as launchd tests — mock `fs` and `child_process`, verify:
- `install()` generates valid systemd unit file with correct `ExecStart`, `WantedBy=default.target`
- `install()` calls `systemctl --user daemon-reload && systemctl --user enable taskcast`
- `uninstall()` disables, daemon-reloads, and deletes
- `start()` calls `systemctl --user start taskcast`
- `stop()` calls `systemctl --user stop taskcast`
- `restart()` calls `systemctl --user restart taskcast` (single atomic command)
- `status()` parses `systemctl --user show taskcast` output for MainPID and ActiveState

```typescript
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

  describe('status', () => {
    it('returns stopped when systemctl show throws', async () => {
      vi.mocked(existsSync).mockReturnValue(true)
      vi.mocked(execFileSync).mockImplementation(() => {
        throw new Error('Failed to connect to bus')
      })

      const result = await mgr.status()
      expect(result).toEqual({ state: 'stopped' })
    })
  })
})
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cd packages/cli && pnpm test -- tests/unit/service-systemd.test.ts`
Expected: FAIL — module not found

- [ ] **Step 3: Implement SystemdServiceManager**

```typescript
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

  const execStart = [opts.nodePath, ...args].join(' ')

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
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cd packages/cli && pnpm test -- tests/unit/service-systemd.test.ts`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add packages/cli/src/service/systemd.ts packages/cli/tests/unit/service-systemd.test.ts
git commit -m "feat(cli): implement SystemdServiceManager for Linux"
```

---

### Task 5: ServiceManager Factory

**Files:**
- Create: `packages/cli/src/service/resolve.ts`
- Test: `packages/cli/tests/unit/service-resolve.test.ts`

- [ ] **Step 1: Write failing tests**

```typescript
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
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cd packages/cli && pnpm test -- tests/unit/service-resolve.test.ts`
Expected: FAIL

- [ ] **Step 3: Implement createServiceManager**

```typescript
// packages/cli/src/service/resolve.ts
import type { ServiceManager } from './interface.js'
import { LaunchdServiceManager } from './launchd.js'
import { SystemdServiceManager } from './systemd.js'

export function createServiceManager(): ServiceManager {
  if (process.platform === 'darwin') return new LaunchdServiceManager()
  if (process.platform === 'linux') return new SystemdServiceManager()
  throw new Error(`Unsupported platform: ${process.platform}`)
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cd packages/cli && pnpm test -- tests/unit/service-resolve.test.ts`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add packages/cli/src/service/resolve.ts packages/cli/tests/unit/service-resolve.test.ts
git commit -m "feat(cli): add createServiceManager factory"
```

---

### Task 6: Service Command (registerServiceCommand)

**Files:**
- Create: `packages/cli/src/commands/service.ts`
- Test: `packages/cli/tests/unit/service-command.test.ts`

This is the largest task. It wires up the `taskcast service` subcommand group.

- [ ] **Step 1: Write failing tests for the service command group**

Test structure: mock `createServiceManager` to return a mock `ServiceManager`, mock `fs` for config creation, verify each subcommand calls the right method with the right args.

```typescript
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
  })),
  LAUNCHD_LABEL: 'com.taskcast.daemon',
}))

vi.mock('fs', () => ({
  existsSync: vi.fn().mockReturnValue(true),
  mkdirSync: vi.fn(),
  writeFileSync: vi.fn(),
  readFileSync: vi.fn().mockReturnValue('port: 3721\n'),
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
      expect(writeFileSync).not.toHaveBeenCalled()
    })

    it('passes --port option to ServiceManager.install', async () => {
      mockInstall.mockResolvedValue(undefined)

      await makeProgram().parseAsync(['node', 'test', 'service', 'install', '--port', '8080'])

      expect(mockInstall).toHaveBeenCalledWith(
        expect.objectContaining({ port: 8080 }),
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
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cd packages/cli && pnpm test -- tests/unit/service-command.test.ts`
Expected: FAIL

- [ ] **Step 3: Implement registerServiceCommand**

```typescript
// packages/cli/src/commands/service.ts
import { Command } from 'commander'
import { existsSync, mkdirSync, readFileSync, writeFileSync } from 'fs'
import { dirname } from 'path'
import { createServiceManager } from '../service/resolve.js'
import { getServicePaths } from '../service/paths.js'
import type { ServiceInstallOptions } from '../service/interface.js'

function resolveNodePath(): string {
  return process.execPath
}

// Note: resolveEntryPoint uses import.meta.url which cannot be mocked in Vitest ESM tests.
// The entryPoint value is verified structurally (mockInstall called) but not by exact path.
/* v8 ignore next 3 */
function resolveEntryPoint(): string {
  return new URL('../index.js', import.meta.url).pathname
}

function ensureConfigFile(paths: ReturnType<typeof getServicePaths>, configOpt?: string): string {
  if (configOpt) return configOpt

  // Check if any config already exists
  if (existsSync(paths.defaultConfigPath)) {
    return paths.defaultConfigPath
  }

  // Create default config with SQLite
  const dir = dirname(paths.defaultConfigPath)
  mkdirSync(dir, { recursive: true })

  const dbPath = paths.defaultDbPath
  const content = `# Taskcast service configuration
port: 3721

adapters:
  shortTermStore:
    provider: sqlite
    path: ${dbPath}
  longTermStore:
    provider: sqlite
    path: ${dbPath}
`
  writeFileSync(paths.defaultConfigPath, content)
  console.log(`[taskcast] Created default config at ${paths.defaultConfigPath}`)

  return paths.defaultConfigPath
}

// Exported action functions — used directly by alias commands in index.ts
// _healthTimeoutMs is injectable for testing (default 5000ms)
export async function runServiceStart(_healthTimeoutMs = 5000): Promise<void> {
  const mgr = createServiceManager()
  const st = await mgr.status()
  if (st.state === 'running') {
    console.log('[taskcast] Service is already running.')
    return
  }
  await mgr.start()
  const paths = getServicePaths()
  const port = await getPortFromConfig()
  const started = await pollHealth(port, _healthTimeoutMs)
  if (started) {
    console.log(`[taskcast] Service started on http://localhost:${port}`)
  } else {
    console.error('[taskcast] Service may have failed to start. Check logs:')
    if (paths.stdoutLog) {
      console.error(`  ${paths.stdoutLog}`)
    } else {
      console.error('  journalctl --user -u taskcast')
    }
  }
}

export async function runServiceStop(): Promise<void> {
  const mgr = createServiceManager()
  const st = await mgr.status()
  if (st.state !== 'running') {
    console.log('[taskcast] Service is not running.')
    return
  }
  await mgr.stop()
  console.log('[taskcast] Service stopped.')
}

export async function runServiceStatus(): Promise<void> {
  const mgr = createServiceManager()
  const st = await mgr.status()
  if (st.state === 'not-installed') {
    console.log('Service:   not installed')
    return
  }
  if (st.state === 'stopped') {
    console.log('Service:   stopped')
    return
  }
  console.log(`Service:   running (pid ${st.pid})`)
  try {
    const port = await getPortFromConfig()
    const res = await fetch(`http://localhost:${port}/health/detail`)
    if (res.ok) {
      const body = await res.json() as { uptime?: number; adapters?: Record<string, { provider: string }> }
      if (body.uptime != null) {
        const h = Math.floor(body.uptime / 3600)
        const m = Math.floor((body.uptime % 3600) / 60)
        console.log(`Uptime:    ${h}h ${m}m`)
      }
      if (body.adapters?.shortTermStore) {
        console.log(`Storage:   ${body.adapters.shortTermStore.provider}`)
      }
    }
  } catch {
    // Health endpoint not reachable — just show basic status
  }
}

// _healthTimeoutMs is injectable for testing (default 5000ms)
export async function runServiceRestart(_healthTimeoutMs = 5000): Promise<void> {
  const mgr = createServiceManager()
  await mgr.restart()
  const paths = getServicePaths()
  const port = await getPortFromConfig()
  const started = await pollHealth(port, _healthTimeoutMs)
  if (started) {
    console.log(`[taskcast] Service restarted on http://localhost:${port}`)
  } else {
    console.error('[taskcast] Service may have failed to restart. Check logs:')
    if (paths.stdoutLog) {
      console.error(`  ${paths.stdoutLog}`)
    } else {
      console.error('  journalctl --user -u taskcast')
    }
  }
}

export function registerServiceCommand(program: Command): void {
  const service = program
    .command('service')
    .description('Manage Taskcast as a system service')

  service
    .command('install')
    .description('Register Taskcast as a system service with auto-start on boot')
    .option('-c, --config <path>', 'config file path')
    .option('-p, --port <port>', 'port to listen on', '3721')
    .option('-s, --storage <type>', 'storage backend: memory | redis | sqlite')
    .option('--db-path <path>', 'SQLite database file path')
    .action(async (opts: { config?: string; port: string; storage?: string; dbPath?: string }) => {
      const mgr = createServiceManager()
      const paths = getServicePaths()

      const configPath = ensureConfigFile(paths, opts.config)

      const installOpts: ServiceInstallOptions = {
        port: Number(opts.port),
        config: configPath,
        storage: opts.storage,
        dbPath: opts.dbPath,
        nodePath: resolveNodePath(),
        entryPoint: resolveEntryPoint(),
      }

      await mgr.install(installOpts)
      console.log('[taskcast] Service installed successfully.')
      console.log('[taskcast] Run `taskcast service start` to start the service.')
    })

  service
    .command('uninstall')
    .description('Remove Taskcast system service registration')
    .action(async () => {
      const mgr = createServiceManager()
      await mgr.uninstall()
      console.log('[taskcast] Service uninstalled successfully.')
    })

  service
    .command('start')
    .description('Start Taskcast via system service manager')
    .action(runServiceStart)

  service
    .command('stop')
    .description('Stop Taskcast system service')
    .action(runServiceStop)

  service
    .command('restart')
    .description('Restart Taskcast system service')
    .action(runServiceRestart)

  service
    .command('reload')
    .description('Restart Taskcast system service (alias for restart)')
    .action(runServiceRestart)

  service
    .command('status')
    .description('Show Taskcast service status')
    .action(runServiceStatus)
}

async function getPortFromConfig(): Promise<number> {
  try {
    const paths = getServicePaths()
    const text = readFileSync(paths.defaultConfigPath, 'utf8')
    const match = text.match(/^port:\s*(\d+)/m)
    if (match) return Number(match[1])
  } catch {
    // Config not readable — use default
  }
  return 3721
}

export async function pollHealth(port: number, timeoutMs = 5000): Promise<boolean> {
  const start = Date.now()
  while (Date.now() - start < timeoutMs) {
    try {
      const res = await fetch(`http://localhost:${port}/health`)
      if (res.ok) return true
    } catch {
      // Not ready yet
    }
    await new Promise(r => setTimeout(r, 500))
  }
  return false
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cd packages/cli && pnpm test -- tests/unit/service-command.test.ts`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add packages/cli/src/commands/service.ts packages/cli/tests/unit/service-command.test.ts
git commit -m "feat(cli): add service subcommand group with install/uninstall/start/stop/restart/reload/status"
```

---

### Task 7: Wire Up index.ts — Replace Placeholders & Add Aliases

**Files:**
- Modify: `packages/cli/src/index.ts`
- Modify: `packages/cli/tests/unit/index.test.ts`

- [ ] **Step 1: Update the existing index.test.ts to expect the new structure**

The test should verify:
- `registerServiceCommand` is called
- `daemon`, `stop`, `status` are registered as alias commands
- Old placeholder behavior is gone

Update the test to:
- Add mock for `../../src/commands/service.js`
- Remove assertions for placeholder `daemon`/`stop`/`status` commands (they are now aliases)
- Add assertion that `registerServiceCommand` was called

- [ ] **Step 2: Run the updated test to verify it fails**

Run: `cd packages/cli && pnpm test -- tests/unit/index.test.ts`
Expected: FAIL — `registerServiceCommand` not imported yet

- [ ] **Step 3: Update index.ts**

Replace the placeholder commands with:

```typescript
import { registerServiceCommand } from './commands/service.js'
import { createServiceManager } from './service/resolve.js'
import { getServicePaths } from './service/paths.js'

// ... after other register calls:
registerServiceCommand(program)

// Aliases for backward compat — call the service manager directly (no Commander delegation)
program.command('daemon').description('Alias for `service start`')
  .action(async () => {
    const { runServiceStart } = await import('./commands/service.js')
    await runServiceStart()
  })
program.command('stop').description('Alias for `service stop`')
  .action(async () => {
    const { runServiceStop } = await import('./commands/service.js')
    await runServiceStop()
  })
program.command('status').description('Alias for `service status`')
  .action(async () => {
    const { runServiceStatus } = await import('./commands/service.js')
    await runServiceStatus()
  })
```

- [ ] **Step 4: Run tests**

Run: `cd packages/cli && pnpm test -- tests/unit/index.test.ts`
Expected: PASS

- [ ] **Step 5: Run full CLI test suite**

Run: `cd packages/cli && pnpm test`
Expected: All tests pass

- [ ] **Step 6: Commit**

```bash
git add packages/cli/src/index.ts packages/cli/tests/unit/index.test.ts
git commit -m "feat(cli): wire up service command and replace daemon/stop/status placeholders with aliases"
```

---

### Task 8: Build & Type Check

**Files:** None (verify only)

- [ ] **Step 1: Run type check across the monorepo**

Run: `pnpm lint`
Expected: No type errors

- [ ] **Step 2: Run full build**

Run: `pnpm build`
Expected: Build succeeds

- [ ] **Step 3: Run CLI test suite with coverage**

Run: `cd packages/cli && pnpm test:coverage`
Expected: All tests pass, coverage meets thresholds (100% lines, 90% branches)

- [ ] **Step 4: Fix any coverage gaps**

If coverage is below threshold, add tests for uncovered branches. Common gaps:
- Error paths in `ensureConfigFile`
- Edge cases in `pollHealth` timeout
- `reload` delegation

- [ ] **Step 5: Commit any coverage fixes**

```bash
git add packages/cli/
git commit -m "test(cli): achieve coverage targets for service management"
```

---

### Task 9: Manual Smoke Test (macOS only — skip on CI)

**Files:** None

- [ ] **Step 1: Build the CLI**

Run: `pnpm build`

- [ ] **Step 2: Test the install flow**

Run: `node packages/cli/dist/index.js service install`
Expected: Plist created at `~/Library/LaunchAgents/com.taskcast.daemon.plist`, config created at `~/.taskcast/taskcast.config.yaml` if not existing

- [ ] **Step 3: Test start**

Run: `node packages/cli/dist/index.js service start`
Expected: Service starts, health check passes, prints URL

- [ ] **Step 4: Test status**

Run: `node packages/cli/dist/index.js service status`
Expected: Shows running with PID

- [ ] **Step 5: Test alias**

Run: `node packages/cli/dist/index.js daemon`
Expected: Same behavior as `service start` (already running message)

- [ ] **Step 6: Test stop**

Run: `node packages/cli/dist/index.js service stop`
Expected: Service stops

- [ ] **Step 7: Test uninstall**

Run: `node packages/cli/dist/index.js service uninstall`
Expected: Plist deleted, config file preserved

- [ ] **Step 8: Commit any fixes discovered during smoke test**
