# Rust CLI Daemon / Service Management — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers-extended-cc:subagent-driven-development (recommended) or superpowers-extended-cc:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Implement `taskcast service` subcommand group and graceful shutdown in the Rust CLI, mirroring the Node.js CLI's service management (launchd on macOS, systemd on Linux).

**Architecture:** Trait-based `ServiceManager` with compile-time platform selection (`#[cfg]`). Platform implementations generate service config files (plist/unit) and shell out to system commands (launchctl/systemctl). Health check polls `/health` after start/restart. Graceful shutdown uses `tokio::signal` + Axum's `with_graceful_shutdown`.

**Tech Stack:** Rust, Axum 0.8, Tokio, Clap 4, reqwest 0.12, libc (new), dirs 6

**Spec:** `docs/superpowers/specs/2026-04-05-rust-daemon-design.md`

---

## File Map

| File | Action | Responsibility |
|------|--------|---------------|
| `rust/taskcast-cli/Cargo.toml` | Modify | Add `libc` dependency |
| `rust/taskcast-cli/src/commands/start.rs` | Modify | Add graceful shutdown signal handler |
| `rust/taskcast-cli/src/commands/mod.rs` | Modify | Add `pub mod service;` |
| `rust/taskcast-cli/src/main.rs` | Modify | Replace Daemon/Stop/Status placeholders with Service subcommand + aliases |
| `rust/taskcast-cli/src/commands/service/mod.rs` | Create | Service subcommand entry, command handler functions |
| `rust/taskcast-cli/src/commands/service/manager.rs` | Create | ServiceManager trait, ServiceInstallOptions, ServiceStatus |
| `rust/taskcast-cli/src/commands/service/paths.rs` | Create | ServicePaths struct, platform path resolution, config/state file helpers |
| `rust/taskcast-cli/src/commands/service/launchd.rs` | Create | LaunchdServiceManager (macOS, `#[cfg]`) |
| `rust/taskcast-cli/src/commands/service/systemd.rs` | Create | SystemdServiceManager (Linux, `#[cfg]`) |
| `rust/taskcast-cli/src/commands/service/health.rs` | Create | Health check polling |

---

### Task 0: Graceful Shutdown

**Goal:** Add SIGTERM/SIGINT signal handling to `taskcast start` so the server shuts down cleanly when stopped by a service manager or Ctrl+C.

**Files:**
- Modify: `rust/taskcast-cli/src/commands/start.rs:274-276`
- Test: `rust/taskcast-cli/src/commands/start.rs` (inline tests)

**Acceptance Criteria:**
- [ ] Server responds to SIGTERM by stopping gracefully
- [ ] Server responds to SIGINT (Ctrl+C) by stopping gracefully
- [ ] `[taskcast] Shutting down gracefully...` is printed to stderr on shutdown
- [ ] On non-Unix (Windows), only Ctrl+C is handled
- [ ] Existing tests still pass

**Verify:** `cd rust && cargo test -p taskcast-cli` → all tests pass, `cargo build -p taskcast-cli` → compiles clean

**Steps:**

- [ ] **Step 1: Add shutdown_signal function to start.rs**

Add this function at the bottom of `rust/taskcast-cli/src/commands/start.rs`, before any `#[cfg(test)]` block:

```rust
async fn shutdown_signal() {
    let ctrl_c = tokio::signal::ctrl_c();

    #[cfg(unix)]
    {
        let mut sigterm = tokio::signal::unix::signal(
            tokio::signal::unix::SignalKind::terminate(),
        )
        .expect("failed to register SIGTERM handler");

        tokio::select! {
            _ = ctrl_c => {},
            _ = sigterm.recv() => {},
        }
    }

    #[cfg(not(unix))]
    {
        ctrl_c.await.ok();
    }

    eprintln!("[taskcast] Shutting down gracefully...");
}
```

- [ ] **Step 2: Wire shutdown_signal into axum::serve**

In the `run()` function, change line 276 from:

```rust
    axum::serve(listener, app).await?;
```

to:

```rust
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;
```

- [ ] **Step 3: Verify compilation and tests**

Run: `cd rust && cargo build -p taskcast-cli && cargo test -p taskcast-cli`
Expected: compiles clean, all existing tests pass

- [ ] **Step 4: Commit**

```bash
git add rust/taskcast-cli/src/commands/start.rs
git commit -m "feat(cli): add graceful shutdown signal handling for taskcast start"
```

---

### Task 1: Core Types — ServiceManager Trait

**Goal:** Define the `ServiceManager` trait, associated types, and the `create_service_manager()` factory.

**Files:**
- Create: `rust/taskcast-cli/src/commands/service/manager.rs`
- Create: `rust/taskcast-cli/src/commands/service/mod.rs` (stub)
- Create: `rust/taskcast-cli/src/commands/service/paths.rs` (stub)
- Create: `rust/taskcast-cli/src/commands/service/health.rs` (stub)
- Create: `rust/taskcast-cli/src/commands/service/launchd.rs` (stub, macOS)
- Create: `rust/taskcast-cli/src/commands/service/systemd.rs` (stub, Linux)
- Modify: `rust/taskcast-cli/src/commands/mod.rs`
- Test: inline `#[cfg(test)]` in `manager.rs`

**Acceptance Criteria:**
- [ ] `ServiceManager` trait with install/uninstall/start/stop/restart/status methods
- [ ] `ServiceInstallOptions` struct with all fields
- [ ] `ServiceStatus` enum with Display impl
- [ ] `create_service_manager()` returns correct platform impl or error
- [ ] Module compiles and tests pass

**Verify:** `cd rust && cargo test -p taskcast-cli -- service::manager` → all pass

**Steps:**

- [ ] **Step 1: Create service module directory**

```bash
mkdir -p rust/taskcast-cli/src/commands/service
```

- [ ] **Step 2: Create manager.rs**

Create `rust/taskcast-cli/src/commands/service/manager.rs`:

```rust
use std::fmt;

/// Options for service installation.
pub struct ServiceInstallOptions {
    pub port: u16,
    pub config: Option<String>,
    pub storage: Option<String>,
    pub db_path: Option<String>,
    pub exec_path: String,
}

/// Service status.
#[derive(Debug, PartialEq)]
pub enum ServiceStatus {
    Running { pid: u32 },
    Stopped,
    NotInstalled,
}

impl fmt::Display for ServiceStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ServiceStatus::Running { pid } => write!(f, "running (pid {pid})"),
            ServiceStatus::Stopped => write!(f, "stopped"),
            ServiceStatus::NotInstalled => write!(f, "not installed"),
        }
    }
}

/// Platform-specific service manager.
pub trait ServiceManager {
    fn install(&self, opts: &ServiceInstallOptions) -> Result<(), Box<dyn std::error::Error>>;
    fn uninstall(&self) -> Result<(), Box<dyn std::error::Error>>;
    fn start(&self) -> Result<(), Box<dyn std::error::Error>>;
    fn stop(&self) -> Result<(), Box<dyn std::error::Error>>;
    fn restart(&self) -> Result<(), Box<dyn std::error::Error>>;
    fn status(&self) -> Result<ServiceStatus, Box<dyn std::error::Error>>;
}

/// Create a platform-appropriate ServiceManager.
pub fn create_service_manager() -> Result<Box<dyn ServiceManager>, Box<dyn std::error::Error>> {
    #[cfg(target_os = "macos")]
    {
        return Ok(Box::new(super::launchd::LaunchdServiceManager));
    }

    #[cfg(target_os = "linux")]
    {
        return Ok(Box::new(super::systemd::SystemdServiceManager));
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        Err("Service management is not supported on this platform".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_display_running() {
        assert_eq!(ServiceStatus::Running { pid: 12345 }.to_string(), "running (pid 12345)");
    }

    #[test]
    fn status_display_stopped() {
        assert_eq!(ServiceStatus::Stopped.to_string(), "stopped");
    }

    #[test]
    fn status_display_not_installed() {
        assert_eq!(ServiceStatus::NotInstalled.to_string(), "not installed");
    }

    #[test]
    fn status_equality() {
        assert_eq!(ServiceStatus::Running { pid: 1 }, ServiceStatus::Running { pid: 1 });
        assert_ne!(ServiceStatus::Running { pid: 1 }, ServiceStatus::Running { pid: 2 });
        assert_ne!(ServiceStatus::Running { pid: 1 }, ServiceStatus::Stopped);
    }

    #[test]
    fn install_options_fields() {
        let opts = ServiceInstallOptions {
            port: 8080,
            config: Some("/path/to/config.yaml".into()),
            storage: Some("sqlite".into()),
            db_path: Some("/path/to/db".into()),
            exec_path: "/usr/bin/taskcast".into(),
        };
        assert_eq!(opts.port, 8080);
        assert_eq!(opts.config.as_deref(), Some("/path/to/config.yaml"));
        assert_eq!(opts.storage.as_deref(), Some("sqlite"));
        assert_eq!(opts.db_path.as_deref(), Some("/path/to/db"));
        assert_eq!(opts.exec_path, "/usr/bin/taskcast");
    }

    #[test]
    fn install_options_optional_fields_none() {
        let opts = ServiceInstallOptions {
            port: 3721,
            config: None,
            storage: None,
            db_path: None,
            exec_path: "/usr/bin/taskcast".into(),
        };
        assert!(opts.config.is_none());
        assert!(opts.storage.is_none());
        assert!(opts.db_path.is_none());
    }

    #[test]
    fn create_service_manager_on_current_platform() {
        let result = create_service_manager();
        if cfg!(any(target_os = "macos", target_os = "linux")) {
            assert!(result.is_ok());
        } else {
            assert!(result.is_err());
            assert!(result.unwrap_err().to_string().contains("not supported"));
        }
    }
}
```

- [ ] **Step 3: Create stub files for compilation**

Create `rust/taskcast-cli/src/commands/service/mod.rs`:

```rust
pub mod manager;
pub mod paths;
pub mod health;

#[cfg(target_os = "macos")]
pub mod launchd;

#[cfg(target_os = "linux")]
pub mod systemd;
```

Create `rust/taskcast-cli/src/commands/service/paths.rs`:

```rust
// Placeholder — implemented in Task 2
```

Create `rust/taskcast-cli/src/commands/service/health.rs`:

```rust
// Placeholder — implemented in Task 5
```

Create `rust/taskcast-cli/src/commands/service/launchd.rs` (compiles only on macOS):

```rust
use super::manager::{ServiceInstallOptions, ServiceManager, ServiceStatus};

pub struct LaunchdServiceManager;

impl ServiceManager for LaunchdServiceManager {
    fn install(&self, _opts: &ServiceInstallOptions) -> Result<(), Box<dyn std::error::Error>> { todo!() }
    fn uninstall(&self) -> Result<(), Box<dyn std::error::Error>> { todo!() }
    fn start(&self) -> Result<(), Box<dyn std::error::Error>> { todo!() }
    fn stop(&self) -> Result<(), Box<dyn std::error::Error>> { todo!() }
    fn restart(&self) -> Result<(), Box<dyn std::error::Error>> { todo!() }
    fn status(&self) -> Result<ServiceStatus, Box<dyn std::error::Error>> { todo!() }
}
```

Create `rust/taskcast-cli/src/commands/service/systemd.rs` (compiles only on Linux):

```rust
use super::manager::{ServiceInstallOptions, ServiceManager, ServiceStatus};

pub struct SystemdServiceManager;

impl ServiceManager for SystemdServiceManager {
    fn install(&self, _opts: &ServiceInstallOptions) -> Result<(), Box<dyn std::error::Error>> { todo!() }
    fn uninstall(&self) -> Result<(), Box<dyn std::error::Error>> { todo!() }
    fn start(&self) -> Result<(), Box<dyn std::error::Error>> { todo!() }
    fn stop(&self) -> Result<(), Box<dyn std::error::Error>> { todo!() }
    fn restart(&self) -> Result<(), Box<dyn std::error::Error>> { todo!() }
    fn status(&self) -> Result<ServiceStatus, Box<dyn std::error::Error>> { todo!() }
}
```

- [ ] **Step 4: Add service module to commands/mod.rs**

Add this line to `rust/taskcast-cli/src/commands/mod.rs`:

```rust
pub mod service;
```

- [ ] **Step 5: Verify**

Run: `cd rust && cargo test -p taskcast-cli -- service::manager`
Expected: 7 tests pass

- [ ] **Step 6: Commit**

```bash
git add rust/taskcast-cli/src/commands/service/ rust/taskcast-cli/src/commands/mod.rs
git commit -m "feat(cli): add ServiceManager trait, types, and module structure"
```

---

### Task 2: Path Resolution and Config/State Helpers

**Goal:** Implement `ServicePaths` with platform-specific paths, plus helpers for auto-creating config and reading/writing the state file.

**Files:**
- Modify: `rust/taskcast-cli/src/commands/service/paths.rs`
- Modify: `rust/taskcast-cli/Cargo.toml` (add `libc`)
- Test: inline `#[cfg(test)]` in `paths.rs`

**Acceptance Criteria:**
- [ ] `ServicePaths::new()` returns correct paths per platform (macOS/Linux)
- [ ] `ensure_config()` creates default YAML with SQLite config when file doesn't exist
- [ ] `ensure_config()` returns existing path if config already exists
- [ ] `write_state()` / `read_state_port()` round-trip correctly
- [ ] `delete_state()` removes the state file
- [ ] `get_port()` resolves port from state file → config file → default 3721
- [ ] All directories are auto-created as needed

**Verify:** `cd rust && cargo test -p taskcast-cli -- service::paths` → all pass

**Steps:**

- [ ] **Step 1: Add libc to Cargo.toml**

Add to `rust/taskcast-cli/Cargo.toml` dependencies section:

```toml
libc = "0.2"
```

- [ ] **Step 2: Implement paths.rs**

Replace `rust/taskcast-cli/src/commands/service/paths.rs` with:

```rust
use std::fs;
use std::path::{Path, PathBuf};

const LAUNCHD_LABEL: &str = "com.taskcast.daemon";
const DEFAULT_PORT: u16 = 3721;

/// Platform-specific file paths for service management.
pub struct ServicePaths {
    /// Path to the plist (macOS) or unit file (Linux).
    pub service_file: PathBuf,
    /// Log directory (macOS only; Linux uses journalctl).
    pub log_dir: Option<PathBuf>,
    /// Stdout log file (macOS only).
    pub stdout_log: Option<PathBuf>,
    /// Stderr log file (macOS only).
    pub stderr_log: Option<PathBuf>,
    /// Default config file path.
    pub default_config: PathBuf,
    /// Default SQLite database path.
    pub default_db: PathBuf,
    /// State file tracking the installed port.
    pub state_file: PathBuf,
}

impl ServicePaths {
    /// Build paths for the current platform.
    pub fn new() -> Result<Self, Box<dyn std::error::Error>> {
        let home = dirs::home_dir().ok_or("cannot determine home directory")?;
        let taskcast_dir = home.join(".taskcast");

        #[cfg(target_os = "macos")]
        {
            let log_dir = home.join("Library/Application Support/taskcast");
            Ok(ServicePaths {
                service_file: home.join(format!("Library/LaunchAgents/{LAUNCHD_LABEL}.plist")),
                stdout_log: Some(log_dir.join("taskcast.log")),
                stderr_log: Some(log_dir.join("taskcast.err.log")),
                log_dir: Some(log_dir),
                default_config: taskcast_dir.join("taskcast.config.yaml"),
                default_db: taskcast_dir.join("taskcast.db"),
                state_file: taskcast_dir.join("service.state.json"),
            })
        }

        #[cfg(target_os = "linux")]
        {
            let systemd_dir = home.join(".config/systemd/user");
            Ok(ServicePaths {
                service_file: systemd_dir.join("taskcast.service"),
                log_dir: None,
                stdout_log: None,
                stderr_log: None,
                default_config: taskcast_dir.join("taskcast.config.yaml"),
                default_db: taskcast_dir.join("taskcast.db"),
                state_file: taskcast_dir.join("service.state.json"),
            })
        }

        #[cfg(not(any(target_os = "macos", target_os = "linux")))]
        {
            Err("Service paths are not defined for this platform".into())
        }
    }

    /// Check whether the service is installed (service file exists).
    pub fn is_installed(&self) -> bool {
        self.service_file.exists()
    }
}

/// Ensure a config file exists. If `config_opt` is provided, return it as-is.
/// Otherwise, if the default config doesn't exist, create it with SQLite defaults.
/// Returns the absolute path to the config file.
pub fn ensure_config(paths: &ServicePaths, config_opt: Option<&str>) -> Result<String, Box<dyn std::error::Error>> {
    if let Some(config) = config_opt {
        return Ok(config.to_string());
    }

    let config_path = &paths.default_config;
    if config_path.exists() {
        return Ok(config_path.to_string_lossy().into_owned());
    }

    // Create parent directory
    if let Some(parent) = config_path.parent() {
        fs::create_dir_all(parent)?;
    }

    let db_path = paths.default_db.to_string_lossy();
    let content = format!(
        "# Taskcast service configuration\n\
         port: {DEFAULT_PORT}\n\
         \n\
         adapters:\n\
         \x20 shortTermStore:\n\
         \x20\x20\x20 provider: sqlite\n\
         \x20\x20\x20 path: {db_path}\n\
         \x20 longTermStore:\n\
         \x20\x20\x20 provider: sqlite\n\
         \x20\x20\x20 path: {db_path}\n"
    );

    fs::write(config_path, &content)?;
    eprintln!("[taskcast] Created default config at {}", config_path.display());

    Ok(config_path.to_string_lossy().into_owned())
}

/// Write the installed port to the state file.
pub fn write_state(paths: &ServicePaths, port: u16) -> Result<(), Box<dyn std::error::Error>> {
    if let Some(parent) = paths.state_file.parent() {
        fs::create_dir_all(parent)?;
    }
    let json = serde_json::json!({ "port": port });
    fs::write(&paths.state_file, serde_json::to_string_pretty(&json)?)?;
    Ok(())
}

/// Read the port from the state file, if it exists.
pub fn read_state_port(paths: &ServicePaths) -> Option<u16> {
    let content = fs::read_to_string(&paths.state_file).ok()?;
    let value: serde_json::Value = serde_json::from_str(&content).ok()?;
    value.get("port")?.as_u64().map(|p| p as u16)
}

/// Delete the state file.
pub fn delete_state(paths: &ServicePaths) {
    let _ = fs::remove_file(&paths.state_file);
}

/// Resolve the port: state file → config file regex → default 3721.
pub fn get_port(paths: &ServicePaths) -> u16 {
    if let Some(port) = read_state_port(paths) {
        return port;
    }
    if let Ok(content) = fs::read_to_string(&paths.default_config) {
        for line in content.lines() {
            let trimmed = line.trim();
            if let Some(rest) = trimmed.strip_prefix("port:") {
                if let Ok(port) = rest.trim().parse::<u16>() {
                    return port;
                }
            }
        }
    }
    DEFAULT_PORT
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    /// Build a ServicePaths pointing at a temp directory for test isolation.
    fn test_paths(tmp: &Path) -> ServicePaths {
        ServicePaths {
            service_file: tmp.join("service-file"),
            log_dir: Some(tmp.join("logs")),
            stdout_log: Some(tmp.join("logs/taskcast.log")),
            stderr_log: Some(tmp.join("logs/taskcast.err.log")),
            default_config: tmp.join("taskcast.config.yaml"),
            default_db: tmp.join("taskcast.db"),
            state_file: tmp.join("service.state.json"),
        }
    }

    // ─── ServicePaths ───────────────────────────────────────────────────

    #[test]
    fn paths_new_succeeds_on_supported_platform() {
        if cfg!(any(target_os = "macos", target_os = "linux")) {
            let paths = ServicePaths::new().unwrap();
            assert!(paths.service_file.to_string_lossy().len() > 0);
            assert!(paths.default_config.to_string_lossy().contains(".taskcast"));
        }
    }

    #[test]
    fn is_installed_false_when_no_file() {
        let tmp = TempDir::new().unwrap();
        let paths = test_paths(tmp.path());
        assert!(!paths.is_installed());
    }

    #[test]
    fn is_installed_true_when_file_exists() {
        let tmp = TempDir::new().unwrap();
        let paths = test_paths(tmp.path());
        fs::write(&paths.service_file, "test").unwrap();
        assert!(paths.is_installed());
    }

    // ─── ensure_config ──────────────────────────────────────────────────

    #[test]
    fn ensure_config_returns_explicit_path() {
        let tmp = TempDir::new().unwrap();
        let paths = test_paths(tmp.path());
        let result = ensure_config(&paths, Some("/custom/config.yaml")).unwrap();
        assert_eq!(result, "/custom/config.yaml");
    }

    #[test]
    fn ensure_config_returns_existing_file() {
        let tmp = TempDir::new().unwrap();
        let paths = test_paths(tmp.path());
        fs::write(&paths.default_config, "existing content").unwrap();
        let result = ensure_config(&paths, None).unwrap();
        assert_eq!(result, paths.default_config.to_string_lossy());
        // Should NOT overwrite
        assert_eq!(fs::read_to_string(&paths.default_config).unwrap(), "existing content");
    }

    #[test]
    fn ensure_config_creates_default_with_sqlite() {
        let tmp = TempDir::new().unwrap();
        let paths = test_paths(tmp.path());
        let result = ensure_config(&paths, None).unwrap();
        assert_eq!(result, paths.default_config.to_string_lossy());

        let content = fs::read_to_string(&paths.default_config).unwrap();
        assert!(content.contains("port: 3721"));
        assert!(content.contains("provider: sqlite"));
        assert!(content.contains(&paths.default_db.to_string_lossy().to_string()));
    }

    // ─── state file ─────────────────────────────────────────────────────

    #[test]
    fn write_and_read_state_roundtrip() {
        let tmp = TempDir::new().unwrap();
        let paths = test_paths(tmp.path());
        write_state(&paths, 8080).unwrap();
        assert_eq!(read_state_port(&paths), Some(8080));
    }

    #[test]
    fn read_state_port_returns_none_when_missing() {
        let tmp = TempDir::new().unwrap();
        let paths = test_paths(tmp.path());
        assert_eq!(read_state_port(&paths), None);
    }

    #[test]
    fn read_state_port_returns_none_for_invalid_json() {
        let tmp = TempDir::new().unwrap();
        let paths = test_paths(tmp.path());
        fs::write(&paths.state_file, "not json").unwrap();
        assert_eq!(read_state_port(&paths), None);
    }

    #[test]
    fn delete_state_removes_file() {
        let tmp = TempDir::new().unwrap();
        let paths = test_paths(tmp.path());
        write_state(&paths, 3721).unwrap();
        assert!(paths.state_file.exists());
        delete_state(&paths);
        assert!(!paths.state_file.exists());
    }

    #[test]
    fn delete_state_noop_when_missing() {
        let tmp = TempDir::new().unwrap();
        let paths = test_paths(tmp.path());
        delete_state(&paths); // should not panic
    }

    // ─── get_port ───────────────────────────────────────────────────────

    #[test]
    fn get_port_from_state_file() {
        let tmp = TempDir::new().unwrap();
        let paths = test_paths(tmp.path());
        write_state(&paths, 9090).unwrap();
        assert_eq!(get_port(&paths), 9090);
    }

    #[test]
    fn get_port_from_config_file() {
        let tmp = TempDir::new().unwrap();
        let paths = test_paths(tmp.path());
        fs::write(&paths.default_config, "port: 4000\n").unwrap();
        assert_eq!(get_port(&paths), 4000);
    }

    #[test]
    fn get_port_state_takes_precedence_over_config() {
        let tmp = TempDir::new().unwrap();
        let paths = test_paths(tmp.path());
        write_state(&paths, 9090).unwrap();
        fs::write(&paths.default_config, "port: 4000\n").unwrap();
        assert_eq!(get_port(&paths), 9090);
    }

    #[test]
    fn get_port_defaults_to_3721() {
        let tmp = TempDir::new().unwrap();
        let paths = test_paths(tmp.path());
        assert_eq!(get_port(&paths), 3721);
    }

    #[test]
    fn get_port_ignores_invalid_config() {
        let tmp = TempDir::new().unwrap();
        let paths = test_paths(tmp.path());
        fs::write(&paths.default_config, "port: not_a_number\n").unwrap();
        assert_eq!(get_port(&paths), 3721);
    }

    // ─── macOS-specific paths ───────────────────────────────────────────

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_paths_are_correct() {
        let paths = ServicePaths::new().unwrap();
        let home = dirs::home_dir().unwrap();
        assert_eq!(paths.service_file, home.join("Library/LaunchAgents/com.taskcast.daemon.plist"));
        assert_eq!(paths.stdout_log.unwrap(), home.join("Library/Application Support/taskcast/taskcast.log"));
        assert_eq!(paths.stderr_log.unwrap(), home.join("Library/Application Support/taskcast/taskcast.err.log"));
        assert!(paths.log_dir.is_some());
    }

    // ─── Linux-specific paths ───────────────────────────────────────────

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_paths_are_correct() {
        let paths = ServicePaths::new().unwrap();
        let home = dirs::home_dir().unwrap();
        assert_eq!(paths.service_file, home.join(".config/systemd/user/taskcast.service"));
        assert!(paths.log_dir.is_none());
        assert!(paths.stdout_log.is_none());
        assert!(paths.stderr_log.is_none());
    }
}
```

- [ ] **Step 3: Verify**

Run: `cd rust && cargo test -p taskcast-cli -- service::paths`
Expected: all tests pass (15+ on macOS, 15+ on Linux)

- [ ] **Step 4: Commit**

```bash
git add rust/taskcast-cli/src/commands/service/paths.rs rust/taskcast-cli/Cargo.toml
git commit -m "feat(cli): add ServicePaths and config/state helpers"
```

---

### Task 3: macOS launchd Implementation

**Goal:** Implement `LaunchdServiceManager` — plist generation, launchctl commands, status parsing.

**Files:**
- Modify: `rust/taskcast-cli/src/commands/service/launchd.rs`
- Test: inline `#[cfg(test)]` in `launchd.rs`

**Acceptance Criteria:**
- [ ] `generate_plist()` produces correct XML for all option combinations
- [ ] `install()` writes plist file, creates log directory
- [ ] `uninstall()` stops service (ignoring errors), deletes plist
- [ ] `start()` calls `launchctl bootstrap gui/{uid} {path}`
- [ ] `stop()` calls `launchctl bootout gui/{uid}/com.taskcast.daemon`
- [ ] `restart()` calls stop then start
- [ ] `status()` parses `launchctl list` output for PID
- [ ] Returns `NotInstalled` when plist doesn't exist

**Verify:** `cd rust && cargo test -p taskcast-cli -- service::launchd` → all pass (macOS only)

**Steps:**

- [ ] **Step 1: Write tests for plist generation**

Add to the top of `launchd.rs` test module — tests for `generate_plist()`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plist_minimal_options() {
        let opts = ServiceInstallOptions {
            port: 3721,
            config: None,
            storage: None,
            db_path: None,
            exec_path: "/usr/local/bin/taskcast".into(),
        };
        let stdout = "/tmp/taskcast.log";
        let stderr = "/tmp/taskcast.err.log";
        let plist = generate_plist(&opts, stdout, stderr);

        assert!(plist.contains("<string>com.taskcast.daemon</string>"));
        assert!(plist.contains("<string>/usr/local/bin/taskcast</string>"));
        assert!(plist.contains("<string>start</string>"));
        assert!(plist.contains("<string>--port</string>"));
        assert!(plist.contains("<string>3721</string>"));
        assert!(plist.contains("<string>/tmp/taskcast.log</string>"));
        assert!(plist.contains("<string>/tmp/taskcast.err.log</string>"));
        assert!(plist.contains("<true/>"));  // RunAtLoad
        assert!(plist.contains("<false/>")); // KeepAlive
        // Should NOT contain optional args
        assert!(!plist.contains("--config"));
        assert!(!plist.contains("--storage"));
        assert!(!plist.contains("--db-path"));
    }

    #[test]
    fn plist_all_options() {
        let opts = ServiceInstallOptions {
            port: 8080,
            config: Some("/etc/taskcast.yaml".into()),
            storage: Some("sqlite".into()),
            db_path: Some("/data/tasks.db".into()),
            exec_path: "/opt/taskcast".into(),
        };
        let plist = generate_plist(&opts, "/log/out", "/log/err");

        assert!(plist.contains("<string>--config</string>"));
        assert!(plist.contains("<string>/etc/taskcast.yaml</string>"));
        assert!(plist.contains("<string>--storage</string>"));
        assert!(plist.contains("<string>sqlite</string>"));
        assert!(plist.contains("<string>--db-path</string>"));
        assert!(plist.contains("<string>/data/tasks.db</string>"));
        assert!(plist.contains("<string>8080</string>"));
    }

    #[test]
    fn plist_is_valid_xml() {
        let opts = ServiceInstallOptions {
            port: 3721,
            config: None,
            storage: None,
            db_path: None,
            exec_path: "/usr/bin/taskcast".into(),
        };
        let plist = generate_plist(&opts, "/tmp/out", "/tmp/err");
        assert!(plist.starts_with("<?xml version=\"1.0\""));
        assert!(plist.contains("</plist>"));
    }

    #[test]
    fn parse_pid_from_launchctl_output() {
        let output = r#"{
            "LimitLoadToSessionType" = "Aqua";
            "Label" = "com.taskcast.daemon";
            "OnDemand" = false;
            "LastExitStatus" = 0;
            "PID" = 12345;
            "Program" = "/usr/local/bin/taskcast";
        };"#;
        assert_eq!(parse_launchctl_pid(output), Some(12345));
    }

    #[test]
    fn parse_pid_returns_none_when_not_running() {
        let output = r#"{
            "LimitLoadToSessionType" = "Aqua";
            "Label" = "com.taskcast.daemon";
            "OnDemand" = false;
            "LastExitStatus" = 256;
        };"#;
        assert_eq!(parse_launchctl_pid(output), None);
    }

    #[test]
    fn parse_pid_returns_none_for_empty() {
        assert_eq!(parse_launchctl_pid(""), None);
    }
}
```

- [ ] **Step 2: Implement launchd.rs**

Replace `rust/taskcast-cli/src/commands/service/launchd.rs` with:

```rust
use std::fs;
use std::process::Command;

use super::manager::{ServiceInstallOptions, ServiceManager, ServiceStatus};
use super::paths::ServicePaths;

const LAUNCHD_LABEL: &str = "com.taskcast.daemon";

pub struct LaunchdServiceManager;

/// Generate the plist XML content.
pub fn generate_plist(opts: &ServiceInstallOptions, stdout_log: &str, stderr_log: &str) -> String {
    let mut args = vec![
        format!("        <string>{}</string>", opts.exec_path),
        "        <string>start</string>".to_string(),
        "        <string>--port</string>".to_string(),
        format!("        <string>{}</string>", opts.port),
    ];

    if let Some(ref config) = opts.config {
        args.push("        <string>--config</string>".to_string());
        args.push(format!("        <string>{config}</string>"));
    }
    if let Some(ref storage) = opts.storage {
        args.push("        <string>--storage</string>".to_string());
        args.push(format!("        <string>{storage}</string>"));
    }
    if let Some(ref db_path) = opts.db_path {
        args.push("        <string>--db-path</string>".to_string());
        args.push(format!("        <string>{db_path}</string>"));
    }

    let args_xml = args.join("\n");

    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN"
  "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>{LAUNCHD_LABEL}</string>
    <key>ProgramArguments</key>
    <array>
{args_xml}
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
"#
    )
}

/// Parse the PID from `launchctl list <label>` output.
pub fn parse_launchctl_pid(output: &str) -> Option<u32> {
    for line in output.lines() {
        let trimmed = line.trim().trim_end_matches(';');
        if let Some(rest) = trimmed.strip_prefix("\"PID\"") {
            let rest = rest.trim().strip_prefix('=')?.trim();
            return rest.parse().ok();
        }
    }
    None
}

fn get_uid() -> u32 {
    unsafe { libc::getuid() }
}

impl ServiceManager for LaunchdServiceManager {
    fn install(&self, opts: &ServiceInstallOptions) -> Result<(), Box<dyn std::error::Error>> {
        let paths = ServicePaths::new()?;

        if paths.is_installed() {
            return Err("Taskcast service is already installed. Run `taskcast service uninstall` first.".into());
        }

        // Create log directory
        if let Some(ref log_dir) = paths.log_dir {
            fs::create_dir_all(log_dir)?;
        }

        // Create parent directory for plist
        if let Some(parent) = paths.service_file.parent() {
            fs::create_dir_all(parent)?;
        }

        let stdout_log = paths.stdout_log.as_ref().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
        let stderr_log = paths.stderr_log.as_ref().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();

        let plist = generate_plist(opts, &stdout_log, &stderr_log);
        fs::write(&paths.service_file, plist)?;

        Ok(())
    }

    fn uninstall(&self) -> Result<(), Box<dyn std::error::Error>> {
        let paths = ServicePaths::new()?;

        if !paths.is_installed() {
            return Err("Taskcast service is not installed.".into());
        }

        // Stop first, ignoring errors (service might not be running)
        let _ = self.stop();

        fs::remove_file(&paths.service_file)?;
        Ok(())
    }

    fn start(&self) -> Result<(), Box<dyn std::error::Error>> {
        let paths = ServicePaths::new()?;

        if !paths.is_installed() {
            return Err("Taskcast service is not installed. Run `taskcast service install` first.".into());
        }

        let uid = get_uid();
        let plist_path = paths.service_file.to_string_lossy();

        let output = Command::new("launchctl")
            .args(["bootstrap", &format!("gui/{uid}"), &plist_path])
            .output()?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            // "service already loaded" is not a fatal error
            if !stderr.contains("already loaded") {
                return Err(format!("launchctl bootstrap failed: {stderr}").into());
            }
        }

        Ok(())
    }

    fn stop(&self) -> Result<(), Box<dyn std::error::Error>> {
        let uid = get_uid();

        let output = Command::new("launchctl")
            .args(["bootout", &format!("gui/{uid}/{LAUNCHD_LABEL}")])
            .output()?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            if !stderr.contains("not found") {
                return Err(format!("launchctl bootout failed: {stderr}").into());
            }
        }

        Ok(())
    }

    fn restart(&self) -> Result<(), Box<dyn std::error::Error>> {
        let _ = self.stop();
        self.start()
    }

    fn status(&self) -> Result<ServiceStatus, Box<dyn std::error::Error>> {
        let paths = ServicePaths::new()?;

        if !paths.is_installed() {
            return Ok(ServiceStatus::NotInstalled);
        }

        let output = Command::new("launchctl")
            .args(["list", LAUNCHD_LABEL])
            .output()?;

        if !output.status.success() {
            return Ok(ServiceStatus::Stopped);
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        match parse_launchctl_pid(&stdout) {
            Some(pid) => Ok(ServiceStatus::Running { pid }),
            None => Ok(ServiceStatus::Stopped),
        }
    }
}

// Tests at the bottom — see Step 1 above for test code
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plist_minimal_options() {
        let opts = ServiceInstallOptions {
            port: 3721,
            config: None,
            storage: None,
            db_path: None,
            exec_path: "/usr/local/bin/taskcast".into(),
        };
        let plist = generate_plist(&opts, "/tmp/taskcast.log", "/tmp/taskcast.err.log");

        assert!(plist.contains("<string>com.taskcast.daemon</string>"));
        assert!(plist.contains("<string>/usr/local/bin/taskcast</string>"));
        assert!(plist.contains("<string>start</string>"));
        assert!(plist.contains("<string>--port</string>"));
        assert!(plist.contains("<string>3721</string>"));
        assert!(plist.contains("<string>/tmp/taskcast.log</string>"));
        assert!(plist.contains("<string>/tmp/taskcast.err.log</string>"));
        assert!(plist.contains("<true/>"));
        assert!(plist.contains("<false/>"));
        assert!(!plist.contains("--config"));
        assert!(!plist.contains("--storage"));
        assert!(!plist.contains("--db-path"));
    }

    #[test]
    fn plist_all_options() {
        let opts = ServiceInstallOptions {
            port: 8080,
            config: Some("/etc/taskcast.yaml".into()),
            storage: Some("sqlite".into()),
            db_path: Some("/data/tasks.db".into()),
            exec_path: "/opt/taskcast".into(),
        };
        let plist = generate_plist(&opts, "/log/out", "/log/err");

        assert!(plist.contains("<string>--config</string>"));
        assert!(plist.contains("<string>/etc/taskcast.yaml</string>"));
        assert!(plist.contains("<string>--storage</string>"));
        assert!(plist.contains("<string>sqlite</string>"));
        assert!(plist.contains("<string>--db-path</string>"));
        assert!(plist.contains("<string>/data/tasks.db</string>"));
        assert!(plist.contains("<string>8080</string>"));
    }

    #[test]
    fn plist_is_valid_xml_structure() {
        let opts = ServiceInstallOptions {
            port: 3721,
            config: None,
            storage: None,
            db_path: None,
            exec_path: "/usr/bin/taskcast".into(),
        };
        let plist = generate_plist(&opts, "/tmp/out", "/tmp/err");
        assert!(plist.starts_with("<?xml version=\"1.0\""));
        assert!(plist.contains("</plist>"));
        assert!(plist.contains("<!DOCTYPE plist"));
    }

    #[test]
    fn parse_pid_from_launchctl_output() {
        let output = "{\n\t\"PID\" = 12345;\n\t\"Label\" = \"com.taskcast.daemon\";\n};";
        assert_eq!(parse_launchctl_pid(output), Some(12345));
    }

    #[test]
    fn parse_pid_returns_none_when_not_running() {
        let output = "{\n\t\"Label\" = \"com.taskcast.daemon\";\n\t\"LastExitStatus\" = 256;\n};";
        assert_eq!(parse_launchctl_pid(output), None);
    }

    #[test]
    fn parse_pid_returns_none_for_empty() {
        assert_eq!(parse_launchctl_pid(""), None);
    }

    #[test]
    fn parse_pid_handles_zero() {
        let output = "{\n\t\"PID\" = 0;\n};";
        assert_eq!(parse_launchctl_pid(output), Some(0));
    }
}
```

- [ ] **Step 3: Verify**

Run: `cd rust && cargo test -p taskcast-cli -- service::launchd`
Expected: all 7 tests pass (macOS), 0 tests on Linux (module not compiled)

- [ ] **Step 4: Commit**

```bash
git add rust/taskcast-cli/src/commands/service/launchd.rs
git commit -m "feat(cli): implement LaunchdServiceManager for macOS"
```

---

### Task 4: Linux systemd Implementation

**Goal:** Implement `SystemdServiceManager` — unit file generation, systemctl commands, status parsing.

**Files:**
- Modify: `rust/taskcast-cli/src/commands/service/systemd.rs`
- Test: inline `#[cfg(test)]` in `systemd.rs`

**Acceptance Criteria:**
- [ ] `generate_unit_file()` produces correct INI content for all option combinations
- [ ] `install()` writes unit file, runs daemon-reload and enable
- [ ] `uninstall()` disables, daemon-reloads, deletes unit file
- [ ] `start/stop/restart()` call correct systemctl commands
- [ ] `status()` parses `systemctl --user show` output
- [ ] Returns `NotInstalled` when unit file doesn't exist

**Verify:** `cd rust && cargo test -p taskcast-cli -- service::systemd` → all pass (Linux only)

**Steps:**

- [ ] **Step 1: Implement systemd.rs**

Replace `rust/taskcast-cli/src/commands/service/systemd.rs` with:

```rust
use std::fs;
use std::process::Command;

use super::manager::{ServiceInstallOptions, ServiceManager, ServiceStatus};
use super::paths::ServicePaths;

pub struct SystemdServiceManager;

/// Quote a string if it contains spaces.
fn quote_if_needed(s: &str) -> String {
    if s.contains(' ') {
        format!("\"{s}\"")
    } else {
        s.to_string()
    }
}

/// Generate the systemd unit file content.
pub fn generate_unit_file(opts: &ServiceInstallOptions) -> String {
    let mut exec_parts = vec![
        quote_if_needed(&opts.exec_path),
        "start".to_string(),
        "--port".to_string(),
        opts.port.to_string(),
    ];

    if let Some(ref config) = opts.config {
        exec_parts.push("--config".to_string());
        exec_parts.push(quote_if_needed(config));
    }
    if let Some(ref storage) = opts.storage {
        exec_parts.push("--storage".to_string());
        exec_parts.push(quote_if_needed(storage));
    }
    if let Some(ref db_path) = opts.db_path {
        exec_parts.push("--db-path".to_string());
        exec_parts.push(quote_if_needed(db_path));
    }

    let exec_start = exec_parts.join(" ");

    format!(
        "[Unit]\n\
         Description=Taskcast \u{2014} unified task tracking and streaming service\n\
         After=network.target\n\
         \n\
         [Service]\n\
         Type=simple\n\
         ExecStart={exec_start}\n\
         Restart=no\n\
         \n\
         [Install]\n\
         WantedBy=default.target\n"
    )
}

/// Parse ActiveState and MainPID from `systemctl --user show` output.
pub fn parse_systemctl_status(output: &str) -> ServiceStatus {
    let mut active_state = "";
    let mut main_pid: u32 = 0;

    for line in output.lines() {
        if let Some(rest) = line.strip_prefix("ActiveState=") {
            active_state = rest.trim();
        } else if let Some(rest) = line.strip_prefix("MainPID=") {
            main_pid = rest.trim().parse().unwrap_or(0);
        }
    }

    if active_state == "active" && main_pid > 0 {
        ServiceStatus::Running { pid: main_pid }
    } else {
        ServiceStatus::Stopped
    }
}

fn systemctl(args: &[&str]) -> Result<std::process::Output, Box<dyn std::error::Error>> {
    let mut cmd_args = vec!["--user"];
    cmd_args.extend_from_slice(args);
    let output = Command::new("systemctl").args(&cmd_args).output()?;
    Ok(output)
}

impl ServiceManager for SystemdServiceManager {
    fn install(&self, opts: &ServiceInstallOptions) -> Result<(), Box<dyn std::error::Error>> {
        let paths = ServicePaths::new()?;

        if paths.is_installed() {
            return Err("Taskcast service is already installed. Run `taskcast service uninstall` first.".into());
        }

        // Create parent directory for unit file
        if let Some(parent) = paths.service_file.parent() {
            fs::create_dir_all(parent)?;
        }

        let unit = generate_unit_file(opts);
        fs::write(&paths.service_file, unit)?;

        // Reload systemd and enable the service
        systemctl(&["daemon-reload"])?;
        systemctl(&["enable", "taskcast"])?;

        Ok(())
    }

    fn uninstall(&self) -> Result<(), Box<dyn std::error::Error>> {
        let paths = ServicePaths::new()?;

        if !paths.is_installed() {
            return Err("Taskcast service is not installed.".into());
        }

        // Disable and stop
        let _ = systemctl(&["disable", "taskcast"]);
        let _ = self.stop();

        systemctl(&["daemon-reload"])?;
        fs::remove_file(&paths.service_file)?;
        Ok(())
    }

    fn start(&self) -> Result<(), Box<dyn std::error::Error>> {
        let paths = ServicePaths::new()?;

        if !paths.is_installed() {
            return Err("Taskcast service is not installed. Run `taskcast service install` first.".into());
        }

        let output = systemctl(&["start", "taskcast"])?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(format!("systemctl start failed: {stderr}").into());
        }
        Ok(())
    }

    fn stop(&self) -> Result<(), Box<dyn std::error::Error>> {
        let output = systemctl(&["stop", "taskcast"])?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            if !stderr.contains("not loaded") {
                return Err(format!("systemctl stop failed: {stderr}").into());
            }
        }
        Ok(())
    }

    fn restart(&self) -> Result<(), Box<dyn std::error::Error>> {
        let output = systemctl(&["restart", "taskcast"])?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(format!("systemctl restart failed: {stderr}").into());
        }
        Ok(())
    }

    fn status(&self) -> Result<ServiceStatus, Box<dyn std::error::Error>> {
        let paths = ServicePaths::new()?;

        if !paths.is_installed() {
            return Ok(ServiceStatus::NotInstalled);
        }

        let output = systemctl(&["show", "taskcast", "--property=ActiveState,MainPID"])?;
        let stdout = String::from_utf8_lossy(&output.stdout);
        Ok(parse_systemctl_status(&stdout))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unit_file_minimal_options() {
        let opts = ServiceInstallOptions {
            port: 3721,
            config: None,
            storage: None,
            db_path: None,
            exec_path: "/usr/local/bin/taskcast".into(),
        };
        let unit = generate_unit_file(&opts);

        assert!(unit.contains("Description=Taskcast"));
        assert!(unit.contains("After=network.target"));
        assert!(unit.contains("Type=simple"));
        assert!(unit.contains("ExecStart=/usr/local/bin/taskcast start --port 3721"));
        assert!(unit.contains("Restart=no"));
        assert!(unit.contains("WantedBy=default.target"));
        assert!(!unit.contains("--config"));
        assert!(!unit.contains("--storage"));
        assert!(!unit.contains("--db-path"));
    }

    #[test]
    fn unit_file_all_options() {
        let opts = ServiceInstallOptions {
            port: 8080,
            config: Some("/etc/taskcast.yaml".into()),
            storage: Some("sqlite".into()),
            db_path: Some("/data/tasks.db".into()),
            exec_path: "/opt/taskcast".into(),
        };
        let unit = generate_unit_file(&opts);

        assert!(unit.contains("ExecStart=/opt/taskcast start --port 8080 --config /etc/taskcast.yaml --storage sqlite --db-path /data/tasks.db"));
    }

    #[test]
    fn unit_file_quotes_paths_with_spaces() {
        let opts = ServiceInstallOptions {
            port: 3721,
            config: Some("/path with spaces/config.yaml".into()),
            storage: None,
            db_path: None,
            exec_path: "/usr/local/bin/taskcast".into(),
        };
        let unit = generate_unit_file(&opts);

        assert!(unit.contains("--config \"/path with spaces/config.yaml\""));
    }

    #[test]
    fn parse_status_running() {
        let output = "ActiveState=active\nMainPID=42\n";
        assert_eq!(parse_systemctl_status(output), ServiceStatus::Running { pid: 42 });
    }

    #[test]
    fn parse_status_stopped() {
        let output = "ActiveState=inactive\nMainPID=0\n";
        assert_eq!(parse_systemctl_status(output), ServiceStatus::Stopped);
    }

    #[test]
    fn parse_status_active_but_no_pid() {
        let output = "ActiveState=active\nMainPID=0\n";
        assert_eq!(parse_systemctl_status(output), ServiceStatus::Stopped);
    }

    #[test]
    fn parse_status_empty_output() {
        assert_eq!(parse_systemctl_status(""), ServiceStatus::Stopped);
    }

    #[test]
    fn parse_status_failed_state() {
        let output = "ActiveState=failed\nMainPID=0\n";
        assert_eq!(parse_systemctl_status(output), ServiceStatus::Stopped);
    }

    #[test]
    fn quote_if_needed_no_spaces() {
        assert_eq!(quote_if_needed("/usr/bin/taskcast"), "/usr/bin/taskcast");
    }

    #[test]
    fn quote_if_needed_with_spaces() {
        assert_eq!(quote_if_needed("/path with spaces"), "\"/path with spaces\"");
    }
}
```

- [ ] **Step 2: Verify**

Run: `cd rust && cargo test -p taskcast-cli -- service::systemd`
Expected: all 10 tests pass (Linux), 0 tests on macOS (module not compiled)

- [ ] **Step 3: Commit**

```bash
git add rust/taskcast-cli/src/commands/service/systemd.rs
git commit -m "feat(cli): implement SystemdServiceManager for Linux"
```

---

### Task 5: Health Check

**Goal:** Implement the health check polling logic used after `service start` and `service restart`.

**Files:**
- Modify: `rust/taskcast-cli/src/commands/service/health.rs`
- Test: inline `#[cfg(test)]` in `health.rs`

**Acceptance Criteria:**
- [ ] `poll_health()` polls `GET /health` at 500ms intervals up to timeout
- [ ] Returns `true` on first successful (2xx) response
- [ ] Returns `false` after timeout expires
- [ ] Silently ignores connection errors during polling
- [ ] Timeout and interval are configurable (for testing)

**Verify:** `cd rust && cargo test -p taskcast-cli -- service::health` → all pass

**Steps:**

- [ ] **Step 1: Implement health.rs**

Replace `rust/taskcast-cli/src/commands/service/health.rs` with:

```rust
use std::time::{Duration, Instant};

/// Poll a health endpoint until it responds 2xx or timeout expires.
///
/// Returns `true` if the server responded successfully within the timeout.
pub async fn poll_health(port: u16, timeout_ms: u64, interval_ms: u64) -> bool {
    let url = format!("http://localhost:{port}/health");
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(2))
        .build()
        .unwrap_or_default();

    let deadline = Instant::now() + Duration::from_millis(timeout_ms);

    while Instant::now() < deadline {
        match client.get(&url).send().await {
            Ok(resp) if resp.status().is_success() => return true,
            _ => {}
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        let sleep_dur = Duration::from_millis(interval_ms).min(remaining);
        if sleep_dur.is_zero() {
            break;
        }
        tokio::time::sleep(sleep_dur).await;
    }

    false
}

/// Fetch detailed health info from `/health/detail`.
/// Returns (uptime_seconds, storage_provider) if available.
pub async fn fetch_health_detail(port: u16) -> Option<(Option<f64>, Option<String>)> {
    let url = format!("http://localhost:{port}/health/detail");
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .ok()?;

    let resp = client.get(&url).send().await.ok()?;
    if !resp.status().is_success() {
        return None;
    }

    let body: serde_json::Value = resp.json().await.ok()?;
    let uptime = body.get("uptime").and_then(|v| v.as_f64());
    let storage = body
        .get("adapters")
        .and_then(|a| a.get("shortTermStore"))
        .and_then(|s| s.get("provider"))
        .and_then(|p| p.as_str())
        .map(|s| s.to_string());

    Some((uptime, storage))
}

/// Format seconds into "Xh Ym" display.
pub fn format_uptime(seconds: f64) -> String {
    let total_secs = seconds as u64;
    let hours = total_secs / 3600;
    let minutes = (total_secs % 3600) / 60;
    if hours > 0 {
        format!("{hours}h {minutes}m")
    } else {
        format!("{minutes}m")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_uptime_hours_and_minutes() {
        assert_eq!(format_uptime(9000.0), "2h 30m");
    }

    #[test]
    fn format_uptime_minutes_only() {
        assert_eq!(format_uptime(300.0), "5m");
    }

    #[test]
    fn format_uptime_zero() {
        assert_eq!(format_uptime(0.0), "0m");
    }

    #[test]
    fn format_uptime_less_than_minute() {
        assert_eq!(format_uptime(45.0), "0m");
    }

    #[test]
    fn format_uptime_exact_hour() {
        assert_eq!(format_uptime(3600.0), "1h 0m");
    }

    #[tokio::test]
    async fn poll_health_returns_false_on_no_server() {
        // Use a port that's very unlikely to be in use
        let result = poll_health(19999, 500, 100).await;
        assert!(!result);
    }
}
```

- [ ] **Step 2: Verify**

Run: `cd rust && cargo test -p taskcast-cli -- service::health`
Expected: all 6 tests pass

- [ ] **Step 3: Commit**

```bash
git add rust/taskcast-cli/src/commands/service/health.rs
git commit -m "feat(cli): add health check polling for service management"
```

---

### Task 6: Service Command Handlers

**Goal:** Implement the orchestration logic in `service/mod.rs` — the public `run()` function and individual handlers for install, uninstall, start, stop, restart, reload, status.

**Files:**
- Modify: `rust/taskcast-cli/src/commands/service/mod.rs`
- Test: inline `#[cfg(test)]` in `mod.rs`

**Acceptance Criteria:**
- [ ] `ServiceArgs` struct with Clap-derived subcommands matching spec
- [ ] `run()` dispatches to correct handler
- [ ] `run_install()` validates port, calls ensure_config, mgr.install, writes state
- [ ] `run_start()` checks status, calls mgr.start, runs health check, prints result
- [ ] `run_stop()` checks status, calls mgr.stop
- [ ] `run_status()` shows service/pid/uptime/storage
- [ ] All output prefixed with `[taskcast]`
- [ ] Port validation rejects 0 and > 65535

**Verify:** `cd rust && cargo test -p taskcast-cli -- service::tests` → all pass

**Steps:**

- [ ] **Step 1: Implement service/mod.rs**

Replace `rust/taskcast-cli/src/commands/service/mod.rs` with:

```rust
pub mod manager;
pub mod paths;
pub mod health;

#[cfg(target_os = "macos")]
pub mod launchd;

#[cfg(target_os = "linux")]
pub mod systemd;

use clap::{Args, Subcommand};

use self::manager::{create_service_manager, ServiceInstallOptions, ServiceStatus};
use self::paths::{ServicePaths, delete_state, ensure_config, get_port, write_state};
use self::health::{fetch_health_detail, format_uptime, poll_health};

const HEALTH_TIMEOUT_MS: u64 = 5000;
const HEALTH_INTERVAL_MS: u64 = 500;

#[derive(Args)]
pub struct ServiceArgs {
    #[command(subcommand)]
    pub command: ServiceCommands,
}

#[derive(Subcommand)]
pub enum ServiceCommands {
    /// Register and enable the Taskcast service for auto-start
    Install {
        /// Path to config file
        #[arg(short = 'c', long)]
        config: Option<String>,

        /// Port to listen on
        #[arg(short = 'p', long, default_value_t = 3721)]
        port: u16,

        /// Storage backend: memory | redis | sqlite
        #[arg(short = 's', long)]
        storage: Option<String>,

        /// SQLite database file path
        #[arg(long)]
        db_path: Option<String>,
    },
    /// Remove the Taskcast service registration
    Uninstall,
    /// Start the Taskcast service
    Start,
    /// Stop the Taskcast service
    Stop,
    /// Restart the Taskcast service
    Restart,
    /// Reload the Taskcast service (alias for restart)
    Reload,
    /// Show Taskcast service status
    Status,
}

pub async fn run(args: ServiceArgs) -> Result<(), Box<dyn std::error::Error>> {
    match args.command {
        ServiceCommands::Install { config, port, storage, db_path } => {
            run_install(config, port, storage, db_path)?;
        }
        ServiceCommands::Uninstall => {
            run_uninstall()?;
        }
        ServiceCommands::Start => {
            run_start().await?;
        }
        ServiceCommands::Stop => {
            run_stop()?;
        }
        ServiceCommands::Restart | ServiceCommands::Reload => {
            run_restart().await?;
        }
        ServiceCommands::Status => {
            run_status().await?;
        }
    }
    Ok(())
}

fn run_install(
    config: Option<String>,
    port: u16,
    storage: Option<String>,
    db_path: Option<String>,
) -> Result<(), Box<dyn std::error::Error>> {
    if port == 0 {
        eprintln!("[taskcast] Invalid port: {port}");
        std::process::exit(1);
    }

    let mgr = create_service_manager()?;
    let paths = ServicePaths::new()?;

    let config_path = ensure_config(&paths, config.as_deref())?;

    // Default to sqlite if no explicit config or storage specified
    let storage = if config.is_none() && storage.is_none() {
        Some("sqlite".to_string())
    } else {
        storage
    };

    let exec_path = std::env::current_exe()?
        .to_string_lossy()
        .into_owned();

    let opts = ServiceInstallOptions {
        port,
        config: Some(config_path),
        storage,
        db_path,
        exec_path,
    };

    mgr.install(&opts)?;
    write_state(&paths, port)?;

    eprintln!("[taskcast] Service installed successfully.");
    eprintln!("[taskcast] Run 'taskcast service start' to start the service.");
    Ok(())
}

fn run_uninstall() -> Result<(), Box<dyn std::error::Error>> {
    let mgr = create_service_manager()?;
    let paths = ServicePaths::new()?;

    mgr.uninstall()?;
    delete_state(&paths);

    eprintln!("[taskcast] Service uninstalled.");
    Ok(())
}

pub async fn run_start() -> Result<(), Box<dyn std::error::Error>> {
    let mgr = create_service_manager()?;
    let paths = ServicePaths::new()?;

    let status = mgr.status()?;
    if let ServiceStatus::Running { .. } = status {
        eprintln!("[taskcast] Service is already running.");
        return Ok(());
    }

    mgr.start()?;

    let port = get_port(&paths);
    if poll_health(port, HEALTH_TIMEOUT_MS, HEALTH_INTERVAL_MS).await {
        eprintln!("[taskcast] Service started on http://localhost:{port}");
    } else {
        eprintln!("[taskcast] Service may have failed to start. Check logs:");
        print_log_hint(&paths);
    }

    Ok(())
}

pub fn run_stop() -> Result<(), Box<dyn std::error::Error>> {
    let mgr = create_service_manager()?;

    let status = mgr.status()?;
    if !matches!(status, ServiceStatus::Running { .. }) {
        eprintln!("[taskcast] Service is not running.");
        return Ok(());
    }

    mgr.stop()?;
    eprintln!("[taskcast] Service stopped.");
    Ok(())
}

async fn run_restart() -> Result<(), Box<dyn std::error::Error>> {
    let mgr = create_service_manager()?;
    let paths = ServicePaths::new()?;

    mgr.restart()?;

    let port = get_port(&paths);
    if poll_health(port, HEALTH_TIMEOUT_MS, HEALTH_INTERVAL_MS).await {
        eprintln!("[taskcast] Service started on http://localhost:{port}");
    } else {
        eprintln!("[taskcast] Service may have failed to start. Check logs:");
        print_log_hint(&paths);
    }

    Ok(())
}

pub async fn run_status() -> Result<(), Box<dyn std::error::Error>> {
    let mgr = create_service_manager()?;
    let paths = ServicePaths::new()?;

    let status = mgr.status()?;
    eprintln!("Service:   {status}");

    if let ServiceStatus::Running { .. } = &status {
        let port = get_port(&paths);
        if let Some((uptime, storage)) = fetch_health_detail(port).await {
            if let Some(secs) = uptime {
                eprintln!("Uptime:    {}", format_uptime(secs));
            }
            if let Some(provider) = storage {
                eprintln!("Storage:   {provider}");
            }
        }
    }

    Ok(())
}

fn print_log_hint(paths: &ServicePaths) {
    if let Some(ref stdout_log) = paths.stdout_log {
        eprintln!("  {}", stdout_log.display());
    }
    if let Some(ref stderr_log) = paths.stderr_log {
        eprintln!("  {}", stderr_log.display());
    }
    if paths.stdout_log.is_none() {
        // Linux: no file logs, use journalctl
        eprintln!("  journalctl --user -u taskcast");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn service_commands_install_defaults() {
        // Verify the default port value is 3721
        use clap::Parser;

        #[derive(Parser)]
        struct TestCli {
            #[command(subcommand)]
            cmd: ServiceCommands,
        }

        let cli = TestCli::parse_from(["test", "install"]);
        match cli.cmd {
            ServiceCommands::Install { port, config, storage, db_path } => {
                assert_eq!(port, 3721);
                assert!(config.is_none());
                assert!(storage.is_none());
                assert!(db_path.is_none());
            }
            _ => panic!("expected Install"),
        }
    }

    #[test]
    fn service_commands_install_with_all_flags() {
        use clap::Parser;

        #[derive(Parser)]
        struct TestCli {
            #[command(subcommand)]
            cmd: ServiceCommands,
        }

        let cli = TestCli::parse_from([
            "test", "install",
            "-c", "/etc/tc.yaml",
            "-p", "8080",
            "-s", "sqlite",
            "--db-path", "/data/tc.db",
        ]);
        match cli.cmd {
            ServiceCommands::Install { config, port, storage, db_path } => {
                assert_eq!(config, Some("/etc/tc.yaml".to_string()));
                assert_eq!(port, 8080);
                assert_eq!(storage, Some("sqlite".to_string()));
                assert_eq!(db_path, Some("/data/tc.db".to_string()));
            }
            _ => panic!("expected Install"),
        }
    }

    #[test]
    fn service_commands_all_variants_parse() {
        use clap::Parser;

        #[derive(Parser)]
        struct TestCli {
            #[command(subcommand)]
            cmd: ServiceCommands,
        }

        for cmd in ["uninstall", "start", "stop", "restart", "reload", "status"] {
            let cli = TestCli::parse_from(["test", cmd]);
            match (cmd, cli.cmd) {
                ("uninstall", ServiceCommands::Uninstall) => {},
                ("start", ServiceCommands::Start) => {},
                ("stop", ServiceCommands::Stop) => {},
                ("restart", ServiceCommands::Restart) => {},
                ("reload", ServiceCommands::Reload) => {},
                ("status", ServiceCommands::Status) => {},
                _ => panic!("unexpected parse for {cmd}"),
            }
        }
    }
}
```

- [ ] **Step 2: Verify**

Run: `cd rust && cargo test -p taskcast-cli -- service::tests`
Expected: 3 tests pass

- [ ] **Step 3: Commit**

```bash
git add rust/taskcast-cli/src/commands/service/mod.rs
git commit -m "feat(cli): add service command handlers (install/start/stop/status)"
```

---

### Task 7: CLI Wiring — main.rs Integration

**Goal:** Wire the new `service` subcommand into `main.rs`, replace the Daemon/Stop/Status placeholders with working aliases, and update existing tests.

**Files:**
- Modify: `rust/taskcast-cli/src/main.rs:20-49` (Commands enum) and `rust/taskcast-cli/src/main.rs:51-107` (main fn)
- Test: update existing tests in `main.rs`

**Acceptance Criteria:**
- [ ] `taskcast service install/uninstall/start/stop/restart/reload/status` work
- [ ] `taskcast daemon` delegates to `run_start()`
- [ ] `taskcast stop` delegates to `run_stop()`
- [ ] `taskcast status` delegates to `run_status()`
- [ ] Old placeholder behavior (eprintln + exit 1) is removed
- [ ] Existing CLI parsing tests updated for new command structure
- [ ] New tests for service subcommand parsing and alias parsing

**Verify:** `cd rust && cargo test -p taskcast-cli` → all tests pass, `cargo build -p taskcast-cli` → compiles

**Steps:**

- [ ] **Step 1: Update Commands enum in main.rs**

Replace the Commands enum (lines 20-49) with:

```rust
#[derive(Subcommand)]
enum Commands {
    /// Start the taskcast server in foreground (default)
    Start(commands::start::StartArgs),
    /// Serve only the playground UI (no engine)
    Playground(commands::playground::PlaygroundArgs),
    /// Run Postgres database migrations
    Migrate(commands::migrate::MigrateArgs),
    /// Manage Taskcast server connections
    Node {
        #[command(subcommand)]
        command: commands::node::NodeCommands,
    },
    /// Deep health check against a Taskcast server
    Doctor(commands::doctor::DoctorArgs),
    /// Quick connectivity check against a Taskcast server
    Ping(commands::ping::PingArgs),
    /// Stream events from a task in real-time
    Logs(commands::logs::LogsArgs),
    /// Stream events from all tasks in real-time
    Tail(commands::logs::TailArgs),
    /// Manage tasks on a Taskcast server
    Tasks(commands::tasks::TasksArgs),
    /// Manage Taskcast as a background system service
    Service(commands::service::ServiceArgs),
    /// Alias for `taskcast service start`
    Daemon,
    /// Alias for `taskcast service stop`
    Stop,
    /// Alias for `taskcast service status`
    Status,
}
```

- [ ] **Step 2: Update main() match arms**

Replace the match block (lines 54-104) — change the Daemon/Stop/Status arms from printing "not yet implemented" to delegating:

```rust
    match cli.command {
        None => {
            commands::start::run(commands::start::StartArgs::default()).await?;
        }
        Some(Commands::Start(args)) => {
            commands::start::run(args).await?;
        }
        Some(Commands::Migrate(args)) => {
            commands::migrate::run(args).await?;
        }
        Some(Commands::Playground(args)) => {
            commands::playground::run(args).await?;
        }
        Some(Commands::Node { command }) => {
            if let Err(e) = commands::node::run(command) {
                eprintln!("{e}");
                std::process::exit(1);
            }
        }
        Some(Commands::Doctor(args)) => {
            commands::doctor::run(args).await?;
        }
        Some(Commands::Ping(args)) => {
            if let Err(e) = commands::ping::run(args).await {
                eprintln!("{e}");
                std::process::exit(1);
            }
        }
        Some(Commands::Logs(args)) => {
            commands::logs::run_logs(args).await?;
        }
        Some(Commands::Tail(args)) => {
            commands::logs::run_tail(args).await?;
        }
        Some(Commands::Tasks(args)) => {
            commands::tasks::run(args).await?;
        }
        Some(Commands::Service(args)) => {
            commands::service::run(args).await?;
        }
        Some(Commands::Daemon) => {
            commands::service::run_start().await?;
        }
        Some(Commands::Stop) => {
            commands::service::run_stop()?;
        }
        Some(Commands::Status) => {
            commands::service::run_status().await?;
        }
    }
```

Note: `run_stop` is a sync function, so no `.await`. `run_start` and `run_status` are async.

- [ ] **Step 3: Update existing tests in main.rs**

Update the test for `cli_daemon_subcommand_parses`:

```rust
    #[test]
    fn cli_daemon_subcommand_parses() {
        let cli = Cli::parse_from(["taskcast", "daemon"]);
        assert!(matches!(cli.command.unwrap(), Commands::Daemon));
    }
```

(This test should remain unchanged since Daemon is still a variant.)

Update `cli_stop_subcommand_parses` and `cli_status_subcommand_parses` similarly — they should still work since the variants still exist.

Add new tests for the `service` subcommand:

```rust
    #[test]
    fn cli_service_install_defaults() {
        let cli = Cli::parse_from(["taskcast", "service", "install"]);
        match cli.command.unwrap() {
            Commands::Service(args) => match args.command {
                commands::service::ServiceCommands::Install { port, config, storage, db_path } => {
                    assert_eq!(port, 3721);
                    assert!(config.is_none());
                    assert!(storage.is_none());
                    assert!(db_path.is_none());
                }
                _ => panic!("expected Install"),
            },
            _ => panic!("expected Service"),
        }
    }

    #[test]
    fn cli_service_install_with_all_flags() {
        let cli = Cli::parse_from([
            "taskcast", "service", "install",
            "-c", "/etc/tc.yaml",
            "-p", "8080",
            "-s", "sqlite",
            "--db-path", "/data/tc.db",
        ]);
        match cli.command.unwrap() {
            Commands::Service(args) => match args.command {
                commands::service::ServiceCommands::Install { config, port, storage, db_path } => {
                    assert_eq!(config, Some("/etc/tc.yaml".to_string()));
                    assert_eq!(port, 8080);
                    assert_eq!(storage, Some("sqlite".to_string()));
                    assert_eq!(db_path, Some("/data/tc.db".to_string()));
                }
                _ => panic!("expected Install"),
            },
            _ => panic!("expected Service"),
        }
    }

    #[test]
    fn cli_service_all_subcommands_parse() {
        for cmd in ["uninstall", "start", "stop", "restart", "reload", "status"] {
            let cli = Cli::parse_from(["taskcast", "service", cmd]);
            assert!(matches!(cli.command.unwrap(), Commands::Service(_)), "failed to parse service {cmd}");
        }
    }
```

- [ ] **Step 4: Verify**

Run: `cd rust && cargo test -p taskcast-cli`
Expected: all tests pass (existing + new)

Run: `cd rust && cargo build -p taskcast-cli`
Expected: compiles clean

- [ ] **Step 5: Manual smoke test**

Run: `cd rust && cargo run -p taskcast-cli -- service status`
Expected: `Service:   not installed` (since nothing is installed yet)

Run: `cd rust && cargo run -p taskcast-cli -- daemon`
Expected: Either starts the service or shows "not installed" error

- [ ] **Step 6: Commit**

```bash
git add rust/taskcast-cli/src/main.rs rust/taskcast-cli/src/commands/service/mod.rs
git commit -m "feat(cli): wire service subcommand and daemon/stop/status aliases into CLI"
```
