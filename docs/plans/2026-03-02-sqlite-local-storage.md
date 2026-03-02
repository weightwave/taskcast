# SQLite Local Storage Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Add a `@taskcast/sqlite` package (and Rust `taskcast-sqlite`) that implements both ShortTermStore and LongTermStore using a single SQLite file, enabling zero-dependency local development with persistent storage.

**Architecture:** A single SQLite database file provides both short-term (event buffer + task state) and long-term (archive) storage via four tables. BroadcastProvider stays in-memory (single-process). WAL mode enabled by default. CLI gains `--storage sqlite` flag.

**Tech Stack:** TypeScript: `better-sqlite3` (sync API). Rust: `sqlx` with SQLite feature. Tests: Vitest + temp files (no testcontainers needed).

---

## Task 1: Scaffold TS package

**Files:**
- Create: `packages/sqlite/package.json`
- Create: `packages/sqlite/tsconfig.json`
- Create: `packages/sqlite/vitest.config.ts`
- Create: `packages/sqlite/src/index.ts` (empty placeholder)
- Modify: `tsconfig.json` (root — add project reference)
- Modify: `.changeset/config.json` (add to fixed group)

**Step 1: Create `packages/sqlite/package.json`**

```json
{
  "name": "@taskcast/sqlite",
  "version": "0.1.1",
  "description": "SQLite local storage adapter for Taskcast.",
  "repository": {
    "type": "git",
    "url": "https://github.com/weightwave/taskcast.git",
    "directory": "packages/sqlite"
  },
  "homepage": "https://github.com/weightwave/taskcast/tree/main/packages/sqlite#readme",
  "bugs": {
    "url": "https://github.com/weightwave/taskcast/issues"
  },
  "license": "MIT",
  "files": [
    "dist",
    "migrations",
    "LICENSE",
    "README.md"
  ],
  "type": "module",
  "exports": {
    ".": {
      "types": "./dist/index.d.ts",
      "import": "./dist/index.js"
    }
  },
  "publishConfig": {
    "provenance": true
  },
  "scripts": {
    "build": "tsc",
    "test": "vitest run",
    "test:watch": "vitest"
  },
  "dependencies": {
    "@taskcast/core": "workspace:*",
    "better-sqlite3": "^11.0.0"
  },
  "devDependencies": {
    "@types/better-sqlite3": "^7.6.0",
    "typescript": "^5.7.0",
    "vitest": "^2.1.0",
    "@vitest/coverage-v8": "^2.1.0"
  }
}
```

**Step 2: Create `packages/sqlite/tsconfig.json`**

```json
{
  "extends": "../../tsconfig.base.json",
  "compilerOptions": {
    "rootDir": "src",
    "outDir": "dist",
    "composite": true
  },
  "include": ["src"],
  "references": [{ "path": "../core" }]
}
```

**Step 3: Create `packages/sqlite/vitest.config.ts`**

```typescript
import { defineConfig } from 'vitest/config'

export default defineConfig({
  test: {
    include: ['tests/**/*.test.ts'],
    coverage: {
      provider: 'v8',
      reporter: ['text', 'lcov'],
      include: ['src/**'],
      thresholds: { lines: 85, functions: 85 },
    },
  },
})
```

**Step 4: Create `packages/sqlite/src/index.ts`** (placeholder)

```typescript
// SQLite adapter — implementation coming in subsequent tasks
```

**Step 5: Add to root `tsconfig.json` references**

Add `{ "path": "packages/sqlite" }` to the references array.

**Step 6: Add `@taskcast/sqlite` to `.changeset/config.json` fixed group**

**Step 7: Run `pnpm install` and `pnpm build`**

Verify the new package is recognized and builds.

**Step 8: Commit**

```bash
git add packages/sqlite/ tsconfig.json .changeset/config.json
git commit -m "chore: scaffold @taskcast/sqlite package"
```

---

## Task 2: Create SQLite migration

**Files:**
- Create: `packages/sqlite/migrations/001_initial.sql`

**Step 1: Write the migration file**

```sql
-- Tasks table (mirrors Postgres schema, JSONB → TEXT)
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

-- Events table
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

-- Series latest tracking (ShortTermStore-specific)
CREATE TABLE IF NOT EXISTS taskcast_series_latest (
  task_id TEXT NOT NULL,
  series_id TEXT NOT NULL,
  event_json TEXT NOT NULL,
  PRIMARY KEY (task_id, series_id)
);

-- Atomic index counters (ShortTermStore-specific)
CREATE TABLE IF NOT EXISTS taskcast_index_counters (
  task_id TEXT PRIMARY KEY,
  counter INTEGER NOT NULL DEFAULT -1
);

CREATE INDEX IF NOT EXISTS idx_events_task_idx ON taskcast_events(task_id, idx);
CREATE INDEX IF NOT EXISTS idx_events_task_ts ON taskcast_events(task_id, timestamp);
```

**Step 2: Commit**

```bash
git add packages/sqlite/migrations/
git commit -m "feat(sqlite): add initial migration schema"
```

---

## Task 3: Implement SqliteShortTermStore

**Files:**
- Create: `packages/sqlite/src/short-term.ts`
- Modify: `packages/sqlite/src/index.ts`

**Step 1: Write the failing test** (create `packages/sqlite/tests/short-term.test.ts`)

```typescript
import { describe, it, expect, beforeEach, afterEach } from 'vitest'
import { mkdtempSync, rmSync, readFileSync } from 'node:fs'
import { join } from 'node:path'
import { tmpdir } from 'node:os'
import Database from 'better-sqlite3'
import { SqliteShortTermStore } from '../src/short-term.js'
import type { Task, TaskEvent } from '@taskcast/core'

function makeTask(id = 'task-1'): Task {
  return { id, status: 'pending', params: { prompt: 'hello' }, createdAt: 1000, updatedAt: 1000 }
}

function makeEvent(taskId: string, index: number): TaskEvent {
  return {
    id: `evt-${taskId}-${index}`,
    taskId,
    index,
    timestamp: 1000 + index * 100,
    type: 'llm.delta',
    level: 'info',
    data: { text: `msg-${index}` },
  }
}

describe('SqliteShortTermStore', () => {
  let dir: string
  let db: Database.Database
  let store: SqliteShortTermStore

  beforeEach(() => {
    dir = mkdtempSync(join(tmpdir(), 'taskcast-sqlite-'))
    db = new Database(join(dir, 'test.db'))
    const migration = readFileSync(
      join(import.meta.dirname, '../migrations/001_initial.sql'),
      'utf8',
    )
    db.exec(migration)
    db.pragma('journal_mode = WAL')
    db.pragma('foreign_keys = ON')
    store = new SqliteShortTermStore(db)
  })

  afterEach(() => {
    db.close()
    rmSync(dir, { recursive: true, force: true })
  })

  it('saves and retrieves a task', async () => {
    await store.saveTask(makeTask())
    const task = await store.getTask('task-1')
    expect(task?.id).toBe('task-1')
    expect(task?.status).toBe('pending')
    expect(task?.params).toEqual({ prompt: 'hello' })
  })

  it('returns null for missing task', async () => {
    expect(await store.getTask('nope')).toBeNull()
  })

  it('upserts task on conflict', async () => {
    await store.saveTask(makeTask())
    await store.saveTask({ ...makeTask(), status: 'running', updatedAt: 2000 })
    const task = await store.getTask('task-1')
    expect(task?.status).toBe('running')
  })

  it('generates monotonic indices', async () => {
    expect(await store.nextIndex('task-1')).toBe(0)
    expect(await store.nextIndex('task-1')).toBe(1)
    expect(await store.nextIndex('task-1')).toBe(2)
  })

  it('appends and retrieves events in order', async () => {
    await store.saveTask(makeTask())
    for (let i = 0; i < 3; i++) await store.appendEvent('task-1', makeEvent('task-1', i))
    const events = await store.getEvents('task-1')
    expect(events).toHaveLength(3)
    expect(events.map((e) => e.index)).toEqual([0, 1, 2])
  })

  it('filters events by since.index', async () => {
    await store.saveTask(makeTask())
    for (let i = 0; i < 5; i++) await store.appendEvent('task-1', makeEvent('task-1', i))
    const events = await store.getEvents('task-1', { since: { index: 2 } })
    expect(events.map((e) => e.index)).toEqual([3, 4])
  })

  it('filters events by since.timestamp', async () => {
    await store.saveTask(makeTask())
    for (let i = 0; i < 5; i++) await store.appendEvent('task-1', makeEvent('task-1', i))
    const events = await store.getEvents('task-1', { since: { timestamp: 1200 } })
    expect(events.every((e) => e.timestamp > 1200)).toBe(true)
  })

  it('filters events by since.id', async () => {
    await store.saveTask(makeTask())
    for (let i = 0; i < 5; i++) await store.appendEvent('task-1', makeEvent('task-1', i))
    const events = await store.getEvents('task-1', { since: { id: 'evt-task-1-2' } })
    expect(events.map((e) => e.index)).toEqual([3, 4])
  })

  it('returns all events when since.id not found', async () => {
    await store.saveTask(makeTask())
    for (let i = 0; i < 3; i++) await store.appendEvent('task-1', makeEvent('task-1', i))
    const events = await store.getEvents('task-1', { since: { id: 'nonexistent' } })
    expect(events).toHaveLength(3)
  })

  it('respects limit parameter', async () => {
    await store.saveTask(makeTask())
    for (let i = 0; i < 10; i++) await store.appendEvent('task-1', makeEvent('task-1', i))
    const events = await store.getEvents('task-1', { limit: 3 })
    expect(events).toHaveLength(3)
  })

  it('manages series latest', async () => {
    await store.saveTask(makeTask())
    const evt = makeEvent('task-1', 0)
    await store.setSeriesLatest('task-1', 'tokens', evt)
    const latest = await store.getSeriesLatest('task-1', 'tokens')
    expect(latest?.id).toBe(evt.id)
  })

  it('returns null for missing series', async () => {
    expect(await store.getSeriesLatest('task-1', 'nope')).toBeNull()
  })

  it('replaces last series event in event list', async () => {
    await store.saveTask(makeTask())
    const evt0 = makeEvent('task-1', 0)
    await store.appendEvent('task-1', evt0)
    await store.setSeriesLatest('task-1', 'tokens', evt0)

    const replacement = { ...evt0, data: { text: 'replaced' } }
    await store.replaceLastSeriesEvent('task-1', 'tokens', replacement)

    const events = await store.getEvents('task-1')
    expect(events).toHaveLength(1)
    expect((events[0]?.data as Record<string, unknown>)?.['text']).toBe('replaced')
  })

  it('appends when no previous series event exists', async () => {
    await store.saveTask(makeTask())
    const evt = makeEvent('task-1', 0)
    await store.replaceLastSeriesEvent('task-1', 'tokens', evt)
    const events = await store.getEvents('task-1')
    expect(events).toHaveLength(1)
  })

  it('handles setTTL as no-op', async () => {
    // SQLite doesn't support key-level TTL; this should not throw
    await store.saveTask(makeTask())
    await store.setTTL('task-1', 60)
  })
})
```

**Step 2: Run test to verify it fails**

Run: `cd packages/sqlite && pnpm test`
Expected: FAIL — `SqliteShortTermStore` not found

**Step 3: Implement `packages/sqlite/src/short-term.ts`**

```typescript
import type Database from 'better-sqlite3'
import type {
  Task,
  TaskEvent,
  ShortTermStore,
  EventQueryOptions,
  TaskError,
  TaskAuthConfig,
  WebhookConfig,
  CleanupRule,
  SeriesMode,
} from '@taskcast/core'

export class SqliteShortTermStore implements ShortTermStore {
  constructor(private db: Database.Database) {}

  async saveTask(task: Task): Promise<void> {
    this.db.prepare(`
      INSERT INTO taskcast_tasks (
        id, type, status, params, result, error, metadata,
        auth_config, webhooks, cleanup, created_at, updated_at, completed_at, ttl
      ) VALUES (
        @id, @type, @status, @params, @result, @error, @metadata,
        @authConfig, @webhooks, @cleanup, @createdAt, @updatedAt, @completedAt, @ttl
      )
      ON CONFLICT (id) DO UPDATE SET
        status = excluded.status,
        result = excluded.result,
        error = excluded.error,
        metadata = excluded.metadata,
        updated_at = excluded.updated_at,
        completed_at = excluded.completed_at
    `).run({
      id: task.id,
      type: task.type ?? null,
      status: task.status,
      params: task.params ? JSON.stringify(task.params) : null,
      result: task.result ? JSON.stringify(task.result) : null,
      error: task.error ? JSON.stringify(task.error) : null,
      metadata: task.metadata ? JSON.stringify(task.metadata) : null,
      authConfig: task.authConfig ? JSON.stringify(task.authConfig) : null,
      webhooks: task.webhooks ? JSON.stringify(task.webhooks) : null,
      cleanup: task.cleanup ? JSON.stringify(task.cleanup) : null,
      createdAt: task.createdAt,
      updatedAt: task.updatedAt,
      completedAt: task.completedAt ?? null,
      ttl: task.ttl ?? null,
    })
  }

  async getTask(taskId: string): Promise<Task | null> {
    const row = this.db.prepare('SELECT * FROM taskcast_tasks WHERE id = ?').get(taskId) as Record<string, unknown> | undefined
    if (!row) return null
    return rowToTask(row)
  }

  async nextIndex(taskId: string): Promise<number> {
    const result = this.db.prepare(`
      INSERT INTO taskcast_index_counters (task_id, counter) VALUES (?, 0)
      ON CONFLICT (task_id) DO UPDATE SET counter = counter + 1
      RETURNING counter
    `).get(taskId) as { counter: number }
    return result.counter
  }

  async appendEvent(taskId: string, event: TaskEvent): Promise<void> {
    this.db.prepare(`
      INSERT INTO taskcast_events (id, task_id, idx, timestamp, type, level, data, series_id, series_mode)
      VALUES (@id, @taskId, @idx, @timestamp, @type, @level, @data, @seriesId, @seriesMode)
    `).run({
      id: event.id,
      taskId: event.taskId,
      idx: event.index,
      timestamp: event.timestamp,
      type: event.type,
      level: event.level,
      data: event.data != null ? JSON.stringify(event.data) : null,
      seriesId: event.seriesId ?? null,
      seriesMode: event.seriesMode ?? null,
    })
  }

  async getEvents(taskId: string, opts?: EventQueryOptions): Promise<TaskEvent[]> {
    const since = opts?.since
    const limit = opts?.limit

    let rows: Record<string, unknown>[]
    if (since?.index !== undefined) {
      rows = limit
        ? this.db.prepare('SELECT * FROM taskcast_events WHERE task_id = ? AND idx > ? ORDER BY idx ASC LIMIT ?').all(taskId, since.index, limit) as Record<string, unknown>[]
        : this.db.prepare('SELECT * FROM taskcast_events WHERE task_id = ? AND idx > ? ORDER BY idx ASC').all(taskId, since.index) as Record<string, unknown>[]
    } else if (since?.timestamp !== undefined) {
      rows = limit
        ? this.db.prepare('SELECT * FROM taskcast_events WHERE task_id = ? AND timestamp > ? ORDER BY idx ASC LIMIT ?').all(taskId, since.timestamp, limit) as Record<string, unknown>[]
        : this.db.prepare('SELECT * FROM taskcast_events WHERE task_id = ? AND timestamp > ? ORDER BY idx ASC').all(taskId, since.timestamp) as Record<string, unknown>[]
    } else if (since?.id) {
      const anchor = this.db.prepare('SELECT idx FROM taskcast_events WHERE id = ?').get(since.id) as { idx: number } | undefined
      const anchorIdx = anchor?.idx ?? -1
      rows = limit
        ? this.db.prepare('SELECT * FROM taskcast_events WHERE task_id = ? AND idx > ? ORDER BY idx ASC LIMIT ?').all(taskId, anchorIdx, limit) as Record<string, unknown>[]
        : this.db.prepare('SELECT * FROM taskcast_events WHERE task_id = ? AND idx > ? ORDER BY idx ASC').all(taskId, anchorIdx) as Record<string, unknown>[]
    } else {
      rows = limit
        ? this.db.prepare('SELECT * FROM taskcast_events WHERE task_id = ? ORDER BY idx ASC LIMIT ?').all(taskId, limit) as Record<string, unknown>[]
        : this.db.prepare('SELECT * FROM taskcast_events WHERE task_id = ? ORDER BY idx ASC').all(taskId) as Record<string, unknown>[]
    }

    return rows.map(rowToEvent)
  }

  async setTTL(_taskId: string, _ttlSeconds: number): Promise<void> {
    // No-op: SQLite doesn't have key-level TTL.
    // Cleanup is handled by the engine's cleanup rules.
  }

  async getSeriesLatest(taskId: string, seriesId: string): Promise<TaskEvent | null> {
    const row = this.db.prepare(
      'SELECT event_json FROM taskcast_series_latest WHERE task_id = ? AND series_id = ?',
    ).get(taskId, seriesId) as { event_json: string } | undefined
    if (!row) return null
    return JSON.parse(row.event_json) as TaskEvent
  }

  async setSeriesLatest(taskId: string, seriesId: string, event: TaskEvent): Promise<void> {
    this.db.prepare(`
      INSERT INTO taskcast_series_latest (task_id, series_id, event_json) VALUES (?, ?, ?)
      ON CONFLICT (task_id, series_id) DO UPDATE SET event_json = excluded.event_json
    `).run(taskId, seriesId, JSON.stringify(event))
  }

  async replaceLastSeriesEvent(taskId: string, seriesId: string, event: TaskEvent): Promise<void> {
    const prev = await this.getSeriesLatest(taskId, seriesId)
    if (prev) {
      this.db.prepare('UPDATE taskcast_events SET data = ?, type = ?, level = ? WHERE id = ?').run(
        event.data != null ? JSON.stringify(event.data) : null,
        event.type,
        event.level,
        prev.id,
      )
    } else {
      await this.appendEvent(taskId, event)
    }
    await this.setSeriesLatest(taskId, seriesId, event)
  }
}

function rowToTask(row: Record<string, unknown>): Task {
  const task: Task = {
    id: row['id'] as string,
    status: row['status'] as Task['status'],
    createdAt: row['created_at'] as number,
    updatedAt: row['updated_at'] as number,
  }
  if (row['type'] != null) task.type = row['type'] as string
  if (row['params'] != null) task.params = JSON.parse(row['params'] as string) as Record<string, unknown>
  if (row['result'] != null) task.result = JSON.parse(row['result'] as string) as Record<string, unknown>
  if (row['error'] != null) task.error = JSON.parse(row['error'] as string) as TaskError
  if (row['metadata'] != null) task.metadata = JSON.parse(row['metadata'] as string) as Record<string, unknown>
  if (row['auth_config'] != null) task.authConfig = JSON.parse(row['auth_config'] as string) as TaskAuthConfig
  if (row['webhooks'] != null) task.webhooks = JSON.parse(row['webhooks'] as string) as WebhookConfig[]
  if (row['cleanup'] != null) task.cleanup = JSON.parse(row['cleanup'] as string) as { rules: CleanupRule[] }
  if (row['completed_at'] != null) task.completedAt = row['completed_at'] as number
  if (row['ttl'] != null) task.ttl = row['ttl'] as number
  return task
}

function rowToEvent(row: Record<string, unknown>): TaskEvent {
  const event: TaskEvent = {
    id: row['id'] as string,
    taskId: row['task_id'] as string,
    index: row['idx'] as number,
    timestamp: row['timestamp'] as number,
    type: row['type'] as string,
    level: row['level'] as TaskEvent['level'],
    data: row['data'] != null ? JSON.parse(row['data'] as string) as unknown : null,
  }
  if (row['series_id'] != null) event.seriesId = row['series_id'] as string
  if (row['series_mode'] != null) event.seriesMode = row['series_mode'] as SeriesMode
  return event
}
```

**Step 4: Update `packages/sqlite/src/index.ts`**

```typescript
export { SqliteShortTermStore } from './short-term.js'
```

**Step 5: Run tests to verify they pass**

Run: `cd packages/sqlite && pnpm test`
Expected: All 15 tests PASS

**Step 6: Commit**

```bash
git add packages/sqlite/src/ packages/sqlite/tests/
git commit -m "feat(sqlite): implement SqliteShortTermStore"
```

---

## Task 4: Implement SqliteLongTermStore

**Files:**
- Create: `packages/sqlite/src/long-term.ts`
- Modify: `packages/sqlite/src/index.ts`

**Step 1: Write the failing test** (create `packages/sqlite/tests/long-term.test.ts`)

```typescript
import { describe, it, expect, beforeEach, afterEach } from 'vitest'
import { mkdtempSync, rmSync, readFileSync } from 'node:fs'
import { join } from 'node:path'
import { tmpdir } from 'node:os'
import Database from 'better-sqlite3'
import { SqliteLongTermStore } from '../src/long-term.js'
import type { Task, TaskEvent } from '@taskcast/core'

function makeTask(id = 'task-1'): Task {
  return { id, status: 'pending', params: { prompt: 'hello' }, createdAt: 1000, updatedAt: 1000 }
}

function makeEvent(taskId: string, index: number): TaskEvent {
  return {
    id: `evt-${taskId}-${index}`,
    taskId,
    index,
    timestamp: 1000 + index * 100,
    type: 'llm.delta',
    level: 'info',
    data: { text: `msg-${index}` },
  }
}

describe('SqliteLongTermStore', () => {
  let dir: string
  let db: Database.Database
  let store: SqliteLongTermStore

  beforeEach(() => {
    dir = mkdtempSync(join(tmpdir(), 'taskcast-sqlite-lt-'))
    db = new Database(join(dir, 'test.db'))
    const migration = readFileSync(
      join(import.meta.dirname, '../migrations/001_initial.sql'),
      'utf8',
    )
    db.exec(migration)
    db.pragma('journal_mode = WAL')
    db.pragma('foreign_keys = ON')
    store = new SqliteLongTermStore(db)
  })

  afterEach(() => {
    db.close()
    rmSync(dir, { recursive: true, force: true })
  })

  it('saves and retrieves a task', async () => {
    await store.saveTask(makeTask())
    const task = await store.getTask('task-1')
    expect(task?.id).toBe('task-1')
    expect(task?.status).toBe('pending')
  })

  it('returns null for missing task', async () => {
    expect(await store.getTask('nope')).toBeNull()
  })

  it('upserts task on conflict', async () => {
    await store.saveTask(makeTask())
    await store.saveTask({ ...makeTask(), status: 'running', updatedAt: 2000 })
    const task = await store.getTask('task-1')
    expect(task?.status).toBe('running')
  })

  it('saves and retrieves events', async () => {
    await store.saveTask(makeTask())
    for (let i = 0; i < 3; i++) await store.saveEvent(makeEvent('task-1', i))
    const events = await store.getEvents('task-1')
    expect(events).toHaveLength(3)
    expect(events.map((e) => e.index)).toEqual([0, 1, 2])
  })

  it('ignores duplicate events', async () => {
    await store.saveTask(makeTask())
    const evt = makeEvent('task-1', 0)
    await store.saveEvent(evt)
    await store.saveEvent(evt) // duplicate
    const events = await store.getEvents('task-1')
    expect(events).toHaveLength(1)
  })

  it('filters by since.index', async () => {
    await store.saveTask(makeTask())
    for (let i = 0; i < 5; i++) await store.saveEvent(makeEvent('task-1', i))
    const events = await store.getEvents('task-1', { since: { index: 2 } })
    expect(events.map((e) => e.index)).toEqual([3, 4])
  })

  it('filters by since.timestamp', async () => {
    await store.saveTask(makeTask())
    for (let i = 0; i < 5; i++) await store.saveEvent(makeEvent('task-1', i))
    const events = await store.getEvents('task-1', { since: { timestamp: 1200 } })
    expect(events.every((e) => e.timestamp > 1200)).toBe(true)
  })

  it('filters by since.id', async () => {
    await store.saveTask(makeTask())
    for (let i = 0; i < 5; i++) await store.saveEvent(makeEvent('task-1', i))
    const events = await store.getEvents('task-1', { since: { id: 'evt-task-1-2' } })
    expect(events.map((e) => e.index)).toEqual([3, 4])
  })

  it('returns all when since.id not found', async () => {
    await store.saveTask(makeTask())
    for (let i = 0; i < 3; i++) await store.saveEvent(makeEvent('task-1', i))
    const events = await store.getEvents('task-1', { since: { id: 'nonexistent' } })
    expect(events).toHaveLength(3)
  })

  it('respects limit', async () => {
    await store.saveTask(makeTask())
    for (let i = 0; i < 10; i++) await store.saveEvent(makeEvent('task-1', i))
    const events = await store.getEvents('task-1', { limit: 3 })
    expect(events).toHaveLength(3)
  })

  it('saves task with all optional fields', async () => {
    const full: Task = {
      ...makeTask(),
      type: 'llm',
      result: { output: 'done' },
      error: { code: 'E1', message: 'fail' },
      metadata: { user: 'test' },
      authConfig: { rules: [] },
      webhooks: [{ url: 'https://example.com', secret: 's' }],
      cleanup: { rules: [{ trigger: { afterMs: 1000 }, target: 'all' }] },
      completedAt: 2000,
      ttl: 3600,
    }
    await store.saveTask(full)
    const task = await store.getTask('task-1')
    expect(task?.type).toBe('llm')
    expect(task?.result).toEqual({ output: 'done' })
    expect(task?.error?.message).toBe('fail')
    expect(task?.webhooks).toHaveLength(1)
    expect(task?.ttl).toBe(3600)
  })

  it('saves event with series fields', async () => {
    await store.saveTask(makeTask())
    const evt: TaskEvent = { ...makeEvent('task-1', 0), seriesId: 'tokens', seriesMode: 'accumulate' }
    await store.saveEvent(evt)
    const events = await store.getEvents('task-1')
    expect(events[0]?.seriesId).toBe('tokens')
    expect(events[0]?.seriesMode).toBe('accumulate')
  })
})
```

**Step 2: Run test to verify it fails**

**Step 3: Implement `packages/sqlite/src/long-term.ts`**

The LongTermStore is a subset of ShortTermStore (same tables, fewer methods). Reuse `rowToTask` and `rowToEvent` helpers — extract them to a shared file `packages/sqlite/src/row-mappers.ts`.

```typescript
import type Database from 'better-sqlite3'
import type { Task, TaskEvent, LongTermStore, EventQueryOptions } from '@taskcast/core'
import { rowToTask, rowToEvent } from './row-mappers.js'

export class SqliteLongTermStore implements LongTermStore {
  constructor(private db: Database.Database) {}

  async saveTask(task: Task): Promise<void> {
    this.db.prepare(`
      INSERT INTO taskcast_tasks (
        id, type, status, params, result, error, metadata,
        auth_config, webhooks, cleanup, created_at, updated_at, completed_at, ttl
      ) VALUES (
        @id, @type, @status, @params, @result, @error, @metadata,
        @authConfig, @webhooks, @cleanup, @createdAt, @updatedAt, @completedAt, @ttl
      )
      ON CONFLICT (id) DO UPDATE SET
        status = excluded.status,
        result = excluded.result,
        error = excluded.error,
        metadata = excluded.metadata,
        updated_at = excluded.updated_at,
        completed_at = excluded.completed_at
    `).run({
      id: task.id,
      type: task.type ?? null,
      status: task.status,
      params: task.params ? JSON.stringify(task.params) : null,
      result: task.result ? JSON.stringify(task.result) : null,
      error: task.error ? JSON.stringify(task.error) : null,
      metadata: task.metadata ? JSON.stringify(task.metadata) : null,
      authConfig: task.authConfig ? JSON.stringify(task.authConfig) : null,
      webhooks: task.webhooks ? JSON.stringify(task.webhooks) : null,
      cleanup: task.cleanup ? JSON.stringify(task.cleanup) : null,
      createdAt: task.createdAt,
      updatedAt: task.updatedAt,
      completedAt: task.completedAt ?? null,
      ttl: task.ttl ?? null,
    })
  }

  async getTask(taskId: string): Promise<Task | null> {
    const row = this.db.prepare('SELECT * FROM taskcast_tasks WHERE id = ?').get(taskId) as Record<string, unknown> | undefined
    if (!row) return null
    return rowToTask(row)
  }

  async saveEvent(event: TaskEvent): Promise<void> {
    this.db.prepare(`
      INSERT INTO taskcast_events (id, task_id, idx, timestamp, type, level, data, series_id, series_mode)
      VALUES (@id, @taskId, @idx, @timestamp, @type, @level, @data, @seriesId, @seriesMode)
      ON CONFLICT (id) DO NOTHING
    `).run({
      id: event.id,
      taskId: event.taskId,
      idx: event.index,
      timestamp: event.timestamp,
      type: event.type,
      level: event.level,
      data: event.data != null ? JSON.stringify(event.data) : null,
      seriesId: event.seriesId ?? null,
      seriesMode: event.seriesMode ?? null,
    })
  }

  async getEvents(taskId: string, opts?: EventQueryOptions): Promise<TaskEvent[]> {
    // Same implementation as ShortTermStore.getEvents — uses SQL filtering
    const since = opts?.since
    const limit = opts?.limit

    let rows: Record<string, unknown>[]
    if (since?.index !== undefined) {
      rows = limit
        ? this.db.prepare('SELECT * FROM taskcast_events WHERE task_id = ? AND idx > ? ORDER BY idx ASC LIMIT ?').all(taskId, since.index, limit) as Record<string, unknown>[]
        : this.db.prepare('SELECT * FROM taskcast_events WHERE task_id = ? AND idx > ? ORDER BY idx ASC').all(taskId, since.index) as Record<string, unknown>[]
    } else if (since?.timestamp !== undefined) {
      rows = limit
        ? this.db.prepare('SELECT * FROM taskcast_events WHERE task_id = ? AND timestamp > ? ORDER BY idx ASC LIMIT ?').all(taskId, since.timestamp, limit) as Record<string, unknown>[]
        : this.db.prepare('SELECT * FROM taskcast_events WHERE task_id = ? AND timestamp > ? ORDER BY idx ASC').all(taskId, since.timestamp) as Record<string, unknown>[]
    } else if (since?.id) {
      const anchor = this.db.prepare('SELECT idx FROM taskcast_events WHERE id = ?').get(since.id) as { idx: number } | undefined
      const anchorIdx = anchor?.idx ?? -1
      rows = limit
        ? this.db.prepare('SELECT * FROM taskcast_events WHERE task_id = ? AND idx > ? ORDER BY idx ASC LIMIT ?').all(taskId, anchorIdx, limit) as Record<string, unknown>[]
        : this.db.prepare('SELECT * FROM taskcast_events WHERE task_id = ? AND idx > ? ORDER BY idx ASC').all(taskId, anchorIdx) as Record<string, unknown>[]
    } else {
      rows = limit
        ? this.db.prepare('SELECT * FROM taskcast_events WHERE task_id = ? ORDER BY idx ASC LIMIT ?').all(taskId, limit) as Record<string, unknown>[]
        : this.db.prepare('SELECT * FROM taskcast_events WHERE task_id = ? ORDER BY idx ASC').all(taskId) as Record<string, unknown>[]
    }

    return rows.map(rowToEvent)
  }
}
```

Note: Also extract `rowToTask` and `rowToEvent` from `short-term.ts` into a new `row-mappers.ts` file and import from both.

**Step 4: Update `packages/sqlite/src/index.ts`**

```typescript
export { SqliteShortTermStore } from './short-term.js'
export { SqliteLongTermStore } from './long-term.js'

import Database from 'better-sqlite3'
import { readFileSync } from 'node:fs'
import { join, dirname } from 'node:path'
import { fileURLToPath } from 'node:url'
import { SqliteShortTermStore } from './short-term.js'
import { SqliteLongTermStore } from './long-term.js'

export interface SqliteAdapterOptions {
  path?: string
}

export function createSqliteAdapters(options: SqliteAdapterOptions = {}) {
  const dbPath = options.path ?? process.env['TASKCAST_SQLITE_PATH'] ?? './taskcast.db'
  const db = new Database(dbPath)
  db.pragma('journal_mode = WAL')
  db.pragma('foreign_keys = ON')

  const __dirname = dirname(fileURLToPath(import.meta.url))
  const migration = readFileSync(join(__dirname, '../migrations/001_initial.sql'), 'utf8')
  db.exec(migration)

  return {
    shortTerm: new SqliteShortTermStore(db),
    longTerm: new SqliteLongTermStore(db),
    db,
  }
}
```

**Step 5: Run all tests**

Run: `cd packages/sqlite && pnpm test`
Expected: All tests PASS (both short-term and long-term suites)

**Step 6: Commit**

```bash
git add packages/sqlite/src/ packages/sqlite/tests/
git commit -m "feat(sqlite): implement SqliteLongTermStore and createSqliteAdapters"
```

---

## Task 5: Wire SQLite into CLI

**Files:**
- Modify: `packages/cli/src/index.ts`
- Modify: `packages/cli/package.json` (add `@taskcast/sqlite` dependency)

**Step 1: Add dependency**

In `packages/cli/package.json`, add to dependencies:
```json
"@taskcast/sqlite": "workspace:*"
```

Run `pnpm install`.

**Step 2: Modify `packages/cli/src/index.ts`**

Add `--storage` option to the `start` command. When `sqlite`, use `createSqliteAdapters`:

```typescript
// Add import at top
import { createSqliteAdapters } from '@taskcast/sqlite'

// Modify start command options — add:
.option('-s, --storage <type>', 'storage backend: memory | redis | sqlite', 'memory')
.option('--db-path <path>', 'SQLite database file path (default: ./taskcast.db)')

// In action handler, replace adapter selection logic:
const storage = options.storage ?? process.env['TASKCAST_STORAGE'] ?? (redisUrl ? 'redis' : 'memory')

if (storage === 'sqlite') {
  const adapters = createSqliteAdapters({ path: options.dbPath })
  broadcast = new MemoryBroadcastProvider()
  shortTerm = adapters.shortTerm
  longTerm = adapters.longTerm
} else if (storage === 'redis' || redisUrl) {
  // existing Redis logic
} else {
  // existing memory fallback
}
```

**Step 3: Build and verify**

Run: `pnpm build`
Expected: Build succeeds

**Step 4: Manual smoke test**

```bash
timeout 3 node packages/cli/dist/index.js start --storage sqlite --db-path /tmp/test-taskcast.db
```

Expected: Server starts, `taskcast.db` file created.

**Step 5: Commit**

```bash
git add packages/cli/
git commit -m "feat(cli): add --storage sqlite option"
```

---

## Task 6: Scaffold Rust crate

**Files:**
- Create: `rust/taskcast-sqlite/Cargo.toml`
- Create: `rust/taskcast-sqlite/src/lib.rs`
- Create: `rust/taskcast-sqlite/migrations/001_initial.sql` (copy from TS)
- Modify: `rust/Cargo.toml` (add to workspace members)

**Step 1: Create `rust/taskcast-sqlite/Cargo.toml`**

```toml
[package]
name = "taskcast-sqlite"
version = "0.1.0"
edition = "2021"

[dependencies]
taskcast-core = { path = "../taskcast-core" }
sqlx = { version = "0.8", features = ["runtime-tokio", "sqlite"] }
serde = { workspace = true }
serde_json = { workspace = true }
async-trait = { workspace = true }
tokio = { workspace = true }

[dev-dependencies]
tempfile = { workspace = true }
```

**Step 2: Create `rust/taskcast-sqlite/src/lib.rs`** (placeholder)

```rust
mod short_term;
mod long_term;

pub use short_term::SqliteShortTermStore;
pub use long_term::SqliteLongTermStore;
```

**Step 3: Copy migration to `rust/taskcast-sqlite/migrations/001_initial.sql`**

Same SQL as the TS version.

**Step 4: Add to workspace `rust/Cargo.toml`**

Add `"taskcast-sqlite"` to the `members` array.

**Step 5: Create placeholder source files so it compiles**

Create `rust/taskcast-sqlite/src/short_term.rs` and `long_term.rs` with empty structs.

**Step 6: Run `cargo check -p taskcast-sqlite`**

Expected: Compiles successfully.

**Step 7: Commit**

```bash
git add rust/taskcast-sqlite/ rust/Cargo.toml
git commit -m "chore: scaffold taskcast-sqlite Rust crate"
```

---

## Task 7: Implement Rust SqliteShortTermStore

**Files:**
- Create: `rust/taskcast-sqlite/src/short_term.rs`

Follow the same pattern as `rust/taskcast-postgres/src/store.rs` but adapted for SQLite. Use `sqlx::SqlitePool`. Implement `ShortTermStore` trait. Methods mirror the TypeScript implementation exactly.

Key differences from Postgres:
- Use `sqlx::SqlitePool` instead of `PgPool`
- No JSONB — store JSON as TEXT, parse with `serde_json`
- `RETURNING` clause works in SQLite 3.35+
- Use `INSERT ... ON CONFLICT` same as TS

**Step 1: Write tests, Step 2: Verify fail, Step 3: Implement, Step 4: Verify pass, Step 5: Commit**

```bash
git commit -m "feat(sqlite): implement Rust SqliteShortTermStore"
```

---

## Task 8: Implement Rust SqliteLongTermStore

**Files:**
- Create: `rust/taskcast-sqlite/src/long_term.rs`

Same pattern as Task 7 but for `LongTermStore` trait. Reuse row conversion helpers.

**Step 1-5: TDD cycle + commit**

```bash
git commit -m "feat(sqlite): implement Rust SqliteLongTermStore"
```

---

## Task 9: Wire SQLite into Rust CLI

**Files:**
- Modify: `rust/taskcast-cli/Cargo.toml` (add `taskcast-sqlite` dependency)
- Modify: `rust/taskcast-cli/src/main.rs` (add `--storage` flag and SQLite wiring)

Mirror the same CLI changes as Task 5: add `--storage` and `--db-path` flags to the `Start` command.

**Step 1-5: Implement + verify + commit**

```bash
git commit -m "feat(cli): add --storage sqlite option to Rust CLI"
```

---

## Task 10: Update documentation and add changeset

**Files:**
- Modify: `packages/cli/README.md` (add SQLite docs)
- Modify: `CLAUDE.md` (add `@taskcast/sqlite` to package map)
- Create: `.changeset/<name>.md`

**Step 1: Update CLI README** — add `--storage` and `--db-path` options, SQLite section

**Step 2: Update CLAUDE.md package map** — add `@taskcast/sqlite` entry

**Step 3: Create changeset**

```bash
pnpm changeset
```

Select all packages, minor bump. Message: "Add SQLite local storage adapter for zero-dependency development"

**Step 4: Commit**

```bash
git add .
git commit -m "docs: add SQLite adapter documentation and changeset"
```

---

## Task 11: Final verification

**Step 1: Run full TS test suite**

```bash
pnpm test
```

Expected: All tests pass including new SQLite tests.

**Step 2: Run Rust check + tests**

```bash
cd rust && cargo check && cargo test
```

Expected: All pass.

**Step 3: Build everything**

```bash
pnpm build
```

Expected: Clean build.
