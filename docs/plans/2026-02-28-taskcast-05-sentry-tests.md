# Taskcast Phase 5: Sentry + Integration & Concurrent Tests

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** 实现 `@taskcast/sentry` 可选监控集成，编写跨层集成测试（testcontainers）和并发压力测试，确保系统在多实例场景下正确工作。

**Architecture:** Sentry 集成通过 hooks 接口注入，不侵入核心；集成测试用真实 Redis + Postgres 验证多层写入一致性；并发测试验证多订阅者竞争条件。

**Tech Stack:** @sentry/node ^8.x, testcontainers-node ^10.x, Vitest

**前置条件：** Phase 1-4 全部完成。

---

## Task 25: 创建 `@taskcast/sentry` 包

**Files:**
- Create: `packages/sentry/package.json`
- Create: `packages/sentry/tsconfig.json`
- Create: `packages/sentry/src/index.ts`
- Create: `packages/sentry/src/hooks.ts`
- Create: `packages/sentry/tests/hooks.test.ts`

**Step 1: 创建包结构**

```bash
mkdir -p packages/sentry/src packages/sentry/tests
```

`packages/sentry/package.json`:
```json
{
  "name": "@taskcast/sentry",
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
    "@taskcast/core": "workspace:*"
  },
  "peerDependencies": {
    "@sentry/node": ">=8.0.0"
  },
  "peerDependenciesMeta": {
    "@sentry/node": { "optional": true }
  },
  "devDependencies": {
    "typescript": "^5.7.0",
    "vitest": "^2.1.0",
    "@sentry/node": "^8.42.0"
  }
}
```

`packages/sentry/tsconfig.json`:
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

**Step 2: 写失败测试**

`packages/sentry/tests/hooks.test.ts`:
```typescript
import { describe, it, expect, vi } from 'vitest'
import { createSentryHooks } from '../src/hooks.js'
import type { Task, TaskError, TaskEvent } from '@taskcast/core'

const makeTask = (): Task => ({
  id: 'task-1',
  status: 'failed',
  createdAt: 1000,
  updatedAt: 2000,
  completedAt: 2000,
})

const makeError = (): TaskError => ({
  code: 'LLM_TIMEOUT',
  message: 'Model took too long',
})

const makeEvent = (): TaskEvent => ({
  id: 'evt-1',
  taskId: 'task-1',
  index: 0,
  timestamp: 1000,
  type: 'llm.delta',
  level: 'info',
  data: null,
})

describe('createSentryHooks', () => {
  it('calls captureException on task failure when enabled', () => {
    const captureException = vi.fn()
    const sentry = { captureException } as never

    const hooks = createSentryHooks(sentry, {
      captureTaskFailures: true,
    })

    hooks.onTaskFailed!(makeTask(), makeError())
    expect(captureException).toHaveBeenCalledOnce()
    const [err, opts] = captureException.mock.calls[0]!
    expect(err).toBeInstanceOf(Error)
    expect((err as Error).message).toContain('Model took too long')
    expect(opts.tags.taskId).toBe('task-1')
  })

  it('does not call captureException when captureTaskFailures is false', () => {
    const captureException = vi.fn()
    const sentry = { captureException } as never

    const hooks = createSentryHooks(sentry, {
      captureTaskFailures: false,
    })

    hooks.onTaskFailed!(makeTask(), makeError())
    expect(captureException).not.toHaveBeenCalled()
  })

  it('calls captureException on task timeout when enabled', () => {
    const captureException = vi.fn()
    const sentry = { captureException } as never

    const hooks = createSentryHooks(sentry, { captureTaskTimeouts: true })
    hooks.onTaskTimeout!(makeTask())
    expect(captureException).toHaveBeenCalledOnce()
  })

  it('calls captureException on dropped event when enabled', () => {
    const captureException = vi.fn()
    const sentry = { captureException } as never

    const hooks = createSentryHooks(sentry, { captureDroppedEvents: true })
    hooks.onEventDropped!(makeEvent(), 'redis write failed')
    expect(captureException).toHaveBeenCalledOnce()
    expect((captureException.mock.calls[0]![0] as Error).message).toContain('redis write failed')
  })

  it('calls captureException on unhandled error when enabled', () => {
    const captureException = vi.fn()
    const sentry = { captureException } as never

    const hooks = createSentryHooks(sentry, { captureUnhandledErrors: true })
    const err = new Error('Unexpected failure')
    hooks.onUnhandledError!(err, { operation: 'appendEvent', taskId: 'task-1' })
    expect(captureException).toHaveBeenCalledWith(err, expect.objectContaining({
      tags: expect.objectContaining({ operation: 'appendEvent' }),
    }))
  })

  it('enables all captures by default', () => {
    const captureException = vi.fn()
    const sentry = { captureException } as never

    const hooks = createSentryHooks(sentry) // no options = all enabled
    hooks.onTaskFailed!(makeTask(), makeError())
    expect(captureException).toHaveBeenCalled()
  })
})
```

**Step 3: 运行测试，确认失败**

```bash
pnpm --filter @taskcast/sentry install
pnpm --filter @taskcast/sentry vitest run tests/hooks.test.ts
```

Expected: FAIL。

**Step 4: 实现 Sentry hooks**

`packages/sentry/src/hooks.ts`:
```typescript
import type { TaskcastHooks, Task, TaskError, TaskEvent, ErrorContext } from '@taskcast/core'

interface SentryLike {
  captureException(err: unknown, opts?: {
    tags?: Record<string, string>
    extra?: Record<string, unknown>
  }): void
}

export interface SentryHooksOptions {
  captureTaskFailures?: boolean
  captureTaskTimeouts?: boolean
  captureUnhandledErrors?: boolean
  captureDroppedEvents?: boolean
  captureStorageErrors?: boolean
  captureBroadcastErrors?: boolean
  traceSSEConnections?: boolean
  traceEventPublish?: boolean
}

const DEFAULT_OPTIONS: Required<SentryHooksOptions> = {
  captureTaskFailures: true,
  captureTaskTimeouts: true,
  captureUnhandledErrors: true,
  captureDroppedEvents: true,
  captureStorageErrors: true,
  captureBroadcastErrors: true,
  traceSSEConnections: false,
  traceEventPublish: false,
}

export function createSentryHooks(
  sentry: SentryLike,
  opts: SentryHooksOptions = {},
): TaskcastHooks {
  const options = { ...DEFAULT_OPTIONS, ...opts }

  return {
    onTaskFailed(task: Task, error: TaskError) {
      if (!options.captureTaskFailures) return
      const err = new Error(`Task failed [${task.id}]: ${error.message}`)
      sentry.captureException(err, {
        tags: { taskId: task.id, status: task.status, errorCode: error.code ?? 'unknown' },
        extra: { params: task.params, error: task.error },
      })
    },

    onTaskTimeout(task: Task) {
      if (!options.captureTaskTimeouts) return
      const err = new Error(`Task timed out [${task.id}]`)
      sentry.captureException(err, {
        tags: { taskId: task.id, status: 'timeout' },
        extra: { params: task.params },
      })
    },

    onUnhandledError(err: unknown, context: ErrorContext) {
      if (!options.captureUnhandledErrors) return
      sentry.captureException(err, {
        tags: { operation: context.operation, ...(context.taskId ? { taskId: context.taskId } : {}) },
      })
    },

    onEventDropped(event: TaskEvent, reason: string) {
      if (!options.captureDroppedEvents) return
      const err = new Error(`Event dropped [${event.id}]: ${reason}`)
      sentry.captureException(err, {
        tags: { taskId: event.taskId, eventType: event.type },
        extra: { reason, eventId: event.id },
      })
    },

    onWebhookFailed(config, err) {
      // Webhook failures counted as dropped event captures
      if (!options.captureDroppedEvents) return
      sentry.captureException(err, {
        tags: { webhookUrl: config.url },
      })
    },
  }
}
```

`packages/sentry/src/index.ts`:
```typescript
export { createSentryHooks } from './hooks.js'
export type { SentryHooksOptions } from './hooks.js'
```

**Step 5: 运行测试，确认通过**

```bash
pnpm --filter @taskcast/sentry vitest run tests/hooks.test.ts
```

Expected: PASS。

**Step 6: Commit**

```bash
git add packages/sentry/
git commit -m "feat(sentry): add createSentryHooks for error and event monitoring"
```

---

## Task 26: 端到端集成测试（真实 Redis + Postgres）

**Files:**
- Create: `packages/core/tests/integration/engine-full.test.ts`

**Step 1: 写集成测试**

`packages/core/tests/integration/engine-full.test.ts`:
```typescript
import { describe, it, expect, beforeAll, afterAll, beforeEach } from 'vitest'
import { Redis } from 'ioredis'
import postgres from 'postgres'
import { GenericContainer, PostgresContainer, type StartedTestContainer } from 'testcontainers'
import { readFileSync } from 'fs'
import { join } from 'path'
import { TaskEngine } from '../../src/engine.js'
import { RedisBroadcastProvider } from '../../../redis/src/broadcast.js'
import { RedisShortTermStore } from '../../../redis/src/short-term.js'
import { PostgresLongTermStore } from '../../../postgres/src/long-term.js'

let redisContainer: StartedTestContainer
let pgContainer: StartedTestContainer
let engine: TaskEngine
let pubClient: Redis
let subClient: Redis
let storeClient: Redis
let sql: ReturnType<typeof postgres>

beforeAll(async () => {
  // Start Redis and Postgres in parallel
  ;[redisContainer, pgContainer] = await Promise.all([
    new GenericContainer('redis:7-alpine').withExposedPorts(6379).start(),
    new PostgresContainer('postgres:16-alpine').start(),
  ])

  const redisUrl = `redis://localhost:${redisContainer.getMappedPort(6379)}`
  pubClient = new Redis(redisUrl)
  subClient = new Redis(redisUrl)
  storeClient = new Redis(redisUrl)

  sql = postgres((pgContainer as PostgresContainer).getConnectionUri())

  // Run migration
  const migration = readFileSync(
    join(import.meta.dirname, '../../../postgres/migrations/001_initial.sql'),
    'utf8',
  )
  await sql.unsafe(migration)

  const broadcast = new RedisBroadcastProvider(pubClient, subClient)
  const shortTerm = new RedisShortTermStore(storeClient)
  const longTerm = new PostgresLongTermStore(sql)

  engine = new TaskEngine({ broadcast, shortTerm, longTerm })
}, 120000)

afterAll(async () => {
  pubClient.disconnect()
  subClient.disconnect()
  storeClient.disconnect()
  await sql.end()
  await Promise.all([redisContainer?.stop(), pgContainer?.stop()])
})

beforeEach(async () => {
  await storeClient.flushall()
  await sql`TRUNCATE taskcast_events, taskcast_tasks CASCADE`
})

describe('Full stack: task lifecycle', () => {
  it('creates task, publishes events, completes, persists to Postgres', async () => {
    const task = await engine.createTask({ type: 'llm.chat', params: { prompt: 'hello' } })
    await engine.transitionTask(task.id, 'running')

    await engine.publishEvent(task.id, {
      type: 'llm.delta',
      level: 'info',
      data: { text: 'Hello' },
      seriesId: 'msg-1',
      seriesMode: 'accumulate',
    })
    await engine.publishEvent(task.id, {
      type: 'llm.delta',
      level: 'info',
      data: { text: ' World' },
      seriesId: 'msg-1',
      seriesMode: 'accumulate',
    })

    await engine.transitionTask(task.id, 'completed', { result: { answer: 'Hello World' } })

    // Verify Redis short-term store
    const redisEvents = await engine.getEvents(task.id)
    const userEvents = redisEvents.filter((e) => e.type !== 'taskcast:status')
    expect(userEvents).toHaveLength(2)

    // Verify Postgres long-term store
    const pgTask = await sql`SELECT * FROM taskcast_tasks WHERE id = ${task.id}`
    expect(pgTask[0]?.status).toBe('completed')
    expect((pgTask[0]?.result as { answer: string })?.answer).toBe('Hello World')

    const pgEvents = await sql`SELECT * FROM taskcast_events WHERE task_id = ${task.id} ORDER BY idx`
    expect(pgEvents.length).toBeGreaterThan(0)
  })

  it('SSE broadcast reaches subscriber in real time', async () => {
    const task = await engine.createTask({})
    await engine.transitionTask(task.id, 'running')

    const received: string[] = []
    const unsub = engine.subscribe(task.id, (event) => {
      received.push(event.type)
    })

    await new Promise((r) => setTimeout(r, 50)) // wait for redis subscription

    await engine.publishEvent(task.id, { type: 'tool.call', level: 'info', data: null })
    await engine.publishEvent(task.id, { type: 'tool.result', level: 'info', data: null })

    await new Promise((r) => setTimeout(r, 100)) // wait for delivery

    expect(received).toContain('tool.call')
    expect(received).toContain('tool.result')
    unsub()
  })

  it('reconnect with since.index resumes filtered stream correctly', async () => {
    const task = await engine.createTask({})
    await engine.transitionTask(task.id, 'running')

    // Publish mixed events
    for (let i = 0; i < 6; i++) {
      await engine.publishEvent(task.id, {
        type: i % 2 === 0 ? 'llm.delta' : 'tool.call',
        level: 'info',
        data: { i },
      })
    }
    await engine.transitionTask(task.id, 'completed')

    // First "session": get filtered events (llm.* only), stop at filteredIndex=1
    const { applyFilteredIndex } = await import('../../src/filter.js')
    const allEvents = await engine.getEvents(task.id)
    const firstSession = applyFilteredIndex(allEvents, { types: ['llm.*'] })
    expect(firstSession).toHaveLength(3) // indices 0, 2, 4 → filteredIndex 0,1,2

    // Reconnect from filteredIndex=1 (last seen=1, so get from 2 onwards)
    const secondSession = applyFilteredIndex(allEvents, {
      types: ['llm.*'],
      since: { index: 1 },
    })
    expect(secondSession).toHaveLength(1) // only filteredIndex=2
    expect(secondSession[0]?.filteredIndex).toBe(2)
  })
})
```

**Step 2: 运行测试（首次运行会拉取 Docker 镜像，较慢）**

```bash
pnpm --filter @taskcast/core vitest run tests/integration/engine-full.test.ts
```

Expected: PASS（需要 Docker 运行中）。

**Step 3: Commit**

```bash
git add packages/core/tests/integration/engine-full.test.ts
git commit -m "test(core): add full-stack integration tests with real Redis and Postgres"
```

---

## Task 27: 并发压力测试

**Files:**
- Create: `packages/core/tests/integration/concurrent.test.ts`

**Step 1: 写并发测试**

`packages/core/tests/integration/concurrent.test.ts`:
```typescript
import { describe, it, expect, beforeAll, afterAll, beforeEach } from 'vitest'
import { Redis } from 'ioredis'
import { GenericContainer, type StartedTestContainer } from 'testcontainers'
import { TaskEngine } from '../../src/engine.js'
import { RedisBroadcastProvider } from '../../../redis/src/broadcast.js'
import { RedisShortTermStore } from '../../../redis/src/short-term.js'
import { MemoryShortTermStore, MemoryBroadcastProvider } from '../../src/memory-adapters.js'

function makeMemoryEngine() {
  const store = new MemoryShortTermStore()
  const broadcast = new MemoryBroadcastProvider()
  return new TaskEngine({ shortTerm: store, broadcast })
}

// ─── Concurrent subscriber tests (memory, no IO) ─────────────────────────────

describe('Concurrent subscribers - memory engine', () => {
  it('50 subscribers all receive 200 events in correct order', async () => {
    const engine = makeMemoryEngine()
    const task = await engine.createTask({})
    await engine.transitionTask(task.id, 'running')

    const SUBSCRIBER_COUNT = 50
    const EVENT_COUNT = 200

    // Set up subscribers BEFORE publishing
    const receivedBySubscriber: string[][] = Array.from({ length: SUBSCRIBER_COUNT }, () => [])
    const unsubs = receivedBySubscriber.map((arr, i) =>
      engine.subscribe(task.id, (e) => {
        if (e.type !== 'taskcast:status') arr.push(e.id)
      })
    )

    // Publish events sequentially (engine guarantees ordering)
    const publishedIds: string[] = []
    for (let i = 0; i < EVENT_COUNT; i++) {
      const event = await engine.publishEvent(task.id, {
        type: 'load.test',
        level: 'info',
        data: { seq: i },
      })
      publishedIds.push(event.id)
    }

    // Allow micro-tasks to flush
    await new Promise((r) => setTimeout(r, 10))

    for (let i = 0; i < SUBSCRIBER_COUNT; i++) {
      expect(receivedBySubscriber[i]).toHaveLength(EVENT_COUNT)
      // Order must match
      expect(receivedBySubscriber[i]).toEqual(publishedIds)
    }

    unsubs.forEach((u) => u())
  })

  it('concurrent status transitions: only one succeeds', async () => {
    const engine = makeMemoryEngine()
    const task = await engine.createTask({})
    await engine.transitionTask(task.id, 'running')

    // 20 concurrent attempts to complete the same task
    const results = await Promise.allSettled(
      Array.from({ length: 20 }, () =>
        engine.transitionTask(task.id, 'completed'),
      )
    )

    const succeeded = results.filter((r) => r.status === 'fulfilled')
    const failed = results.filter((r) => r.status === 'rejected')

    expect(succeeded).toHaveLength(1)
    expect(failed).toHaveLength(19)
  })

  it('100 tasks created concurrently all get unique IDs', async () => {
    const engine = makeMemoryEngine()
    const tasks = await Promise.all(
      Array.from({ length: 100 }, () => engine.createTask({}))
    )
    const ids = tasks.map((t) => t.id)
    const unique = new Set(ids)
    expect(unique.size).toBe(100)
  })

  it('concurrent publishEvent maintains monotonic index', async () => {
    const engine = makeMemoryEngine()
    const task = await engine.createTask({})
    await engine.transitionTask(task.id, 'running')

    // Publish 50 events concurrently
    const events = await Promise.all(
      Array.from({ length: 50 }, (_, i) =>
        engine.publishEvent(task.id, { type: 'parallel', level: 'info', data: { i } })
      )
    )

    const indices = events.map((e) => e.index).sort((a, b) => a - b)
    // Should be 0..49 (+ the status event from transition = starts at 1)
    const minIndex = Math.min(...indices)
    const maxIndex = Math.max(...indices)
    expect(maxIndex - minIndex).toBe(49) // 50 unique consecutive indices
    expect(new Set(indices).size).toBe(50) // all unique
  })
})

// ─── Redis pub/sub fan-out (requires Docker) ─────────────────────────────────

describe('Redis concurrent fan-out', () => {
  let container: StartedTestContainer
  let engine: TaskEngine
  let pub: Redis, sub: Redis, store: Redis

  beforeAll(async () => {
    container = await new GenericContainer('redis:7-alpine').withExposedPorts(6379).start()
    const url = `redis://localhost:${container.getMappedPort(6379)}`
    pub = new Redis(url)
    sub = new Redis(url)
    store = new Redis(url)
    engine = new TaskEngine({
      broadcast: new RedisBroadcastProvider(pub, sub),
      shortTerm: new RedisShortTermStore(store),
    })
  }, 60000)

  afterAll(async () => {
    pub.disconnect(); sub.disconnect(); store.disconnect()
    await container?.stop()
  })

  beforeEach(async () => {
    await store.flushall()
  })

  it('20 subscribers all receive 100 events via Redis pub/sub', async () => {
    const task = await engine.createTask({})
    await engine.transitionTask(task.id, 'running')

    await new Promise((r) => setTimeout(r, 50)) // allow subscription setup

    const SUBSCRIBER_COUNT = 20
    const EVENT_COUNT = 100

    const receivedCounts = Array(SUBSCRIBER_COUNT).fill(0)
    const unsubs = receivedCounts.map((_, i) =>
      engine.subscribe(task.id, (e) => {
        if (e.type !== 'taskcast:status') receivedCounts[i]++
      })
    )

    await new Promise((r) => setTimeout(r, 100)) // allow Redis subscriptions to register

    for (let i = 0; i < EVENT_COUNT; i++) {
      await engine.publishEvent(task.id, { type: 'fan.out', level: 'info', data: { i } })
    }

    await new Promise((r) => setTimeout(r, 500)) // allow delivery

    for (let i = 0; i < SUBSCRIBER_COUNT; i++) {
      expect(receivedCounts[i]).toBe(EVENT_COUNT)
    }
    unsubs.forEach((u) => u())
  })
})
```

**Step 2: 运行并发测试**

先运行内存部分（快速）：
```bash
pnpm --filter @taskcast/core vitest run tests/integration/concurrent.test.ts -t "memory engine"
```

Expected: PASS。

然后运行完整（含 Redis）：
```bash
pnpm --filter @taskcast/core vitest run tests/integration/concurrent.test.ts
```

Expected: PASS（需要 Docker）。

**Step 3: Commit**

```bash
git add packages/core/tests/integration/concurrent.test.ts
git commit -m "test(core): add concurrent subscriber and state transition stress tests"
```

---

## Task 28: 最终覆盖率检查与修补

**Step 1: 运行全工作区测试**

```bash
pnpm test
```

Expected: 全部 PASS。

**Step 2: 检查各包覆盖率**

```bash
pnpm --filter @taskcast/core vitest run --coverage
pnpm --filter @taskcast/server vitest run --coverage
pnpm --filter @taskcast/redis vitest run --coverage
pnpm --filter @taskcast/postgres vitest run --coverage
pnpm --filter @taskcast/server-sdk vitest run --coverage
pnpm --filter @taskcast/client vitest run --coverage
pnpm --filter @taskcast/sentry vitest run --coverage
```

Expected: 各包覆盖率 ≥ 80%，`@taskcast/core` ≥ 85%。

**Step 3: TypeScript 全量检查**

```bash
pnpm -r exec tsc --noEmit
```

Expected: 无报错。

**Step 4: 修补覆盖率不足处**

针对覆盖率报告中的未覆盖分支，补充测试用例（边界情况优先）：
- `series.ts`：`accumulate` 模式 `data` 为非对象的边界
- `filter.ts`：`since.id` 查找失败的分支
- `cleanup.ts`：`olderThanMs` 无 `completedAt` 的情况
- `auth.ts`：`publicKeyFile` 分支（可 mock `fs.readFileSync`）

**Step 5: 最终 Commit**

```bash
git add -A
git commit -m "test: achieve >85% coverage across all packages, add edge case tests"
```

---

## Phase 5 完成检查

```bash
# 完整测试套件
pnpm test

# 构建所有包
pnpm build

# 验证 CLI 可用
npx --no packages/cli/dist/index.js --help

# 覆盖率汇总
pnpm test:coverage
```

**整个项目完成！** 所有 Phase 计划文件：
- [Phase 1: Monorepo + Core](./2026-02-28-taskcast-01-monorepo-core.md)
- [Phase 2: Adapters](./2026-02-28-taskcast-02-adapters.md)
- [Phase 3: Server](./2026-02-28-taskcast-03-server.md)
- [Phase 4: SDKs + CLI](./2026-02-28-taskcast-04-sdks-cli.md)
- [Phase 5: Sentry + Tests](./2026-02-28-taskcast-05-sentry-tests.md)
