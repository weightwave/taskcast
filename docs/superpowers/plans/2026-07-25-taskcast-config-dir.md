# Taskcast CLI Config Directory Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a cross-platform `TASKCAST_CONFIG_DIR` override to the Rust CLI, remove real-home dependencies from affected integration tests, and clear the two Windows-only workspace test failures.

**Architecture:** A focused `config_dir` module owns the environment-variable and fallback policy while `NodeConfigManager` remains path-injected. Command entry points resolve the directory and propagate errors; service management keeps OS-owned files under the real home directory while Taskcast-owned config, database, and state files use the resolver.

**Tech Stack:** Rust 2021, Cargo, `dirs` 6, Tokio integration tests, `tempfile`, TypeScript, Vitest, pnpm.

## Global Constraints

- `TASKCAST_CONFIG_DIR` is used only when present and non-empty.
- The default remains `dirs::home_dir()/.taskcast`.
- Relative overrides are preserved and therefore resolve from the CLI process working directory.
- Non-Unicode override paths must remain supported through `std::env::var_os`.
- An explicit override never falls back to or merges with `~/.taskcast`.
- `NodeConfigManager` remains path-injected and must not read process environment itself.
- systemd units, launchd plists, and macOS service logs remain under the real OS home directory.
- `taskcast.config.yaml`, `taskcast.db`, `service.state.json`, and `nodes.json` follow the resolved Taskcast config directory.
- Tests must not read or write the real user `~/.taskcast`.
- Preserve the existing minimum dependency versions and add no new dependency.
- Preserve the two current uncommitted test edits until their owning tasks commit them; do not reset or overwrite them.
- All Rust commands run with `CARGO_TARGET_DIR=D:\Projects\weightwave\taskcast\rust\target`.
- Loopback integration tests set both `NO_PROXY` and `no_proxy` to `127.0.0.1,localhost`.

---

## File Structure

- Create `rust/taskcast-cli/src/config_dir.rs`: the only production module that reads `TASKCAST_CONFIG_DIR` and applies fallback policy.
- Modify `rust/taskcast-cli/src/lib.rs`: export the new module to command code and integration tests.
- Create `rust/taskcast-cli/tests/common/mod.rs`: integration-test helper module declaration.
- Create `rust/taskcast-cli/tests/common/config_dir.rs`: panic-safe, poison-tolerant environment guard backed by a temporary config directory.
- Modify `rust/taskcast-cli/src/commands/{node,ping,doctor,logs,tasks}.rs`: replace duplicated home-directory resolution with `taskcast_config_dir()`.
- Modify `rust/taskcast-cli/src/commands/service/paths.rs`: separate OS-home paths from Taskcast-owned paths.
- Modify `rust/taskcast-cli/tests/{node_run_tests,ping_run_tests,doctor_run_tests,logs_tests,tasks_tests}.rs`: use `TASKCAST_CONFIG_DIR` isolation and remove `HOME` mutation.
- Modify `packages/core/tests/unit/config.test.ts`: make one explicit-path assertion platform-neutral.
- Modify `packages/cli/README.md`: document the override and its service-path boundary.

### Task 1: Commit the Cross-Platform TypeScript Config Test

**Files:**

- Modify: `packages/core/tests/unit/config.test.ts:1-2`
- Modify: `packages/core/tests/unit/config.test.ts:177-187`

**Interfaces:**

- Consumes: existing `loadConfigFile(path)` behavior, which returns an absolute resolved path.
- Produces: a platform-neutral test only; no production interface changes.

The RED phase is already captured from the original test: Windows returned
`D:\tmp\taskcast-nonexistent-xyz-12345.yaml` while the test expected the
literal POSIX path `/tmp/taskcast-nonexistent-xyz-12345.yaml`. The worktree
already contains the candidate test-only correction.

- [ ] **Step 1: Verify that the worktree contains exactly the intended test change**

The imports and test body must be:

```ts
import { writeFileSync, unlinkSync, mkdirSync, rmSync, existsSync } from 'fs'
import { join, resolve } from 'path'
```

```ts
it('returns source "explicit" with path when explicit path does not exist', async () => {
  const nonexistentPath = join(tmpdir(), `taskcast-nonexistent-${Date.now()}.yaml`)
  expect(existsSync(nonexistentPath)).toBe(false)

  const result = await loadConfigFile(nonexistentPath)
  expect(result.config).toEqual({})
  expect(result.source).toBe('explicit')
  expect(result.path).toBe(resolve(nonexistentPath))
})
```

Run:

```powershell
git diff -- packages/core/tests/unit/config.test.ts
```

Expected: only the import and explicit-missing-path test shown above differ.

- [ ] **Step 2: Run the targeted test**

Run:

```powershell
pnpm --filter @taskcast/core test -- tests/unit/config.test.ts
```

Expected: `1` test file passes and all `36` tests pass.

- [ ] **Step 3: Commit the test correction**

```powershell
git add -- packages/core/tests/unit/config.test.ts
git commit -m "test(core): make explicit config path portable"
```

Expected: the commit contains only `packages/core/tests/unit/config.test.ts`.

### Task 2: Add the Central Rust Config Directory Resolver

**Files:**

- Create: `rust/taskcast-cli/src/config_dir.rs`
- Modify: `rust/taskcast-cli/src/lib.rs:1-7`

**Interfaces:**

- Consumes: `std::env::var_os("TASKCAST_CONFIG_DIR")` and `dirs::home_dir()`.
- Produces: `pub fn taskcast_config_dir() -> Result<PathBuf, &'static str>` and `pub const TASKCAST_CONFIG_DIR_ENV: &str`.

- [ ] **Step 1: Write resolver policy tests before the implementation**

Add `pub mod config_dir;` to `rust/taskcast-cli/src/lib.rs`.

Create `rust/taskcast-cli/src/config_dir.rs` with the test module first:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;

    #[test]
    fn non_empty_override_wins() {
        let override_dir = OsString::from("isolated-config");
        let result = resolve_taskcast_config_dir(Some(override_dir), || {
            panic!("home lookup must not run when an override is present")
        });
        assert_eq!(result.unwrap(), PathBuf::from("isolated-config"));
    }

    #[test]
    fn relative_override_is_preserved() {
        let result = resolve_taskcast_config_dir(
            Some(OsString::from("relative/config")),
            || Some(PathBuf::from("unused-home")),
        );
        assert_eq!(result.unwrap(), PathBuf::from("relative/config"));
    }

    #[test]
    fn empty_override_falls_back_to_dot_taskcast() {
        let home = PathBuf::from("fake-home");
        let result = resolve_taskcast_config_dir(Some(OsString::new()), || {
            Some(home.clone())
        });
        assert_eq!(result.unwrap(), home.join(".taskcast"));
    }

    #[test]
    fn absent_override_falls_back_to_dot_taskcast() {
        let home = PathBuf::from("fake-home");
        let result = resolve_taskcast_config_dir(None, || Some(home.clone()));
        assert_eq!(result.unwrap(), home.join(".taskcast"));
    }

    #[test]
    fn missing_override_and_home_returns_clear_error() {
        let result = resolve_taskcast_config_dir(None, || None);
        assert_eq!(result.unwrap_err(), CONFIG_DIR_ERROR);
    }

    #[cfg(unix)]
    #[test]
    fn unix_non_unicode_override_is_preserved() {
        use std::os::unix::ffi::OsStringExt;
        let raw = OsString::from_vec(vec![b'c', b'f', b'g', 0xff]);
        let result = resolve_taskcast_config_dir(Some(raw.clone()), || None);
        assert_eq!(result.unwrap(), PathBuf::from(raw));
    }

    #[cfg(windows)]
    #[test]
    fn windows_non_unicode_override_is_preserved() {
        use std::os::windows::ffi::OsStringExt;
        let raw = OsString::from_wide(&[b'c' as u16, b'f' as u16, b'g' as u16, 0xd800]);
        let result = resolve_taskcast_config_dir(Some(raw.clone()), || None);
        assert_eq!(result.unwrap(), PathBuf::from(raw));
    }
}
```

- [ ] **Step 2: Run the resolver test and verify RED**

Run:

```powershell
$env:CARGO_TARGET_DIR='D:\Projects\weightwave\taskcast\rust\target'
cargo test -p taskcast-cli config_dir::tests
```

Expected: compilation fails because `resolve_taskcast_config_dir`,
`CONFIG_DIR_ERROR`, and the required imports are not defined.

- [ ] **Step 3: Add the minimal resolver above the test module**

```rust
use std::ffi::OsString;
use std::path::PathBuf;

pub const TASKCAST_CONFIG_DIR_ENV: &str = "TASKCAST_CONFIG_DIR";
const CONFIG_DIR_ERROR: &str = "cannot determine Taskcast config directory";

fn resolve_taskcast_config_dir(
    override_dir: Option<OsString>,
    home_dir: impl FnOnce() -> Option<PathBuf>,
) -> Result<PathBuf, &'static str> {
    match override_dir {
        Some(path) if !path.is_empty() => Ok(PathBuf::from(path)),
        _ => home_dir()
            .map(|home| home.join(".taskcast"))
            .ok_or(CONFIG_DIR_ERROR),
    }
}

pub fn taskcast_config_dir() -> Result<PathBuf, &'static str> {
    resolve_taskcast_config_dir(
        std::env::var_os(TASKCAST_CONFIG_DIR_ENV),
        dirs::home_dir,
    )
}
```

- [ ] **Step 4: Format and run the resolver tests**

Run:

```powershell
cargo fmt --all
cargo test -p taskcast-cli config_dir::tests
```

Expected: all six platform-applicable resolver tests pass.

- [ ] **Step 5: Commit the resolver**

```powershell
git add -- rust/taskcast-cli/src/config_dir.rs rust/taskcast-cli/src/lib.rs
git commit -m "feat(cli): resolve configurable config directory"
```

### Task 3: Migrate Node, Ping, and Doctor Commands

**Files:**

- Create: `rust/taskcast-cli/tests/common/mod.rs`
- Create: `rust/taskcast-cli/tests/common/config_dir.rs`
- Modify: `rust/taskcast-cli/src/commands/node.rs:1-57`
- Modify: `rust/taskcast-cli/src/commands/ping.rs:1-49`
- Modify: `rust/taskcast-cli/src/commands/doctor.rs:1-219`
- Modify: `rust/taskcast-cli/tests/node_run_tests.rs`
- Modify: `rust/taskcast-cli/tests/ping_run_tests.rs`
- Modify: `rust/taskcast-cli/tests/doctor_run_tests.rs`

**Interfaces:**

- Consumes: `taskcast_cli::config_dir::taskcast_config_dir()`.
- Produces: `IsolatedConfigDir::new()` and `IsolatedConfigDir::path()` for integration tests; `node`, `ping`, and `doctor` honor `TASKCAST_CONFIG_DIR`.

- [ ] **Step 1: Add the shared panic-safe integration-test guard**

Create `rust/taskcast-cli/tests/common/mod.rs`:

```rust
pub mod config_dir;
```

Create `rust/taskcast-cli/tests/common/config_dir.rs`:

```rust
use std::ffi::OsString;
use std::path::Path;
use std::sync::{Mutex, MutexGuard};

use tempfile::TempDir;

const CONFIG_DIR_ENV: &str = "TASKCAST_CONFIG_DIR";
static CONFIG_DIR_LOCK: Mutex<()> = Mutex::new(());

pub struct IsolatedConfigDir {
    dir: TempDir,
    previous: Option<OsString>,
    _lock: MutexGuard<'static, ()>,
}

impl IsolatedConfigDir {
    pub fn new() -> Self {
        let lock = CONFIG_DIR_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let previous = std::env::var_os(CONFIG_DIR_ENV);
        let dir = TempDir::new().expect("create isolated Taskcast config directory");
        std::env::set_var(CONFIG_DIR_ENV, dir.path());

        Self {
            dir,
            previous,
            _lock: lock,
        }
    }

    pub fn path(&self) -> &Path {
        self.dir.path()
    }
}

impl Drop for IsolatedConfigDir {
    fn drop(&mut self) {
        match &self.previous {
            Some(value) => std::env::set_var(CONFIG_DIR_ENV, value),
            None => std::env::remove_var(CONFIG_DIR_ENV),
        }
    }
}
```

- [ ] **Step 2: Convert the three command integration tests to the guard**

At the top of each of
`node_run_tests.rs`, `ping_run_tests.rs`, and `doctor_run_tests.rs`, add:

```rust
mod common;

use common::config_dir::IsolatedConfigDir;
```

Delete each file's `HOME_LOCK`, `HOME_MUTEX`, `HomeEnvGuard`, direct
`HOME`/`USERPROFILE` mutation, and their now-unused `Mutex`, `MutexGuard`,
`OsString`, `Path`, and `TempDir` imports.

Use this helper in all three files:

```rust
fn setup_config_dir() -> IsolatedConfigDir {
    IsolatedConfigDir::new()
}
```

Apply these exact replacements throughout the three files:

```rust
let _lock = HOME_LOCK.lock().unwrap();
```

is deleted;

```rust
let dir = setup_home();
let _dir = setup_home();
let (dir, _home) = setup_home();
let (_dir, _home) = setup_home();
```

become, respectively:

```rust
let dir = setup_config_dir();
let _dir = setup_config_dir();
let dir = setup_config_dir();
let _dir = setup_config_dir();
```

and every manager path:

```rust
NodeConfigManager::new(dir.path().join(".taskcast"))
```

becomes:

```rust
NodeConfigManager::new(dir.path().to_path_buf())
```

The environment guard must remain alive until each test completes.

- [ ] **Step 3: Confirm the recorded integration RED without rerunning unsafe old wiring**

The Windows RED evidence already captured before this plan is:

- `doctor_run_tests`: two error-path tests passed.
- Both success-path tests failed because the command resolved the real profile
  and returned the default `http://localhost:3721` node instead of the node
  written under the temporary directory.
- The first assertion failure poisoned the former test mutex and caused a
  follow-on `PoisonError`.

Do not run the converted node/ping/doctor tests before Step 4. The old `node`
production path can write to the real `~/.taskcast`, and ping/doctor can contact
a node selected from real user configuration. The resolver's executable RED
cycle is owned by Task 2; this step records the already-observed command-level
RED safely.

- [ ] **Step 4: Make the three commands use the resolver**

In `node.rs`, add:

```rust
use crate::config_dir::taskcast_config_dir;
```

Replace `get_config_manager` and its call with:

```rust
fn get_config_manager() -> Result<NodeConfigManager, Box<dyn std::error::Error>> {
    Ok(NodeConfigManager::new(taskcast_config_dir()?))
}
```

```rust
pub fn run(command: NodeCommands) -> Result<(), Box<dyn std::error::Error>> {
    let mgr = get_config_manager()?;
```

In `ping.rs`, import:

```rust
use crate::config_dir::taskcast_config_dir;
```

In `doctor.rs`, replace the existing `NodeEntry` import with:

```rust
use crate::config_dir::taskcast_config_dir;
use crate::node_config::{NodeConfigManager, NodeEntry};
```

Replace each three-line `dirs::home_dir().expect(...).join(".taskcast")`
block and manager construction with:

```rust
let mgr = NodeConfigManager::new(taskcast_config_dir()?);
```

- [ ] **Step 5: Run targeted tests and library lint**

Run:

```powershell
cargo fmt --all
cargo test -p taskcast-cli --test node_run_tests --test ping_run_tests --test doctor_run_tests
cargo clippy -p taskcast-cli --lib --no-deps -- -D warnings
```

Expected: all three integration-test binaries pass. Clippy may still report the
known Windows-only unused `taskcast_dir` in `service/paths.rs`; it must report
no diagnostic in files changed by this task.

- [ ] **Step 6: Commit the command migration**

```powershell
git add -- rust/taskcast-cli/tests/common/mod.rs rust/taskcast-cli/tests/common/config_dir.rs rust/taskcast-cli/src/commands/node.rs rust/taskcast-cli/src/commands/ping.rs rust/taskcast-cli/src/commands/doctor.rs rust/taskcast-cli/tests/node_run_tests.rs rust/taskcast-cli/tests/ping_run_tests.rs rust/taskcast-cli/tests/doctor_run_tests.rs
git commit -m "fix(cli): isolate node config resolution"
```

### Task 4: Migrate Logs and Tasks Commands

**Files:**

- Modify: `rust/taskcast-cli/src/commands/logs.rs:1-219`
- Modify: `rust/taskcast-cli/src/commands/tasks.rs:1-240`
- Modify: `rust/taskcast-cli/tests/logs_tests.rs:1218-2000`
- Modify: `rust/taskcast-cli/tests/tasks_tests.rs:918-1609`

**Interfaces:**

- Consumes: `taskcast_config_dir()` and `IsolatedConfigDir`.
- Produces: logs, tail, task list, and task inspect paths that all honor the same config directory.

- [ ] **Step 1: Convert logs/tasks test setup to `IsolatedConfigDir`**

Add to both integration-test files:

```rust
mod common;

use common::config_dir::IsolatedConfigDir;
```

Delete `HOME_MUTEX`, its `std::sync::Mutex` import, every
`let _lock = HOME_MUTEX.lock().unwrap();`, and every direct `HOME` set/remove.

In `logs_tests.rs`, replace the two setup helpers with:

```rust
fn setup_config_dir_with_node_at(path: &std::path::Path, base_url: &str) {
    std::fs::create_dir_all(path).unwrap();
    let mgr = NodeConfigManager::new(path.to_path_buf());
    mgr.add(
        "default",
        NodeEntry {
            url: base_url.to_string(),
            token: None,
            token_type: None,
        },
    );
    mgr.set_current("default").unwrap();
}

fn setup_config_dir_with_node(base_url: &str, node_name: &str) -> IsolatedConfigDir {
    let config_dir = IsolatedConfigDir::new();
    let mgr = NodeConfigManager::new(config_dir.path().to_path_buf());
    mgr.add(
        node_name,
        NodeEntry {
            url: base_url.to_string(),
            token: None,
            token_type: None,
        },
    );
    mgr.set_current(node_name).unwrap();
    config_dir
}
```

Use the same `setup_config_dir_with_node` implementation in `tasks_tests.rs`.

Apply these exact replacements in both files:

```text
setup_temp_home_with_node_at  -> setup_config_dir_with_node_at
setup_temp_home_with_node     -> setup_config_dir_with_node
temp_dir.path().join(".taskcast") -> config_dir.path().to_path_buf()
dirs::home_dir().unwrap().join(".taskcast") -> taskcast_cli::config_dir::taskcast_config_dir().unwrap()
```

Rename each local returned by `setup_config_dir_with_node` from `temp_dir` to
`config_dir`, and keep it in scope until the test ends. For tests that only
need an empty config, use:

```rust
let config_dir = IsolatedConfigDir::new();
let _mgr = NodeConfigManager::new(config_dir.path().to_path_buf());
```

For the two direct-tail tests that currently create a `TempDir`, use:

```rust
let config_dir = IsolatedConfigDir::new();
setup_config_dir_with_node_at(config_dir.path(), &base_url);
```

- [ ] **Step 2: Verify that production still has four unresolved call sites**

Run:

```powershell
rg -n 'dirs::home_dir' rust/taskcast-cli/src/commands/logs.rs rust/taskcast-cli/src/commands/tasks.rs
```

Expected: exactly four production matches: `run_logs`, `run_tail`, `run_list`,
and `run_inspect`.

Do not execute the converted tests until Step 3 is complete. Old production
wiring could read credentials from the real profile and contact a real
configured node.

- [ ] **Step 3: Replace all four production resolution sites**

Add to both production files:

```rust
use crate::config_dir::taskcast_config_dir;
```

In `run_logs`, `run_tail`, `run_list`, and `run_inspect`, replace:

```rust
let config_dir = dirs::home_dir()
    .expect("could not determine home directory")
    .join(".taskcast");
let mgr = NodeConfigManager::new(config_dir);
```

with:

```rust
let mgr = NodeConfigManager::new(taskcast_config_dir()?);
```

- [ ] **Step 4: Prove the affected tests no longer depend on HOME**

Run:

```powershell
rg -n 'set_var\("HOME"|remove_var\("HOME"|dirs::home_dir' rust/taskcast-cli/tests/logs_tests.rs rust/taskcast-cli/tests/tasks_tests.rs
```

Expected: no matches.

Then run:

```powershell
cargo fmt --all
cargo test -p taskcast-cli --test logs_tests --test tasks_tests
```

Expected: both integration-test binaries pass.

- [ ] **Step 5: Commit logs/tasks migration**

```powershell
git add -- rust/taskcast-cli/src/commands/logs.rs rust/taskcast-cli/src/commands/tasks.rs rust/taskcast-cli/tests/logs_tests.rs rust/taskcast-cli/tests/tasks_tests.rs
git commit -m "fix(cli): share config directory across commands"
```

### Task 5: Separate Service-Manager and Taskcast-Owned Paths

**Files:**

- Modify: `rust/taskcast-cli/src/commands/service/paths.rs:1-45`
- Test: `rust/taskcast-cli/src/commands/service/paths.rs` test module

**Interfaces:**

- Consumes: `taskcast_config_dir()`.
- Produces: `ServicePaths::new()` with separate real-home and Taskcast config roots; private `ServicePaths::from_roots(home, taskcast_dir)` on macOS/Linux.

- [ ] **Step 1: Write the service-root separation test**

Add this test to the existing test module:

```rust
#[cfg(any(target_os = "macos", target_os = "linux"))]
#[test]
fn taskcast_owned_files_use_config_root_without_relocating_service_files() {
    let tmp = TempDir::new().unwrap();
    let home = tmp.path().join("home");
    let config = tmp.path().join("isolated-config");
    let paths = ServicePaths::from_roots(home.clone(), config.clone()).unwrap();

    assert_eq!(paths.default_config, config.join("taskcast.config.yaml"));
    assert_eq!(paths.default_db, config.join("taskcast.db"));
    assert_eq!(paths.state_file, config.join("service.state.json"));

    #[cfg(target_os = "macos")]
    {
        assert_eq!(
            paths.service_file,
            home.join("Library/LaunchAgents/com.taskcast.daemon.plist")
        );
        assert_eq!(
            paths.stdout_log.unwrap(),
            home.join("Library/Application Support/taskcast/taskcast.log")
        );
    }

    #[cfg(target_os = "linux")]
    {
        assert_eq!(
            paths.service_file,
            home.join(".config/systemd/user/taskcast.service")
        );
    }
}
```

- [ ] **Step 2: Run the service test and verify RED on a supported platform**

Run:

```powershell
$env:CARGO_TARGET_DIR='D:\Projects\weightwave\taskcast\rust\target'
cargo test -p taskcast-cli commands::service::paths::tests
```

Expected on macOS/Linux: compilation fails because `ServicePaths::from_roots`
does not exist. On Windows the platform-specific test is not compiled; the
new behavior is verified by the subsequent cross-platform compile plus CI on
supported service platforms.

- [ ] **Step 3: Refactor `ServicePaths::new` and add `from_roots`**

Import the resolver:

```rust
use crate::config_dir::taskcast_config_dir;
```

Replace `ServicePaths::new()` and its platform construction with:

```rust
impl ServicePaths {
    pub fn new() -> Result<Self, Box<dyn std::error::Error>> {
        #[cfg(any(target_os = "macos", target_os = "linux"))]
        {
            let home = dirs::home_dir().ok_or("cannot determine home directory")?;
            let taskcast_dir = taskcast_config_dir()?;
            Self::from_roots(home, taskcast_dir)
        }

        #[cfg(not(any(target_os = "macos", target_os = "linux")))]
        {
            Err("Service paths are not defined for this platform".into())
        }
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    fn from_roots(
        home: PathBuf,
        taskcast_dir: PathBuf,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        #[cfg(target_os = "macos")]
        {
            let log_dir = home.join("Library/Application Support/taskcast");
            return Ok(ServicePaths {
                service_file: home.join("Library/LaunchAgents/com.taskcast.daemon.plist"),
                stdout_log: Some(log_dir.join("taskcast.log")),
                stderr_log: Some(log_dir.join("taskcast.err.log")),
                log_dir: Some(log_dir),
                default_config: taskcast_dir.join("taskcast.config.yaml"),
                default_db: taskcast_dir.join("taskcast.db"),
                state_file: taskcast_dir.join("service.state.json"),
            });
        }

        #[cfg(target_os = "linux")]
        {
            Ok(ServicePaths {
                service_file: home.join(".config/systemd/user/taskcast.service"),
                log_dir: None,
                stdout_log: None,
                stderr_log: None,
                default_config: taskcast_dir.join("taskcast.config.yaml"),
                default_db: taskcast_dir.join("taskcast.db"),
                state_file: taskcast_dir.join("service.state.json"),
            })
        }
    }

    pub fn is_installed(&self) -> bool {
        self.service_file.exists()
    }
}
```

The platform `cfg` around the local variables is required: it also removes the
pre-existing Windows `unused variable: taskcast_dir` CLI library lint.

- [ ] **Step 4: Run service tests and CLI library lint**

Run:

```powershell
cargo fmt --all
cargo test -p taskcast-cli commands::service::paths::tests
cargo clippy -p taskcast-cli --lib --no-deps -- -D warnings
```

Expected: service tests pass and CLI library clippy passes with no warnings.

- [ ] **Step 5: Commit service path separation**

```powershell
git add -- rust/taskcast-cli/src/commands/service/paths.rs
git commit -m "fix(cli): separate config and service paths"
```

### Task 6: Document and Audit the Config Directory Contract

**Files:**

- Modify: `packages/cli/README.md:53-66`
- Modify: `packages/cli/README.md` immediately after the environment table

**Interfaces:**

- Consumes: the implemented environment contract.
- Produces: user-facing documentation and a source audit proving that command call sites are centralized.

- [ ] **Step 1: Add the environment variable row**

Add this row to the environment table:

```markdown
| `TASKCAST_CONFIG_DIR` | Rust CLI configuration and state directory; relative paths use the process working directory | `~/.taskcast` |
```

- [ ] **Step 2: Document scope and service boundaries**

Add this paragraph immediately below the table:

```markdown
The Rust CLI stores `nodes.json`, its default service configuration, SQLite
database, and service state under `TASKCAST_CONFIG_DIR`. An unset or empty value
keeps the default `~/.taskcast`. The override does not relocate systemd unit
files, launchd plist files, or macOS service logs, whose locations remain
controlled by the operating system.
```

- [ ] **Step 3: Audit production and test call sites**

Run:

```powershell
rg -n 'dirs::home_dir|\.join\("\.taskcast"\)' rust/taskcast-cli/src rust/taskcast-cli/tests
```

Expected production matches are limited to:

- `config_dir.rs`, where the default is defined.
- `commands/service/paths.rs`, where the real home is required for OS-owned
  service files.
- Existing unit assertions that explicitly test fallback or service-manager
  locations.

Run:

```powershell
rg -n 'set_var\("HOME"|remove_var\("HOME"|set_var\("USERPROFILE"|remove_var\("USERPROFILE"' rust/taskcast-cli/tests/node_run_tests.rs rust/taskcast-cli/tests/ping_run_tests.rs rust/taskcast-cli/tests/doctor_run_tests.rs rust/taskcast-cli/tests/logs_tests.rs rust/taskcast-cli/tests/tasks_tests.rs
```

Expected: no matches.

- [ ] **Step 4: Commit documentation**

```powershell
git add -- packages/cli/README.md
git commit -m "docs(cli): document config directory override"
```

### Task 7: Run Complete Gates and Prepare Review

**Files:**

- Verify all files changed since `e4be624`.
- Update only files required to fix diagnostics introduced by this plan.

**Interfaces:**

- Consumes: all prior task commits.
- Produces: fresh verification evidence suitable for final branch review.

- [ ] **Step 1: Check formatting and whitespace**

Run:

```powershell
cargo fmt --all -- --check
git diff --check e4be624..HEAD
```

Expected: both commands exit `0`.

- [ ] **Step 2: Run TypeScript lint and build**

Run:

```powershell
pnpm lint
pnpm build
```

Expected: both commands exit `0`; the existing Vite dynamic/static import
chunk notice is non-fatal.

- [ ] **Step 3: Run the complete TypeScript suite**

Run:

```powershell
$env:NO_PROXY='127.0.0.1,localhost'
$env:no_proxy=$env:NO_PROXY
pnpm test
```

Expected: exit `0`, including
`packages/core/tests/unit/config.test.ts`.

- [ ] **Step 4: Run the CLI lint and focused Rust suites**

Run:

```powershell
$env:CARGO_TARGET_DIR='D:\Projects\weightwave\taskcast\rust\target'
cargo clippy -p taskcast-cli --lib --no-deps -- -D warnings
cargo test -p taskcast-cli --test node_run_tests --test ping_run_tests --test doctor_run_tests --test logs_tests --test tasks_tests
```

Expected: both commands exit `0`.

- [ ] **Step 5: Run the complete Rust workspace suite**

Run:

```powershell
cargo test --workspace
```

Expected: exit `0`, including all four `doctor_run_tests`. If a Docker image
pull fails with a independently reproducible registry/network error, preserve
the output and report it separately; do not retag, delete, or restart the
user's Docker resources.

- [ ] **Step 6: Confirm branch scope**

Run:

```powershell
git status --short --branch
git log --oneline e4be624..HEAD
git diff --stat e4be624..HEAD
git diff --check e4be624..HEAD
```

Expected: the worktree is clean, commits correspond to Tasks 1-6, and the
final diff contains only the implementation plan plus the planned test,
resolver, command, service-path, integration-test, and documentation changes.

- [ ] **Step 7: Request final code review**

Use `superpowers:requesting-code-review` against the complete range
`e4be624..HEAD`. Any Critical or Important finding must be fixed and all
relevant verification rerun before branch completion is claimed.
