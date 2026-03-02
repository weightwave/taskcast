# SQLite Local Storage

**Date:** 2026-03-02
**Status:** Approved

## Problem

Local development requires Redis + PostgreSQL, which is heavy for a quick `npx taskcast` experience. We need a zero-config persistent storage option that survives server restarts without external databases.

## Design

### New Package

- **TS:** `@taskcast/sqlite` at `packages/sqlite`
- **Rust:** `taskcast-sqlite` at `rust/taskcast-sqlite`

### Interfaces Implemented

One SQLite file implements both **ShortTermStore** and **LongTermStore**. BroadcastProvider stays in-memory (single-process is sufficient for local dev).

### Schema

```sql
CREATE TABLE IF NOT EXISTS taskcast_tasks (
  id TEXT PRIMARY KEY,
  type TEXT,
  status TEXT NOT NULL,
  params TEXT,
  result TEXT,
  error TEXT,
  metadata TEXT,
  auth_config TEXT,
  webhooks TEXT,
  cleanup TEXT,
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL,
  completed_at INTEGER,
  ttl INTEGER
);

CREATE TABLE IF NOT EXISTS taskcast_events (
  id TEXT PRIMARY KEY,
  task_id TEXT NOT NULL REFERENCES taskcast_tasks(id) ON DELETE CASCADE,
  idx INTEGER NOT NULL,
  timestamp INTEGER NOT NULL,
  type TEXT NOT NULL,
  level TEXT NOT NULL,
  data TEXT,
  series_id TEXT,
  series_mode TEXT,
  UNIQUE(task_id, idx)
);

CREATE TABLE IF NOT EXISTS taskcast_series_latest (
  task_id TEXT NOT NULL,
  series_id TEXT NOT NULL,
  event_json TEXT NOT NULL,
  PRIMARY KEY (task_id, series_id)
);

CREATE TABLE IF NOT EXISTS taskcast_index_counters (
  task_id TEXT PRIMARY KEY,
  counter INTEGER NOT NULL DEFAULT -1
);

CREATE INDEX IF NOT EXISTS idx_events_task_idx ON taskcast_events(task_id, idx);
CREATE INDEX IF NOT EXISTS idx_events_task_ts ON taskcast_events(task_id, timestamp);
```

JSON columns use TEXT (SQLite has no native JSONB). Schema mirrors the PostgreSQL migration for maximum code reuse.

### Configuration

WAL mode is enabled by default for better read/write concurrency.

### Technology Choices

- **TS:** `better-sqlite3` — synchronous API, fast, fits ShortTermStore's sync write semantics
- **Rust:** `sqlx` with SQLite feature — consistent with existing PostgreSQL adapter's sqlx usage

### CLI Integration

```bash
taskcast start                                        # default: memory
taskcast start --storage sqlite                       # SQLite at ./taskcast.db
taskcast start --storage sqlite --db-path /tmp/my.db  # custom path
```

Environment variables: `TASKCAST_STORAGE=sqlite`, `TASKCAST_SQLITE_PATH=./taskcast.db`

### Package Structure

```
packages/sqlite/
  src/
    index.ts         # exports + createSqliteAdapters()
    short-term.ts    # SqliteShortTermStore implements ShortTermStore
    long-term.ts     # SqliteLongTermStore implements LongTermStore
  migrations/
    001_initial.sql
  tests/
    unit/
    integration/
  package.json

rust/taskcast-sqlite/
  src/
    lib.rs
    short_term.rs
    long_term.rs
  migrations/
    001_initial.sql
  Cargo.toml
```

### Out of Scope

- BroadcastProvider implementation (memory is sufficient for single-process)
- Advanced PRAGMA tuning beyond WAL mode
- Cross-process locking (single-process use case)
