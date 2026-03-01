# CLI Subcommand Restructuring

**Date:** 2026-03-02
**Status:** Approved

## Problem

Currently `taskcast` (bare command) directly starts the server. This works for foreground mode but leaves no room for daemon/service management subcommands (`daemon`, `stop`, `status`). We need a proper subcommand structure now, even though daemon functionality will be implemented later.

## Design

### Command Structure

```
taskcast [command] [options]

Commands:
  start   Start the server in foreground (default)
  daemon  Start the server as a background service (not yet implemented)
  stop    Stop the background service (not yet implemented)
  status  Show server status (not yet implemented)

Global Options:
  -V, --version  Show version
  -h, --help     Show help
```

### `start` subcommand

Default subcommand. `taskcast` = `taskcast start`. Behavior unchanged from current implementation.

```
taskcast start [options]

Options:
  -c, --config <path>   Config file path
  -p, --port <port>     Port to listen on (default: 3721)
```

### Placeholder subcommands

`daemon`, `stop`, `status` are registered but print a "not yet implemented" message and exit. This reserves the command names and documents the intended CLI surface.

## Scope

### Changes

| File | Change |
|------|--------|
| `packages/cli/src/index.ts` | Add daemon/stop/status subcommand placeholders |
| `rust/taskcast-cli/src/main.rs` | Add Daemon/Stop/Status to Commands enum with placeholder match arms |
| `packages/cli/README.md` | Update command documentation |

### Non-changes

- Default behavior unchanged (`taskcast` = `taskcast start`)
- `start` options and logic unchanged
- No new dependencies

## Future Work

- `taskcast daemon`: fork background process, PID file, log file, auto-restart, health check
- `taskcast stop`: read PID file, send SIGTERM, confirm shutdown
- `taskcast status`: read PID file, check process liveness, show uptime/port/connections
