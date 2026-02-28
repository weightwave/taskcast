# Taskcast Phase 2: Storage Adapters Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** 实现 `@taskcast/redis`（BroadcastProvider + ShortTermStore）和 `@taskcast/postgres`（LongTermStore），支持多实例水平伸缩。

**Architecture:** Redis pub/sub 作为跨实例广播层，是多实例部署的关键——所有实例订阅同一 Redis channel，发布到任意实例的事件均能送达所有订阅者。Memory 适配器仅适合单进程开发/测试，不支持伸缩。

**Tech Stack:** ioredis ^5.x, postgres (sql tag) ^3.x, Vitest + testcontainers-node

**前置条件：** Phase 1 完成，`@taskcast/core` 已构建。

---

## Task 10: 创建 `@taskcast/redis` 包骨架

**Files:**
- Create: `packages/redis/package.json`
- Create: `packages/redis/tsconfig.json`
- Create: `packages/redis/vitest.config.ts`
- Create: `packages/redis/src/index.ts`

**Step 1: 创建包结构**

```bash
mkdir -p packages/redis/src packages/redis/tests
```

`packages/redis/package.json`:
```json
{
  "name": "@taskcast/redis",
  "version": "0.1.0",
  "type": "module",
  "exports": {
    ".": {
      "import": "./dist/index.js",
      "types": "./dist/index.d.ts"
    }
  },
  "scripts": {
    "build": "tsc",
    "test": "vitest run",
    "test:watch": "vitest"
  },
  "dependencies": {
    "@taskcast/core": "workspace:*",
    "ioredis": "^5.4.0"
  },
  "devDependencies": {
    "typescript": "^5.7.0",
    "vitest": "^2.1.0",
    "@vitest/coverage-v8": "^2.1.0",
    "testcontainers": "^10.13.0"
  }
}
```

`packages/redis/tsconfig.json`:
```json
{
  "extends": "../../tsconfig.base.json",
  "compilerOptions": {
    "rootDir": "src",
    "outDir": "dist"
  },
  "include": ["src"],
  "references": [{ "path": "../core" }]
}
```

`packages/redis/vitest.config.ts`:
```typescript
import { defineConfig } from 'vitest/config'

export default defineConfig({
  test: {
    include: ['tests/**/*.test.ts'],
    testTimeout: 30000, // testcontainers 需要较长超时
    coverage: {
      provider: 'v8',
      reporter: ['text', 'lcov'],
      include: ['src/**'],
      thresholds: { lines: 80, functions: 80 },
    },
  },
})
```

**Step 2: 安装依赖**

```bash
pnpm --filter @taskcast/redis install
```

**Step 3: Commit**

```bash
git add packages/redis
git commit -m "chore: scaffold @taskcast/redis package"
```

---

## Task 11: Redis BroadcastProvider

**Files:**
- Create: `packages/redis/src/broadcast.ts`
- Create: `packages/redis/tests/broadcast.test.ts`

**Step 1: 写失败测试**

`packages/redis/tests/broadcast.test.ts`:
```typescript
import { describe, it, expect, beforeAll, afterAll } from 'vitest'
import { Redis } from 'ioredis'
import { GenericContainer, type StartedTestContainer } from 'testcontainers'
import { RedisBroadcastProvider } from '../src/broadcast.js'
import type { TaskEvent } from '@taskcast/core'

let container: StartedTestContainer
let redisUrl: string

beforeAll(async () => {
  container = await new GenericContainer('redis:7-alpine')
    .withExposedPorts(6379)
    .start()
  redisUrl = `redis://localhost:${container.getMappedPort(6379)}`
}, 60000)

afterAll(async () => {
  await container?.stop()
})

const makeEvent = (): TaskEvent => ({
  id: 'evt-1',
  taskId: 'task-1',
  index: 0,
  timestamp: Date.now(),
  type: 'llm.delta',
  level: 'info',
  data: { text: 'hello' },
})

describe('RedisBroadcastProvider', () => {
  it('delivers published events to subscribers', async () => {
    const pub = new Redis(redisUrl)
    const sub = new Redis(redisUrl)
    const provider = new RedisBroadcastProvider(pub, sub)

    const received: TaskEvent[] = []
    const unsub = provider.subscribe('task-1', (e) => received.push(e))

    // wait for subscription to be ready
    await new Promise((r) => setTimeout(r, 100))

    const event = makeEvent()
    await provider.publish('task-1', event)

    await new Promise((r) => setTimeout(r, 100))
    expect(received).toHaveLength(1)
    expect(received[0]?.type).toBe('llm.delta')

    unsub()
    pub.disconnect()
    sub.disconnect()
  })

  it('multiple subscribers on same channel all receive events', async () => {
    const pub = new Redis(redisUrl)
    const sub1 = new Redis(redisUrl)
    const sub2 = new Redis(redisUrl)
    const p1 = new RedisBroadcastProvider(pub, sub1)
    const p2 = new RedisBroadcastProvider(new Redis(redisUrl), sub2)

    const r1: TaskEvent[] = []
    const r2: TaskEvent[] = []
    const u1 = p1.subscribe('task-1', (e) => r1.push(e))
    const u2 = p2.subscribe('task-1', (e) => r2.push(e))

    await new Promise((r) => setTimeout(r, 100))
    await p1.publish('task-1', makeEvent())
    await new Promise((r) => setTimeout(r, 100))

    expect(r1).toHaveLength(1)
    expect(r2).toHaveLength(1)

    u1(); u2()
    pub.disconnect(); sub1.disconnect(); sub2.disconnect()
  })

  it('unsubscribe stops delivery', async () => {
    const pub = new Redis(redisUrl)
    const sub = new Redis(redisUrl)
    const provider = new RedisBroadcastProvider(pub, sub)

    const received: TaskEvent[] = []
    const unsub = provider.subscribe('task-1', (e) => received.push(e))
    await new Promise((r) => setTimeout(r, 100))

    await provider.publish('task-1', makeEvent())
    await new Promise((r) => setTimeout(r, 100))
    unsub()

    await provider.publish('task-1', makeEvent())
    await new Promise((r) => setTimeout(r, 100))

    expect(received).toHaveLength(1)
    pub.disconnect(); sub.disconnect()
  })
})
```

**Step 2: 运行测试，确认失败**

```bash
pnpm --filter @taskcast/redis vitest run tests/broadcast.test.ts
```

Expected: FAIL — 模块未找到。

**Step 3: 实现 RedisBroadcastProvider**

`packages/redis/src/broadcast.ts`:
```typescript
import type { Redis } from 'ioredis'
import type { BroadcastProvider, TaskEvent } from '@taskcast/core'

const CHANNEL_PREFIX = 'taskcast:task:'

export class RedisBroadcastProvider implements BroadcastProvider {
  // 每个 channel 的本地 handlers，在接收到 Redis 消息后转发
  private handlers = new Map<string, Set<(event: TaskEvent) => void>>()

  constructor(
    private pub: Redis,
    private sub: Redis,
  ) {
    this.sub.on('message', (channel: string, message: string) => {
      const taskId = channel.replace(CHANNEL_PREFIX, '')
      const handlers = this.handlers.get(taskId)
      if (!handlers) return
      try {
        const event = JSON.parse(message) as TaskEvent
        for (const handler of handlers) handler(event)
      } catch {
        // malformed message, ignore
      }
    })
  }

  async publish(channel: string, event: TaskEvent): Promise<void> {
    await this.pub.publish(CHANNEL_PREFIX + channel, JSON.stringify(event))
  }

  subscribe(channel: string, handler: (event: TaskEvent) => void): () => void {
    if (!this.handlers.has(channel)) {
      this.handlers.set(channel, new Set())
      this.sub.subscribe(CHANNEL_PREFIX + channel)
    }
    this.handlers.get(channel)!.add(handler)

    return () => {
      const set = this.handlers.get(channel)
      if (!set) return
      set.delete(handler)
      if (set.size === 0) {
        this.handlers.delete(channel)
        this.sub.unsubscribe(CHANNEL_PREFIX + channel)
      }
    }
  }
}
```

**Step 4: 运行测试，确认通过**

```bash
pnpm --filter @taskcast/redis vitest run tests/broadcast.test.ts
```

Expected: PASS（testcontainers 会自动拉取 redis:7-alpine）。

**Step 5: Commit**

```bash
git add packages/redis/src/broadcast.ts packages/redis/tests/broadcast.test.ts
git commit -m "feat(redis): add RedisBroadcastProvider with pub/sub fan-out"
```

---

## Task 12: Redis ShortTermStore

**Files:**
- Create: `packages/redis/src/short-term.ts`
- Create: `packages/redis/tests/short-term.test.ts`

**Step 1: 写失败测试**

`packages/redis/tests/short-term.test.ts`:
```typescript
import { describe, it, expect, beforeAll, afterAll, beforeEach } from 'vitest'
import { Redis } from 'ioredis'
import { GenericContainer, type StartedTestContainer } from 'testcontainers'
import { RedisShortTermStore } from '../src/short-term.js'
import type { Task, TaskEvent } from '@taskcast/core'

let container: StartedTestContainer
let redis: Redis
let store: RedisShortTermStore

beforeAll(async () => {
  container = await new GenericContainer('redis:7-alpine')
    .withExposedPorts(6379)
    .start()
  redis = new Redis(`redis://localhost:${container.getMappedPort(6379)}`)
  store = new RedisShortTermStore(redis)
}, 60000)

afterAll(async () => {
  redis.disconnect()
  await container?.stop()
})

beforeEach(async () => {
  await redis.flushall()
})

const makeTask = (id = 'task-1'): Task => ({
  id,
  status: 'pending',
  createdAt: 1000,
  updatedAt: 1000,
})

const makeEvent = (index = 0): TaskEvent => ({
  id: `evt-${index}`,
  taskId: 'task-1',
  index,
  timestamp: 1000 + index,
  type: 'llm.delta',
  level: 'info',
  data: { text: `msg-${index}` },
})

describe('RedisShortTermStore - task', () => {
  it('saves and retrieves a task', async () => {
    await store.saveTask(makeTask())
    const task = await store.getTask('task-1')
    expect(task?.status).toBe('pending')
  })

  it('returns null for missing task', async () => {
    expect(await store.getTask('missing')).toBeNull()
  })

  it('overwrites existing task on save', async () => {
    await store.saveTask(makeTask())
    await store.saveTask({ ...makeTask(), status: 'running' })
    const task = await store.getTask('task-1')
    expect(task?.status).toBe('running')
  })
})

describe('RedisShortTermStore - events', () => {
  it('appends events in order', async () => {
    await store.appendEvent('task-1', makeEvent(0))
    await store.appendEvent('task-1', makeEvent(1))
    const events = await store.getEvents('task-1')
    expect(events).toHaveLength(2)
    expect(events[0]?.index).toBe(0)
    expect(events[1]?.index).toBe(1)
  })

  it('filters events by since.index', async () => {
    for (let i = 0; i < 5; i++) await store.appendEvent('task-1', makeEvent(i))
    const events = await store.getEvents('task-1', { since: { index: 2 } })
    expect(events.map((e) => e.index)).toEqual([3, 4])
  })

  it('filters events by since.timestamp', async () => {
    for (let i = 0; i < 5; i++) await store.appendEvent('task-1', makeEvent(i))
    // event timestamps are 1000, 1001, 1002, 1003, 1004
    const events = await store.getEvents('task-1', { since: { timestamp: 1002 } })
    expect(events.map((e) => e.index)).toEqual([3, 4])
  })
})

describe('RedisShortTermStore - series', () => {
  it('setSeriesLatest and getSeriesLatest roundtrip', async () => {
    const event = makeEvent()
    await store.setSeriesLatest('task-1', 's1', event)
    const latest = await store.getSeriesLatest('task-1', 's1')
    expect(latest?.id).toBe(event.id)
  })

  it('returns null for missing series', async () => {
    expect(await store.getSeriesLatest('task-1', 'missing')).toBeNull()
  })
})
```

**Step 2: 运行测试，确认失败**

```bash
pnpm --filter @taskcast/redis vitest run tests/short-term.test.ts
```

Expected: FAIL。

**Step 3: 实现 RedisShortTermStore**

`packages/redis/src/short-term.ts`:
```typescript
import type { Redis } from 'ioredis'
import type { Task, TaskEvent, ShortTermStore, EventQueryOptions } from '@taskcast/core'

const KEY = {
  task: (id: string) => `taskcast:task:${id}`,
  events: (id: string) => `taskcast:events:${id}`,
  seriesLatest: (taskId: string, seriesId: string) =>
    `taskcast:series:${taskId}:${seriesId}`,
}

export class RedisShortTermStore implements ShortTermStore {
  constructor(private redis: Redis) {}

  async saveTask(task: Task): Promise<void> {
    await this.redis.set(KEY.task(task.id), JSON.stringify(task))
  }

  async getTask(taskId: string): Promise<Task | null> {
    const raw = await this.redis.get(KEY.task(taskId))
    return raw ? (JSON.parse(raw) as Task) : null
  }

  async appendEvent(taskId: string, event: TaskEvent): Promise<void> {
    await this.redis.rpush(KEY.events(taskId), JSON.stringify(event))
  }

  async getEvents(taskId: string, opts?: EventQueryOptions): Promise<TaskEvent[]> {
    const raw = await this.redis.lrange(KEY.events(taskId), 0, -1)
    let events = raw.map((r) => JSON.parse(r) as TaskEvent)

    const since = opts?.since
    if (since?.id) {
      const idx = events.findIndex((e) => e.id === since.id)
      events = idx >= 0 ? events.slice(idx + 1) : events
    } else if (since?.index !== undefined) {
      events = events.filter((e) => e.index > since.index!)
    } else if (since?.timestamp !== undefined) {
      events = events.filter((e) => e.timestamp > since.timestamp!)
    }

    if (opts?.limit) events = events.slice(0, opts.limit)
    return events
  }

  async setTTL(taskId: string, ttlSeconds: number): Promise<void> {
    await this.redis.expire(KEY.task(taskId), ttlSeconds)
    await this.redis.expire(KEY.events(taskId), ttlSeconds)
  }

  async getSeriesLatest(taskId: string, seriesId: string): Promise<TaskEvent | null> {
    const raw = await this.redis.get(KEY.seriesLatest(taskId, seriesId))
    return raw ? (JSON.parse(raw) as TaskEvent) : null
  }

  async setSeriesLatest(taskId: string, seriesId: string, event: TaskEvent): Promise<void> {
    await this.redis.set(KEY.seriesLatest(taskId, seriesId), JSON.stringify(event))
  }

  async replaceLastSeriesEvent(taskId: string, seriesId: string, event: TaskEvent): Promise<void> {
    const prev = await this.getSeriesLatest(taskId, seriesId)
    if (prev) {
      // Find and replace the previous event in the list
      const raw = await this.redis.lrange(KEY.events(taskId), 0, -1)
      const idx = raw.findLastIndex((r) => {
        try { return (JSON.parse(r) as TaskEvent).id === prev.id } catch { return false }
      })
      if (idx >= 0) {
        await this.redis.lset(KEY.events(taskId), idx, JSON.stringify(event))
      }
    } else {
      await this.appendEvent(taskId, event)
    }
    await this.setSeriesLatest(taskId, seriesId, event)
  }
}
```

**Step 4: 运行测试，确认通过**

```bash
pnpm --filter @taskcast/redis vitest run tests/short-term.test.ts
```

Expected: PASS。

**Step 5: 创建 index.ts**

`packages/redis/src/index.ts`:
```typescript
export { RedisBroadcastProvider } from './broadcast.js'
export { RedisShortTermStore } from './short-term.js'

import type { Redis } from 'ioredis'
import { RedisBroadcastProvider } from './broadcast.js'
import { RedisShortTermStore } from './short-term.js'

export interface RedisAdapterOptions {
  url?: string
  client?: Redis
}

/**
 * Convenience factory: creates a Redis instance configured for both
 * broadcast and short-term store.
 *
 * NOTE: Requires two separate Redis connections (pub and sub cannot share
 * a connection in subscribe mode).
 */
export function createRedisAdapters(pubClient: Redis, subClient: Redis, storeClient: Redis) {
  return {
    broadcast: new RedisBroadcastProvider(pubClient, subClient),
    shortTerm: new RedisShortTermStore(storeClient),
  }
}
```

**Step 6: Commit**

```bash
git add packages/redis/src/ packages/redis/tests/
git commit -m "feat(redis): add RedisShortTermStore with event ordering and series support"
```

---

## Task 13: 创建 `@taskcast/postgres` 包骨架

**Files:**
- Create: `packages/postgres/package.json`
- Create: `packages/postgres/tsconfig.json`
- Create: `packages/postgres/vitest.config.ts`
- Create: `packages/postgres/src/index.ts`

**Step 1: 创建包结构**

```bash
mkdir -p packages/postgres/src packages/postgres/tests packages/postgres/migrations
```

`packages/postgres/package.json`:
```json
{
  "name": "@taskcast/postgres",
  "version": "0.1.0",
  "type": "module",
  "exports": {
    ".": {
      "import": "./dist/index.js",
      "types": "./dist/index.d.ts"
    }
  },
  "scripts": {
    "build": "tsc",
    "test": "vitest run",
    "test:watch": "vitest"
  },
  "dependencies": {
    "@taskcast/core": "workspace:*",
    "postgres": "^3.4.5"
  },
  "devDependencies": {
    "typescript": "^5.7.0",
    "vitest": "^2.1.0",
    "@vitest/coverage-v8": "^2.1.0",
    "testcontainers": "^10.13.0"
  }
}
```

`packages/postgres/tsconfig.json`:
```json
{
  "extends": "../../tsconfig.base.json",
  "compilerOptions": {
    "rootDir": "src",
    "outDir": "dist"
  },
  "include": ["src"],
  "references": [{ "path": "../core" }]
}
```

`packages/postgres/vitest.config.ts`:
```typescript
import { defineConfig } from 'vitest/config'

export default defineConfig({
  test: {
    include: ['tests/**/*.test.ts'],
    testTimeout: 60000,
    coverage: {
      provider: 'v8',
      reporter: ['text', 'lcov'],
      include: ['src/**'],
      thresholds: { lines: 80, functions: 80 },
    },
  },
})
```

**Step 2: 创建 SQL Migration**

`packages/postgres/migrations/001_initial.sql`:
```sql
CREATE TABLE IF NOT EXISTS taskcast_tasks (
  id TEXT PRIMARY KEY,
  type TEXT,
  status TEXT NOT NULL,
  params JSONB,
  result JSONB,
  error JSONB,
  metadata JSONB,
  auth_config JSONB,
  webhooks JSONB,
  cleanup JSONB,
  created_at BIGINT NOT NULL,
  updated_at BIGINT NOT NULL,
  completed_at BIGINT,
  ttl INTEGER
);

CREATE TABLE IF NOT EXISTS taskcast_events (
  id TEXT PRIMARY KEY,
  task_id TEXT NOT NULL REFERENCES taskcast_tasks(id) ON DELETE CASCADE,
  idx INTEGER NOT NULL,
  timestamp BIGINT NOT NULL,
  type TEXT NOT NULL,
  level TEXT NOT NULL,
  data JSONB,
  series_id TEXT,
  series_mode TEXT,
  UNIQUE(task_id, idx)
);

CREATE INDEX IF NOT EXISTS taskcast_events_task_id_idx ON taskcast_events(task_id, idx);
CREATE INDEX IF NOT EXISTS taskcast_events_task_id_timestamp ON taskcast_events(task_id, timestamp);
```

**Step 3: Commit**

```bash
git add packages/postgres
git commit -m "chore: scaffold @taskcast/postgres package with SQL migration"
```

---

## Task 14: PostgreSQL LongTermStore

**Files:**
- Create: `packages/postgres/src/long-term.ts`
- Create: `packages/postgres/src/index.ts`
- Create: `packages/postgres/tests/long-term.test.ts`

**Step 1: 写失败测试**

`packages/postgres/tests/long-term.test.ts`:
```typescript
import { describe, it, expect, beforeAll, afterAll, beforeEach } from 'vitest'
import postgres from 'postgres'
import { PostgresContainer, type StartedPostgreSqlContainer } from 'testcontainers'
import { PostgresLongTermStore } from '../src/long-term.js'
import { readFileSync } from 'fs'
import { join } from 'path'
import type { Task, TaskEvent } from '@taskcast/core'

let container: StartedPostgreSqlContainer
let sql: ReturnType<typeof postgres>
let store: PostgresLongTermStore

beforeAll(async () => {
  container = await new PostgresContainer('postgres:16-alpine').start()
  sql = postgres(container.getConnectionUri())
  store = new PostgresLongTermStore(sql)

  // Run migration
  const migration = readFileSync(
    join(import.meta.dirname, '../migrations/001_initial.sql'),
    'utf8'
  )
  await sql.unsafe(migration)
}, 120000)

afterAll(async () => {
  await sql.end()
  await container?.stop()
})

beforeEach(async () => {
  await sql`TRUNCATE taskcast_events, taskcast_tasks CASCADE`
})

const makeTask = (id = 'task-1'): Task => ({
  id,
  status: 'pending',
  params: { prompt: 'hello' },
  createdAt: 1000,
  updatedAt: 1000,
})

const makeEvent = (taskId = 'task-1', index = 0): TaskEvent => ({
  id: `evt-${taskId}-${index}`,
  taskId,
  index,
  timestamp: 1000 + index,
  type: 'llm.delta',
  level: 'info',
  data: { text: `msg-${index}` },
})

describe('PostgresLongTermStore - tasks', () => {
  it('saves and retrieves a task', async () => {
    await store.saveTask(makeTask())
    const task = await store.getTask('task-1')
    expect(task?.status).toBe('pending')
    expect(task?.params).toEqual({ prompt: 'hello' })
  })

  it('returns null for missing task', async () => {
    expect(await store.getTask('missing')).toBeNull()
  })

  it('upserts task on conflict', async () => {
    await store.saveTask(makeTask())
    await store.saveTask({ ...makeTask(), status: 'running' })
    const task = await store.getTask('task-1')
    expect(task?.status).toBe('running')
  })
})

describe('PostgresLongTermStore - events', () => {
  it('saves and retrieves events in order', async () => {
    await store.saveTask(makeTask())
    await store.saveEvent(makeEvent('task-1', 0))
    await store.saveEvent(makeEvent('task-1', 1))
    const events = await store.getEvents('task-1')
    expect(events).toHaveLength(2)
    expect(events[0]?.index).toBe(0)
    expect(events[1]?.index).toBe(1)
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
    const events = await store.getEvents('task-1', { since: { timestamp: 1002 } })
    expect(events.map((e) => e.index)).toEqual([3, 4])
  })

  it('ignores duplicate event id (upsert)', async () => {
    await store.saveTask(makeTask())
    const event = makeEvent('task-1', 0)
    await store.saveEvent(event)
    await store.saveEvent(event) // should not throw
    const events = await store.getEvents('task-1')
    expect(events).toHaveLength(1)
  })
})
```

**Step 2: 运行测试，确认失败**

```bash
pnpm --filter @taskcast/postgres vitest run tests/long-term.test.ts
```

Expected: FAIL。

**Step 3: 实现 PostgresLongTermStore**

`packages/postgres/src/long-term.ts`:
```typescript
import type postgres from 'postgres'
import type { Task, TaskEvent, LongTermStore, EventQueryOptions } from '@taskcast/core'

export class PostgresLongTermStore implements LongTermStore {
  constructor(private sql: ReturnType<typeof postgres>) {}

  async saveTask(task: Task): Promise<void> {
    await this.sql`
      INSERT INTO taskcast_tasks (
        id, type, status, params, result, error, metadata,
        auth_config, webhooks, cleanup, created_at, updated_at, completed_at, ttl
      ) VALUES (
        ${task.id}, ${task.type ?? null}, ${task.status},
        ${task.params ? this.sql.json(task.params) : null},
        ${task.result ? this.sql.json(task.result) : null},
        ${task.error ? this.sql.json(task.error) : null},
        ${task.metadata ? this.sql.json(task.metadata) : null},
        ${task.authConfig ? this.sql.json(task.authConfig) : null},
        ${task.webhooks ? this.sql.json(task.webhooks) : null},
        ${task.cleanup ? this.sql.json(task.cleanup) : null},
        ${task.createdAt}, ${task.updatedAt},
        ${task.completedAt ?? null}, ${task.ttl ?? null}
      )
      ON CONFLICT (id) DO UPDATE SET
        status = EXCLUDED.status,
        result = EXCLUDED.result,
        error = EXCLUDED.error,
        metadata = EXCLUDED.metadata,
        updated_at = EXCLUDED.updated_at,
        completed_at = EXCLUDED.completed_at
    `
  }

  async getTask(taskId: string): Promise<Task | null> {
    const rows = await this.sql`
      SELECT * FROM taskcast_tasks WHERE id = ${taskId}
    `
    const row = rows[0]
    if (!row) return null
    return this._rowToTask(row)
  }

  async saveEvent(event: TaskEvent): Promise<void> {
    await this.sql`
      INSERT INTO taskcast_events (
        id, task_id, idx, timestamp, type, level, data, series_id, series_mode
      ) VALUES (
        ${event.id}, ${event.taskId}, ${event.index}, ${event.timestamp},
        ${event.type}, ${event.level},
        ${event.data ? this.sql.json(event.data as Record<string, unknown>) : null},
        ${event.seriesId ?? null}, ${event.seriesMode ?? null}
      )
      ON CONFLICT (id) DO NOTHING
    `
  }

  async getEvents(taskId: string, opts?: EventQueryOptions): Promise<TaskEvent[]> {
    const since = opts?.since

    let rows
    if (since?.id) {
      const anchor = await this.sql`
        SELECT idx FROM taskcast_events WHERE id = ${since.id}
      `
      const anchorIdx = anchor[0]?.idx ?? -1
      rows = await this.sql`
        SELECT * FROM taskcast_events
        WHERE task_id = ${taskId} AND idx > ${anchorIdx}
        ORDER BY idx ASC
        ${opts?.limit ? this.sql`LIMIT ${opts.limit}` : this.sql``}
      `
    } else if (since?.index !== undefined) {
      rows = await this.sql`
        SELECT * FROM taskcast_events
        WHERE task_id = ${taskId} AND idx > ${since.index}
        ORDER BY idx ASC
        ${opts?.limit ? this.sql`LIMIT ${opts.limit}` : this.sql``}
      `
    } else if (since?.timestamp !== undefined) {
      rows = await this.sql`
        SELECT * FROM taskcast_events
        WHERE task_id = ${taskId} AND timestamp > ${since.timestamp}
        ORDER BY idx ASC
        ${opts?.limit ? this.sql`LIMIT ${opts.limit}` : this.sql``}
      `
    } else {
      rows = await this.sql`
        SELECT * FROM taskcast_events
        WHERE task_id = ${taskId}
        ORDER BY idx ASC
        ${opts?.limit ? this.sql`LIMIT ${opts.limit}` : this.sql``}
      `
    }

    return rows.map((r) => this._rowToEvent(r))
  }

  private _rowToTask(row: Record<string, unknown>): Task {
    return {
      id: row['id'] as string,
      type: (row['type'] as string | null) ?? undefined,
      status: row['status'] as Task['status'],
      params: (row['params'] as Record<string, unknown> | null) ?? undefined,
      result: (row['result'] as Record<string, unknown> | null) ?? undefined,
      error: (row['error'] as Task['error'] | null) ?? undefined,
      metadata: (row['metadata'] as Record<string, unknown> | null) ?? undefined,
      authConfig: (row['auth_config'] as Task['authConfig'] | null) ?? undefined,
      webhooks: (row['webhooks'] as Task['webhooks'] | null) ?? undefined,
      cleanup: (row['cleanup'] as Task['cleanup'] | null) ?? undefined,
      createdAt: row['created_at'] as number,
      updatedAt: row['updated_at'] as number,
      completedAt: (row['completed_at'] as number | null) ?? undefined,
      ttl: (row['ttl'] as number | null) ?? undefined,
    }
  }

  private _rowToEvent(row: Record<string, unknown>): TaskEvent {
    return {
      id: row['id'] as string,
      taskId: row['task_id'] as string,
      index: row['idx'] as number,
      timestamp: row['timestamp'] as number,
      type: row['type'] as string,
      level: row['level'] as TaskEvent['level'],
      data: row['data'] ?? null,
      seriesId: (row['series_id'] as string | null) ?? undefined,
      seriesMode: (row['series_mode'] as TaskEvent['seriesMode'] | null) ?? undefined,
    }
  }
}
```

`packages/postgres/src/index.ts`:
```typescript
export { PostgresLongTermStore } from './long-term.js'

import postgres from 'postgres'
import { PostgresLongTermStore } from './long-term.js'

export interface PostgresAdapterOptions {
  url: string
  ssl?: boolean
}

export function createPostgresAdapter(options: PostgresAdapterOptions): PostgresLongTermStore {
  const sql = postgres(options.url, { ssl: options.ssl ? 'require' : false })
  return new PostgresLongTermStore(sql)
}
```

**Step 4: 运行测试，确认通过**

```bash
pnpm --filter @taskcast/postgres vitest run tests/long-term.test.ts
```

Expected: PASS。

**Step 5: Commit**

```bash
git add packages/postgres/src/ packages/postgres/tests/ packages/postgres/migrations/
git commit -m "feat(postgres): add PostgresLongTermStore with upsert and event pagination"
```

---

## Phase 2 完成检查

```bash
# 运行所有适配器测试
pnpm --filter "@taskcast/redis" vitest run
pnpm --filter "@taskcast/postgres" vitest run

# TypeScript 检查
pnpm --filter "@taskcast/redis" exec tsc --noEmit
pnpm --filter "@taskcast/postgres" exec tsc --noEmit
```

**伸缩性说明：**
- `RedisBroadcastProvider`：多实例共享同一 Redis，任何实例发布的事件通过 pub/sub 广播到所有实例的订阅者，天然支持水平伸缩。
- `RedisShortTermStore`：共享 Redis 存储，所有实例读写同一数据。
- `MemoryBroadcastProvider` / `MemoryShortTermStore`：进程内存，**不支持多实例**，仅用于单机开发/测试。

**下一步：** 继续 [Phase 3: Server](./2026-02-28-taskcast-03-server.md)
