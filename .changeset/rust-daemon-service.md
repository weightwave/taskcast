---
"@taskcast/cli": minor
---

feat(rust-cli): add daemon/service management for Rust CLI

- Add `taskcast service` subcommand group (install/uninstall/start/stop/restart/reload/status)
- Add graceful shutdown (SIGTERM/SIGINT) handling for `taskcast start`
- macOS: launchd plist generation + launchctl commands
- Linux: systemd unit file generation + systemctl --user commands
- `daemon`/`stop`/`status` top-level aliases for backward compatibility
- Health check polling after start/restart
- Auto-config creation with SQLite defaults on `service install`
- Windows: trait reserved, returns unsupported error at runtime
