# Taskcast Phase 4: SDKs + CLI Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** 实现 `@taskcast/server-sdk`（生产者 HTTP 客户端）、`@taskcast/client`（浏览器 SSE 客户端）、`@taskcast/react`（React hooks）、`@taskcast/cli`（`npx taskcast`）及配置加载系统。

**Architecture:** server-sdk 是 taskcast REST API 的纯类型安全 HTTP 封装；client 封装 EventSource 处理断点续传；react 封装 client 为 hooks；cli 通过配置加载系统启动独立服务器。

**Tech Stack:** Hono (server-sdk 复用类型), eventsource-parser (client), React ^18, commander (cli), js-yaml (config)

**前置条件：** Phase 1-3 完成。

---

## Task 20: 创建 `@taskcast/server-sdk` 包

**Files:**
- Create: `packages/server-sdk/package.json`
- Create: `packages/server-sdk/tsconfig.json`
- Create: `packages/server-sdk/src/index.ts`
- Create: `packages/server-sdk/src/client.ts`
- Create: `packages/server-sdk/tests/client.test.ts`

**Step 1: 创建包结构**

```bash
mkdir -p packages/server-sdk/src packages/server-sdk/tests
```

`packages/server-sdk/package.json`:
```json
{
  "name": "@taskcast/server-sdk",
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
  "devDependencies": {
    "typescript": "^5.7.0",
    "vitest": "^2.1.0",
    "@vitest/coverage-v8": "^2.1.0"
  }
}
```

`packages/server-sdk/tsconfig.json`:
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

`packages/server-sdk/tests/client.test.ts`:
```typescript
import { describe, it, expect, vi, beforeEach } from 'vitest'
import { TaskcastServerClient } from '../src/client.js'

function makeFetch(responses: Array<{ status: number; body: unknown }>) {
  let i = 0
  return vi.fn().mockImplementation(() => {
    const r = responses[i++] ?? { status: 200, body: {} }
    return Promise.resolve(
      new Response(JSON.stringify(r.body), {
        status: r.status,
        headers: { 'Content-Type': 'application/json' },
      }),
    )
  })
}

describe('TaskcastServerClient.createTask', () => {
  it('POST /tasks and returns created task', async () => {
    const fetch = makeFetch([{ status: 201, body: { id: 'task-1', status: 'pending' } }])
    const client = new TaskcastServerClient({ baseUrl: 'http://taskcast', fetch })

    const task = await client.createTask({ params: { prompt: 'hi' } })
    expect(task.id).toBe('task-1')
    expect(task.status).toBe('pending')

    expect(fetch).toHaveBeenCalledOnce()
    const [url, opts] = fetch.mock.calls[0]!
    expect(url).toBe('http://taskcast/tasks')
    expect(opts.method).toBe('POST')
    expect(JSON.parse(opts.body)).toEqual({ params: { prompt: 'hi' } })
  })
})

describe('TaskcastServerClient.getTask', () => {
  it('GET /tasks/:id and returns task', async () => {
    const fetch = makeFetch([{ status: 200, body: { id: 'task-1', status: 'running' } }])
    const client = new TaskcastServerClient({ baseUrl: 'http://taskcast', fetch })

    const task = await client.getTask('task-1')
    expect(task.status).toBe('running')
    expect(fetch.mock.calls[0]![0]).toBe('http://taskcast/tasks/task-1')
  })

  it('throws on 404', async () => {
    const fetch = makeFetch([{ status: 404, body: { error: 'Task not found' } }])
    const client = new TaskcastServerClient({ baseUrl: 'http://taskcast', fetch })
    await expect(client.getTask('missing')).rejects.toThrow(/not found/i)
  })
})

describe('TaskcastServerClient.transitionTask', () => {
  it('PATCH /tasks/:id/status', async () => {
    const fetch = makeFetch([{ status: 200, body: { id: 'task-1', status: 'running' } }])
    const client = new TaskcastServerClient({ baseUrl: 'http://taskcast', fetch })

    await client.transitionTask('task-1', 'running')
    const [url, opts] = fetch.mock.calls[0]!
    expect(url).toBe('http://taskcast/tasks/task-1/status')
    expect(opts.method).toBe('PATCH')
    expect(JSON.parse(opts.body)).toEqual({ status: 'running' })
  })
})

describe('TaskcastServerClient.publishEvent', () => {
  it('POST /tasks/:id/events single event', async () => {
    const fetch = makeFetch([{ status: 201, body: { id: 'evt-1', type: 'llm.delta' } }])
    const client = new TaskcastServerClient({ baseUrl: 'http://taskcast', fetch })

    await client.publishEvent('task-1', { type: 'llm.delta', level: 'info', data: null })
    const [url] = fetch.mock.calls[0]!
    expect(url).toBe('http://taskcast/tasks/task-1/events')
  })

  it('POST /tasks/:id/events batch', async () => {
    const fetch = makeFetch([{ status: 201, body: [{ id: 'e1' }, { id: 'e2' }] }])
    const client = new TaskcastServerClient({ baseUrl: 'http://taskcast', fetch })

    const events = await client.publishEvents('task-1', [
      { type: 'a', level: 'info', data: null },
      { type: 'b', level: 'info', data: null },
    ])
    expect(events).toHaveLength(2)
  })
})

describe('Authorization header', () => {
  it('sends Bearer token when configured', async () => {
    const fetch = makeFetch([{ status: 201, body: { id: 'task-1', status: 'pending' } }])
    const client = new TaskcastServerClient({
      baseUrl: 'http://taskcast',
      token: 'my-jwt-token',
      fetch,
    })
    await client.createTask({})
    const [, opts] = fetch.mock.calls[0]!
    expect(opts.headers['Authorization']).toBe('Bearer my-jwt-token')
  })
})
```

**Step 3: 运行测试，确认失败**

```bash
pnpm --filter @taskcast/server-sdk vitest run tests/client.test.ts
```

Expected: FAIL。

**Step 4: 实现 TaskcastServerClient**

`packages/server-sdk/src/client.ts`:
```typescript
import type { Task, TaskEvent, TaskStatus, CreateTaskInput, PublishEventInput } from '@taskcast/core'

export interface TaskcastServerClientOptions {
  baseUrl: string
  token?: string
  fetch?: typeof globalThis.fetch
}

export class TaskcastServerClient {
  private fetch: typeof globalThis.fetch
  private baseUrl: string
  private token?: string

  constructor(opts: TaskcastServerClientOptions) {
    this.baseUrl = opts.baseUrl.replace(/\/$/, '')
    this.token = opts.token
    this.fetch = opts.fetch ?? globalThis.fetch
  }

  async createTask(input: Omit<CreateTaskInput, never>): Promise<Task> {
    return this._request<Task>('POST', '/tasks', input, 201)
  }

  async getTask(taskId: string): Promise<Task> {
    return this._request<Task>('GET', `/tasks/${taskId}`)
  }

  async transitionTask(
    taskId: string,
    status: TaskStatus,
    payload?: { result?: Task['result']; error?: Task['error'] },
  ): Promise<Task> {
    return this._request<Task>('PATCH', `/tasks/${taskId}/status`, {
      status,
      ...payload,
    })
  }

  async publishEvent(taskId: string, input: PublishEventInput): Promise<TaskEvent> {
    return this._request<TaskEvent>('POST', `/tasks/${taskId}/events`, input, 201)
  }

  async publishEvents(taskId: string, inputs: PublishEventInput[]): Promise<TaskEvent[]> {
    return this._request<TaskEvent[]>('POST', `/tasks/${taskId}/events`, inputs, 201)
  }

  async getHistory(
    taskId: string,
    opts?: { since?: { id?: string; index?: number; timestamp?: number } },
  ): Promise<TaskEvent[]> {
    const params = new URLSearchParams()
    if (opts?.since?.id) params.set('since.id', opts.since.id)
    if (opts?.since?.index !== undefined) params.set('since.index', String(opts.since.index))
    if (opts?.since?.timestamp !== undefined)
      params.set('since.timestamp', String(opts.since.timestamp))
    const qs = params.toString()
    return this._request<TaskEvent[]>('GET', `/tasks/${taskId}/events/history${qs ? `?${qs}` : ''}`)
  }

  private async _request<T>(
    method: string,
    path: string,
    body?: unknown,
    expectedStatus = 200,
  ): Promise<T> {
    const headers: Record<string, string> = {
      'Content-Type': 'application/json',
      Accept: 'application/json',
    }
    if (this.token) headers['Authorization'] = `Bearer ${this.token}`

    const res = await this.fetch(`${this.baseUrl}${path}`, {
      method,
      headers,
      body: body !== undefined ? JSON.stringify(body) : undefined,
    })

    if (!res.ok) {
      let message = `HTTP ${res.status}`
      try {
        const err = await res.json()
        message = (err as { error?: string }).error ?? message
      } catch {}
      throw new Error(message)
    }

    return res.json() as Promise<T>
  }
}
```

`packages/server-sdk/src/index.ts`:
```typescript
export { TaskcastServerClient } from './client.js'
export type { TaskcastServerClientOptions } from './client.js'
```

**Step 5: 运行测试，确认通过**

```bash
pnpm --filter @taskcast/server-sdk vitest run tests/client.test.ts
```

Expected: PASS。

**Step 6: Commit**

```bash
git add packages/server-sdk/
git commit -m "feat(server-sdk): add TaskcastServerClient HTTP client for remote mode"
```

---

## Task 21: 创建 `@taskcast/client`（浏览器 SSE 客户端）

**Files:**
- Create: `packages/client/package.json`
- Create: `packages/client/tsconfig.json`
- Create: `packages/client/src/index.ts`
- Create: `packages/client/src/client.ts`
- Create: `packages/client/tests/client.test.ts`

**Step 1: 创建包结构**

```bash
mkdir -p packages/client/src packages/client/tests
```

`packages/client/package.json`:
```json
{
  "name": "@taskcast/client",
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
    "eventsource-parser": "^2.0.1"
  },
  "devDependencies": {
    "typescript": "^5.7.0",
    "vitest": "^2.1.0",
    "@vitest/coverage-v8": "^2.1.0"
  }
}
```

**Step 2: 写失败测试**

`packages/client/tests/client.test.ts`:
```typescript
import { describe, it, expect, vi } from 'vitest'
import { TaskcastClient } from '../src/client.js'

function makeMockSSEStream(events: Array<{ event: string; data: string; id?: string }>) {
  const lines = events.flatMap((e) => [
    `event: ${e.event}`,
    e.id ? `id: ${e.id}` : '',
    `data: ${e.data}`,
    '',
    '',
  ]).filter((l, i, a) => !(l === '' && a[i - 1] === ''))

  const body = lines.join('\n')
  return new Response(body, {
    status: 200,
    headers: { 'Content-Type': 'text/event-stream' },
  })
}

describe('TaskcastClient.subscribe', () => {
  it('parses SSE events and calls onEvent', async () => {
    const fetch = vi.fn().mockResolvedValue(
      makeMockSSEStream([
        { event: 'taskcast.event', data: JSON.stringify({ filteredIndex: 0, type: 'llm.delta' }), id: 'e1' },
        { event: 'taskcast.done', data: JSON.stringify({ reason: 'completed' }) },
      ])
    )

    const client = new TaskcastClient({ baseUrl: 'http://taskcast', fetch })
    const received: unknown[] = []

    await client.subscribe('task-1', {
      onEvent: (e) => received.push(e),
      onDone: () => {},
    })

    expect(received).toHaveLength(1)
    expect((received[0] as { type: string }).type).toBe('llm.delta')
  })

  it('calls onDone with reason when taskcast.done received', async () => {
    const fetch = vi.fn().mockResolvedValue(
      makeMockSSEStream([
        { event: 'taskcast.done', data: JSON.stringify({ reason: 'completed' }) },
      ])
    )
    const client = new TaskcastClient({ baseUrl: 'http://taskcast', fetch })
    const doneReasons: string[] = []

    await client.subscribe('task-1', {
      onEvent: () => {},
      onDone: (reason) => doneReasons.push(reason),
    })

    expect(doneReasons).toEqual(['completed'])
  })

  it('passes filter query params', async () => {
    const fetch = vi.fn().mockResolvedValue(
      makeMockSSEStream([
        { event: 'taskcast.done', data: JSON.stringify({ reason: 'completed' }) },
      ])
    )
    const client = new TaskcastClient({ baseUrl: 'http://taskcast', fetch })

    await client.subscribe('task-1', {
      filter: { types: ['llm.*'], levels: ['info'], since: { index: 5 } },
      onEvent: () => {},
      onDone: () => {},
    })

    const [url] = fetch.mock.calls[0]!
    expect(url).toContain('types=llm.*')
    expect(url).toContain('levels=info')
    expect(url).toContain('since.index=5')
  })
})
```

**Step 3: 运行测试，确认失败**

```bash
pnpm --filter @taskcast/client vitest run tests/client.test.ts
```

Expected: FAIL。

**Step 4: 实现 TaskcastClient**

`packages/client/src/client.ts`:
```typescript
import { createParser } from 'eventsource-parser'
import type { SSEEnvelope, SubscribeFilter } from '@taskcast/core'

export interface SubscribeOptions {
  filter?: SubscribeFilter
  onEvent: (envelope: SSEEnvelope) => void
  onDone: (reason: string) => void
  onError?: (err: Error) => void
}

export interface TaskcastClientOptions {
  baseUrl: string
  token?: string
  fetch?: typeof globalThis.fetch
}

export class TaskcastClient {
  private baseUrl: string
  private token?: string
  private fetch: typeof globalThis.fetch

  constructor(opts: TaskcastClientOptions) {
    this.baseUrl = opts.baseUrl.replace(/\/$/, '')
    this.token = opts.token
    this.fetch = opts.fetch ?? globalThis.fetch
  }

  async subscribe(taskId: string, opts: SubscribeOptions): Promise<void> {
    const url = this._buildURL(taskId, opts.filter)
    const headers: Record<string, string> = { Accept: 'text/event-stream' }
    if (this.token) headers['Authorization'] = `Bearer ${this.token}`

    const res = await this.fetch(url, { headers })
    if (!res.ok) {
      throw new Error(`Failed to subscribe: HTTP ${res.status}`)
    }
    if (!res.body) throw new Error('No response body')

    const reader = res.body.getReader()
    const decoder = new TextDecoder()

    const parser = createParser({
      onEvent: (event) => {
        if (event.event === 'taskcast.event') {
          try {
            const envelope = JSON.parse(event.data) as SSEEnvelope
            opts.onEvent(envelope)
          } catch {}
        } else if (event.event === 'taskcast.done') {
          try {
            const { reason } = JSON.parse(event.data) as { reason: string }
            opts.onDone(reason)
          } catch {}
        }
      },
    })

    while (true) {
      const { done, value } = await reader.read()
      if (done) break
      parser.feed(decoder.decode(value, { stream: true }))
    }
  }

  private _buildURL(taskId: string, filter?: SubscribeFilter): string {
    const params = new URLSearchParams()
    if (filter?.types) params.set('types', filter.types.join(','))
    if (filter?.levels) params.set('levels', filter.levels.join(','))
    if (filter?.includeStatus === false) params.set('includeStatus', 'false')
    if (filter?.wrap === false) params.set('wrap', 'false')
    if (filter?.since?.id) params.set('since.id', filter.since.id)
    if (filter?.since?.index !== undefined) params.set('since.index', String(filter.since.index))
    if (filter?.since?.timestamp !== undefined)
      params.set('since.timestamp', String(filter.since.timestamp))

    const qs = params.toString()
    return `${this.baseUrl}/tasks/${taskId}/events${qs ? `?${qs}` : ''}`
  }
}
```

`packages/client/src/index.ts`:
```typescript
export { TaskcastClient } from './client.js'
export type { TaskcastClientOptions, SubscribeOptions } from './client.js'
```

**Step 5: 安装依赖并运行测试**

```bash
pnpm --filter @taskcast/client install
pnpm --filter @taskcast/client vitest run tests/client.test.ts
```

Expected: PASS。

**Step 6: Commit**

```bash
git add packages/client/
git commit -m "feat(client): add browser SSE client with filter query params and done handling"
```

---

## Task 22: 创建 `@taskcast/react`（React hooks）

**Files:**
- Create: `packages/react/package.json`
- Create: `packages/react/tsconfig.json`
- Create: `packages/react/src/index.ts`
- Create: `packages/react/src/useTaskcast.ts`
- Create: `packages/react/tests/useTaskcast.test.tsx`

**Step 1: 创建包结构**

```bash
mkdir -p packages/react/src packages/react/tests
```

`packages/react/package.json`:
```json
{
  "name": "@taskcast/react",
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
    "@taskcast/client": "workspace:*",
    "@taskcast/core": "workspace:*"
  },
  "peerDependencies": {
    "react": "^18.0.0"
  },
  "devDependencies": {
    "typescript": "^5.7.0",
    "vitest": "^2.1.0",
    "@vitest/coverage-v8": "^2.1.0",
    "@testing-library/react": "^16.0.0",
    "@testing-library/dom": "^10.0.0",
    "react": "^18.3.0",
    "react-dom": "^18.3.0",
    "jsdom": "^25.0.0"
  }
}
```

`packages/react/tsconfig.json`:
```json
{
  "extends": "../../tsconfig.base.json",
  "compilerOptions": {
    "rootDir": "src",
    "outDir": "dist",
    "jsx": "react-jsx",
    "lib": ["ES2022", "DOM"]
  },
  "include": ["src"],
  "references": [
    { "path": "../core" },
    { "path": "../client" }
  ]
}
```

Add to `packages/react/vitest.config.ts`:
```typescript
import { defineConfig } from 'vitest/config'

export default defineConfig({
  test: {
    environment: 'jsdom',
    include: ['tests/**/*.test.tsx', 'tests/**/*.test.ts'],
    coverage: {
      provider: 'v8',
      reporter: ['text', 'lcov'],
      include: ['src/**'],
    },
  },
})
```

**Step 2: 写失败测试**

`packages/react/tests/useTaskcast.test.tsx`:
```typescript
import { describe, it, expect, vi, beforeEach } from 'vitest'
import { renderHook, act, waitFor } from '@testing-library/react'
import { useTaskEvents } from '../src/useTaskcast.js'
import type { SSEEnvelope } from '@taskcast/core'

// Mock TaskcastClient
vi.mock('@taskcast/client', () => ({
  TaskcastClient: vi.fn().mockImplementation(() => ({
    subscribe: vi.fn(async (_taskId: string, opts: { onEvent: (e: SSEEnvelope) => void; onDone: (r: string) => void }) => {
      opts.onEvent({ filteredIndex: 0, rawIndex: 0, eventId: 'e1', taskId: 'task-1', type: 'llm.delta', timestamp: 1000, level: 'info', data: { text: 'hello' } })
      opts.onDone('completed')
    }),
  })),
}))

describe('useTaskEvents', () => {
  it('subscribes to task and collects events', async () => {
    const { result } = renderHook(() =>
      useTaskEvents('task-1', { baseUrl: 'http://taskcast' })
    )

    await waitFor(() => expect(result.current.isDone).toBe(true))

    expect(result.current.events).toHaveLength(1)
    expect(result.current.events[0]?.type).toBe('llm.delta')
    expect(result.current.doneReason).toBe('completed')
    expect(result.current.error).toBeNull()
  })

  it('initializes with empty state', () => {
    const { result } = renderHook(() =>
      useTaskEvents('task-1', { baseUrl: 'http://taskcast' })
    )
    expect(result.current.events).toEqual([])
    expect(result.current.isDone).toBe(false)
    expect(result.current.error).toBeNull()
  })
})
```

**Step 3: 运行测试，确认失败**

```bash
pnpm --filter @taskcast/react install
pnpm --filter @taskcast/react vitest run tests/useTaskcast.test.tsx
```

Expected: FAIL。

**Step 4: 实现 React Hook**

`packages/react/src/useTaskcast.ts`:
```typescript
import { useState, useEffect, useCallback } from 'react'
import { TaskcastClient } from '@taskcast/client'
import type { TaskcastClientOptions, SubscribeOptions } from '@taskcast/client'
import type { SSEEnvelope, SubscribeFilter } from '@taskcast/core'

export interface UseTaskEventsOptions extends TaskcastClientOptions {
  filter?: SubscribeFilter
  enabled?: boolean
}

export interface UseTaskEventsResult {
  events: SSEEnvelope[]
  isDone: boolean
  doneReason: string | null
  error: Error | null
}

export function useTaskEvents(
  taskId: string,
  opts: UseTaskEventsOptions,
): UseTaskEventsResult {
  const [events, setEvents] = useState<SSEEnvelope[]>([])
  const [isDone, setIsDone] = useState(false)
  const [doneReason, setDoneReason] = useState<string | null>(null)
  const [error, setError] = useState<Error | null>(null)

  const enabled = opts.enabled ?? true

  useEffect(() => {
    if (!enabled || !taskId) return

    const client = new TaskcastClient({
      baseUrl: opts.baseUrl,
      token: opts.token,
      fetch: opts.fetch,
    })

    let cancelled = false

    client.subscribe(taskId, {
      filter: opts.filter,
      onEvent: (envelope) => {
        if (!cancelled) setEvents((prev) => [...prev, envelope])
      },
      onDone: (reason) => {
        if (!cancelled) {
          setDoneReason(reason)
          setIsDone(true)
        }
      },
      onError: (err) => {
        if (!cancelled) setError(err)
      },
    }).catch((err) => {
      if (!cancelled) setError(err instanceof Error ? err : new Error(String(err)))
    })

    return () => {
      cancelled = true
    }
  }, [taskId, opts.baseUrl, opts.token, enabled])

  return { events, isDone, doneReason, error }
}
```

`packages/react/src/index.ts`:
```typescript
export { useTaskEvents } from './useTaskcast.js'
export type { UseTaskEventsOptions, UseTaskEventsResult } from './useTaskcast.js'
```

**Step 5: 运行测试，确认通过**

```bash
pnpm --filter @taskcast/react vitest run tests/useTaskcast.test.tsx
```

Expected: PASS。

**Step 6: Commit**

```bash
git add packages/react/
git commit -m "feat(react): add useTaskEvents hook for SSE subscription"
```

---

## Task 23: 配置加载系统

**Files:**
- Create: `packages/core/src/config.ts`
- Create: `packages/core/tests/unit/config.test.ts`

**Step 1: 写失败测试**

`packages/core/tests/unit/config.test.ts`:
```typescript
import { describe, it, expect } from 'vitest'
import { interpolateEnvVars, parseConfig } from '../../src/config.js'

describe('interpolateEnvVars', () => {
  it('replaces ${VAR} with env value', () => {
    process.env['TEST_VAR'] = 'hello'
    expect(interpolateEnvVars('prefix_${TEST_VAR}_suffix')).toBe('prefix_hello_suffix')
  })

  it('leaves ${MISSING} unchanged when var not set', () => {
    delete process.env['MISSING_VAR_XYZ']
    expect(interpolateEnvVars('${MISSING_VAR_XYZ}')).toBe('${MISSING_VAR_XYZ}')
  })

  it('handles multiple vars in same string', () => {
    process.env['A'] = 'foo'
    process.env['B'] = 'bar'
    expect(interpolateEnvVars('${A}:${B}')).toBe('foo:bar')
  })
})

describe('parseConfig - JSON', () => {
  it('parses valid JSON config', () => {
    const json = JSON.stringify({ port: 3721, auth: { mode: 'none' } })
    const config = parseConfig(json, 'json')
    expect(config.port).toBe(3721)
    expect(config.auth?.mode).toBe('none')
  })
})

describe('parseConfig - YAML', () => {
  it('parses valid YAML config', () => {
    const yaml = `
port: 3721
auth:
  mode: jwt
  jwt:
    algorithm: HS256
    secret: my-secret
`
    const config = parseConfig(yaml, 'yaml')
    expect(config.port).toBe(3721)
    expect(config.auth?.mode).toBe('jwt')
    expect(config.auth?.jwt?.secret).toBe('my-secret')
  })

  it('interpolates env vars in YAML values', () => {
    process.env['TEST_PORT'] = '4000'
    const yaml = 'port: ${TEST_PORT}'
    const config = parseConfig(yaml, 'yaml')
    expect(config.port).toBe(4000)
  })
})
```

**Step 2: 运行测试，确认失败**

```bash
pnpm --filter @taskcast/core vitest run tests/unit/config.test.ts
```

Expected: FAIL。

**Step 3: 实现配置加载**

先添加 `js-yaml` 到 core 依赖：
```bash
pnpm --filter @taskcast/core add js-yaml
pnpm --filter @taskcast/core add -D @types/js-yaml
```

`packages/core/src/config.ts`:
```typescript
import { load as yamlLoad } from 'js-yaml'
import type { AuthConfig } from '../src/types.js'

export interface TaskcastConfig {
  port?: number
  logLevel?: 'debug' | 'info' | 'warn' | 'error'
  auth?: {
    mode: 'none' | 'jwt' | 'custom'
    jwt?: {
      algorithm?: string
      secret?: string
      publicKey?: string
      publicKeyFile?: string
      issuer?: string
      audience?: string
    }
  }
  adapters?: {
    broadcast?: { provider: string; url?: string }
    shortTerm?: { provider: string; url?: string }
    longTerm?: { provider: string; url?: string }
  }
  sentry?: {
    dsn?: string
    captureTaskFailures?: boolean
    captureTaskTimeouts?: boolean
    captureUnhandledErrors?: boolean
    captureDroppedEvents?: boolean
    captureStorageErrors?: boolean
    captureBroadcastErrors?: boolean
    traceSSEConnections?: boolean
    traceEventPublish?: boolean
  }
  webhook?: {
    defaultRetry?: {
      retries?: number
      backoff?: 'fixed' | 'exponential' | 'linear'
      initialDelayMs?: number
      maxDelayMs?: number
      timeoutMs?: number
    }
  }
  cleanup?: {
    rules?: unknown[]
  }
}

export function interpolateEnvVars(value: string): string {
  return value.replace(/\$\{([^}]+)\}/g, (_match, varName: string) => {
    return process.env[varName] ?? _match
  })
}

function interpolateObject(obj: unknown): unknown {
  if (typeof obj === 'string') return interpolateEnvVars(obj)
  if (Array.isArray(obj)) return obj.map(interpolateObject)
  if (obj !== null && typeof obj === 'object') {
    return Object.fromEntries(
      Object.entries(obj as Record<string, unknown>).map(([k, v]) => [k, interpolateObject(v)])
    )
  }
  return obj
}

export function parseConfig(content: string, format: 'json' | 'yaml'): TaskcastConfig {
  let raw: unknown
  if (format === 'json') {
    raw = JSON.parse(content)
  } else {
    const interpolated = interpolateEnvVars(content)
    raw = yamlLoad(interpolated)
  }
  const config = interpolateObject(raw) as TaskcastConfig
  // Coerce port to number if it's a string (from env var interpolation in JSON)
  if (typeof config.port === 'string') {
    config.port = parseInt(config.port, 10)
  }
  return config
}

export async function loadConfigFile(configPath?: string): Promise<TaskcastConfig> {
  const { readFileSync, existsSync } = await import('fs')
  const { resolve, extname } = await import('path')

  const candidates = configPath
    ? [configPath]
    : [
        'taskcast.config.ts',
        'taskcast.config.js',
        'taskcast.config.mjs',
        'taskcast.config.yaml',
        'taskcast.config.yml',
        'taskcast.config.json',
      ]

  for (const candidate of candidates) {
    const fullPath = resolve(candidate)
    if (!existsSync(fullPath)) continue

    const ext = extname(fullPath).toLowerCase()
    if (ext === '.ts' || ext === '.js' || ext === '.mjs') {
      const mod = await import(fullPath) as { default?: TaskcastConfig }
      return mod.default ?? {}
    }

    const content = readFileSync(fullPath, 'utf8')
    const format = ext === '.json' ? 'json' : 'yaml'
    return parseConfig(content, format)
  }

  return {}
}
```

**Step 4: 运行测试，确认通过**

```bash
pnpm --filter @taskcast/core vitest run tests/unit/config.test.ts
```

Expected: PASS。

**Step 5: Commit**

```bash
git add packages/core/src/config.ts packages/core/tests/unit/config.test.ts
git commit -m "feat(core): add config loading with env var interpolation and YAML/JSON/TS support"
```

---

## Task 24: 创建 `@taskcast/cli`（`npx taskcast`）

**Files:**
- Create: `packages/cli/package.json`
- Create: `packages/cli/tsconfig.json`
- Create: `packages/cli/src/index.ts`

**Step 1: 创建包结构**

```bash
mkdir -p packages/cli/src
```

`packages/cli/package.json`:
```json
{
  "name": "@taskcast/cli",
  "version": "0.1.0",
  "type": "module",
  "bin": {
    "taskcast": "./dist/index.js"
  },
  "exports": {
    ".": {
      "import": "./dist/index.js",
      "types": "./dist/index.d.ts"
    }
  },
  "scripts": {
    "build": "tsc",
    "start": "node dist/index.js"
  },
  "dependencies": {
    "@taskcast/core": "workspace:*",
    "@taskcast/server": "workspace:*",
    "@taskcast/redis": "workspace:*",
    "@taskcast/postgres": "workspace:*",
    "commander": "^12.1.0",
    "ioredis": "^5.4.0",
    "postgres": "^3.4.5"
  },
  "devDependencies": {
    "typescript": "^5.7.0"
  }
}
```

`packages/cli/tsconfig.json`:
```json
{
  "extends": "../../tsconfig.base.json",
  "compilerOptions": {
    "rootDir": "src",
    "outDir": "dist"
  },
  "include": ["src"],
  "references": [
    { "path": "../core" },
    { "path": "../server" },
    { "path": "../redis" },
    { "path": "../postgres" }
  ]
}
```

**Step 2: 实现 CLI 入口**

`packages/cli/src/index.ts`:
```typescript
#!/usr/bin/env node
import { Command } from 'commander'
import { serve } from '@hono/node-server'
import { Redis } from 'ioredis'
import postgres from 'postgres'
import {
  TaskEngine,
  loadConfigFile,
  MemoryBroadcastProvider,
  MemoryShortTermStore,
} from '@taskcast/core'
import { createTaskcastApp } from '@taskcast/server'
import { createRedisAdapters } from '@taskcast/redis'
import { PostgresLongTermStore } from '@taskcast/postgres'

const program = new Command()

program
  .name('taskcast')
  .description('Taskcast — unified task tracking and streaming service')
  .version('0.1.0')

program
  .command('start', { isDefault: true })
  .description('Start the taskcast server')
  .option('-c, --config <path>', 'config file path')
  .option('-p, --port <port>', 'port to listen on', '3721')
  .option('--log-level <level>', 'log level', 'info')
  .action(async (options: { config?: string; port: string; logLevel: string }) => {
    const fileConfig = await loadConfigFile(options.config)

    const port = Number(process.env['TASKCAST_PORT'] ?? options.port ?? fileConfig.port ?? 3721)
    const redisUrl = process.env['TASKCAST_REDIS_URL'] ?? fileConfig.adapters?.broadcast?.url
    const postgresUrl = process.env['TASKCAST_POSTGRES_URL'] ?? fileConfig.adapters?.longTerm?.url

    // Build adapters
    let shortTerm: Parameters<typeof TaskEngine['prototype']['constructor']>[0]['shortTerm']
    let broadcast: Parameters<typeof TaskEngine['prototype']['constructor']>[0]['broadcast']
    let longTerm: Parameters<typeof TaskEngine['prototype']['constructor']>[0]['longTerm']

    if (redisUrl) {
      const pubClient = new Redis(redisUrl)
      const subClient = new Redis(redisUrl)
      const storeClient = new Redis(redisUrl)
      const adapters = createRedisAdapters(pubClient, subClient, storeClient)
      broadcast = adapters.broadcast
      shortTerm = adapters.shortTerm
    } else {
      console.warn('[taskcast] No REDIS_URL configured — using in-memory adapters (single-instance only, not suitable for production)')
      broadcast = new MemoryBroadcastProvider()
      shortTerm = new MemoryShortTermStore()
    }

    if (postgresUrl) {
      const sql = postgres(postgresUrl)
      longTerm = new PostgresLongTermStore(sql)
    }

    const engine = new TaskEngine({ shortTerm, broadcast, longTerm })

    const authMode = process.env['TASKCAST_AUTH_MODE'] ?? fileConfig.auth?.mode ?? 'none'
    const app = createTaskcastApp({
      engine,
      auth: { mode: authMode as 'none' | 'jwt', jwt: fileConfig.auth?.jwt as never },
    })

    serve({ fetch: app.fetch, port }, () => {
      console.log(`[taskcast] Server started on http://localhost:${port}`)
    })
  })

program.parse()
```

**Step 3: 安装依赖**

```bash
pnpm --filter @taskcast/cli install
```

**Step 4: 构建并验证 CLI**

```bash
pnpm --filter @taskcast/cli build
node packages/cli/dist/index.js --help
```

Expected:
```
Usage: taskcast [options] [command]
...
```

**Step 5: Commit**

```bash
git add packages/cli/
git commit -m "feat(cli): add npx taskcast CLI with config file, Redis, and Postgres support"
```

---

## Phase 4 完成检查

```bash
# 运行所有 SDK 测试
pnpm --filter @taskcast/server-sdk vitest run
pnpm --filter @taskcast/client vitest run
pnpm --filter @taskcast/react vitest run

# TypeScript 全局检查
pnpm -r exec tsc --noEmit

# 验证 CLI 启动（需要 Redis 运行中或无 REDIS_URL 使用内存）
node packages/cli/dist/index.js start --help
```

**下一步：** 继续 [Phase 5: Sentry + Integration Tests](./2026-02-28-taskcast-05-sentry-tests.md)
