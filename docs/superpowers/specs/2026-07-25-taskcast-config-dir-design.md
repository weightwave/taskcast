# Taskcast CLI Config Directory Design

**Date:** 2026-07-25

## Context

The Rust CLI currently constructs `~/.taskcast` independently in the `node`,
`ping`, `doctor`, `logs`, `tasks`, and service commands. This duplicates path
policy and makes integration tests unsafe on Windows: `dirs::home_dir()` uses
the Windows profile API, so changing `HOME` or `USERPROFILE` does not redirect
the CLI away from the real user profile.

Taskcast needs one supported way to isolate CLI configuration for tests,
containers, and automation while preserving existing behavior for normal
users.

## Goals

- Add `TASKCAST_CONFIG_DIR` as the single supported override for Taskcast CLI
  configuration data.
- Centralize configuration-directory selection so all CLI commands follow the
  same policy.
- Prevent integration tests from reading or writing the real user profile on
  every supported platform.
- Preserve the current `~/.taskcast` location when no non-empty override is
  configured.
- Keep errors explicit and avoid logging configuration contents or stored
  credentials.

## Non-goals

- Moving OS-managed systemd or launchd service files.
- Moving the macOS service log directory.
- Migrating, merging, or searching multiple configuration directories.
- Adding command-line flags for the configuration directory.
- Changing the format of `nodes.json`, `taskcast.config.yaml`, the SQLite
  database, or the service state file.
- Changing TypeScript runtime configuration behavior.

## Architecture

Add a small Rust CLI module that owns configuration-directory policy and
exposes a resolver such as:

```rust
pub fn taskcast_config_dir() -> Result<PathBuf, ConfigDirError>
```

The exact error representation may follow the CLI's existing error conventions,
but the resolver must return an error instead of panicking when the fallback
home directory cannot be determined.

The resolver reads the override with `std::env::var_os` so non-Unicode paths
remain valid. Its policy is:

1. If `TASKCAST_CONFIG_DIR` is present and non-empty, return that path.
2. Otherwise, return `dirs::home_dir()/.taskcast`.
3. If neither produces a directory, return a clear
   `cannot determine Taskcast config directory` error.

An override path is not canonicalized and does not need to exist. A relative
override therefore has the operating system's normal meaning: filesystem
operations resolve it against the CLI process's current working directory.
Commands that write data continue to create required parent directories using
their existing behavior.

`NodeConfigManager` remains path-injected and does not read environment
variables itself. The command layer resolves the directory once and passes it
to the manager. This keeps the manager independently testable and prevents
hidden global-state dependencies.

## Command Integration

The following command paths must use the central resolver:

- `node`
- `ping`
- `doctor`
- `logs`
- `tail`
- all `tasks` paths that load a selected node

Any helper that currently returns a `NodeConfigManager` directly must propagate
resolver failure through the command's existing `Result` instead of unwrapping
or panicking.

`ServicePaths` needs two distinct roots:

- The real OS home directory remains the root for the systemd unit, launchd
  plist, and macOS service logs.
- The resolved Taskcast config directory becomes the root for
  `taskcast.config.yaml`, `taskcast.db`, and `service.state.json`.

Consequently, setting `TASKCAST_CONFIG_DIR` changes Taskcast-owned configuration
and state, but never redirects files whose locations are prescribed by the
operating system's service manager.

## Data Flow

For commands that select a node:

1. The command calls `taskcast_config_dir()`.
2. The command constructs `NodeConfigManager` from the returned path.
3. The manager reads or writes `<config-dir>/nodes.json`.
4. Existing node selection and credential behavior remains unchanged.

For service commands:

1. `ServicePaths::new()` determines the real OS home directory.
2. It independently calls `taskcast_config_dir()`.
3. It builds service-manager paths from the OS home and Taskcast-owned paths
   from the resolved config directory.

The resolver must not read, print, or log `nodes.json` or any token value.

## Error Handling

- An empty `TASKCAST_CONFIG_DIR` is treated as unset. It must never make the
  current directory the implicit configuration directory.
- A missing override directory is valid; existing write operations may create
  it when needed.
- Failure to determine the fallback home directory is returned as a
  human-readable command error, not a panic.
- Filesystem read and write behavior remains owned by existing managers and
  commands; the resolver only selects a path.
- There is no fallback from an explicitly selected override to `~/.taskcast`.
  This avoids surprising cross-directory reads and credential selection.

## Testing

Development follows test-driven order:

1. Add failing resolver tests for a non-empty override, an empty override,
   fallback behavior, relative paths, and supported non-Unicode OS strings.
2. Add or update failing command integration tests proving that an isolated
   temporary config directory supplies node selection.
3. Implement the resolver and migrate command call sites.
4. Update service-path tests to prove that Taskcast-owned files follow the
   override while OS service-manager files do not.

Resolver policy should be factored so most unit tests can pass explicit inputs
without mutating process environment. Integration tests that must exercise the
public environment-reading boundary use:

- A temporary directory.
- A process-global mutex within each integration-test binary.
- A panic-safe RAII guard that restores the prior environment value.
- Poisoned-mutex recovery so one failed assertion does not cause unrelated
  follow-on failures.

Affected `doctor`, `logs`, and `tasks` tests must stop constructing managers
under the real home directory. Tests must not create or modify the user's real
`~/.taskcast`.

The unrelated TypeScript completion-gate failure is corrected only in its test:
the missing explicit config path is constructed under `os.tmpdir()` and the
assertion compares against `path.resolve()`. TypeScript production behavior is
unchanged.

Final verification includes:

- Targeted TypeScript core config tests.
- Targeted Rust resolver and command integration tests.
- `pnpm test`.
- `cargo test --workspace`.
- The repository's build and relevant lint gates.

## Documentation

The CLI README must document:

- `TASKCAST_CONFIG_DIR` controls Taskcast-owned CLI configuration and state.
- The default remains `~/.taskcast`.
- Relative paths are interpreted from the process working directory.
- The variable does not relocate systemd units, launchd plists, or macOS
  service logs.

## Acceptance Criteria

- Normal CLI behavior is unchanged when `TASKCAST_CONFIG_DIR` is absent or
  empty.
- Every Rust CLI command uses the central resolver for Taskcast-owned
  configuration.
- No affected integration test relies on changing `HOME` or `USERPROFILE`.
- The Windows `doctor` success tests read their node from a temporary
  directory.
- Service configuration, database, and state honor the override without moving
  OS service-manager files.
- No real user configuration is created or changed by the test suite.
- Full TypeScript and Rust workspace test gates pass, unless a newly observed
  failure is independently demonstrated to be an external infrastructure
  failure and reported separately.
