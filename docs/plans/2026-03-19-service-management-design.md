# Service Management — Daemon & Auto-Start

**Date:** 2026-03-19
**Status:** Approved

## Problem

Taskcast CLI currently only supports foreground mode (`taskcast start`). Users need a way to run Taskcast as a background service with OS-level auto-start on boot, supporting both macOS (launchd) and Linux (systemd).

## Design

### Command Structure

```
taskcast service install   # Register system service + auto-start on boot
taskcast service uninstall # Remove system service registration
taskcast service start     # Start via system service manager
taskcast service stop      # Stop via system service manager
taskcast service restart   # Stop + start (equivalent to stop && start)
taskcast service reload    # Alias for restart (full process restart)
taskcast service status    # Show service state (running/stopped/not installed)
```

**Aliases** (backward compat with existing placeholders):

```
taskcast daemon  → taskcast service start
taskcast stop    → taskcast service stop
taskcast status  → taskcast service status
```

Aliases are silent — they invoke the corresponding service action directly without migration notices.

### `service install` Options

Inherited from `start` command:

| Option | Description |
|--------|-------------|
| `-c, --config <path>` | Config file path |
| `-p, --port <port>` | Port (default: 3721) |
| `-s, --storage <type>` | Storage backend: memory \| redis \| sqlite |
| `--db-path <path>` | SQLite database file path |

These values are baked into the generated service configuration (plist/unit file). Changing them requires `uninstall` + `install`.

### File Locations

**Config & data** (shared with `start` command, both platforms):

| File | Path |
|------|------|
| Config file | `~/.taskcast/taskcast.config.yaml` (existing convention) |
| SQLite DB | `~/.taskcast/taskcast.db` |
| Node config | `~/.taskcast/nodes.json` |

**macOS service files:**

| File | Path |
|------|------|
| Service config | `~/Library/LaunchAgents/com.taskcast.daemon.plist` |
| Log (stdout) | `~/Library/Application Support/taskcast/taskcast.log` |
| Log (stderr) | `~/Library/Application Support/taskcast/taskcast.err.log` |

**Linux service files:**

| File | Path |
|------|------|
| Service config | `~/.config/systemd/user/taskcast.service` |
| Logs | `journalctl --user -u taskcast` (systemd-managed) |

**No PID file** — both platform service managers handle process lifecycle natively.

### Architecture — Strategy Pattern

```
ServiceManager (interface)
├── LaunchdServiceManager   // plist generation + launchctl
└── SystemdServiceManager   // unit file generation + systemctl
```

```typescript
interface ServiceManager {
  install(opts: ServiceInstallOptions): Promise<void>
  uninstall(): Promise<void>
  start(): Promise<void>
  stop(): Promise<void>
  restart(): Promise<void>
  status(): Promise<ServiceStatus>
}

interface ServiceInstallOptions {
  port: number
  config?: string      // Absolute path to config file
  storage?: string
  dbPath?: string
  nodePath: string     // Absolute path to node executable
  entryPoint: string   // Absolute path to taskcast CLI entry
}

type ServiceStatus =
  | { state: 'running'; pid: number; port?: number }
  | { state: 'stopped' }
  | { state: 'not-installed' }
```

**File structure:**

```
packages/cli/src/
  commands/
    service.ts              # registerServiceCommand — register service subcommand group
  service/
    interface.ts            # ServiceManager interface + types
    launchd.ts              # LaunchdServiceManager
    systemd.ts              # SystemdServiceManager
    resolve.ts              # createServiceManager() — returns impl by platform
    paths.ts                # Platform-specific path constants
```

### Command Behaviors

#### `service install`

1. Detect platform, create ServiceManager
2. Check if already installed — if yes, error with "run `uninstall` first"
3. Resolve absolute paths for `node` executable and `taskcast` entry point
4. Check for config file — if none exists, auto-create default config (SQLite storage) at platform-native path
5. Generate service config:
   - macOS: write plist to `~/Library/LaunchAgents/`, set `RunAtLoad: true`, `KeepAlive: false`
   - Linux: write unit file to `~/.config/systemd/user/`, run `systemctl --user daemon-reload && systemctl --user enable taskcast`
6. Print success + hint to run `taskcast service start`

#### `service uninstall`

1. Check if service is running — if yes, auto-stop first
2. macOS: `launchctl bootout gui/<uid>/com.taskcast.daemon`, delete plist
3. Linux: `systemctl --user disable taskcast && systemctl --user daemon-reload`, delete unit file
4. Print success
5. **Does not delete config or data files**

#### `service start`

1. Check if installed — if not, error with "run `service install` first"
2. Check if already running — if yes, print "already running"
3. macOS: `launchctl bootstrap gui/<uid> <plist-path>`
4. Linux: `systemctl --user start taskcast`
5. Poll health endpoint (max 5 seconds) to confirm startup
6. Success: print `Taskcast running on http://localhost:<port>`
7. Failure: print error + log file path

#### `service stop`

1. Check if running — if not, print "not running"
2. macOS: `launchctl bootout gui/<uid>/com.taskcast.daemon` — this unloads the service from the current session, but since the plist remains in `~/Library/LaunchAgents/` with `RunAtLoad: true`, launchd will re-load it on next login (auto-start preserved)
3. Linux: `systemctl --user stop taskcast`
4. Confirm stopped

#### `service restart` / `service reload`

Both commands perform the same action — full process restart via `ServiceManager.restart()`:

- macOS: `stop` then `start` (launchd has no native restart). If stop succeeds but start fails, the service remains stopped and the error is reported with log path.
- Linux: `systemctl --user restart taskcast` (single atomic command)
- Same health check polling as `start`

`reload` is an alias for `restart`. No hot-reload / SIGHUP support — the process fully restarts.

#### `service status`

1. Check if installed — if not, print `not installed`
2. Query system service manager for process state
3. If running, request `/health/detail` for detailed info
4. Output format:
   ```
   Service:   running (pid 12345)
   Uptime:    2h 15m
   Port:      3721
   Storage:   sqlite
   ```

### Auto-Created Config File

When `service install` finds no config file, it writes its own YAML template directly to `~/.taskcast/taskcast.config.yaml` (does not reuse `createDefaultGlobalConfig()`, since the service default includes SQLite storage):

```yaml
port: 3721
adapters:
  shortTermStore:
    provider: sqlite
    path: ~/.taskcast/taskcast.db
  longTermStore:
    provider: sqlite
    path: ~/.taskcast/taskcast.db
```

The `path` values use the absolute expanded path at write time (e.g., `/Users/alice/.taskcast/taskcast.db`), since services run with unpredictable CWD.

This ensures `taskcast start` and `taskcast service start` share the same config file and data directory.

### Error Handling

| Scenario | Behavior |
|----------|----------|
| `service start` without install | Error + hint `taskcast service install` |
| `service install` when already installed | Error + hint `uninstall` first |
| Health check timeout after start | Error + hint to check log path |
| Unsupported platform (Windows etc.) | Error `Unsupported platform: <platform>` |
| Service manager command fails | Pass through stderr + non-zero exit code |
| `uninstall` while running | Auto-stop, then uninstall |
| Baked-in `node`/`entryPoint` path no longer exists | `service status` warns; `start` fails with clear error pointing to `uninstall` + `install` |

### Out of Scope

- Multiple instances (single taskcast service only)
- Automatic updates
- Windows support
- Rust CLI implementation (separate follow-up)
