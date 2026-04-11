# Rust CLI Daemon / Service Management Design

**Date:** 2026-04-05
**Status:** Approved

## Overview

Implement `taskcast service` subcommand group in the Rust CLI (`taskcast-cli`) to manage Taskcast as a background system service. This mirrors the existing Node.js CLI service management, using the same paths, service names, and behavior so the two CLIs are interchangeable.

Additionally, add graceful shutdown (SIGTERM/SIGINT handling) to the Rust server's foreground `start` command, which is a prerequisite for service managers to stop the process cleanly.

## Scope

**In scope:**
- `service install/uninstall/start/stop/restart/reload/status` subcommands
- `daemon`/`stop`/`status` top-level aliases
- macOS launchd implementation
- Linux systemd (user session) implementation
- Graceful shutdown signal handling for `taskcast start`
- Health check polling after start/restart
- Auto-creation of default config on install
- State file for tracking installed port

**Out of scope:**
- Windows service support (trait pre-reserved, returns unsupported error)
- Multiple concurrent instances
- Custom restart policies (KeepAlive/Restart remain off)

## Command Structure

### New `service` Subcommand Group

```
taskcast service install   [-c config] [-p port] [-s storage] [--db-path path]
taskcast service uninstall
taskcast service start
taskcast service stop
taskcast service restart
taskcast service reload     # alias for restart
taskcast service status
```

### Top-Level Aliases (Backward Compatibility)

```
taskcast daemon  -> service start
taskcast stop    -> service stop
taskcast status  -> service status
```

Implemented as Clap `Subcommand` variants that delegate to the corresponding service functions.

## Architecture

### ServiceManager Trait

```rust
pub trait ServiceManager {
    fn install(&self, opts: &ServiceInstallOptions) -> Result<()>;
    fn uninstall(&self) -> Result<()>;
    fn start(&self) -> Result<()>;
    fn stop(&self) -> Result<()>;
    fn restart(&self) -> Result<()>;
    fn status(&self) -> Result<ServiceStatus>;
}
```

### Associated Types

```rust
pub struct ServiceInstallOptions {
    pub port: u16,
    pub config: Option<String>,   // absolute path
    pub storage: Option<String>,
    pub db_path: Option<String>,
    pub exec_path: String,        // resolved via std::env::current_exe()
}

pub enum ServiceStatus {
    Running { pid: u32 },
    Stopped,
    NotInstalled,
}
```

### Platform Selection

Compile-time selection via `#[cfg]`:

```rust
pub fn create_service_manager() -> Result<Box<dyn ServiceManager>> {
    #[cfg(target_os = "macos")]
    return Ok(Box::new(LaunchdServiceManager));

    #[cfg(target_os = "linux")]
    return Ok(Box::new(SystemdServiceManager));

    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    bail!("Service management is not supported on this platform")
}
```

Windows builds compile successfully but return an error at runtime when `service` commands are invoked. The trait is pre-reserved for a future `WindowsServiceManager`.

## File Paths

All paths match the Node.js CLI exactly so the two CLIs share config and service registration.

| Item | macOS | Linux |
|------|-------|-------|
| Service file | `~/Library/LaunchAgents/com.taskcast.daemon.plist` | `~/.config/systemd/user/taskcast.service` |
| stdout log | `~/Library/Application Support/taskcast/taskcast.log` | (journalctl) |
| stderr log | `~/Library/Application Support/taskcast/taskcast.err.log` | (journalctl) |
| Default config | `~/.taskcast/taskcast.config.yaml` | same |
| Default DB | `~/.taskcast/taskcast.db` | same |
| State file | `~/.taskcast/service.state.json` | same |

Implemented as a `ServicePaths` struct with platform-specific constructors.

## macOS: launchd Implementation

### Plist Template

```xml
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN"
  "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>com.taskcast.daemon</string>
    <key>ProgramArguments</key>
    <array>
        <string>{exec_path}</string>
        <string>start</string>
        <string>--port</string>
        <string>{port}</string>
        <!-- conditional: --config, --storage, --db-path -->
    </array>
    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <false/>
    <key>StandardOutPath</key>
    <string>{stdout_log}</string>
    <key>StandardErrorPath</key>
    <string>{stderr_log}</string>
</dict>
</plist>
```

### launchctl Commands

| Operation | Command |
|-----------|---------|
| start | `launchctl bootstrap gui/{uid} {plist_path}` |
| stop | `launchctl bootout gui/{uid}/com.taskcast.daemon` |
| status | `launchctl list com.taskcast.daemon` — parse PID from output |
| install | write plist file + create log directory (NotInstalled check: plist file does not exist) |
| uninstall | stop (ignore errors) + delete plist file |
| restart | stop then start |

UID obtained via `unsafe { libc::getuid() }` (single libc call, no need for nix crate).

## Linux: systemd Implementation

### Unit File Template

```ini
[Unit]
Description=Taskcast — unified task tracking and streaming service
After=network.target

[Service]
Type=simple
ExecStart={exec_path} start --port {port} [--config ...] [--storage ...] [--db-path ...]
Restart=no

[Install]
WantedBy=default.target
```

Arguments containing spaces are double-quoted in ExecStart.

### systemctl Commands

| Operation | Command |
|-----------|---------|
| install | write unit file → `systemctl --user daemon-reload` → `systemctl --user enable taskcast` |
| start | `systemctl --user start taskcast` |
| stop | `systemctl --user stop taskcast` |
| restart | `systemctl --user restart taskcast` |
| uninstall | `systemctl --user disable taskcast` → `systemctl --user daemon-reload` → delete unit file |
| status | `systemctl --user show taskcast --property=ActiveState,MainPID` — parse both fields |

### Status Parsing

- `ActiveState=active` and `MainPID > 0` → `Running { pid }`
- Otherwise → `Stopped`
- Unit file does not exist → `NotInstalled`

## Health Check

After `service start` and `service restart`, poll the server to confirm it's responsive.

- **Endpoint:** `GET http://localhost:{port}/health`
- **Polling interval:** 500ms
- **Timeout:** 5000ms
- **Port resolution priority:** state file → config file → default 3721

On success: `[taskcast] Service started on http://localhost:{port}`

On failure:
- macOS: `[taskcast] Service may have failed to start. Check logs:\n  {stdout_log}\n  {stderr_log}`
- Linux: `[taskcast] Service may have failed to start. Check logs:\n  journalctl --user -u taskcast`

## Command Handler Logic

| Command | Pre-check | Action | Output |
|---------|-----------|--------|--------|
| install | Already installed? → error | Validate port (1-65535) → ensureConfig → mgr.install → write state.json | `Service installed successfully.` + hint to run start |
| uninstall | Not installed? → error | mgr.uninstall → delete state.json | `Service uninstalled.` |
| start | Already running? → log and return | mgr.start → health check | Success/failure message |
| stop | Not running? → log and return | mgr.stop | `Service stopped.` |
| restart | — | mgr.restart → health check | Same as start |
| reload | — | Same as restart | Same as restart |
| status | — | mgr.status → optional GET /health/detail | Service/pid/uptime/storage |

### Auto-Config Creation (install)

When no `--config` is provided and `~/.taskcast/taskcast.config.yaml` does not exist, auto-generate:

```yaml
# Taskcast service configuration
port: 3721

adapters:
  shortTermStore:
    provider: sqlite
    path: ~/.taskcast/taskcast.db
  longTermStore:
    provider: sqlite
    path: ~/.taskcast/taskcast.db
```

If no explicit `--config` and no explicit `--storage`, default to `storage=sqlite`.

### Status Detail

Basic output: `Service: running (pid 12345)` / `stopped` / `not installed`

Extended (fetch `GET /health/detail`, silently ignore errors):
- `Uptime: 2h 30m` (if uptime field present)
- `Storage: sqlite` (if shortTermStore adapter present)

## Graceful Shutdown

Modify `commands/start.rs` to handle SIGTERM and SIGINT for clean shutdown.

**Current code:**
```rust
axum::serve(listener, app).await?
```

**New code:**
```rust
axum::serve(listener, app)
    .with_graceful_shutdown(shutdown_signal())
    .await?
```

**Signal handler:**

```rust
async fn shutdown_signal() {
    let ctrl_c = tokio::signal::ctrl_c();

    #[cfg(unix)]
    let mut sigterm = tokio::signal::unix::signal(
        tokio::signal::unix::SignalKind::terminate()
    ).expect("failed to register SIGTERM handler");

    #[cfg(unix)]
    tokio::select! {
        _ = ctrl_c => {},
        _ = sigterm.recv() => {},
    }

    #[cfg(not(unix))]
    ctrl_c.await.ok();

    eprintln!("[taskcast] Shutting down gracefully...");
}
```

Axum's `with_graceful_shutdown` stops accepting new connections and waits for in-flight requests to complete. SSE connections receive close signals. Database connection pools are cleaned up via Rust's RAII (Drop). No additional cleanup code is required.

Non-Unix (Windows): only handles Ctrl+C, which is sufficient for foreground operation.

## Module Structure

New files under `rust/taskcast-cli/src/`:

```
commands/
  service/
    mod.rs          # service subcommand entry, install/start/stop/etc handler functions
    manager.rs      # ServiceManager trait, ServiceInstallOptions, ServiceStatus
    paths.rs        # ServicePaths struct, platform-specific path resolution
    launchd.rs      # LaunchdServiceManager (#[cfg(target_os = "macos")])
    systemd.rs      # SystemdServiceManager (#[cfg(target_os = "linux")])
    health.rs       # health check polling logic
```

### Existing File Changes

| File | Change |
|------|--------|
| `main.rs` | Remove `Daemon`/`Stop`/`Status` placeholders; add `Service` subcommand group + top-level aliases |
| `commands/mod.rs` | Add `pub mod service;` |
| `commands/start.rs` | Add `with_graceful_shutdown(shutdown_signal())` |
| `Cargo.toml` | Add `libc` dependency (for macOS `getuid()`) |

`reqwest` (for health check HTTP) and `dirs` (for home directory) are already dependencies.

## Dependencies

| Crate | Purpose | New? |
|-------|---------|------|
| `libc` | `getuid()` on macOS for launchctl gui/{uid} | Yes (added unconditionally; zero-cost on non-macOS since only used behind `#[cfg]`) |
| `reqwest` | Health check HTTP requests | No (already present) |
| `dirs` | Home directory resolution | No (already present) |
| `tokio` | Signal handling (`tokio::signal`) | No (already present) |
| `serde_json` | Read/write state file | No (already present) |

## Testing Strategy

### Unit Tests

- Plist generation: verify XML output matches expected template for various option combinations
- Systemd unit generation: verify unit file content for various option combinations
- Path resolution: verify correct paths on each platform
- Port validation: boundary values (0, 1, 65535, 65536)
- Config auto-creation: verify YAML content
- State file read/write round-trip
- Status parsing: launchctl output with/without PID, systemctl property parsing
- Health check: mock server responding/not responding, timeout behavior
- Command handler logic: pre-check flows (already running, not installed, etc.)

### Integration Tests

- Full install → start → status → stop → uninstall cycle on the current platform
- Verify plist/unit file is actually written to the correct path
- Verify service is actually running after start (health check passes)
- Graceful shutdown: send SIGTERM to running server, verify clean exit

### Platform Coverage

- macOS tests run in CI on macOS runner
- Linux tests run in CI on Linux runner
- Windows: verify `create_service_manager()` returns unsupported error
