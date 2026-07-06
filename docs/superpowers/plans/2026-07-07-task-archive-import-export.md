# Task Archive Import/Export Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add Taskcast-native single-task archive export/import plus public server version reporting.

**Architecture:** The archive is a core protocol object, not a Dashboard-specific backup format. Core owns validation, conflict behavior, silent restore, index restoration, and series-state rebuilding; HTTP servers and SDK expose that core capability. TypeScript and Rust implementations must keep the same API paths, JSON shapes, and status codes.

**Tech Stack:** TypeScript ESM, Vitest, Hono, Zod/OpenAPI, Rust, Tokio, Axum, Utoipa, Redis, SQLite, Postgres.

---

## Scope

This plan implements the Taskcast-native part of the approved spec:

- `TaskArchive` type and validation
- core engine `exportTaskArchive` / `importTaskArchive`
- restore support in memory, Redis, SQLite, and Postgres stores
- TypeScript HTTP routes, SDK methods, and version handshake
- Rust core/server parity for the same behavior

This plan intentionally does not implement the downstream agent-pi / Claw Hive Dashboard session bundle. That is a separate plan after Taskcast exposes this stable primitive.

Before executing this plan, create or use an isolated worktree. The current `/Users/winrey/Projects/taskcast` checkout has unrelated modified Rust files, so implementation work should not be mixed into that dirty tree.

## File Map

### TypeScript Core

- Modify `packages/core/src/types.ts`
  - Add `TaskArchive`, `TaskArchiveImportOptions`, `TaskArchiveImportResult`, `TaskArchiveRestoreData`.
  - Add `restoreTaskArchive` to `ShortTermStore` and `LongTermStore`.
- Create `packages/core/src/archive.ts`
  - Validate archives.
  - Normalize event order.
  - Build derived state: next event index and latest series state.
  - Define archive-specific error classes.
- Modify `packages/core/src/index.ts`
  - Export archive helpers.
- Modify `packages/core/src/engine.ts`
  - Add `exportTaskArchive(taskId)`.
  - Add `importTaskArchive(archive, options)`.
- Modify `packages/core/src/memory-adapters.ts`
  - Implement archive restore for `MemoryShortTermStore`.
- Create `packages/core/tests/unit/archive.test.ts`
  - Pure archive validation and derived-state tests.
- Create `packages/core/tests/unit/engine-archive.test.ts`
  - Engine round-trip and silent-import tests.

### TypeScript Adapters

- Modify `packages/redis/src/short-term.ts`
  - Implement `restoreTaskArchive`.
- Modify `packages/redis/tests/short-term.test.ts`
  - Add restore behavior tests.
- Modify `packages/sqlite/src/short-term.ts`
  - Implement short-term restore in a transaction.
- Modify `packages/sqlite/src/long-term.ts`
  - Implement long-term restore in a transaction.
- Modify `packages/sqlite/tests/short-term.test.ts`
  - Add short-term restore tests.
- Modify `packages/sqlite/tests/long-term.test.ts`
  - Add long-term restore tests.
- Modify `packages/postgres/src/long-term.ts`
  - Implement long-term restore in a SQL transaction.
- Modify `packages/postgres/tests/long-term.test.ts`
  - Add long-term restore tests.

### TypeScript Server and SDK

- Modify `packages/server/src/schemas.ts`
  - Add `TaskArchiveSchema`, `ImportTaskArchiveSchema`, `ImportTaskArchiveResultSchema`, `ServerInfoSchema`.
- Modify `packages/server/src/routes/tasks.ts`
  - Add `GET /tasks/{taskId}/archive`.
  - Add `POST /tasks/import`.
- Modify `packages/server/src/index.ts`
  - Add public `GET /`.
  - Add version fields to `/health` and `/health/detail`.
  - Source OpenAPI `info.version` from the same version helper.
- Create `packages/server/src/version.ts`
  - Centralize TypeScript package version lookup.
- Create `packages/server/tests/archive-routes.test.ts`
  - Route success and error behavior.
- Modify `packages/server/tests/health-detail.test.ts`
  - Version fields on health endpoints.
- Modify `packages/server/tests/openapi.test.ts`
  - OpenAPI version uses server package version.
- Modify `packages/server-sdk/src/client.ts`
  - Add `getServerInfo`, `exportTaskArchive`, and `importTaskArchive`.
- Modify `packages/server-sdk/src/index.ts`
  - Export new SDK types.
- Modify `packages/server-sdk/tests/client.test.ts`
  - Unit tests for new SDK requests.

### Rust Core and Server

- Modify `rust/taskcast-core/src/types.rs`
  - Add `TaskArchive`, `TaskArchiveImportOptions`, `TaskArchiveImportResult`, `TaskArchiveRestoreData`, `SeriesLatestEntry`.
  - Add `restore_task_archive` to `ShortTermStore` and `LongTermStore`.
- Create `rust/taskcast-core/src/archive.rs`
  - Rust parity for archive validation and derived-state building.
- Modify `rust/taskcast-core/src/lib.rs`
  - Export `archive`.
- Modify `rust/taskcast-core/src/engine.rs`
  - Add `export_task_archive` and `import_task_archive`.
- Modify `rust/taskcast-core/src/memory_adapters.rs`
  - Implement restore for memory store.
- Create `rust/taskcast-core/tests/archive.rs`
  - Core validation and engine behavior tests.
- Modify `rust/taskcast-server/src/app.rs`
  - Add public `GET /`, version fields on health responses.
  - Mount archive routes.
- Modify `rust/taskcast-server/src/routes/tasks.rs`
  - Add archive handlers.
- Modify `rust/taskcast-server/src/error.rs`
  - Map archive validation and conflicts to the same status codes as TypeScript.
- Create `rust/taskcast-server/tests/archive_routes.rs`
  - HTTP parity tests.
- Modify `rust/taskcast-server/tests/health_detail.rs`
  - Version fields.

---

### Task 1: TypeScript Archive Types and Validation

**Files:**
- Modify: `packages/core/src/types.ts`
- Create: `packages/core/src/archive.ts`
- Modify: `packages/core/src/index.ts`
- Test: `packages/core/tests/unit/archive.test.ts`

- [ ] **Step 1: Write failing validation tests**

Create `packages/core/tests/unit/archive.test.ts`:

```ts
import { describe, expect, it } from 'vitest'
import type { Task, TaskArchive, TaskEvent } from '../../src/types.js'
import { InvalidTaskArchiveError, buildTaskArchiveRestoreData, normalizeTaskArchive } from '../../src/archive.js'

function makeTask(id = 'task-1'): Task {
  return {
    id,
    status: 'running',
    createdAt: 1000,
    updatedAt: 2000,
    type: 'demo',
  }
}

function makeEvent(id: string, taskId: string, index: number, data: unknown = null): TaskEvent {
  return {
    id,
    taskId,
    index,
    timestamp: 3000 + index,
    type: 'demo.event',
    level: 'info',
    data,
  }
}

function makeArchive(events: TaskEvent[]): TaskArchive {
  return {
    schema: 'taskcast.taskArchive',
    version: 1,
    exportedAt: 5000,
    task: makeTask('task-1'),
    events,
  }
}

describe('normalizeTaskArchive', () => {
  it('sorts events by index without changing event identity fields', () => {
    const archive = makeArchive([
      makeEvent('event-2', 'task-1', 1),
      makeEvent('event-1', 'task-1', 0),
    ])

    const normalized = normalizeTaskArchive(archive)

    expect(normalized.events.map((event) => event.id)).toEqual(['event-1', 'event-2'])
    expect(normalized.events[0]).toMatchObject({
      id: 'event-1',
      taskId: 'task-1',
      index: 0,
      timestamp: 3000,
    })
  })

  it('rejects unsupported archive version', () => {
    const archive = { ...makeArchive([]), version: 2 as 1 }
    expect(() => normalizeTaskArchive(archive)).toThrow(InvalidTaskArchiveError)
  })

  it('rejects event taskId mismatch', () => {
    const archive = makeArchive([makeEvent('event-1', 'other-task', 0)])
    expect(() => normalizeTaskArchive(archive)).toThrow(/taskId/)
  })

  it('rejects duplicate event ids', () => {
    const archive = makeArchive([
      makeEvent('event-1', 'task-1', 0),
      makeEvent('event-1', 'task-1', 1),
    ])
    expect(() => normalizeTaskArchive(archive)).toThrow(/duplicate event id/)
  })

  it('rejects duplicate event indexes', () => {
    const archive = makeArchive([
      makeEvent('event-1', 'task-1', 0),
      makeEvent('event-2', 'task-1', 0),
    ])
    expect(() => normalizeTaskArchive(archive)).toThrow(/duplicate event index/)
  })

  it('rejects non-contiguous indexes', () => {
    const archive = makeArchive([
      makeEvent('event-1', 'task-1', 0),
      makeEvent('event-2', 'task-1', 2),
    ])
    expect(() => normalizeTaskArchive(archive)).toThrow(/contiguous/)
  })
})

describe('buildTaskArchiveRestoreData', () => {
  it('sets nextIndex to max index plus one', () => {
    const restore = buildTaskArchiveRestoreData(makeArchive([
      makeEvent('event-1', 'task-1', 0),
      makeEvent('event-2', 'task-1', 1),
    ]))

    expect(restore.nextIndex).toBe(2)
  })

  it('rebuilds latest series state', () => {
    const latestEvent = {
      ...makeEvent('event-2', 'task-1', 1, { value: 'new' }),
      seriesId: 'series-1',
      seriesMode: 'latest' as const,
    }
    const restore = buildTaskArchiveRestoreData(makeArchive([
      {
        ...makeEvent('event-1', 'task-1', 0, { value: 'old' }),
        seriesId: 'series-1',
        seriesMode: 'latest' as const,
      },
      latestEvent,
    ]))

    expect(restore.seriesLatest).toEqual([
      { taskId: 'task-1', seriesId: 'series-1', event: latestEvent },
    ])
  })

  it('rebuilds accumulate series state by concatenating the configured field', () => {
    const restore = buildTaskArchiveRestoreData(makeArchive([
      {
        ...makeEvent('event-1', 'task-1', 0, { delta: 'hello ' }),
        seriesId: 'series-1',
        seriesMode: 'accumulate' as const,
        seriesAccField: 'delta',
      },
      {
        ...makeEvent('event-2', 'task-1', 1, { delta: 'world' }),
        seriesId: 'series-1',
        seriesMode: 'accumulate' as const,
        seriesAccField: 'delta',
      },
    ]))

    expect(restore.seriesLatest[0]?.event.data).toEqual({ delta: 'hello world' })
  })
})
```

- [ ] **Step 2: Run tests and verify failure**

Run:

```bash
cd packages/core
pnpm test tests/unit/archive.test.ts
```

Expected: FAIL because `TaskArchive` and `archive.js` do not exist.

- [ ] **Step 3: Add core archive types**

In `packages/core/src/types.ts`, add after `TaskEvent`:

```ts
export interface TaskArchive {
  schema: 'taskcast.taskArchive'
  version: 1
  exportedAt: number
  task: Task
  events: TaskEvent[]
}

export interface TaskArchiveImportOptions {
  overwrite?: boolean
}

export interface TaskArchiveImportResult {
  taskId: string
  eventCount: number
  overwritten: boolean
}

export interface SeriesLatestEntry {
  taskId: string
  seriesId: string
  event: TaskEvent
}

export interface TaskArchiveRestoreData {
  task: Task
  events: TaskEvent[]
  nextIndex: number
  seriesLatest: SeriesLatestEntry[]
}
```

Add these methods to `ShortTermStore`:

```ts
  restoreTaskArchive(
    data: TaskArchiveRestoreData,
    options?: TaskArchiveImportOptions,
  ): Promise<{ overwritten: boolean }>
```

Add this method to `LongTermStore`:

```ts
  restoreTaskArchive(
    data: TaskArchiveRestoreData,
    options?: TaskArchiveImportOptions,
  ): Promise<{ overwritten: boolean }>
```

- [ ] **Step 4: Add archive validation helper**

Create `packages/core/src/archive.ts`:

```ts
import type { SeriesLatestEntry, TaskArchive, TaskArchiveRestoreData, TaskEvent } from './types.js'

export const TASK_ARCHIVE_SCHEMA = 'taskcast.taskArchive' as const
export const TASK_ARCHIVE_VERSION = 1 as const

export class InvalidTaskArchiveError extends Error {
  constructor(message: string) {
    super(message)
    this.name = 'InvalidTaskArchiveError'
  }
}

export function normalizeTaskArchive(archive: TaskArchive): TaskArchive {
  if (archive.schema !== TASK_ARCHIVE_SCHEMA) {
    throw new InvalidTaskArchiveError(`Unsupported archive schema: ${String(archive.schema)}`)
  }
  if (archive.version !== TASK_ARCHIVE_VERSION) {
    throw new InvalidTaskArchiveError(`Unsupported archive version: ${String(archive.version)}`)
  }
  if (!archive.task?.id) {
    throw new InvalidTaskArchiveError('Archive task.id is required')
  }

  const sorted = [...archive.events].sort((a, b) => a.index - b.index)
  const seenIds = new Set<string>()
  const seenIndexes = new Set<number>()

  for (let expectedIndex = 0; expectedIndex < sorted.length; expectedIndex++) {
    const event = sorted[expectedIndex]!
    if (event.taskId !== archive.task.id) {
      throw new InvalidTaskArchiveError(`Archive event taskId mismatch for event ${event.id}`)
    }
    if (seenIds.has(event.id)) {
      throw new InvalidTaskArchiveError(`Archive contains duplicate event id: ${event.id}`)
    }
    seenIds.add(event.id)
    if (seenIndexes.has(event.index)) {
      throw new InvalidTaskArchiveError(`Archive contains duplicate event index: ${event.index}`)
    }
    seenIndexes.add(event.index)
    if (event.index !== expectedIndex) {
      throw new InvalidTaskArchiveError(`Archive event indexes must be contiguous from 0; expected ${expectedIndex}, got ${event.index}`)
    }
  }

  return { ...archive, events: sorted.map((event) => ({ ...event })) }
}

export function buildTaskArchiveRestoreData(archive: TaskArchive): TaskArchiveRestoreData {
  const normalized = normalizeTaskArchive(archive)
  return {
    task: { ...normalized.task },
    events: normalized.events.map((event) => ({ ...event })),
    nextIndex: normalized.events.length,
    seriesLatest: buildSeriesLatest(normalized.events),
  }
}

function buildSeriesLatest(events: TaskEvent[]): SeriesLatestEntry[] {
  const latest = new Map<string, TaskEvent>()

  for (const event of events) {
    if (!event.seriesId || !event.seriesMode) continue
    if (event.seriesMode === 'keep-all') continue

    const key = `${event.taskId}:${event.seriesId}`
    if (event.seriesMode === 'latest') {
      latest.set(key, { ...event })
      continue
    }

    const field = event.seriesAccField ?? 'delta'
    const previous = latest.get(key)
    if (!previous) {
      latest.set(key, { ...event })
      continue
    }

    const prevData = isRecord(previous.data) ? previous.data : {}
    const newData = isRecord(event.data) ? event.data : {}
    if (typeof prevData[field] === 'string' && typeof newData[field] === 'string') {
      latest.set(key, {
        ...event,
        data: { ...newData, [field]: prevData[field] + newData[field] },
      })
    } else {
      latest.set(key, { ...event })
    }
  }

  return Array.from(latest.values()).map((event) => ({
    taskId: event.taskId,
    seriesId: event.seriesId!,
    event,
  }))
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value)
}
```

Modify `packages/core/src/index.ts`:

```ts
export * from './archive.js'
```

- [ ] **Step 5: Run validation tests**

Run:

```bash
cd packages/core
pnpm test tests/unit/archive.test.ts
```

Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add packages/core/src/types.ts packages/core/src/archive.ts packages/core/src/index.ts packages/core/tests/unit/archive.test.ts
git commit -m "feat(core): add task archive validation"
```

---

### Task 2: TypeScript Memory Store Restore and Engine Archive API

**Files:**
- Modify: `packages/core/src/memory-adapters.ts`
- Modify: `packages/core/src/engine.ts`
- Test: `packages/core/tests/unit/engine-archive.test.ts`

- [ ] **Step 1: Write failing engine tests**

Create `packages/core/tests/unit/engine-archive.test.ts`:

```ts
import { describe, expect, it, vi } from 'vitest'
import {
  MemoryBroadcastProvider,
  MemoryShortTermStore,
  TaskConflictError,
  TaskEngine,
} from '../../src/index.js'
import type { TaskArchive } from '../../src/types.js'

function makeEngine(broadcast = new MemoryBroadcastProvider()) {
  return new TaskEngine({
    broadcast,
    shortTermStore: new MemoryShortTermStore(),
  })
}

describe('TaskEngine archive import/export', () => {
  it('exports task and events preserving original fields', async () => {
    const engine = makeEngine()
    const task = await engine.createTask({ id: 'task-1', type: 'demo' })
    const event = await engine.publishEvent(task.id, {
      type: 'demo.event',
      level: 'info',
      data: { value: 1 },
    })

    const archive = await engine.exportTaskArchive(task.id)

    expect(archive).toMatchObject({
      schema: 'taskcast.taskArchive',
      version: 1,
      task: { id: 'task-1', createdAt: task.createdAt, updatedAt: task.updatedAt },
      events: [{ id: event.id, index: 0, timestamp: event.timestamp }],
    })
  })

  it('imports archive silently and allows publish to continue at the next index', async () => {
    const source = makeEngine()
    await source.createTask({ id: 'task-1' })
    await source.publishEvent('task-1', { type: 'demo.one', level: 'info', data: null })
    const archive = await source.exportTaskArchive('task-1')

    const broadcast = new MemoryBroadcastProvider()
    const handler = vi.fn()
    broadcast.subscribe('task-1', handler)
    const target = makeEngine(broadcast)

    const result = await target.importTaskArchive(archive)
    const next = await target.publishEvent('task-1', { type: 'demo.two', level: 'info', data: null })

    expect(result).toEqual({ taskId: 'task-1', eventCount: 1, overwritten: false })
    expect(handler).toHaveBeenCalledTimes(1)
    expect(handler.mock.calls[0]![0].type).toBe('demo.two')
    expect(next.index).toBe(1)
  })

  it('rejects an existing task unless overwrite is true', async () => {
    const engine = makeEngine()
    await engine.createTask({ id: 'task-1' })

    const archive: TaskArchive = {
      schema: 'taskcast.taskArchive',
      version: 1,
      exportedAt: 5000,
      task: { id: 'task-1', status: 'running', createdAt: 1000, updatedAt: 2000 },
      events: [],
    }

    await expect(engine.importTaskArchive(archive)).rejects.toThrow(TaskConflictError)
  })

  it('overwrite replaces the full old history', async () => {
    const engine = makeEngine()
    await engine.createTask({ id: 'task-1' })
    await engine.publishEvent('task-1', { type: 'old.event', level: 'info', data: null })

    const archive: TaskArchive = {
      schema: 'taskcast.taskArchive',
      version: 1,
      exportedAt: 5000,
      task: { id: 'task-1', status: 'running', createdAt: 1000, updatedAt: 2000 },
      events: [
        {
          id: 'imported-event',
          taskId: 'task-1',
          index: 0,
          timestamp: 3000,
          type: 'new.event',
          level: 'info',
          data: null,
        },
      ],
    }

    const result = await engine.importTaskArchive(archive, { overwrite: true })
    const events = await engine.getEvents('task-1')

    expect(result.overwritten).toBe(true)
    expect(events.map((event) => event.type)).toEqual(['new.event'])
  })
})
```

- [ ] **Step 2: Run tests and verify failure**

Run:

```bash
cd packages/core
pnpm test tests/unit/engine-archive.test.ts
```

Expected: FAIL because store restore and engine methods are missing.

- [ ] **Step 3: Implement memory store restore**

In `packages/core/src/memory-adapters.ts`, import the archive types:

```ts
  TaskArchiveImportOptions,
  TaskArchiveRestoreData,
```

Add this method inside `MemoryShortTermStore`:

```ts
  async restoreTaskArchive(
    data: TaskArchiveRestoreData,
    options?: TaskArchiveImportOptions,
  ): Promise<{ overwritten: boolean }> {
    const exists = this.tasks.has(data.task.id)
    if (exists && options?.overwrite !== true) {
      throw new Error(`Task already exists: ${data.task.id}`)
    }

    this.tasks.set(data.task.id, { ...data.task })
    this.events.set(data.task.id, data.events.map((event) => ({ ...event })))
    this.indexCounters.set(data.task.id, data.nextIndex - 1)

    for (const key of Array.from(this.seriesLatest.keys())) {
      if (key.startsWith(`${data.task.id}:`)) this.seriesLatest.delete(key)
    }
    for (const entry of data.seriesLatest) {
      this.seriesLatest.set(`${entry.taskId}:${entry.seriesId}`, { ...entry.event })
    }

    return { overwritten: exists }
  }
```

- [ ] **Step 4: Implement engine methods**

In `packages/core/src/engine.ts`, update imports:

```ts
import {
  buildTaskArchiveRestoreData,
  normalizeTaskArchive,
} from './archive.js'
```

Add imported types:

```ts
  TaskArchive,
  TaskArchiveImportOptions,
  TaskArchiveImportResult,
```

Add these public methods before `publishEvent(...)`:

```ts
  async exportTaskArchive(taskId: string): Promise<TaskArchive> {
    const task = await this.getTask(taskId)
    if (!task) throw new Error(`Task not found: ${taskId}`)
    const events = await this.getEvents(taskId)
    return {
      schema: 'taskcast.taskArchive',
      version: 1,
      exportedAt: Date.now(),
      task: { ...task },
      events: events.map((event) => ({ ...event })).sort((a, b) => a.index - b.index),
    }
  }

  async importTaskArchive(
    archive: TaskArchive,
    options?: TaskArchiveImportOptions,
  ): Promise<TaskArchiveImportResult> {
    const normalized = normalizeTaskArchive(archive)
    const existing = await this.shortTermStore.getTask(normalized.task.id)
    if (existing && options?.overwrite !== true) {
      throw new TaskConflictError(normalized.task.id)
    }

    const restoreData = buildTaskArchiveRestoreData(normalized)
    const shortResult = await this.shortTermStore.restoreTaskArchive(restoreData, options)
    if (this.longTermStore) {
      await this.longTermStore.restoreTaskArchive(restoreData, options)
    }
    this._emitChains.delete(normalized.task.id)

    return {
      taskId: normalized.task.id,
      eventCount: normalized.events.length,
      overwritten: shortResult.overwritten,
    }
  }
```

- [ ] **Step 5: Run engine tests**

Run:

```bash
cd packages/core
pnpm test tests/unit/archive.test.ts tests/unit/engine-archive.test.ts
```

Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add packages/core/src/memory-adapters.ts packages/core/src/engine.ts packages/core/tests/unit/engine-archive.test.ts
git commit -m "feat(core): restore task archives in memory"
```

---

### Task 3: TypeScript Persistent Store Restore

**Files:**
- Modify: `packages/redis/src/short-term.ts`
- Modify: `packages/sqlite/src/short-term.ts`
- Modify: `packages/sqlite/src/long-term.ts`
- Modify: `packages/postgres/src/long-term.ts`
- Test: `packages/redis/tests/short-term.test.ts`
- Test: `packages/sqlite/tests/short-term.test.ts`
- Test: `packages/sqlite/tests/long-term.test.ts`
- Test: `packages/postgres/tests/long-term.test.ts`

- [ ] **Step 1: Write adapter restore tests**

Add these Redis tests to `packages/redis/tests/short-term.test.ts`:

```ts
it('restores a task archive and continues indexes after imported events', async () => {
  await store.restoreTaskArchive({
    task: makeTask('task-archive', { status: 'running', createdAt: 1000, updatedAt: 2000 }),
    events: [
      {
        id: 'event-1',
        taskId: 'task-archive',
        index: 0,
        timestamp: 3000,
        type: 'demo.event',
        level: 'info',
        data: null,
      },
    ],
    nextIndex: 1,
    seriesLatest: [],
  })

  expect(await store.getTask('task-archive')).toMatchObject({ id: 'task-archive', status: 'running' })
  expect(await store.getEvents('task-archive')).toHaveLength(1)
  expect(await store.nextIndex('task-archive')).toBe(1)
})

it('rejects restore conflicts unless overwrite is true', async () => {
  const data = {
    task: makeTask('task-archive', { status: 'running', createdAt: 1000, updatedAt: 2000 }),
    events: [],
    nextIndex: 0,
    seriesLatest: [],
  }

  await store.restoreTaskArchive(data)
  await expect(store.restoreTaskArchive(data)).rejects.toThrow(/already exists/i)
  await expect(store.restoreTaskArchive(data, { overwrite: true })).resolves.toEqual({ overwritten: true })
})
```

Add these SQLite short-term tests to `packages/sqlite/tests/short-term.test.ts`:

```ts
it('restores a task archive and continues indexes after imported events', async () => {
  await store.restoreTaskArchive({
    task: { ...makeTask('task-archive'), status: 'running', createdAt: 1000, updatedAt: 2000 },
    events: [
      {
        id: 'event-1',
        taskId: 'task-archive',
        index: 0,
        timestamp: 3000,
        type: 'demo.event',
        level: 'info',
        data: null,
      },
    ],
    nextIndex: 1,
    seriesLatest: [],
  })

  expect(await store.getTask('task-archive')).toMatchObject({ id: 'task-archive', status: 'running' })
  expect(await store.getEvents('task-archive')).toHaveLength(1)
  expect(await store.nextIndex('task-archive')).toBe(1)
})

it('rejects restore conflicts unless overwrite is true', async () => {
  const data = {
    task: { ...makeTask('task-archive'), status: 'running', createdAt: 1000, updatedAt: 2000 },
    events: [],
    nextIndex: 0,
    seriesLatest: [],
  }

  await store.restoreTaskArchive(data)
  await expect(store.restoreTaskArchive(data)).rejects.toThrow(/already exists/i)
  await expect(store.restoreTaskArchive(data, { overwrite: true })).resolves.toEqual({ overwritten: true })
})
```

Add these SQLite long-term tests to `packages/sqlite/tests/long-term.test.ts`:

```ts
it('restores a task archive', async () => {
  await store.restoreTaskArchive({
    task: { ...makeTask('task-archive'), status: 'running', createdAt: 1000, updatedAt: 2000 },
    events: [makeEvent('task-archive', 0)],
    nextIndex: 1,
    seriesLatest: [],
  })

  expect(await store.getTask('task-archive')).toMatchObject({ id: 'task-archive', status: 'running' })
  expect(await store.getEvents('task-archive')).toHaveLength(1)
})

it('rejects long-term restore conflicts unless overwrite is true', async () => {
  const data = {
    task: { ...makeTask('task-archive'), status: 'running', createdAt: 1000, updatedAt: 2000 },
    events: [],
    nextIndex: 0,
    seriesLatest: [],
  }

  await store.restoreTaskArchive(data)
  await expect(store.restoreTaskArchive(data)).rejects.toThrow(/already exists/i)
  await expect(store.restoreTaskArchive(data, { overwrite: true })).resolves.toEqual({ overwritten: true })
})
```

Add these Postgres long-term tests to `packages/postgres/tests/long-term.test.ts`:

```ts
it('restores a task archive', async () => {
  await store.restoreTaskArchive({
    task: { ...makeTask('task-archive'), status: 'running', createdAt: 1000, updatedAt: 2000 },
    events: [makeEvent('task-archive', 0)],
    nextIndex: 1,
    seriesLatest: [],
  })

  expect(await store.getTask('task-archive')).toMatchObject({ id: 'task-archive', status: 'running' })
  expect(await store.getEvents('task-archive')).toHaveLength(1)
})

it('rejects long-term restore conflicts unless overwrite is true', async () => {
  const data = {
    task: { ...makeTask('task-archive'), status: 'running', createdAt: 1000, updatedAt: 2000 },
    events: [],
    nextIndex: 0,
    seriesLatest: [],
  }

  await store.restoreTaskArchive(data)
  await expect(store.restoreTaskArchive(data)).rejects.toThrow(/already exists/i)
  await expect(store.restoreTaskArchive(data, { overwrite: true })).resolves.toEqual({ overwritten: true })
})
```

- [ ] **Step 2: Run adapter tests and verify failure**

Run:

```bash
cd packages/redis
pnpm test tests/short-term.test.ts
cd ../sqlite
pnpm test tests/short-term.test.ts tests/long-term.test.ts
cd ../postgres
pnpm test tests/long-term.test.ts
```

Expected: FAIL because `restoreTaskArchive` is not implemented.

- [ ] **Step 3: Implement Redis restore**

In `packages/redis/src/short-term.ts`, add imports:

```ts
  TaskArchiveImportOptions,
  TaskArchiveRestoreData,
```

Add inside `RedisShortTermStore`:

```ts
  async restoreTaskArchive(
    data: TaskArchiveRestoreData,
    options?: TaskArchiveImportOptions,
  ): Promise<{ overwritten: boolean }> {
    const taskKey = this.KEY.task(data.task.id)
    const exists = await this.redis.exists(taskKey)
    if (exists && options?.overwrite !== true) {
      throw new Error(`Task already exists: ${data.task.id}`)
    }

    const seriesIds = await this.redis.smembers(this.KEY.seriesIds(data.task.id))
    const pipeline = this.redis.pipeline()
    pipeline.set(taskKey, JSON.stringify(data.task))
    pipeline.sadd(this.KEY.taskSet, data.task.id)
    pipeline.del(this.KEY.events(data.task.id))
    for (const event of data.events) {
      pipeline.rpush(this.KEY.events(data.task.id), JSON.stringify(event))
    }
    pipeline.set(this.KEY.idx(data.task.id), String(data.nextIndex))
    for (const seriesId of seriesIds) {
      pipeline.del(this.KEY.seriesLatest(data.task.id, seriesId))
    }
    pipeline.del(this.KEY.seriesIds(data.task.id))
    for (const entry of data.seriesLatest) {
      pipeline.set(this.KEY.seriesLatest(entry.taskId, entry.seriesId), JSON.stringify(entry.event))
      pipeline.sadd(this.KEY.seriesIds(entry.taskId), entry.seriesId)
    }
    await pipeline.exec()

    return { overwritten: Boolean(exists) }
  }
```

This stores `data.nextIndex` because Redis `nextIndex` uses `incr(key) - 1`; the next publish should therefore return exactly `data.nextIndex`.

- [ ] **Step 4: Implement SQLite short-term restore**

In `packages/sqlite/src/short-term.ts`, add imports for archive types. Add a method that runs a transaction:

```ts
  async restoreTaskArchive(
    data: TaskArchiveRestoreData,
    options?: TaskArchiveImportOptions,
  ): Promise<{ overwritten: boolean }> {
    const restore = this.db.transaction(() => {
      const existing = this.db.prepare('SELECT id FROM taskcast_tasks WHERE id = ?').get(data.task.id)
      if (existing && options?.overwrite !== true) {
        throw new Error(`Task already exists: ${data.task.id}`)
      }

      this.db.prepare('DELETE FROM taskcast_events WHERE task_id = ?').run(data.task.id)
      this.db.prepare('DELETE FROM taskcast_series_latest WHERE task_id = ?').run(data.task.id)
      this.saveTask(data.task)
      for (const event of data.events) this.appendEvent(data.task.id, event)
      this.db.prepare(`
        INSERT INTO taskcast_index_counters (task_id, counter)
        VALUES (?, ?)
        ON CONFLICT (task_id) DO UPDATE SET counter = excluded.counter
      `).run(data.task.id, data.nextIndex - 1)
      for (const entry of data.seriesLatest) {
        this.setSeriesLatest(entry.taskId, entry.seriesId, entry.event)
      }

      return { overwritten: Boolean(existing) }
    })

    return restore()
  }
```

If calling async methods inside the synchronous `better-sqlite3` transaction creates promise timing problems, extract the existing SQL bodies of `saveTask`, `appendEvent`, and `setSeriesLatest` into private synchronous helpers and call those helpers from both normal methods and `restoreTaskArchive`.

- [ ] **Step 5: Implement SQLite long-term restore**

In `packages/sqlite/src/long-term.ts`, add:

```ts
  async restoreTaskArchive(
    data: TaskArchiveRestoreData,
    options?: TaskArchiveImportOptions,
  ): Promise<{ overwritten: boolean }> {
    const restore = this.db.transaction(() => {
      const existing = this.db.prepare('SELECT id FROM taskcast_tasks WHERE id = ?').get(data.task.id)
      if (existing && options?.overwrite !== true) {
        throw new Error(`Task already exists: ${data.task.id}`)
      }
      this.db.prepare('DELETE FROM taskcast_events WHERE task_id = ?').run(data.task.id)
      this.saveTask(data.task)
      for (const event of data.events) this.saveEvent(event)
      return { overwritten: Boolean(existing) }
    })

    return restore()
  }
```

Use private synchronous helpers if needed, matching Step 4.

- [ ] **Step 6: Implement Postgres long-term restore**

In `packages/postgres/src/long-term.ts`, add a transaction method equivalent to:

```ts
  async restoreTaskArchive(
    data: TaskArchiveRestoreData,
    options?: TaskArchiveImportOptions,
  ): Promise<{ overwritten: boolean }> {
    return this.sql.begin(async (sql) => {
      const existing = await sql`SELECT id FROM taskcast_tasks WHERE id = ${data.task.id}`
      if (existing.length > 0 && options?.overwrite !== true) {
        throw new Error(`Task already exists: ${data.task.id}`)
      }

      await sql`DELETE FROM taskcast_events WHERE task_id = ${data.task.id}`
      await this.saveTaskWithClient(sql, data.task)
      for (const event of data.events) {
        await this.saveEventWithClient(sql, event)
      }
      return { overwritten: existing.length > 0 }
    })
  }
```

If `saveTaskWithClient` and `saveEventWithClient` do not exist, extract the SQL bodies from `saveTask(...)` and `saveEvent(...)` into private helpers that accept either the root postgres client or transaction client.

- [ ] **Step 7: Run adapter tests**

Run:

```bash
cd packages/redis
pnpm test tests/short-term.test.ts
cd ../sqlite
pnpm test tests/short-term.test.ts tests/long-term.test.ts
cd ../postgres
pnpm test tests/long-term.test.ts
```

Expected: PASS.

- [ ] **Step 8: Commit**

```bash
git add packages/redis/src/short-term.ts packages/redis/tests/short-term.test.ts packages/sqlite/src/short-term.ts packages/sqlite/src/long-term.ts packages/sqlite/tests/short-term.test.ts packages/sqlite/tests/long-term.test.ts packages/postgres/src/long-term.ts packages/postgres/tests/long-term.test.ts
git commit -m "feat(adapters): restore task archives"
```

---

### Task 4: TypeScript Server Routes and Version Handshake

**Files:**
- Create: `packages/server/src/version.ts`
- Modify: `packages/server/src/schemas.ts`
- Modify: `packages/server/src/routes/tasks.ts`
- Modify: `packages/server/src/index.ts`
- Test: `packages/server/tests/archive-routes.test.ts`
- Test: `packages/server/tests/health-detail.test.ts`
- Test: `packages/server/tests/openapi.test.ts`

- [ ] **Step 1: Write failing server tests**

Create `packages/server/tests/archive-routes.test.ts`:

```ts
import { describe, expect, it } from 'vitest'
import { MemoryBroadcastProvider, MemoryShortTermStore, TaskEngine } from '@taskcast/core'
import { createTaskcastApp } from '../src/index.js'

function makeApp() {
  const engine = new TaskEngine({
    broadcast: new MemoryBroadcastProvider(),
    shortTermStore: new MemoryShortTermStore(),
  })
  const { app } = createTaskcastApp({ engine, auth: { mode: 'none' } })
  return { app, engine }
}

describe('task archive routes', () => {
  it('exports a task archive', async () => {
    const { app, engine } = makeApp()
    await engine.createTask({ id: 'task-1' })
    await engine.publishEvent('task-1', { type: 'demo.event', level: 'info', data: null })

    const res = await app.request('/tasks/task-1/archive')
    expect(res.status).toBe(200)
    const body = await res.json()
    expect(body.schema).toBe('taskcast.taskArchive')
    expect(body.task.id).toBe('task-1')
    expect(body.events).toHaveLength(1)
  })

  it('returns 404 when exporting a missing task', async () => {
    const { app } = makeApp()
    const res = await app.request('/tasks/missing/archive')
    expect(res.status).toBe(404)
  })

  it('imports a task archive', async () => {
    const { app } = makeApp()
    const res = await app.request('/tasks/import', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({
        archive: {
          schema: 'taskcast.taskArchive',
          version: 1,
          exportedAt: 5000,
          task: { id: 'task-1', status: 'running', createdAt: 1000, updatedAt: 2000 },
          events: [],
        },
      }),
    })
    expect(res.status).toBe(200)
    await expect(res.json()).resolves.toEqual({
      ok: true,
      taskId: 'task-1',
      eventCount: 0,
      overwritten: false,
    })
  })

  it('returns 409 on import conflict without overwrite', async () => {
    const { app, engine } = makeApp()
    await engine.createTask({ id: 'task-1' })
    const res = await app.request('/tasks/import', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({
        archive: {
          schema: 'taskcast.taskArchive',
          version: 1,
          exportedAt: 5000,
          task: { id: 'task-1', status: 'running', createdAt: 1000, updatedAt: 2000 },
          events: [],
        },
      }),
    })
    expect(res.status).toBe(409)
  })

  it('returns 400 for malformed archive input', async () => {
    const { app } = makeApp()
    const res = await app.request('/tasks/import', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ archive: { schema: 'wrong' } }),
    })
    expect(res.status).toBe(400)
  })
})
```

Extend `packages/server/tests/health-detail.test.ts`:

```ts
it('GET / returns public server info with version', async () => {
  const engine = new TaskEngine({
    broadcast: new MemoryBroadcastProvider(),
    shortTermStore: new MemoryShortTermStore(),
  })
  const { app } = createTaskcastApp({ engine, auth: { mode: 'none' } })

  const res = await app.request('/')
  expect(res.status).toBe(200)
  const body = await res.json()
  expect(body).toMatchObject({
    name: 'taskcast',
    apiVersion: 'v1',
    links: {
      health: '/health',
      healthDetail: '/health/detail',
      openapi: '/openapi.json',
      docs: '/docs',
    },
  })
  expect(body.version).toMatch(/^\d+\.\d+\.\d+/)
})

it('GET /health includes version fields', async () => {
  const engine = new TaskEngine({
    broadcast: new MemoryBroadcastProvider(),
    shortTermStore: new MemoryShortTermStore(),
  })
  const { app } = createTaskcastApp({ engine, auth: { mode: 'none' } })

  const res = await app.request('/health')
  const body = await res.json()
  expect(body).toMatchObject({ ok: true, name: 'taskcast', apiVersion: 'v1' })
  expect(body.version).toMatch(/^\d+\.\d+\.\d+/)
})
```

Modify `packages/server/tests/openapi.test.ts`:

```ts
import { TASKCAST_SERVER_VERSION } from '../src/version.js'
```

Add:

```ts
expect(spec.info.version).toBe(TASKCAST_SERVER_VERSION)
```

- [ ] **Step 2: Run server tests and verify failure**

Run:

```bash
cd packages/server
pnpm test tests/archive-routes.test.ts tests/health-detail.test.ts tests/openapi.test.ts
```

Expected: FAIL because routes, schemas, and version helper are missing.

- [ ] **Step 3: Add server version helper**

Create `packages/server/src/version.ts`:

```ts
import { createRequire } from 'node:module'

const require = createRequire(import.meta.url)
const pkg = require('../package.json') as { version: string }

export const TASKCAST_SERVER_NAME = 'taskcast'
export const TASKCAST_API_VERSION = 'v1'
export const TASKCAST_SERVER_VERSION = pkg.version

export function serverInfo() {
  return {
    name: TASKCAST_SERVER_NAME,
    version: TASKCAST_SERVER_VERSION,
    apiVersion: TASKCAST_API_VERSION,
  }
}
```

- [ ] **Step 4: Add schemas**

In `packages/server/src/schemas.ts`, add:

```ts
export const TaskArchiveSchema = z
  .object({
    schema: z.literal('taskcast.taskArchive'),
    version: z.literal(1),
    exportedAt: z.number(),
    task: TaskSchema,
    events: z.array(TaskEventSchema),
  })
  .openapi('TaskArchive')

export const ImportTaskArchiveSchema = z
  .object({
    archive: TaskArchiveSchema,
    overwrite: z.boolean().optional(),
  })
  .openapi('ImportTaskArchiveInput')

export const ImportTaskArchiveResultSchema = z
  .object({
    ok: z.literal(true),
    taskId: z.string(),
    eventCount: z.number().int().nonnegative(),
    overwritten: z.boolean(),
  })
  .openapi('ImportTaskArchiveResult')

export const ServerInfoSchema = z
  .object({
    name: z.literal('taskcast'),
    version: z.string(),
    apiVersion: z.literal('v1'),
    links: z
      .object({
        health: z.string(),
        healthDetail: z.string(),
        openapi: z.string(),
        docs: z.string(),
      })
      .optional(),
  })
  .openapi('ServerInfo')
```

- [ ] **Step 5: Add archive routes**

In `packages/server/src/routes/tasks.ts`, import:

```ts
  ImportTaskArchiveSchema,
  ImportTaskArchiveResultSchema,
  TaskArchiveSchema,
```

Import archive error:

```ts
import { InvalidTaskArchiveError } from '@taskcast/core'
```

Add OpenAPI route definitions:

```ts
const exportArchiveRoute = createRoute({
  method: 'get',
  path: '/{taskId}/archive',
  tags: ['Tasks'],
  summary: 'Export task archive',
  security: [{ Bearer: [] }],
  request: { params: z.object({ taskId: z.string() }) },
  responses: {
    200: { description: 'Task archive', content: { 'application/json': { schema: TaskArchiveSchema } } },
    403: { description: 'Forbidden', content: { 'application/json': { schema: ErrorSchema } } },
    404: { description: 'Task not found', content: { 'application/json': { schema: ErrorSchema } } },
  },
})

const importArchiveRoute = createRoute({
  method: 'post',
  path: '/import',
  tags: ['Tasks'],
  summary: 'Import task archive',
  security: [{ Bearer: [] }],
  request: {
    body: { content: { 'application/json': { schema: ImportTaskArchiveSchema } } },
  },
  responses: {
    200: { description: 'Import result', content: { 'application/json': { schema: ImportTaskArchiveResultSchema } } },
    400: { description: 'Validation error', content: { 'application/json': { schema: ErrorSchema } } },
    403: { description: 'Forbidden', content: { 'application/json': { schema: ErrorSchema } } },
    409: { description: 'Task ID already exists', content: { 'application/json': { schema: ErrorSchema } } },
  },
})
```

Register handlers before resolve/request handlers:

```ts
  register(exportArchiveRoute, async (c) => {
    const taskId = c.req.param('taskId') as string
    const auth = c.get('auth')
    if (!checkScope(auth, 'event:history', taskId)) return c.json({ error: 'Forbidden' }, 403)
    try {
      return c.json(await engine.exportTaskArchive(taskId))
    } catch (err) {
      const msg = err instanceof Error ? err.message : String(err)
      if (msg.toLowerCase().includes('not found')) return c.json({ error: msg }, 404)
      throw err
    }
  })

  register(importArchiveRoute, async (c) => {
    const auth = c.get('auth')
    if (!checkScope(auth, 'task:manage')) return c.json({ error: 'Forbidden' }, 403)
    const body = await c.req.json()
    const parsed = ImportTaskArchiveSchema.safeParse(body)
    if (!parsed.success) return c.json({ error: parsed.error.flatten() }, 400)

    try {
      const result = await engine.importTaskArchive(parsed.data.archive, {
        overwrite: parsed.data.overwrite,
      })
      return c.json({ ok: true, ...result })
    } catch (err) {
      if (err instanceof TaskConflictError) return c.json({ error: err.message }, 409)
      if (err instanceof InvalidTaskArchiveError) return c.json({ error: err.message }, 400)
      const msg = err instanceof Error ? err.message : String(err)
      return c.json({ error: msg }, 400)
    }
  })
```

- [ ] **Step 6: Add root and health version fields**

In `packages/server/src/index.ts`, import:

```ts
import { serverInfo, TASKCAST_SERVER_VERSION } from './version.js'
```

Add before `/health`:

```ts
  app.get('/', (c) => c.json({
    ...serverInfo(),
    links: {
      health: '/health',
      healthDetail: '/health/detail',
      openapi: '/openapi.json',
      docs: '/docs',
    },
  }))
```

Change `/health`:

```ts
  app.get('/health', (c) => c.json({ ok: true, ...serverInfo() }))
```

Add version fields in `/health/detail` response:

```ts
      ...serverInfo(),
```

Change OpenAPI version:

```ts
      version: TASKCAST_SERVER_VERSION,
```

- [ ] **Step 7: Run server tests**

Run:

```bash
cd packages/server
pnpm test tests/archive-routes.test.ts tests/health-detail.test.ts tests/openapi.test.ts
```

Expected: PASS.

- [ ] **Step 8: Commit**

```bash
git add packages/server/src/version.ts packages/server/src/schemas.ts packages/server/src/routes/tasks.ts packages/server/src/index.ts packages/server/tests/archive-routes.test.ts packages/server/tests/health-detail.test.ts packages/server/tests/openapi.test.ts
git commit -m "feat(server): expose task archive routes"
```

---

### Task 5: TypeScript Server SDK

**Files:**
- Modify: `packages/server-sdk/src/client.ts`
- Modify: `packages/server-sdk/src/index.ts`
- Test: `packages/server-sdk/tests/client.test.ts`

- [ ] **Step 1: Write failing SDK tests**

Add to `packages/server-sdk/tests/client.test.ts`:

```ts
describe('TaskcastServerClient.getServerInfo', () => {
  it('GET / and returns server version info', async () => {
    const fetch = makeFetch([{ status: 200, body: { name: 'taskcast', version: '1.5.1', apiVersion: 'v1' } }])
    const client = new TaskcastServerClient({ baseUrl: 'http://taskcast', fetch })

    const info = await client.getServerInfo()

    expect(info).toEqual({ name: 'taskcast', version: '1.5.1', apiVersion: 'v1' })
    expect(fetch.mock.calls[0]![0]).toBe('http://taskcast/')
  })
})

describe('TaskcastServerClient task archive APIs', () => {
  it('exports task archive', async () => {
    const body = {
      schema: 'taskcast.taskArchive',
      version: 1,
      exportedAt: 5000,
      task: { id: 'task-1', status: 'running', createdAt: 1000, updatedAt: 2000 },
      events: [],
    }
    const fetch = makeFetch([{ status: 200, body }])
    const client = new TaskcastServerClient({ baseUrl: 'http://taskcast', fetch })

    await expect(client.exportTaskArchive('task-1')).resolves.toEqual(body)
    expect(fetch.mock.calls[0]![0]).toBe('http://taskcast/tasks/task-1/archive')
  })

  it('imports task archive', async () => {
    const archive = {
      schema: 'taskcast.taskArchive' as const,
      version: 1 as const,
      exportedAt: 5000,
      task: { id: 'task-1', status: 'running' as const, createdAt: 1000, updatedAt: 2000 },
      events: [],
    }
    const fetch = makeFetch([{ status: 200, body: { ok: true, taskId: 'task-1', eventCount: 0, overwritten: false } }])
    const client = new TaskcastServerClient({ baseUrl: 'http://taskcast', fetch })

    await client.importTaskArchive(archive, { overwrite: true })

    const [url, opts] = fetch.mock.calls[0]!
    expect(url).toBe('http://taskcast/tasks/import')
    expect(opts.method).toBe('POST')
    expect(JSON.parse(opts.body)).toEqual({ archive, overwrite: true })
  })
})
```

- [ ] **Step 2: Run SDK tests and verify failure**

Run:

```bash
cd packages/server-sdk
pnpm test tests/client.test.ts
```

Expected: FAIL because methods are missing.

- [ ] **Step 3: Add SDK types and methods**

In `packages/server-sdk/src/client.ts`, update imports:

```ts
import type { Task, TaskArchive, TaskArchiveImportResult, TaskEvent, TaskStatus, TaskAuthConfig, WebhookConfig, CleanupRule, SeriesMode, SinceCursor, TaskError, SubscribeFilter } from '@taskcast/core'
```

Add:

```ts
export interface TaskcastServerInfo {
  name: 'taskcast'
  version: string
  apiVersion: 'v1'
}
```

Add methods inside `TaskcastServerClient`:

```ts
  async getServerInfo(): Promise<TaskcastServerInfo> {
    return this._request<TaskcastServerInfo>('GET', '/')
  }

  async exportTaskArchive(taskId: string): Promise<TaskArchive> {
    return this._request<TaskArchive>('GET', `/tasks/${taskId}/archive`)
  }

  async importTaskArchive(
    archive: TaskArchive,
    options?: { overwrite?: boolean },
  ): Promise<{ ok: true } & TaskArchiveImportResult> {
    return this._request<{ ok: true } & TaskArchiveImportResult>('POST', '/tasks/import', {
      archive,
      ...(options?.overwrite !== undefined && { overwrite: options.overwrite }),
    })
  }
```

Modify `packages/server-sdk/src/index.ts`:

```ts
export type { TaskcastServerClientOptions, TaskcastServerInfo, CreateTaskInput, PublishEventInput } from './client.js'
```

- [ ] **Step 4: Run SDK tests**

Run:

```bash
cd packages/server-sdk
pnpm test tests/client.test.ts
```

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add packages/server-sdk/src/client.ts packages/server-sdk/src/index.ts packages/server-sdk/tests/client.test.ts
git commit -m "feat(server-sdk): add task archive client APIs"
```

---

### Task 6: Rust Archive Types, Validation, Memory Restore, and Engine API

**Files:**
- Create: `rust/taskcast-core/src/archive.rs`
- Modify: `rust/taskcast-core/src/types.rs`
- Modify: `rust/taskcast-core/src/lib.rs`
- Modify: `rust/taskcast-core/src/memory_adapters.rs`
- Modify: `rust/taskcast-core/src/engine.rs`
- Test: `rust/taskcast-core/tests/archive.rs`

- [ ] **Step 1: Write failing Rust core tests**

Create `rust/taskcast-core/tests/archive.rs`:

```rust
use std::sync::Arc;

use taskcast_core::{
    build_task_archive_restore_data, validate_task_archive, EngineError, Level,
    MemoryBroadcastProvider, MemoryShortTermStore, Task, TaskArchive, TaskEngine,
    TaskEngineOptions, TaskEvent, TaskStatus,
};

fn task(id: &str) -> Task {
    Task {
        id: id.to_string(),
        r#type: Some("demo".to_string()),
        status: TaskStatus::Running,
        params: None,
        result: None,
        error: None,
        metadata: None,
        created_at: 1000.0,
        updated_at: 2000.0,
        completed_at: None,
        ttl: None,
        auth_config: None,
        webhooks: None,
        cleanup: None,
        tags: None,
        assign_mode: None,
        cost: None,
        assigned_worker: None,
        disconnect_policy: None,
        reason: None,
        resume_at: None,
        blocked_request: None,
    }
}

fn event(id: &str, task_id: &str, index: u64) -> TaskEvent {
    TaskEvent {
        id: id.to_string(),
        task_id: task_id.to_string(),
        index,
        timestamp: 3000.0 + index as f64,
        r#type: "demo.event".to_string(),
        level: Level::Info,
        data: serde_json::Value::Null,
        series_id: None,
        series_mode: None,
        series_acc_field: None,
        series_snapshot: None,
        _accumulated_data: None,
    }
}

fn archive(events: Vec<TaskEvent>) -> TaskArchive {
    TaskArchive {
        schema: "taskcast.taskArchive".to_string(),
        version: 1,
        exported_at: 5000.0,
        task: task("task-1"),
        events,
    }
}

fn engine() -> TaskEngine {
    TaskEngine::new(TaskEngineOptions {
        short_term_store: Arc::new(MemoryShortTermStore::new()),
        broadcast: Arc::new(MemoryBroadcastProvider::new()),
        long_term_store: None,
        hooks: None,
    })
}

#[test]
fn validate_rejects_duplicate_indexes() {
    let err = validate_task_archive(&archive(vec![
        event("event-1", "task-1", 0),
        event("event-2", "task-1", 0),
    ]))
    .unwrap_err();
    assert!(err.to_string().contains("duplicate event index"));
}

#[test]
fn restore_data_sets_next_index() {
    let data = build_task_archive_restore_data(&archive(vec![
        event("event-1", "task-1", 0),
        event("event-2", "task-1", 1),
    ]))
    .unwrap();
    assert_eq!(data.next_index, 2);
}

#[tokio::test]
async fn engine_import_preserves_history_and_continues_index() {
    let source = engine();
    source
        .import_task_archive(archive(vec![event("event-1", "task-1", 0)]), None)
        .await
        .unwrap();

    let next = source
        .publish_event(
            "task-1",
            taskcast_core::PublishEventInput {
                r#type: "demo.next".to_string(),
                level: Level::Info,
                data: serde_json::Value::Null,
                series_id: None,
                series_mode: None,
                series_acc_field: None,
            },
        )
        .await
        .unwrap();

    assert_eq!(next.index, 1);
}

#[tokio::test]
async fn engine_import_rejects_conflict_without_overwrite() {
    let engine = engine();
    engine.import_task_archive(archive(vec![]), None).await.unwrap();
    let err = engine.import_task_archive(archive(vec![]), None).await.unwrap_err();
    assert!(matches!(err, EngineError::TaskConflict(_)));
}
```

- [ ] **Step 2: Run tests and verify failure**

Run:

```bash
cd rust
cargo test -p taskcast-core --test archive
```

Expected: FAIL because archive types and methods do not exist.

- [ ] **Step 3: Add Rust archive types and trait methods**

In `rust/taskcast-core/src/types.rs`, add:

```rust
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct TaskArchive {
    pub schema: String,
    pub version: u32,
    pub exported_at: f64,
    pub task: Task,
    pub events: Vec<TaskEvent>,
}

#[derive(Debug, Clone, Default)]
pub struct TaskArchiveImportOptions {
    pub overwrite: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct TaskArchiveImportResult {
    pub task_id: String,
    pub event_count: u64,
    pub overwritten: bool,
}

#[derive(Debug, Clone)]
pub struct SeriesLatestEntry {
    pub task_id: String,
    pub series_id: String,
    pub event: TaskEvent,
}

#[derive(Debug, Clone)]
pub struct TaskArchiveRestoreData {
    pub task: Task,
    pub events: Vec<TaskEvent>,
    pub next_index: u64,
    pub series_latest: Vec<SeriesLatestEntry>,
}
```

Add to `ShortTermStore` and `LongTermStore` traits:

```rust
    async fn restore_task_archive(
        &self,
        data: TaskArchiveRestoreData,
        options: TaskArchiveImportOptions,
    ) -> Result<bool, Box<dyn std::error::Error + Send + Sync>>;
```

- [ ] **Step 4: Add Rust archive module**

Create `rust/taskcast-core/src/archive.rs`:

```rust
use std::collections::{HashMap, HashSet};

use crate::types::{
    SeriesLatestEntry, SeriesMode, TaskArchive, TaskArchiveRestoreData, TaskEvent,
};

#[derive(Debug, thiserror::Error)]
pub enum ArchiveError {
    #[error("Unsupported archive schema: {0}")]
    UnsupportedSchema(String),
    #[error("Unsupported archive version: {0}")]
    UnsupportedVersion(u32),
    #[error("Archive event taskId mismatch for event {0}")]
    TaskIdMismatch(String),
    #[error("Archive contains duplicate event id: {0}")]
    DuplicateEventId(String),
    #[error("Archive contains duplicate event index: {0}")]
    DuplicateEventIndex(u64),
    #[error("Archive event indexes must be contiguous from 0; expected {expected}, got {got}")]
    NonContiguousIndex { expected: u64, got: u64 },
}

pub const TASK_ARCHIVE_SCHEMA: &str = "taskcast.taskArchive";
pub const TASK_ARCHIVE_VERSION: u32 = 1;

pub fn validate_task_archive(archive: &TaskArchive) -> Result<TaskArchive, ArchiveError> {
    if archive.schema != TASK_ARCHIVE_SCHEMA {
        return Err(ArchiveError::UnsupportedSchema(archive.schema.clone()));
    }
    if archive.version != TASK_ARCHIVE_VERSION {
        return Err(ArchiveError::UnsupportedVersion(archive.version));
    }

    let mut events = archive.events.clone();
    events.sort_by_key(|event| event.index);

    let mut seen_ids = HashSet::new();
    let mut seen_indexes = HashSet::new();
    for (expected, event) in events.iter().enumerate() {
        if event.task_id != archive.task.id {
            return Err(ArchiveError::TaskIdMismatch(event.id.clone()));
        }
        if !seen_ids.insert(event.id.clone()) {
            return Err(ArchiveError::DuplicateEventId(event.id.clone()));
        }
        if !seen_indexes.insert(event.index) {
            return Err(ArchiveError::DuplicateEventIndex(event.index));
        }
        if event.index != expected as u64 {
            return Err(ArchiveError::NonContiguousIndex {
                expected: expected as u64,
                got: event.index,
            });
        }
    }

    Ok(TaskArchive {
        events,
        ..archive.clone()
    })
}

pub fn build_task_archive_restore_data(
    archive: &TaskArchive,
) -> Result<TaskArchiveRestoreData, ArchiveError> {
    let normalized = validate_task_archive(archive)?;
    let next_index = normalized.events.len() as u64;
    let series_latest = build_series_latest(&normalized.events);
    Ok(TaskArchiveRestoreData {
        task: normalized.task,
        events: normalized.events,
        next_index,
        series_latest,
    })
}

fn build_series_latest(events: &[TaskEvent]) -> Vec<SeriesLatestEntry> {
    let mut latest: HashMap<String, TaskEvent> = HashMap::new();
    for event in events {
        let Some(series_id) = event.series_id.clone() else { continue };
        let Some(series_mode) = event.series_mode.clone() else { continue };
        if series_mode == SeriesMode::KeepAll {
            continue;
        }
        let key = format!("{}:{}", event.task_id, series_id);
        if series_mode == SeriesMode::Latest {
            latest.insert(key, event.clone());
            continue;
        }

        let field = event.series_acc_field.as_deref().unwrap_or("delta");
        let Some(previous) = latest.get(&key) else {
            latest.insert(key, event.clone());
            continue;
        };

        let prev_text = previous.data.get(field).and_then(|v| v.as_str());
        let new_text = event.data.get(field).and_then(|v| v.as_str());
        if let (Some(prev), Some(new)) = (prev_text, new_text) {
            let mut accumulated = event.clone();
            let mut data = match accumulated.data {
                serde_json::Value::Object(map) => map,
                _ => serde_json::Map::new(),
            };
            data.insert(field.to_string(), serde_json::Value::String(format!("{prev}{new}")));
            accumulated.data = serde_json::Value::Object(data);
            latest.insert(key, accumulated);
        } else {
            latest.insert(key, event.clone());
        }
    }

    latest
        .into_values()
        .filter_map(|event| {
            event.series_id.clone().map(|series_id| SeriesLatestEntry {
                task_id: event.task_id.clone(),
                series_id,
                event,
            })
        })
        .collect()
}
```

Modify `rust/taskcast-core/src/lib.rs`:

```rust
pub mod archive;
pub use archive::*;
```

- [ ] **Step 5: Add memory store restore**

In `rust/taskcast-core/src/memory_adapters.rs`, import:

```rust
    TaskArchiveImportOptions, TaskArchiveRestoreData,
```

Add inside `impl ShortTermStore for MemoryShortTermStore`:

```rust
    async fn restore_task_archive(
        &self,
        data: TaskArchiveRestoreData,
        options: TaskArchiveImportOptions,
    ) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
        let exists = self.tasks.read().unwrap().contains_key(&data.task.id);
        if exists && !options.overwrite {
            return Err(format!("Task already exists: {}", data.task.id).into());
        }

        self.tasks
            .write()
            .unwrap()
            .insert(data.task.id.clone(), data.task.clone());
        self.events
            .write()
            .unwrap()
            .insert(data.task.id.clone(), data.events.clone());
        self.index_counters.write().unwrap().insert(
            data.task.id.clone(),
            Arc::new(AtomicU64::new(data.next_index)),
        );

        let mut series = self.series_latest.write().unwrap();
        series.retain(|key, _| !key.starts_with(&format!("{}:", data.task.id)));
        for entry in data.series_latest {
            series.insert(format!("{}:{}", entry.task_id, entry.series_id), entry.event);
        }

        Ok(exists)
    }
```

- [ ] **Step 6: Add Rust engine archive API**

In `rust/taskcast-core/src/engine.rs`, import:

```rust
use crate::archive::{build_task_archive_restore_data, validate_task_archive};
```

Add types to the existing import list:

```rust
    TaskArchive, TaskArchiveImportOptions, TaskArchiveImportResult,
```

Add methods before `publish_event(...)`:

```rust
    pub async fn export_task_archive(&self, task_id: &str) -> Result<TaskArchive, EngineError> {
        let task = self
            .get_task(task_id)
            .await?
            .ok_or_else(|| EngineError::TaskNotFound(task_id.to_string()))?;
        let mut events = self.get_events(task_id, None).await?;
        events.sort_by_key(|event| event.index);
        Ok(TaskArchive {
            schema: crate::archive::TASK_ARCHIVE_SCHEMA.to_string(),
            version: crate::archive::TASK_ARCHIVE_VERSION,
            exported_at: now_millis(),
            task,
            events,
        })
    }

    pub async fn import_task_archive(
        &self,
        archive: TaskArchive,
        options: Option<TaskArchiveImportOptions>,
    ) -> Result<TaskArchiveImportResult, EngineError> {
        let options = options.unwrap_or_default();
        let normalized = validate_task_archive(&archive)
            .map_err(|err| EngineError::InvalidInput(err.to_string()))?;
        if self
            .short_term_store
            .get_task(&normalized.task.id)
            .await?
            .is_some()
            && !options.overwrite
        {
            return Err(EngineError::TaskConflict(normalized.task.id));
        }

        let data = build_task_archive_restore_data(&normalized)
            .map_err(|err| EngineError::InvalidInput(err.to_string()))?;
        let overwritten = self
            .short_term_store
            .restore_task_archive(data.clone(), options.clone())
            .await?;
        if let Some(ref long_term_store) = self.long_term_store {
            long_term_store.restore_task_archive(data, options).await?;
        }
        self.emit_locks.lock().unwrap().remove(&normalized.task.id);

        Ok(TaskArchiveImportResult {
            task_id: normalized.task.id,
            event_count: normalized.events.len() as u64,
            overwritten,
        })
    }
```

Derive `Clone` for `TaskArchiveImportOptions` in `types.rs`:

```rust
#[derive(Debug, Clone, Default)]
```

- [ ] **Step 7: Run Rust core tests**

Run:

```bash
cd rust
cargo test -p taskcast-core --test archive
```

Expected: PASS.

- [ ] **Step 8: Commit**

```bash
git add rust/taskcast-core/src/archive.rs rust/taskcast-core/src/types.rs rust/taskcast-core/src/lib.rs rust/taskcast-core/src/memory_adapters.rs rust/taskcast-core/src/engine.rs rust/taskcast-core/tests/archive.rs
git commit -m "feat(rust-core): add task archive restore"
```

---

### Task 7: Rust Server Routes and Version Handshake

**Files:**
- Modify: `rust/taskcast-server/src/app.rs`
- Modify: `rust/taskcast-server/src/routes/tasks.rs`
- Modify: `rust/taskcast-server/src/error.rs`
- Test: `rust/taskcast-server/tests/archive_routes.rs`
- Test: `rust/taskcast-server/tests/health_detail.rs`

- [ ] **Step 1: Write failing Rust server tests**

Create `rust/taskcast-server/tests/archive_routes.rs`:

```rust
use std::sync::Arc;

use axum_test::TestServer;
use taskcast_core::{
    MemoryBroadcastProvider, MemoryShortTermStore, TaskEngine, TaskEngineOptions,
};
use taskcast_server::{create_app, AuthMode, CorsConfig};

fn server() -> TestServer {
    let engine = Arc::new(TaskEngine::new(TaskEngineOptions {
        short_term_store: Arc::new(MemoryShortTermStore::new()),
        broadcast: Arc::new(MemoryBroadcastProvider::new()),
        long_term_store: None,
        hooks: None,
    }));
    let (app, _) = create_app(engine, AuthMode::None, None, None, CorsConfig::default());
    TestServer::new(app)
}

#[tokio::test]
async fn imports_task_archive() {
    let server = server();
    let res = server
        .post("/tasks/import")
        .json(&serde_json::json!({
            "archive": {
                "schema": "taskcast.taskArchive",
                "version": 1,
                "exportedAt": 5000.0,
                "task": {
                    "id": "task-1",
                    "status": "running",
                    "createdAt": 1000.0,
                    "updatedAt": 2000.0
                },
                "events": []
            }
        }))
        .await;
    res.assert_status_ok();
    let body: serde_json::Value = res.json();
    assert_eq!(body["ok"], true);
    assert_eq!(body["taskId"], "task-1");
}

#[tokio::test]
async fn export_missing_task_returns_404() {
    let server = server();
    let res = server.get("/tasks/missing/archive").await;
    res.assert_status(axum::http::StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn import_conflict_returns_409() {
    let server = server();
    let body = serde_json::json!({
        "archive": {
            "schema": "taskcast.taskArchive",
            "version": 1,
            "exportedAt": 5000.0,
            "task": {
                "id": "task-1",
                "status": "running",
                "createdAt": 1000.0,
                "updatedAt": 2000.0
            },
            "events": []
        }
    });
    server.post("/tasks/import").json(&body).await.assert_status_ok();
    server
        .post("/tasks/import")
        .json(&body)
        .await
        .assert_status(axum::http::StatusCode::CONFLICT);
}
```

Extend `rust/taskcast-server/tests/health_detail.rs`:

```rust
#[tokio::test]
async fn root_returns_server_info() {
    let server = make_server();
    let res = server.get("/").await;
    res.assert_status_ok();
    let body: serde_json::Value = res.json();
    assert_eq!(body["name"], "taskcast");
    assert_eq!(body["version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(body["apiVersion"], "v1");
    assert_eq!(body["links"]["health"], "/health");
}

#[tokio::test]
async fn health_includes_version() {
    let server = make_server();
    let res = server.get("/health").await;
    res.assert_status_ok();
    let body: serde_json::Value = res.json();
    assert_eq!(body["ok"], true);
    assert_eq!(body["name"], "taskcast");
    assert_eq!(body["version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(body["apiVersion"], "v1");
}
```

- [ ] **Step 2: Run tests and verify failure**

Run:

```bash
cd rust
cargo test -p taskcast-server --test archive_routes
cargo test -p taskcast-server --test health_detail
```

Expected: FAIL because routes and version fields are missing.

- [ ] **Step 3: Add archive request body and handlers**

In `rust/taskcast-server/src/routes/tasks.rs`, add imports:

```rust
    TaskArchive, TaskArchiveImportOptions,
```

Add request body:

```rust
#[derive(Debug, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ImportTaskArchiveBody {
    pub archive: TaskArchive,
    pub overwrite: Option<bool>,
}
```

Add handlers:

```rust
pub async fn export_task_archive(
    State(engine): State<Arc<TaskEngine>>,
    Extension(auth): Extension<AuthContext>,
    Path(task_id): Path<String>,
) -> Result<impl IntoResponse, AppError> {
    if !check_scope(&auth, taskcast_core::PermissionScope::EventHistory, Some(&task_id)) {
        return Err(AppError::Forbidden);
    }
    let archive = engine
        .export_task_archive(&task_id)
        .await
        .map_err(|err| match &err {
            EngineError::TaskNotFound(_) => AppError::NotFound(err.to_string()),
            _ => AppError::Engine(err),
        })?;
    Ok(axum::Json(archive))
}

pub async fn import_task_archive(
    State(engine): State<Arc<TaskEngine>>,
    Extension(auth): Extension<AuthContext>,
    axum::Json(body): axum::Json<ImportTaskArchiveBody>,
) -> Result<impl IntoResponse, AppError> {
    if !check_scope(&auth, taskcast_core::PermissionScope::TaskManage, None) {
        return Err(AppError::Forbidden);
    }
    let result = engine
        .import_task_archive(
            body.archive,
            Some(TaskArchiveImportOptions {
                overwrite: body.overwrite.unwrap_or(false),
            }),
        )
        .await?;
    Ok(axum::Json(serde_json::json!({
        "ok": true,
        "taskId": result.task_id,
        "eventCount": result.event_count,
        "overwritten": result.overwritten
    })))
}
```

- [ ] **Step 4: Mount Rust archive routes**

In `rust/taskcast-server/src/app.rs`, update task routes:

```rust
        .route("/{task_id}/archive", get(tasks::export_task_archive))
        .route("/import", post(tasks::import_task_archive))
```

Add server info helper functions near health:

```rust
fn server_info() -> serde_json::Value {
    serde_json::json!({
        "name": "taskcast",
        "version": env!("CARGO_PKG_VERSION"),
        "apiVersion": "v1"
    })
}

async fn root() -> impl IntoResponse {
    axum::Json(serde_json::json!({
        "name": "taskcast",
        "version": env!("CARGO_PKG_VERSION"),
        "apiVersion": "v1",
        "links": {
            "health": "/health",
            "healthDetail": "/health/detail",
            "openapi": "/openapi.json",
            "docs": "/docs"
        }
    }))
}
```

Add to `public_routes`:

```rust
        .route("/", get(root))
```

Change health:

```rust
async fn health() -> impl IntoResponse {
    axum::Json(serde_json::json!({
        "ok": true,
        "name": "taskcast",
        "version": env!("CARGO_PKG_VERSION"),
        "apiVersion": "v1"
    }))
}
```

Add fields to `health_detail` JSON:

```rust
        "name": "taskcast",
        "version": env!("CARGO_PKG_VERSION"),
        "apiVersion": "v1",
```

- [ ] **Step 5: Confirm error mapping**

In `rust/taskcast-server/src/error.rs`, ensure `EngineError::TaskConflict` maps to `409` and `EngineError::InvalidInput` maps to `400`. The current file already has this shape:

```rust
EngineError::TaskConflict(msg) => {
    (StatusCode::CONFLICT, format!("Task already exists: {msg}"))
}
EngineError::InvalidInput(msg) => (StatusCode::BAD_REQUEST, msg.clone())
```

If implementation changed the exact enum variants in Task 6, update this match so malformed archives still return `400` and conflicts still return `409`.

- [ ] **Step 6: Run Rust server tests**

Run:

```bash
cd rust
cargo test -p taskcast-server --test archive_routes
cargo test -p taskcast-server --test health_detail
```

Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add rust/taskcast-server/src/app.rs rust/taskcast-server/src/routes/tasks.rs rust/taskcast-server/src/error.rs rust/taskcast-server/tests/archive_routes.rs rust/taskcast-server/tests/health_detail.rs
git commit -m "feat(rust-server): expose task archive routes"
```

---

### Task 8: Cross-Implementation Verification

**Files:**
- Modify test files only if verification exposes behavior drift.

- [ ] **Step 1: Run focused TypeScript tests**

Run:

```bash
cd packages/core
pnpm test tests/unit/archive.test.ts tests/unit/engine-archive.test.ts
cd ../server
pnpm test tests/archive-routes.test.ts tests/health-detail.test.ts tests/openapi.test.ts
cd ../server-sdk
pnpm test tests/client.test.ts
```

Expected: all selected tests PASS.

- [ ] **Step 2: Run focused Rust tests**

Run:

```bash
cd rust
cargo test -p taskcast-core --test archive
cargo test -p taskcast-server --test archive_routes
cargo test -p taskcast-server --test health_detail
```

Expected: all selected tests PASS.

- [ ] **Step 3: Run package type checks and builds**

Run:

```bash
cd /Users/winrey/Projects/taskcast
pnpm build
pnpm lint
```

Expected: both commands PASS.

- [ ] **Step 4: Run broader test suites**

Run:

```bash
cd /Users/winrey/Projects/taskcast
pnpm test
cd rust
cargo test --workspace
```

Expected: both commands PASS. If unrelated dirty worktree changes break Rust tests, capture the exact failing command and file a scoped note before deciding whether to isolate further in a fresh worktree.

- [ ] **Step 5: Verify git scope**

Run:

```bash
git status --short
git diff --stat HEAD
```

Expected: only files touched by this plan are modified or committed. No unrelated pre-existing dirty files are staged.

- [ ] **Step 6: Final commit if verification required fixes**

If Step 4 required follow-up fixes, inspect the modified file list:

```bash
git status --short
```

Then stage only files from the task that failed. For example, if only the TypeScript server archive route test needed a fix:

```bash
git add packages/server/src/routes/tasks.ts packages/server/tests/archive-routes.test.ts
git commit -m "test: verify task archive import export"
```

If no fixes were needed, leave the index empty and do not create a commit.
