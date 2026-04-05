---
"@taskcast/cli": patch
---

fix(rust-cli): include --db-path in service config when defaulting to sqlite

When `taskcast service install` auto-selected SQLite storage, the generated
launchd plist / systemd unit file did not include `--db-path`, causing the
server to use the relative default `./taskcast.db`. Since launchd starts
processes with cwd=/, SQLite failed with "unable to open database file".
