# Taskcast Phase 1: Monorepo + Core Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** 搭建 pnpm monorepo，实现 `@taskcast/core` 所有内部逻辑（状态机、过滤、序列合并、清理规则、内存适配器、TaskEngine）。

**Architecture:** SDK-First。核心引擎无任何 HTTP/基础设施依赖，所有 IO 通过抽象接口注入，单元测试使用内存适配器。

**Tech Stack:** TypeScript 5.x, pnpm 9+, Vitest 2.x, ulidx (ULID), zod 3.x

---

## Task 1: Monorepo 初始化

**Files:**
- Create: `pnpm-workspace.yaml`
- Create: `package.json`
- Create: `tsconfig.base.json`
- Create: `vitest.workspace.ts`
- Create: `.gitignore`
- Create: `.npmrc`

**Step 1: 创建根目录配置文件**

`pnpm-workspace.yaml`:
```yaml
packages:
  - 'packages/*'
```

`package.json`:
```json
{
  "name": "taskcast",
  "private": true,
  "scripts": {
    "test": "vitest run",
    "test:watch": "vitest",
    "test:coverage": "vitest run --coverage",
    "build": "pnpm -r build",
    "lint": "tsc -b"
  },
  "devDependencies": {
    "typescript": "^5.7.0",
    "vitest": "^2.1.0",
    "@vitest/coverage-v8": "^2.1.0"
  }
}
```

`tsconfig.base.json`:
```json
{
  "compilerOptions": {
    "target": "ES2022",
    "module": "NodeNext",
    "moduleResolution": "NodeNext",
    "strict": true,
    "exactOptionalPropertyTypes": true,
    "noUncheckedIndexedAccess": true,
    "declaration": true,
    "declarationMap": true,
    "sourceMap": true,
    "outDir": "dist",
    "rootDir": "src"
  }
}
```

`vitest.workspace.ts`:
```typescript
import { defineWorkspace } from 'vitest/config'

export default defineWorkspace([
  'packages/*/vitest.config.ts',
])
```

`.gitignore`:
```
node_modules/
dist/
.env
*.env.local
coverage/
```

`.npmrc`:
```
shamefully-hoist=false
strict-peer-dependencies=false
```

**Step 2: 安装根依赖**

```bash
pnpm install
```

Expected: 创建 `pnpm-lock.yaml`，无报错。

**Step 3: Commit**

```bash
git init
git add .
git commit -m "chore: init pnpm monorepo"
```

---

## Task 2: 创建 `@taskcast/core` 包骨架

**Files:**
- Create: `packages/core/package.json`
- Create: `packages/core/tsconfig.json`
- Create: `packages/core/vitest.config.ts`
- Create: `packages/core/src/index.ts`

**Step 1: 创建包结构**

```bash
mkdir -p packages/core/src packages/core/tests/unit packages/core/tests/integration
```

`packages/core/package.json`:
```json
{
  "name": "@taskcast/core",
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
    "ulidx": "^2.3.0",
    "zod": "^3.23.0"
  },
  "devDependencies": {
    "typescript": "^5.7.0",
    "vitest": "^2.1.0",
    "@vitest/coverage-v8": "^2.1.0"
  }
}
```

`packages/core/tsconfig.json`:
```json
{
  "extends": "../../tsconfig.base.json",
  "compilerOptions": {
    "rootDir": "src",
    "outDir": "dist"
  },
  "include": ["src"]
}
```

`packages/core/vitest.config.ts`:
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

`packages/core/src/index.ts`:
```typescript
// 占位，后续各模块写完后在此导出
export const VERSION = '0.1.0'
```

**Step 2: 安装依赖**

```bash
pnpm --filter @taskcast/core install
```

**Step 3: Commit**

```bash
git add packages/core
git commit -m "chore: scaffold @taskcast/core package"
```

---

## Task 3: Core 类型定义

**Files:**
- Create: `packages/core/src/types.ts`

**Step 1: 写类型文件**

`packages/core/src/types.ts`:
```typescript
// ─── Task ───────────────────────────────────────────────────────────────────

export type TaskStatus =
  | 'pending'
  | 'running'
  | 'completed'
  | 'failed'
  | 'timeout'
  | 'cancelled'

export interface TaskError {
  code?: string
  message: string
  details?: Record<string, unknown>
}

export interface TaskAuthConfig {
  rules: Array<{
    match: { scope: PermissionScope[] }
    require: {
      claims?: Record<string, unknown>
      sub?: string[]
    }
  }>
}

export interface WebhookConfig {
  url: string
  filter?: SubscribeFilter
  secret?: string
  wrap?: boolean
  retry?: RetryConfig
}

export interface RetryConfig {
  retries: number
  backoff: 'fixed' | 'exponential' | 'linear'
  initialDelayMs: number
  maxDelayMs: number
  timeoutMs: number
}

export type SeriesMode = 'keep-all' | 'accumulate' | 'latest'

export type Level = 'debug' | 'info' | 'warn' | 'error'

export type PermissionScope =
  | 'task:create'
  | 'task:manage'
  | 'event:publish'
  | 'event:subscribe'
  | 'event:history'
  | 'webhook:create'
  | '*'

export interface CleanupRule {
  name?: string
  match?: {
    taskTypes?: string[]
    status?: TaskStatus[]
  }
  trigger: {
    afterMs?: number
  }
  target: 'all' | 'events' | 'task'
  eventFilter?: {
    types?: string[]
    levels?: Level[]
    olderThanMs?: number
    seriesMode?: SeriesMode[]
  }
}

export interface Task {
  id: string
  type?: string
  status: TaskStatus
  params?: Record<string, unknown>
  result?: Record<string, unknown>
  error?: TaskError
  metadata?: Record<string, unknown>
  createdAt: number
  updatedAt: number
  completedAt?: number
  ttl?: number
  authConfig?: TaskAuthConfig
  webhooks?: WebhookConfig[]
  cleanup?: { rules: CleanupRule[] }
}

// ─── Events ─────────────────────────────────────────────────────────────────

export interface TaskEvent {
  id: string
  taskId: string
  index: number
  timestamp: number
  type: string
  level: Level
  data: unknown
  seriesId?: string
  seriesMode?: SeriesMode
}

export interface SSEEnvelope {
  filteredIndex: number
  rawIndex: number
  eventId: string
  taskId: string
  type: string
  timestamp: number
  level: Level
  data: unknown
  seriesId?: string
  seriesMode?: SeriesMode
}

// ─── Subscription ────────────────────────────────────────────────────────────

export interface SinceCursor {
  id?: string
  index?: number
  timestamp?: number
}

export interface SubscribeFilter {
  since?: SinceCursor
  types?: string[]
  levels?: Level[]
  includeStatus?: boolean
  wrap?: boolean
}

export interface EventQueryOptions {
  since?: SinceCursor
  limit?: number
}

// ─── Storage Interfaces ──────────────────────────────────────────────────────

export interface BroadcastProvider {
  publish(channel: string, event: TaskEvent): Promise<void>
  subscribe(channel: string, handler: (event: TaskEvent) => void): () => void
}

export interface ShortTermStore {
  saveTask(task: Task): Promise<void>
  getTask(taskId: string): Promise<Task | null>
  appendEvent(taskId: string, event: TaskEvent): Promise<void>
  getEvents(taskId: string, opts?: EventQueryOptions): Promise<TaskEvent[]>
  setTTL(taskId: string, ttlSeconds: number): Promise<void>
  getSeriesLatest(taskId: string, seriesId: string): Promise<TaskEvent | null>
  setSeriesLatest(taskId: string, seriesId: string, event: TaskEvent): Promise<void>
  replaceLastSeriesEvent(taskId: string, seriesId: string, event: TaskEvent): Promise<void>
}

export interface LongTermStore {
  saveTask(task: Task): Promise<void>
  getTask(taskId: string): Promise<Task | null>
  saveEvent(event: TaskEvent): Promise<void>
  getEvents(taskId: string, opts?: EventQueryOptions): Promise<TaskEvent[]>
}

// ─── Hooks ───────────────────────────────────────────────────────────────────

export interface ErrorContext {
  operation: string
  taskId?: string
}

export interface TaskcastHooks {
  onTaskFailed?(task: Task, error: TaskError): void
  onTaskTimeout?(task: Task): void
  onUnhandledError?(err: unknown, context: ErrorContext): void
  onEventDropped?(event: TaskEvent, reason: string): void
  onWebhookFailed?(config: WebhookConfig, err: unknown): void
  onSSEConnect?(taskId: string, clientId: string): void
  onSSEDisconnect?(taskId: string, clientId: string, duration: number): void
}
```

**Step 2: 确认 TypeScript 无报错**

```bash
pnpm --filter @taskcast/core exec tsc --noEmit
```

Expected: 无输出（无报错）。

**Step 3: Commit**

```bash
git add packages/core/src/types.ts
git commit -m "feat(core): add all TypeScript type definitions"
```

---

## Task 4: 任务状态机

**Files:**
- Create: `packages/core/src/state-machine.ts`
- Create: `packages/core/tests/unit/state-machine.test.ts`

**Step 1: 写失败测试**

`packages/core/tests/unit/state-machine.test.ts`:
```typescript
import { describe, it, expect } from 'vitest'
import { canTransition, applyTransition, TERMINAL_STATUSES } from '../../src/state-machine.js'

describe('canTransition', () => {
  it('allows pending → running', () => {
    expect(canTransition('pending', 'running')).toBe(true)
  })

  it('allows running → completed', () => {
    expect(canTransition('running', 'completed')).toBe(true)
  })

  it('allows running → failed', () => {
    expect(canTransition('running', 'failed')).toBe(true)
  })

  it('allows running → timeout', () => {
    expect(canTransition('running', 'timeout')).toBe(true)
  })

  it('allows pending → cancelled', () => {
    expect(canTransition('pending', 'cancelled')).toBe(true)
  })

  it('allows running → cancelled', () => {
    expect(canTransition('running', 'cancelled')).toBe(true)
  })

  it('rejects completed → running (terminal state)', () => {
    expect(canTransition('completed', 'running')).toBe(false)
  })

  it('rejects failed → running (terminal state)', () => {
    expect(canTransition('failed', 'running')).toBe(false)
  })

  it('rejects pending → completed (must go through running)', () => {
    expect(canTransition('pending', 'completed')).toBe(false)
  })

  it('rejects same-state transition', () => {
    expect(canTransition('running', 'running')).toBe(false)
  })
})

describe('TERMINAL_STATUSES', () => {
  it('includes completed, failed, timeout, cancelled', () => {
    expect(TERMINAL_STATUSES).toContain('completed')
    expect(TERMINAL_STATUSES).toContain('failed')
    expect(TERMINAL_STATUSES).toContain('timeout')
    expect(TERMINAL_STATUSES).toContain('cancelled')
    expect(TERMINAL_STATUSES).not.toContain('pending')
    expect(TERMINAL_STATUSES).not.toContain('running')
  })
})

describe('applyTransition', () => {
  it('throws on invalid transition', () => {
    expect(() => applyTransition('completed', 'running')).toThrowError(/invalid transition/i)
  })

  it('returns new status on valid transition', () => {
    expect(applyTransition('pending', 'running')).toBe('running')
  })
})
```

**Step 2: 运行测试，确认失败**

```bash
pnpm --filter @taskcast/core vitest run tests/unit/state-machine.test.ts
```

Expected: FAIL — "Cannot find module '../../src/state-machine.js'"

**Step 3: 实现状态机**

`packages/core/src/state-machine.ts`:
```typescript
import type { TaskStatus } from './types.js'

export const TERMINAL_STATUSES: readonly TaskStatus[] = [
  'completed',
  'failed',
  'timeout',
  'cancelled',
] as const

// 合法的状态转换表
const ALLOWED_TRANSITIONS: Record<TaskStatus, TaskStatus[]> = {
  pending: ['running', 'cancelled'],
  running: ['completed', 'failed', 'timeout', 'cancelled'],
  completed: [],
  failed: [],
  timeout: [],
  cancelled: [],
}

export function canTransition(from: TaskStatus, to: TaskStatus): boolean {
  if (from === to) return false
  return ALLOWED_TRANSITIONS[from]?.includes(to) ?? false
}

export function applyTransition(from: TaskStatus, to: TaskStatus): TaskStatus {
  if (!canTransition(from, to)) {
    throw new Error(`Invalid transition: ${from} → ${to}`)
  }
  return to
}

export function isTerminal(status: TaskStatus): boolean {
  return TERMINAL_STATUSES.includes(status)
}
```

**Step 4: 运行测试，确认通过**

```bash
pnpm --filter @taskcast/core vitest run tests/unit/state-machine.test.ts
```

Expected: PASS — 所有用例通过。

**Step 5: Commit**

```bash
git add packages/core/src/state-machine.ts packages/core/tests/unit/state-machine.test.ts
git commit -m "feat(core): add task state machine with transition validation"
```

---

## Task 5: Wildcard 事件过滤器

**Files:**
- Create: `packages/core/src/filter.ts`
- Create: `packages/core/tests/unit/filter.test.ts`

**Step 1: 写失败测试**

`packages/core/tests/unit/filter.test.ts`:
```typescript
import { describe, it, expect } from 'vitest'
import { matchesType, matchesFilter, applyFilteredIndex } from '../../src/filter.js'
import type { TaskEvent, SubscribeFilter } from '../../src/types.js'

const makeEvent = (overrides: Partial<TaskEvent> = {}): TaskEvent => ({
  id: 'test-id',
  taskId: 'task-1',
  index: 0,
  timestamp: 1000,
  type: 'llm.delta',
  level: 'info',
  data: null,
  ...overrides,
})

describe('matchesType', () => {
  it('matches exact type', () => {
    expect(matchesType('llm.delta', ['llm.delta'])).toBe(true)
  })

  it('matches wildcard prefix', () => {
    expect(matchesType('llm.delta', ['llm.*'])).toBe(true)
  })

  it('matches global wildcard', () => {
    expect(matchesType('anything', ['*'])).toBe(true)
  })

  it('does not match unrelated type', () => {
    expect(matchesType('tool.call', ['llm.*'])).toBe(false)
  })

  it('matches any pattern in array', () => {
    expect(matchesType('tool.call', ['llm.*', 'tool.*'])).toBe(true)
  })

  it('empty patterns array matches nothing', () => {
    expect(matchesType('llm.delta', [])).toBe(false)
  })

  it('undefined patterns matches everything', () => {
    expect(matchesType('llm.delta', undefined)).toBe(true)
  })
})

describe('matchesFilter', () => {
  it('passes event with no filter', () => {
    expect(matchesFilter(makeEvent(), {})).toBe(true)
  })

  it('filters by level', () => {
    expect(matchesFilter(makeEvent({ level: 'debug' }), { levels: ['info', 'warn'] })).toBe(false)
    expect(matchesFilter(makeEvent({ level: 'info' }), { levels: ['info', 'warn'] })).toBe(true)
  })

  it('filters taskcast.status when includeStatus=false', () => {
    const statusEvent = makeEvent({ type: 'taskcast:status' })
    expect(matchesFilter(statusEvent, { includeStatus: false })).toBe(false)
    expect(matchesFilter(statusEvent, { includeStatus: true })).toBe(true)
    expect(matchesFilter(statusEvent, {})).toBe(true) // default: include
  })

  it('filters by type with wildcard', () => {
    expect(matchesFilter(makeEvent({ type: 'llm.delta' }), { types: ['tool.*'] })).toBe(false)
    expect(matchesFilter(makeEvent({ type: 'tool.call' }), { types: ['tool.*'] })).toBe(true)
  })
})

describe('applyFilteredIndex', () => {
  it('assigns sequential filteredIndex to matching events', () => {
    const events = [
      makeEvent({ type: 'llm.delta', index: 0 }),
      makeEvent({ type: 'tool.call', index: 1 }),
      makeEvent({ type: 'llm.delta', index: 2 }),
      makeEvent({ type: 'llm.delta', index: 3 }),
    ]
    const filter: SubscribeFilter = { types: ['llm.*'] }
    const result = applyFilteredIndex(events, filter)
    expect(result).toHaveLength(3)
    expect(result[0]?.filteredIndex).toBe(0)
    expect(result[1]?.filteredIndex).toBe(1)
    expect(result[2]?.filteredIndex).toBe(2)
  })

  it('respects since.index (skips first N+1 filtered events)', () => {
    const events = [
      makeEvent({ type: 'llm.delta', index: 0 }),
      makeEvent({ type: 'llm.delta', index: 1 }),
      makeEvent({ type: 'llm.delta', index: 2 }),
    ]
    const filter: SubscribeFilter = { types: ['llm.*'], since: { index: 1 } }
    const result = applyFilteredIndex(events, filter)
    // since.index=1 → skip first 2 (index 0,1), return from filteredIndex 2
    expect(result).toHaveLength(1)
    expect(result[0]?.filteredIndex).toBe(2)
  })

  it('preserves rawIndex', () => {
    const events = [
      makeEvent({ type: 'tool.call', index: 5 }),
      makeEvent({ type: 'llm.delta', index: 6 }),
    ]
    const result = applyFilteredIndex(events, { types: ['llm.*'] })
    expect(result[0]?.rawIndex).toBe(6)
    expect(result[0]?.filteredIndex).toBe(0)
  })
})
```

**Step 2: 运行测试，确认失败**

```bash
pnpm --filter @taskcast/core vitest run tests/unit/filter.test.ts
```

Expected: FAIL — 模块未找到。

**Step 3: 实现过滤器**

`packages/core/src/filter.ts`:
```typescript
import type { TaskEvent, SubscribeFilter, Level } from './types.js'

export interface FilteredEvent {
  filteredIndex: number
  rawIndex: number
  event: TaskEvent
}

export function matchesType(type: string, patterns: string[] | undefined): boolean {
  if (patterns === undefined) return true
  if (patterns.length === 0) return false
  return patterns.some((pattern) => {
    if (pattern === '*') return true
    if (pattern.endsWith('.*')) {
      const prefix = pattern.slice(0, -2)
      return type === prefix || type.startsWith(prefix + '.')
    }
    return type === pattern
  })
}

export function matchesFilter(event: TaskEvent, filter: SubscribeFilter): boolean {
  const includeStatus = filter.includeStatus ?? true

  if (!includeStatus && event.type === 'taskcast:status') {
    return false
  }

  if (filter.types !== undefined && !matchesType(event.type, filter.types)) {
    return false
  }

  if (filter.levels !== undefined && !filter.levels.includes(event.level as Level)) {
    return false
  }

  return true
}

export function applyFilteredIndex(
  events: TaskEvent[],
  filter: SubscribeFilter,
): FilteredEvent[] {
  const since = filter.since

  let filteredCounter = 0
  const result: FilteredEvent[] = []

  for (const event of events) {
    if (!matchesFilter(event, filter)) continue

    const currentFilteredIndex = filteredCounter
    filteredCounter++

    // since.index: skip events where filteredIndex <= since.index
    if (since?.index !== undefined && currentFilteredIndex <= since.index) {
      continue
    }

    result.push({
      filteredIndex: currentFilteredIndex,
      rawIndex: event.index,
      event,
    })
  }

  return result
}
```

**Step 4: 运行测试，确认通过**

```bash
pnpm --filter @taskcast/core vitest run tests/unit/filter.test.ts
```

Expected: PASS。

**Step 5: Commit**

```bash
git add packages/core/src/filter.ts packages/core/tests/unit/filter.test.ts
git commit -m "feat(core): add wildcard event filter and filteredIndex logic"
```

---

## Task 6: 序列消息合并（Series Merging）

**Files:**
- Create: `packages/core/src/series.ts`
- Create: `packages/core/tests/unit/series.test.ts`

**Step 1: 写失败测试**

`packages/core/tests/unit/series.test.ts`:
```typescript
import { describe, it, expect, vi } from 'vitest'
import { processSeries } from '../../src/series.js'
import type { TaskEvent, ShortTermStore } from '../../src/types.js'

const makeEvent = (overrides: Partial<TaskEvent> = {}): TaskEvent => ({
  id: 'evt-1',
  taskId: 'task-1',
  index: 0,
  timestamp: 1000,
  type: 'llm.delta',
  level: 'info',
  data: { text: 'hello' },
  ...overrides,
})

const makeStore = (latestEvent?: TaskEvent): ShortTermStore => ({
  saveTask: vi.fn(),
  getTask: vi.fn(),
  appendEvent: vi.fn(),
  getEvents: vi.fn(),
  setTTL: vi.fn(),
  getSeriesLatest: vi.fn().mockResolvedValue(latestEvent ?? null),
  setSeriesLatest: vi.fn(),
  replaceLastSeriesEvent: vi.fn(),
})

describe('processSeries - keep-all', () => {
  it('returns event unchanged, no store mutation', async () => {
    const store = makeStore()
    const event = makeEvent({ seriesId: 's1', seriesMode: 'keep-all' })
    const result = await processSeries(event, store)
    expect(result).toEqual(event)
    expect(store.setSeriesLatest).not.toHaveBeenCalled()
    expect(store.replaceLastSeriesEvent).not.toHaveBeenCalled()
  })
})

describe('processSeries - accumulate', () => {
  it('concatenates text when previous exists', async () => {
    const prev = makeEvent({ data: { text: 'hello ' }, seriesId: 's1', seriesMode: 'accumulate' })
    const store = makeStore(prev)
    const event = makeEvent({ data: { text: 'world' }, seriesId: 's1', seriesMode: 'accumulate' })
    const result = await processSeries(event, store)
    expect((result.data as { text: string }).text).toBe('hello world')
    expect(store.setSeriesLatest).toHaveBeenCalledWith('task-1', 's1', result)
  })

  it('returns event unchanged when no previous', async () => {
    const store = makeStore()
    const event = makeEvent({ data: { text: 'start' }, seriesId: 's1', seriesMode: 'accumulate' })
    const result = await processSeries(event, store)
    expect((result.data as { text: string }).text).toBe('start')
    expect(store.setSeriesLatest).toHaveBeenCalledWith('task-1', 's1', result)
  })

  it('handles non-text data gracefully (returns event unchanged)', async () => {
    const prev = makeEvent({ data: { count: 1 }, seriesId: 's1', seriesMode: 'accumulate' })
    const store = makeStore(prev)
    const event = makeEvent({ data: { count: 2 }, seriesId: 's1', seriesMode: 'accumulate' })
    const result = await processSeries(event, store)
    expect(result.data).toEqual({ count: 2 }) // no merge for non-text
    expect(store.setSeriesLatest).toHaveBeenCalled()
  })
})

describe('processSeries - latest', () => {
  it('calls replaceLastSeriesEvent with new event', async () => {
    const prev = makeEvent({ seriesId: 's1', seriesMode: 'latest', data: { text: 'old' } })
    const store = makeStore(prev)
    const event = makeEvent({ seriesId: 's1', seriesMode: 'latest', data: { text: 'new' } })
    const result = await processSeries(event, store)
    expect(result).toEqual(event)
    expect(store.replaceLastSeriesEvent).toHaveBeenCalledWith('task-1', 's1', event)
  })

  it('works with no previous event', async () => {
    const store = makeStore()
    const event = makeEvent({ seriesId: 's1', seriesMode: 'latest', data: { text: 'first' } })
    await processSeries(event, store)
    expect(store.replaceLastSeriesEvent).toHaveBeenCalledWith('task-1', 's1', event)
  })
})

describe('processSeries - no seriesId', () => {
  it('returns event unchanged when no seriesId', async () => {
    const store = makeStore()
    const event = makeEvent()
    const result = await processSeries(event, store)
    expect(result).toEqual(event)
    expect(store.getSeriesLatest).not.toHaveBeenCalled()
  })
})
```

**Step 2: 运行测试，确认失败**

```bash
pnpm --filter @taskcast/core vitest run tests/unit/series.test.ts
```

Expected: FAIL。

**Step 3: 实现序列合并**

`packages/core/src/series.ts`:
```typescript
import type { TaskEvent, ShortTermStore } from './types.js'

export async function processSeries(
  event: TaskEvent,
  store: ShortTermStore,
): Promise<TaskEvent> {
  if (!event.seriesId || !event.seriesMode) {
    return event
  }

  const { seriesId, seriesMode, taskId } = event

  if (seriesMode === 'keep-all') {
    return event
  }

  if (seriesMode === 'accumulate') {
    const prev = await store.getSeriesLatest(taskId, seriesId)
    let merged = event

    if (prev !== null) {
      const prevData = prev.data as Record<string, unknown>
      const newData = event.data as Record<string, unknown>
      if (typeof prevData['text'] === 'string' && typeof newData['text'] === 'string') {
        merged = {
          ...event,
          data: { ...newData, text: prevData['text'] + newData['text'] },
        }
      }
    }

    await store.setSeriesLatest(taskId, seriesId, merged)
    return merged
  }

  if (seriesMode === 'latest') {
    await store.replaceLastSeriesEvent(taskId, seriesId, event)
    return event
  }

  return event
}
```

**Step 4: 运行测试，确认通过**

```bash
pnpm --filter @taskcast/core vitest run tests/unit/series.test.ts
```

Expected: PASS。

**Step 5: Commit**

```bash
git add packages/core/src/series.ts packages/core/tests/unit/series.test.ts
git commit -m "feat(core): add series message merging (keep-all/accumulate/latest)"
```

---

## Task 7: 清理规则引擎

**Files:**
- Create: `packages/core/src/cleanup.ts`
- Create: `packages/core/tests/unit/cleanup.test.ts`

**Step 1: 写失败测试**

`packages/core/tests/unit/cleanup.test.ts`:
```typescript
import { describe, it, expect } from 'vitest'
import { matchesCleanupRule, filterEventsForCleanup } from '../../src/cleanup.js'
import type { Task, TaskEvent, CleanupRule } from '../../src/types.js'

const makeTask = (overrides: Partial<Task> = {}): Task => ({
  id: 'task-1',
  status: 'completed',
  createdAt: 0,
  updatedAt: 1000,
  completedAt: 1000,
  ...overrides,
})

const makeEvent = (overrides: Partial<TaskEvent> = {}): TaskEvent => ({
  id: 'evt-1',
  taskId: 'task-1',
  index: 0,
  timestamp: 500,
  type: 'llm.delta',
  level: 'info',
  data: null,
  ...overrides,
})

describe('matchesCleanupRule', () => {
  const now = 2000

  it('matches when no taskType filter', () => {
    const rule: CleanupRule = { trigger: {}, target: 'all' }
    expect(matchesCleanupRule(makeTask(), rule, now)).toBe(true)
  })

  it('matches task type with wildcard', () => {
    const rule: CleanupRule = {
      match: { taskTypes: ['llm.*'] },
      trigger: {},
      target: 'all',
    }
    expect(matchesCleanupRule(makeTask({ type: 'llm.chat' }), rule, now)).toBe(true)
    expect(matchesCleanupRule(makeTask({ type: 'export.pdf' }), rule, now)).toBe(false)
  })

  it('matches specific terminal status', () => {
    const rule: CleanupRule = {
      match: { status: ['completed'] },
      trigger: {},
      target: 'all',
    }
    expect(matchesCleanupRule(makeTask({ status: 'completed' }), rule, now)).toBe(true)
    expect(matchesCleanupRule(makeTask({ status: 'failed' }), rule, now)).toBe(false)
  })

  it('respects afterMs trigger delay', () => {
    const rule: CleanupRule = { trigger: { afterMs: 1500 }, target: 'all' }
    // completedAt=1000, now=2000, elapsed=1000 < 1500 → no match
    expect(matchesCleanupRule(makeTask({ completedAt: 1000 }), rule, 2000)).toBe(false)
    // completedAt=1000, now=2600, elapsed=1600 >= 1500 → match
    expect(matchesCleanupRule(makeTask({ completedAt: 1000 }), rule, 2600)).toBe(true)
  })
})

describe('filterEventsForCleanup', () => {
  it('returns all events when no eventFilter', () => {
    const rule: CleanupRule = { trigger: {}, target: 'events' }
    const events = [makeEvent(), makeEvent({ type: 'tool.call' })]
    expect(filterEventsForCleanup(events, rule, 2000)).toHaveLength(2)
  })

  it('filters by type wildcard', () => {
    const rule: CleanupRule = {
      trigger: {},
      target: 'events',
      eventFilter: { types: ['llm.*'] },
    }
    const events = [
      makeEvent({ type: 'llm.delta' }),
      makeEvent({ type: 'tool.call' }),
    ]
    const result = filterEventsForCleanup(events, rule, 2000)
    expect(result).toHaveLength(1)
    expect(result[0]?.type).toBe('llm.delta')
  })

  it('filters by level', () => {
    const rule: CleanupRule = {
      trigger: {},
      target: 'events',
      eventFilter: { levels: ['debug'] },
    }
    const events = [
      makeEvent({ level: 'debug' }),
      makeEvent({ level: 'info' }),
    ]
    const result = filterEventsForCleanup(events, rule, 2000)
    expect(result).toHaveLength(1)
    expect(result[0]?.level).toBe('debug')
  })

  it('filters by olderThanMs relative to task completedAt', () => {
    // completedAt = 1000, olderThanMs = 600
    // → delete events with timestamp < (1000 - 600) = 400
    const rule: CleanupRule = {
      trigger: {},
      target: 'events',
      eventFilter: { olderThanMs: 600 },
    }
    const completedAt = 1000
    const events = [
      makeEvent({ timestamp: 300 }),  // 300 < 400 → delete
      makeEvent({ timestamp: 500 }),  // 500 >= 400 → keep
    ]
    const result = filterEventsForCleanup(events, rule, 2000, completedAt)
    expect(result).toHaveLength(1)
    expect(result[0]?.timestamp).toBe(300)
  })
})
```

**Step 2: 运行测试，确认失败**

```bash
pnpm --filter @taskcast/core vitest run tests/unit/cleanup.test.ts
```

Expected: FAIL。

**Step 3: 实现清理规则引擎**

`packages/core/src/cleanup.ts`:
```typescript
import { matchesType } from './filter.js'
import type { Task, TaskEvent, CleanupRule } from './types.js'

export function matchesCleanupRule(
  task: Task,
  rule: CleanupRule,
  now: number,
): boolean {
  // 检查任务状态（非终态任务不触发清理）
  const terminalStatuses = ['completed', 'failed', 'timeout', 'cancelled']
  if (!terminalStatuses.includes(task.status)) return false

  // 检查 match.status
  if (rule.match?.status && !rule.match.status.includes(task.status as never)) {
    return false
  }

  // 检查 match.taskTypes（wildcard）
  if (rule.match?.taskTypes) {
    if (!task.type || !matchesType(task.type, rule.match.taskTypes)) {
      return false
    }
  }

  // 检查 afterMs 延迟
  if (rule.trigger.afterMs !== undefined) {
    const completedAt = task.completedAt ?? task.updatedAt
    const elapsed = now - completedAt
    if (elapsed < rule.trigger.afterMs) return false
  }

  return true
}

export function filterEventsForCleanup(
  events: TaskEvent[],
  rule: CleanupRule,
  now: number,
  completedAt?: number,
): TaskEvent[] {
  const ef = rule.eventFilter
  if (!ef) return events

  return events.filter((event) => {
    if (ef.types && !matchesType(event.type, ef.types)) return false
    if (ef.levels && !ef.levels.includes(event.level)) return false
    if (ef.seriesMode && event.seriesMode && !ef.seriesMode.includes(event.seriesMode)) return false
    if (ef.olderThanMs !== undefined && completedAt !== undefined) {
      const cutoff = completedAt - ef.olderThanMs
      if (event.timestamp >= cutoff) return false
    }
    return true
  })
}
```

**Step 4: 运行测试，确认通过**

```bash
pnpm --filter @taskcast/core vitest run tests/unit/cleanup.test.ts
```

Expected: PASS。

**Step 5: Commit**

```bash
git add packages/core/src/cleanup.ts packages/core/tests/unit/cleanup.test.ts
git commit -m "feat(core): add cleanup rules engine with wildcard and time-based matching"
```

---

## Task 8: 内存适配器（测试/开发用）

**Files:**
- Create: `packages/core/src/memory-adapters.ts`
- Create: `packages/core/tests/unit/memory-adapters.test.ts`

**Step 1: 写失败测试**

`packages/core/tests/unit/memory-adapters.test.ts`:
```typescript
import { describe, it, expect, vi } from 'vitest'
import { MemoryBroadcastProvider, MemoryShortTermStore } from '../../src/memory-adapters.js'
import type { TaskEvent } from '../../src/types.js'

const makeEvent = (index = 0): TaskEvent => ({
  id: `evt-${index}`,
  taskId: 'task-1',
  index,
  timestamp: 1000 + index,
  type: 'llm.delta',
  level: 'info',
  data: null,
})

describe('MemoryBroadcastProvider', () => {
  it('delivers published events to subscribers', async () => {
    const provider = new MemoryBroadcastProvider()
    const received: TaskEvent[] = []
    provider.subscribe('task-1', (e) => received.push(e))

    const event = makeEvent()
    await provider.publish('task-1', event)
    expect(received).toHaveLength(1)
    expect(received[0]).toEqual(event)
  })

  it('unsubscribe stops delivery', async () => {
    const provider = new MemoryBroadcastProvider()
    const received: TaskEvent[] = []
    const unsub = provider.subscribe('task-1', (e) => received.push(e))

    await provider.publish('task-1', makeEvent(0))
    unsub()
    await provider.publish('task-1', makeEvent(1))
    expect(received).toHaveLength(1)
  })

  it('delivers to multiple subscribers on same channel', async () => {
    const provider = new MemoryBroadcastProvider()
    const r1: TaskEvent[] = []
    const r2: TaskEvent[] = []
    provider.subscribe('task-1', (e) => r1.push(e))
    provider.subscribe('task-1', (e) => r2.push(e))
    await provider.publish('task-1', makeEvent())
    expect(r1).toHaveLength(1)
    expect(r2).toHaveLength(1)
  })
})

describe('MemoryShortTermStore', () => {
  it('saves and retrieves a task', async () => {
    const store = new MemoryShortTermStore()
    const task = {
      id: 'task-1',
      status: 'pending' as const,
      createdAt: 1000,
      updatedAt: 1000,
    }
    await store.saveTask(task)
    const retrieved = await store.getTask('task-1')
    expect(retrieved).toEqual(task)
  })

  it('returns null for missing task', async () => {
    const store = new MemoryShortTermStore()
    expect(await store.getTask('missing')).toBeNull()
  })

  it('appends events in order', async () => {
    const store = new MemoryShortTermStore()
    await store.appendEvent('task-1', makeEvent(0))
    await store.appendEvent('task-1', makeEvent(1))
    const events = await store.getEvents('task-1')
    expect(events).toHaveLength(2)
    expect(events[0]?.index).toBe(0)
    expect(events[1]?.index).toBe(1)
  })

  it('filters events by since.index', async () => {
    const store = new MemoryShortTermStore()
    for (let i = 0; i < 5; i++) await store.appendEvent('task-1', makeEvent(i))
    const events = await store.getEvents('task-1', { since: { index: 2 } })
    expect(events.map((e) => e.index)).toEqual([3, 4])
  })

  it('getSeriesLatest returns null when no series', async () => {
    const store = new MemoryShortTermStore()
    expect(await store.getSeriesLatest('task-1', 's1')).toBeNull()
  })

  it('setSeriesLatest and getSeriesLatest roundtrip', async () => {
    const store = new MemoryShortTermStore()
    const event = makeEvent()
    await store.setSeriesLatest('task-1', 's1', event)
    expect(await store.getSeriesLatest('task-1', 's1')).toEqual(event)
  })
})
```

**Step 2: 运行测试，确认失败**

```bash
pnpm --filter @taskcast/core vitest run tests/unit/memory-adapters.test.ts
```

Expected: FAIL。

**Step 3: 实现内存适配器**

`packages/core/src/memory-adapters.ts`:
```typescript
import type { Task, TaskEvent, BroadcastProvider, ShortTermStore, EventQueryOptions } from './types.js'

export class MemoryBroadcastProvider implements BroadcastProvider {
  private listeners = new Map<string, Set<(event: TaskEvent) => void>>()

  async publish(channel: string, event: TaskEvent): Promise<void> {
    const handlers = this.listeners.get(channel)
    if (!handlers) return
    for (const handler of handlers) {
      handler(event)
    }
  }

  subscribe(channel: string, handler: (event: TaskEvent) => void): () => void {
    if (!this.listeners.has(channel)) {
      this.listeners.set(channel, new Set())
    }
    this.listeners.get(channel)!.add(handler)
    return () => {
      this.listeners.get(channel)?.delete(handler)
    }
  }
}

export class MemoryShortTermStore implements ShortTermStore {
  private tasks = new Map<string, Task>()
  private events = new Map<string, TaskEvent[]>()
  private seriesLatest = new Map<string, TaskEvent>()

  async saveTask(task: Task): Promise<void> {
    this.tasks.set(task.id, { ...task })
  }

  async getTask(taskId: string): Promise<Task | null> {
    return this.tasks.get(taskId) ?? null
  }

  async appendEvent(taskId: string, event: TaskEvent): Promise<void> {
    if (!this.events.has(taskId)) this.events.set(taskId, [])
    this.events.get(taskId)!.push({ ...event })
  }

  async getEvents(taskId: string, opts?: EventQueryOptions): Promise<TaskEvent[]> {
    const all = this.events.get(taskId) ?? []
    let result = all

    if (opts?.since?.id) {
      const idx = result.findIndex((e) => e.id === opts.since!.id)
      result = idx >= 0 ? result.slice(idx + 1) : result
    } else if (opts?.since?.index !== undefined) {
      result = result.filter((e) => e.index > opts.since!.index!)
    } else if (opts?.since?.timestamp !== undefined) {
      result = result.filter((e) => e.timestamp > opts.since!.timestamp!)
    }

    if (opts?.limit) result = result.slice(0, opts.limit)
    return result
  }

  async setTTL(_taskId: string, _ttlSeconds: number): Promise<void> {
    // no-op in memory adapter
  }

  async getSeriesLatest(taskId: string, seriesId: string): Promise<TaskEvent | null> {
    return this.seriesLatest.get(`${taskId}:${seriesId}`) ?? null
  }

  async setSeriesLatest(taskId: string, seriesId: string, event: TaskEvent): Promise<void> {
    this.seriesLatest.set(`${taskId}:${seriesId}`, { ...event })
  }

  async replaceLastSeriesEvent(taskId: string, seriesId: string, event: TaskEvent): Promise<void> {
    const key = `${taskId}:${seriesId}`
    const prev = this.seriesLatest.get(key)
    if (prev) {
      const taskEvents = this.events.get(taskId)
      if (taskEvents) {
        const idx = taskEvents.findLastIndex((e) => e.id === prev.id)
        if (idx >= 0) taskEvents[idx] = { ...event }
      }
    }
    this.seriesLatest.set(key, { ...event })
    // Also append if no previous
    if (!prev) {
      await this.appendEvent(taskId, event)
    }
  }
}
```

**Step 4: 运行测试，确认通过**

```bash
pnpm --filter @taskcast/core vitest run tests/unit/memory-adapters.test.ts
```

Expected: PASS。

**Step 5: Commit**

```bash
git add packages/core/src/memory-adapters.ts packages/core/tests/unit/memory-adapters.test.ts
git commit -m "feat(core): add in-memory adapters for testing and development"
```

---

## Task 9: TaskEngine（核心调度器）

**Files:**
- Create: `packages/core/src/engine.ts`
- Create: `packages/core/tests/unit/engine.test.ts`

**Step 1: 写失败测试**

`packages/core/tests/unit/engine.test.ts`:
```typescript
import { describe, it, expect, vi, beforeEach } from 'vitest'
import { TaskEngine } from '../../src/engine.js'
import { MemoryBroadcastProvider, MemoryShortTermStore } from '../../src/memory-adapters.js'
import type { Task } from '../../src/types.js'

function makeEngine() {
  const store = new MemoryShortTermStore()
  const broadcast = new MemoryBroadcastProvider()
  const engine = new TaskEngine({ shortTerm: store, broadcast })
  return { engine, store, broadcast }
}

describe('TaskEngine.createTask', () => {
  it('creates a task with pending status', async () => {
    const { engine } = makeEngine()
    const task = await engine.createTask({ params: { prompt: 'hi' } })
    expect(task.status).toBe('pending')
    expect(task.params).toEqual({ prompt: 'hi' })
    expect(task.id).toBeTruthy()
    expect(task.createdAt).toBeGreaterThan(0)
  })

  it('creates a task with user-supplied id', async () => {
    const { engine } = makeEngine()
    const task = await engine.createTask({ id: 'my-task-id' })
    expect(task.id).toBe('my-task-id')
  })
})

describe('TaskEngine.transitionTask', () => {
  it('transitions pending → running and saves task', async () => {
    const { engine, store } = makeEngine()
    const task = await engine.createTask({})
    await engine.transitionTask(task.id, 'running')
    const updated = await store.getTask(task.id)
    expect(updated?.status).toBe('running')
  })

  it('throws on invalid transition', async () => {
    const { engine } = makeEngine()
    const task = await engine.createTask({})
    await expect(engine.transitionTask(task.id, 'completed')).rejects.toThrow()
  })

  it('throws when task not found', async () => {
    const { engine } = makeEngine()
    await expect(engine.transitionTask('missing', 'running')).rejects.toThrow(/not found/i)
  })

  it('emits taskcast:status event on transition', async () => {
    const { engine, broadcast } = makeEngine()
    const received: unknown[] = []
    const task = await engine.createTask({})
    broadcast.subscribe(task.id, (e) => received.push(e))
    await engine.transitionTask(task.id, 'running')
    expect(received).toHaveLength(1)
    expect((received[0] as { type: string }).type).toBe('taskcast:status')
  })
})

describe('TaskEngine.publishEvent', () => {
  it('appends event and broadcasts it', async () => {
    const { engine, store, broadcast } = makeEngine()
    const task = await engine.createTask({})
    await engine.transitionTask(task.id, 'running')

    const received: unknown[] = []
    broadcast.subscribe(task.id, (e) => received.push(e))

    await engine.publishEvent(task.id, {
      type: 'llm.delta',
      level: 'info',
      data: { text: 'hello' },
    })

    const events = await store.getEvents(task.id)
    const userEvents = events.filter((e) => e.type !== 'taskcast:status')
    expect(userEvents).toHaveLength(1)
    expect(userEvents[0]?.type).toBe('llm.delta')
    expect(received).toHaveLength(1)
  })

  it('assigns monotonically increasing index', async () => {
    const { engine, store } = makeEngine()
    const task = await engine.createTask({})
    await engine.transitionTask(task.id, 'running')
    await engine.publishEvent(task.id, { type: 'a', level: 'info', data: null })
    await engine.publishEvent(task.id, { type: 'b', level: 'info', data: null })
    const events = await store.getEvents(task.id)
    const indices = events.map((e) => e.index)
    expect(indices).toEqual([...indices].sort((a, b) => a - b))
  })

  it('rejects publish on completed task', async () => {
    const { engine } = makeEngine()
    const task = await engine.createTask({})
    await engine.transitionTask(task.id, 'running')
    await engine.transitionTask(task.id, 'completed')
    await expect(
      engine.publishEvent(task.id, { type: 'x', level: 'info', data: null })
    ).rejects.toThrow(/terminal/i)
  })
})

describe('TaskEngine.getTask', () => {
  it('returns null for unknown task', async () => {
    const { engine } = makeEngine()
    expect(await engine.getTask('nope')).toBeNull()
  })
})
```

**Step 2: 运行测试，确认失败**

```bash
pnpm --filter @taskcast/core vitest run tests/unit/engine.test.ts
```

Expected: FAIL。

**Step 3: 实现 TaskEngine**

`packages/core/src/engine.ts`:
```typescript
import { ulid } from 'ulidx'
import { canTransition, isTerminal } from './state-machine.js'
import { processSeries } from './series.js'
import type {
  Task,
  TaskStatus,
  TaskEvent,
  BroadcastProvider,
  ShortTermStore,
  LongTermStore,
  TaskcastHooks,
} from './types.js'

export interface TaskEngineOptions {
  shortTerm: ShortTermStore
  broadcast: BroadcastProvider
  longTerm?: LongTermStore
  hooks?: TaskcastHooks
}

export interface PublishEventInput {
  type: string
  level: TaskEvent['level']
  data: unknown
  seriesId?: string
  seriesMode?: TaskEvent['seriesMode']
}

export interface CreateTaskInput {
  id?: string
  type?: string
  params?: Record<string, unknown>
  metadata?: Record<string, unknown>
  ttl?: number
  webhooks?: Task['webhooks']
  cleanup?: Task['cleanup']
  authConfig?: Task['authConfig']
}

export class TaskEngine {
  private indexCounters = new Map<string, number>()

  constructor(private opts: TaskEngineOptions) {}

  async createTask(input: CreateTaskInput): Promise<Task> {
    const now = Date.now()
    const task: Task = {
      id: input.id ?? ulid(),
      type: input.type,
      status: 'pending',
      params: input.params,
      metadata: input.metadata,
      createdAt: now,
      updatedAt: now,
      ttl: input.ttl,
      webhooks: input.webhooks,
      cleanup: input.cleanup,
      authConfig: input.authConfig,
    }
    await this.opts.shortTerm.saveTask(task)
    if (this.opts.longTerm) await this.opts.longTerm.saveTask(task)
    if (task.ttl) await this.opts.shortTerm.setTTL(task.id, task.ttl)
    return task
  }

  async getTask(taskId: string): Promise<Task | null> {
    return this.opts.shortTerm.getTask(taskId)
      ?? this.opts.longTerm?.getTask(taskId)
      ?? null
  }

  async transitionTask(
    taskId: string,
    to: TaskStatus,
    payload?: { result?: Task['result']; error?: Task['error'] },
  ): Promise<Task> {
    const task = await this.getTask(taskId)
    if (!task) throw new Error(`Task not found: ${taskId}`)
    if (!canTransition(task.status, to)) {
      throw new Error(`Invalid transition: ${task.status} → ${to}`)
    }

    const now = Date.now()
    const updated: Task = {
      ...task,
      status: to,
      updatedAt: now,
      completedAt: isTerminal(to) ? now : task.completedAt,
      result: payload?.result ?? task.result,
      error: payload?.error ?? task.error,
    }

    await this.opts.shortTerm.saveTask(updated)
    if (this.opts.longTerm) await this.opts.longTerm.saveTask(updated)

    // Emit status event
    await this._emit(taskId, {
      type: 'taskcast:status',
      level: 'info',
      data: { status: to, result: updated.result, error: updated.error },
    })

    if (to === 'failed' && updated.error) {
      this.opts.hooks?.onTaskFailed?.(updated, updated.error)
    }
    if (to === 'timeout') {
      this.opts.hooks?.onTaskTimeout?.(updated)
    }

    return updated
  }

  async publishEvent(taskId: string, input: PublishEventInput): Promise<TaskEvent> {
    const task = await this.getTask(taskId)
    if (!task) throw new Error(`Task not found: ${taskId}`)
    if (isTerminal(task.status)) {
      throw new Error(`Cannot publish to task in terminal status: ${task.status}`)
    }

    const event = await this._emit(taskId, input)
    return event
  }

  private async _emit(taskId: string, input: PublishEventInput): Promise<TaskEvent> {
    const index = this._nextIndex(taskId)
    const raw: TaskEvent = {
      id: ulid(),
      taskId,
      index,
      timestamp: Date.now(),
      type: input.type,
      level: input.level,
      data: input.data,
      seriesId: input.seriesId,
      seriesMode: input.seriesMode,
    }

    // Process series merging
    const event = await processSeries(raw, this.opts.shortTerm)

    // Write to short-term store
    await this.opts.shortTerm.appendEvent(taskId, event)

    // Broadcast (real-time)
    await this.opts.broadcast.publish(taskId, event)

    // Async write to long-term store
    if (this.opts.longTerm) {
      this.opts.longTerm.saveEvent(event).catch((err) => {
        this.opts.hooks?.onEventDropped?.(event, String(err))
      })
    }

    return event
  }

  private _nextIndex(taskId: string): number {
    const current = this.indexCounters.get(taskId) ?? -1
    const next = current + 1
    this.indexCounters.set(taskId, next)
    return next
  }
}
```

**Step 4: 运行所有 core 单元测试**

```bash
pnpm --filter @taskcast/core vitest run tests/unit/
```

Expected: 全部 PASS。

**Step 5: 更新 index.ts 导出所有公开 API**

`packages/core/src/index.ts`:
```typescript
export * from './types.js'
export * from './state-machine.js'
export * from './filter.js'
export * from './series.js'
export * from './cleanup.js'
export * from './memory-adapters.js'
export * from './engine.js'
```

**Step 6: Commit**

```bash
git add packages/core/src/engine.ts packages/core/src/index.ts packages/core/tests/unit/engine.test.ts
git commit -m "feat(core): add TaskEngine orchestrator with event publishing and state transitions"
```

---

## Phase 1 完成检查

```bash
# 运行所有 core 测试
pnpm --filter @taskcast/core vitest run

# 确认覆盖率 >85%
pnpm --filter @taskcast/core vitest run --coverage

# TypeScript 无报错
pnpm --filter @taskcast/core exec tsc --noEmit
```

**下一步：** 继续 [Phase 2: Adapters](./2026-02-28-taskcast-02-adapters.md)
