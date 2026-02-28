# Taskcast Phase 3: HTTP Server Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** 实现 `@taskcast/server`：基于 Hono 的 HTTP 服务器，包含 REST API、SSE 订阅端点、JWT/中间件认证、Webhook 投递引擎。

**Architecture:** Hono 作为轻量跨运行时 HTTP 框架，所有业务逻辑委托给 `@taskcast/core` 的 `TaskEngine`，HTTP 层只做请求解析/响应格式化/认证校验。

**Tech Stack:** Hono ^4.x, jose ^5.x (JWT), zod ^3.x (请求校验)

**前置条件：** Phase 1（@taskcast/core）和 Phase 2（adapters）完成。

---

## Task 15: 创建 `@taskcast/server` 包骨架

**Files:**
- Create: `packages/server/package.json`
- Create: `packages/server/tsconfig.json`
- Create: `packages/server/vitest.config.ts`
- Create: `packages/server/src/index.ts`

**Step 1: 创建包结构**

```bash
mkdir -p packages/server/src packages/server/tests
```

`packages/server/package.json`:
```json
{
  "name": "@taskcast/server",
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
    "hono": "^4.6.0",
    "jose": "^5.9.0",
    "zod": "^3.23.0"
  },
  "devDependencies": {
    "typescript": "^5.7.0",
    "vitest": "^2.1.0",
    "@vitest/coverage-v8": "^2.1.0"
  }
}
```

`packages/server/tsconfig.json`:
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

`packages/server/vitest.config.ts`:
```typescript
import { defineConfig } from 'vitest/config'

export default defineConfig({
  test: {
    include: ['tests/**/*.test.ts'],
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
pnpm --filter @taskcast/server install
```

**Step 3: Commit**

```bash
git add packages/server
git commit -m "chore: scaffold @taskcast/server package"
```

---

## Task 16: Auth 中间件（JWT + 无认证）

**Files:**
- Create: `packages/server/src/auth.ts`
- Create: `packages/server/tests/auth.test.ts`

**Step 1: 写失败测试**

`packages/server/tests/auth.test.ts`:
```typescript
import { describe, it, expect } from 'vitest'
import { Hono } from 'hono'
import { SignJWT, generateSecret } from 'jose'
import { createAuthMiddleware } from '../src/auth.js'
import type { AuthConfig } from '../src/auth.js'

async function makeJwt(
  secret: Uint8Array,
  payload: Record<string, unknown>,
): Promise<string> {
  return new SignJWT(payload)
    .setProtectedHeader({ alg: 'HS256' })
    .setExpirationTime('1h')
    .sign(secret)
}

describe('auth middleware - mode: none', () => {
  it('allows all requests', async () => {
    const app = new Hono()
    app.use('*', createAuthMiddleware({ mode: 'none' }))
    app.get('/test', (c) => c.json({ ok: true }))

    const res = await app.request('/test')
    expect(res.status).toBe(200)
  })
})

describe('auth middleware - mode: jwt HS256', () => {
  it('rejects request with no token', async () => {
    const config: AuthConfig = {
      mode: 'jwt',
      jwt: { algorithm: 'HS256', secret: 'test-secret-that-is-long-enough' },
    }
    const app = new Hono()
    app.use('*', createAuthMiddleware(config))
    app.get('/test', (c) => c.json({ ok: true }))

    const res = await app.request('/test')
    expect(res.status).toBe(401)
  })

  it('rejects request with invalid token', async () => {
    const config: AuthConfig = {
      mode: 'jwt',
      jwt: { algorithm: 'HS256', secret: 'test-secret-that-is-long-enough' },
    }
    const app = new Hono()
    app.use('*', createAuthMiddleware(config))
    app.get('/test', (c) => c.json({ ok: true }))

    const res = await app.request('/test', {
      headers: { Authorization: 'Bearer invalid.token.here' },
    })
    expect(res.status).toBe(401)
  })

  it('accepts valid HS256 token and sets auth context', async () => {
    const secret = new TextEncoder().encode('test-secret-that-is-long-enough')
    const token = await makeJwt(secret, {
      taskIds: '*',
      scope: ['event:subscribe'],
    })

    const config: AuthConfig = {
      mode: 'jwt',
      jwt: { algorithm: 'HS256', secret: 'test-secret-that-is-long-enough' },
    }
    const app = new Hono()
    app.use('*', createAuthMiddleware(config))
    app.get('/test', (c) => {
      const auth = c.get('auth')
      return c.json({ taskIds: auth.taskIds, scope: auth.scope })
    })

    const res = await app.request('/test', {
      headers: { Authorization: `Bearer ${token}` },
    })
    expect(res.status).toBe(200)
    const body = await res.json()
    expect(body.taskIds).toBe('*')
    expect(body.scope).toContain('event:subscribe')
  })

  it('rejects expired token', async () => {
    const secret = new TextEncoder().encode('test-secret-that-is-long-enough')
    const token = await new SignJWT({ taskIds: '*', scope: ['*'] })
      .setProtectedHeader({ alg: 'HS256' })
      .setExpirationTime('-1s') // already expired
      .sign(secret)

    const config: AuthConfig = {
      mode: 'jwt',
      jwt: { algorithm: 'HS256', secret: 'test-secret-that-is-long-enough' },
    }
    const app = new Hono()
    app.use('*', createAuthMiddleware(config))
    app.get('/test', (c) => c.json({ ok: true }))

    const res = await app.request('/test', {
      headers: { Authorization: `Bearer ${token}` },
    })
    expect(res.status).toBe(401)
  })
})

describe('checkScope', () => {
  it('allows access when scope includes required permission', async () => {
    const { checkScope } = await import('../src/auth.js')
    const auth = { taskIds: '*' as const, scope: ['event:subscribe' as const], sub: undefined }
    expect(checkScope(auth, 'event:subscribe')).toBe(true)
  })

  it('allows access when scope includes wildcard', async () => {
    const { checkScope } = await import('../src/auth.js')
    const auth = { taskIds: '*' as const, scope: ['*' as const], sub: undefined }
    expect(checkScope(auth, 'task:create')).toBe(true)
  })

  it('denies access when scope does not include required permission', async () => {
    const { checkScope } = await import('../src/auth.js')
    const auth = { taskIds: '*' as const, scope: ['event:subscribe' as const], sub: undefined }
    expect(checkScope(auth, 'task:create')).toBe(false)
  })

  it('denies access when taskId not allowed', async () => {
    const { checkScope } = await import('../src/auth.js')
    const auth = { taskIds: ['task-abc'] as string[], scope: ['*' as const], sub: undefined }
    expect(checkScope(auth, 'event:subscribe', 'task-xyz')).toBe(false)
    expect(checkScope(auth, 'event:subscribe', 'task-abc')).toBe(true)
  })
})
```

**Step 2: 运行测试，确认失败**

```bash
pnpm --filter @taskcast/server vitest run tests/auth.test.ts
```

Expected: FAIL。

**Step 3: 实现 Auth 中间件**

`packages/server/src/auth.ts`:
```typescript
import { createMiddleware } from 'hono/factory'
import { jwtVerify, createRemoteJWKSet, importSPKI, type KeyLike } from 'jose'
import type { PermissionScope } from '@taskcast/core'

export interface JWTConfig {
  algorithm: 'HS256' | 'RS256' | 'ES256' | 'ES384' | 'ES512'
  secret?: string              // HMAC
  publicKey?: string           // RSA/ECC PEM
  publicKeyFile?: string       // PEM 文件路径（服务器启动时读取）
  issuer?: string
  audience?: string
}

export type AuthMode = 'none' | 'jwt' | 'custom'

export interface AuthConfig {
  mode: AuthMode
  jwt?: JWTConfig
  middleware?: (req: Request) => Promise<AuthContext | null>
}

export interface AuthContext {
  sub?: string
  taskIds: string[] | '*'
  scope: PermissionScope[]
}

// Extend Hono context variable types
declare module 'hono' {
  interface ContextVariableMap {
    auth: AuthContext
  }
}

const OPEN_AUTH: AuthContext = { taskIds: '*', scope: ['*'] }

export function createAuthMiddleware(config: AuthConfig) {
  return createMiddleware(async (c, next) => {
    if (config.mode === 'none') {
      c.set('auth', OPEN_AUTH)
      return next()
    }

    if (config.mode === 'custom' && config.middleware) {
      const ctx = await config.middleware(c.req.raw)
      if (!ctx) return c.json({ error: 'Unauthorized' }, 401)
      c.set('auth', ctx)
      return next()
    }

    if (config.mode === 'jwt' && config.jwt) {
      const authHeader = c.req.header('Authorization')
      if (!authHeader?.startsWith('Bearer ')) {
        return c.json({ error: 'Missing Bearer token' }, 401)
      }
      const token = authHeader.slice(7)
      try {
        const key = await resolveKey(config.jwt)
        const { payload } = await jwtVerify(token, key, {
          issuer: config.jwt.issuer,
          audience: config.jwt.audience,
        })
        const ctx: AuthContext = {
          sub: payload.sub,
          taskIds: (payload['taskIds'] as string[] | '*') ?? '*',
          scope: (payload['scope'] as PermissionScope[]) ?? [],
        }
        c.set('auth', ctx)
        return next()
      } catch {
        return c.json({ error: 'Invalid or expired token' }, 401)
      }
    }

    return c.json({ error: 'Unauthorized' }, 401)
  })
}

async function resolveKey(cfg: JWTConfig): Promise<KeyLike | Uint8Array> {
  if (cfg.secret) {
    return new TextEncoder().encode(cfg.secret)
  }
  if (cfg.publicKey) {
    return importSPKI(cfg.publicKey, cfg.algorithm)
  }
  if (cfg.publicKeyFile) {
    const { readFileSync } = await import('fs')
    const pem = readFileSync(cfg.publicKeyFile, 'utf8')
    return importSPKI(pem, cfg.algorithm)
  }
  throw new Error('JWT config requires secret or publicKey or publicKeyFile')
}

export function checkScope(
  auth: AuthContext,
  required: PermissionScope,
  taskId?: string,
): boolean {
  // Check task access
  if (taskId && auth.taskIds !== '*') {
    if (!auth.taskIds.includes(taskId)) return false
  }
  // Check scope
  return auth.scope.includes('*') || auth.scope.includes(required)
}
```

**Step 4: 运行测试，确认通过**

```bash
pnpm --filter @taskcast/server vitest run tests/auth.test.ts
```

Expected: PASS。

**Step 5: Commit**

```bash
git add packages/server/src/auth.ts packages/server/tests/auth.test.ts
git commit -m "feat(server): add JWT auth middleware with HS256/RS256/ES256 support and scope checking"
```

---

## Task 17: REST API 路由（任务 CRUD）

**Files:**
- Create: `packages/server/src/routes/tasks.ts`
- Create: `packages/server/tests/tasks.test.ts`

**Step 1: 写失败测试**

`packages/server/tests/tasks.test.ts`:
```typescript
import { describe, it, expect, beforeEach } from 'vitest'
import { Hono } from 'hono'
import { TaskEngine, MemoryBroadcastProvider, MemoryShortTermStore } from '@taskcast/core'
import { createTasksRouter } from '../src/routes/tasks.js'

function makeApp() {
  const store = new MemoryShortTermStore()
  const broadcast = new MemoryBroadcastProvider()
  const engine = new TaskEngine({ shortTerm: store, broadcast })
  const app = new Hono()
  // No auth for these tests
  app.use('*', async (c, next) => {
    c.set('auth', { taskIds: '*', scope: ['*'] })
    await next()
  })
  app.route('/tasks', createTasksRouter(engine))
  return { app, engine }
}

describe('POST /tasks', () => {
  it('creates a task and returns 201', async () => {
    const { app } = makeApp()
    const res = await app.request('/tasks', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ params: { prompt: 'hello' }, type: 'llm.chat' }),
    })
    expect(res.status).toBe(201)
    const body = await res.json()
    expect(body.status).toBe('pending')
    expect(body.type).toBe('llm.chat')
    expect(body.params).toEqual({ prompt: 'hello' })
    expect(body.id).toBeTruthy()
  })

  it('creates a task with user-supplied id', async () => {
    const { app } = makeApp()
    const res = await app.request('/tasks', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ id: 'my-custom-id' }),
    })
    expect(res.status).toBe(201)
    const body = await res.json()
    expect(body.id).toBe('my-custom-id')
  })
})

describe('GET /tasks/:taskId', () => {
  it('returns task by id', async () => {
    const { app, engine } = makeApp()
    const task = await engine.createTask({ type: 'test' })

    const res = await app.request(`/tasks/${task.id}`)
    expect(res.status).toBe(200)
    const body = await res.json()
    expect(body.id).toBe(task.id)
    expect(body.status).toBe('pending')
  })

  it('returns 404 for unknown task', async () => {
    const { app } = makeApp()
    const res = await app.request('/tasks/nonexistent')
    expect(res.status).toBe(404)
  })
})

describe('PATCH /tasks/:taskId/status', () => {
  it('transitions task to running', async () => {
    const { app, engine } = makeApp()
    const task = await engine.createTask({})

    const res = await app.request(`/tasks/${task.id}/status`, {
      method: 'PATCH',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ status: 'running' }),
    })
    expect(res.status).toBe(200)
    const body = await res.json()
    expect(body.status).toBe('running')
  })

  it('returns 400 on invalid transition', async () => {
    const { app, engine } = makeApp()
    const task = await engine.createTask({})

    const res = await app.request(`/tasks/${task.id}/status`, {
      method: 'PATCH',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ status: 'completed' }), // pending → completed is invalid
    })
    expect(res.status).toBe(400)
  })
})

describe('POST /tasks/:taskId/events', () => {
  it('publishes a single event', async () => {
    const { app, engine } = makeApp()
    const task = await engine.createTask({})
    await engine.transitionTask(task.id, 'running')

    const res = await app.request(`/tasks/${task.id}/events`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ type: 'llm.delta', level: 'info', data: { text: 'hi' } }),
    })
    expect(res.status).toBe(201)
    const body = await res.json()
    expect(body.type).toBe('llm.delta')
  })

  it('publishes a batch of events', async () => {
    const { app, engine } = makeApp()
    const task = await engine.createTask({})
    await engine.transitionTask(task.id, 'running')

    const res = await app.request(`/tasks/${task.id}/events`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify([
        { type: 'a', level: 'info', data: null },
        { type: 'b', level: 'info', data: null },
      ]),
    })
    expect(res.status).toBe(201)
    const body = await res.json()
    expect(body).toHaveLength(2)
  })
})
```

**Step 2: 运行测试，确认失败**

```bash
pnpm --filter @taskcast/server vitest run tests/tasks.test.ts
```

Expected: FAIL。

**Step 3: 实现 REST 路由**

`packages/server/src/routes/tasks.ts`:
```typescript
import { Hono } from 'hono'
import { z } from 'zod'
import { checkScope } from '../auth.js'
import type { TaskEngine } from '@taskcast/core'

const CreateTaskSchema = z.object({
  id: z.string().optional(),
  type: z.string().optional(),
  params: z.record(z.unknown()).optional(),
  metadata: z.record(z.unknown()).optional(),
  ttl: z.number().int().positive().optional(),
  webhooks: z.array(z.unknown()).optional(),
  cleanup: z.object({ rules: z.array(z.unknown()) }).optional(),
})

const PublishEventSchema = z.object({
  type: z.string(),
  level: z.enum(['debug', 'info', 'warn', 'error']),
  data: z.unknown(),
  seriesId: z.string().optional(),
  seriesMode: z.enum(['keep-all', 'accumulate', 'latest']).optional(),
})

export function createTasksRouter(engine: TaskEngine) {
  const router = new Hono()

  // POST /tasks — create task
  router.post('/', async (c) => {
    const auth = c.get('auth')
    if (!checkScope(auth, 'task:create')) return c.json({ error: 'Forbidden' }, 403)

    const body = await c.req.json()
    const parsed = CreateTaskSchema.safeParse(body)
    if (!parsed.success) return c.json({ error: parsed.error.flatten() }, 400)

    const task = await engine.createTask(parsed.data)
    return c.json(task, 201)
  })

  // GET /tasks/:taskId — get task
  router.get('/:taskId', async (c) => {
    const { taskId } = c.req.param()
    const auth = c.get('auth')
    if (!checkScope(auth, 'event:subscribe', taskId)) return c.json({ error: 'Forbidden' }, 403)

    const task = await engine.getTask(taskId)
    if (!task) return c.json({ error: 'Task not found' }, 404)
    return c.json(task)
  })

  // PATCH /tasks/:taskId/status — update status
  router.patch('/:taskId/status', async (c) => {
    const { taskId } = c.req.param()
    const auth = c.get('auth')
    if (!checkScope(auth, 'task:manage', taskId)) return c.json({ error: 'Forbidden' }, 403)

    const body = await c.req.json()
    const schema = z.object({
      status: z.enum(['running', 'completed', 'failed', 'timeout', 'cancelled']),
      result: z.record(z.unknown()).optional(),
      error: z.object({
        code: z.string().optional(),
        message: z.string(),
        details: z.record(z.unknown()).optional(),
      }).optional(),
    })
    const parsed = schema.safeParse(body)
    if (!parsed.success) return c.json({ error: parsed.error.flatten() }, 400)

    try {
      const task = await engine.transitionTask(taskId, parsed.data.status, {
        result: parsed.data.result,
        error: parsed.data.error,
      })
      return c.json(task)
    } catch (err) {
      const msg = err instanceof Error ? err.message : String(err)
      if (msg.toLowerCase().includes('not found')) return c.json({ error: msg }, 404)
      return c.json({ error: msg }, 400)
    }
  })

  // POST /tasks/:taskId/events — publish event(s)
  router.post('/:taskId/events', async (c) => {
    const { taskId } = c.req.param()
    const auth = c.get('auth')
    if (!checkScope(auth, 'event:publish', taskId)) return c.json({ error: 'Forbidden' }, 403)

    const body = await c.req.json()
    const isBatch = Array.isArray(body)
    const inputs = isBatch ? body : [body]

    const events = []
    for (const input of inputs) {
      const parsed = PublishEventSchema.safeParse(input)
      if (!parsed.success) return c.json({ error: parsed.error.flatten() }, 400)
      try {
        const event = await engine.publishEvent(taskId, parsed.data)
        events.push(event)
      } catch (err) {
        const msg = err instanceof Error ? err.message : String(err)
        if (msg.toLowerCase().includes('not found')) return c.json({ error: msg }, 404)
        return c.json({ error: msg }, 400)
      }
    }

    return c.json(isBatch ? events : events[0], 201)
  })

  // GET /tasks/:taskId/events/history — REST history query
  router.get('/:taskId/events/history', async (c) => {
    const { taskId } = c.req.param()
    const auth = c.get('auth')
    if (!checkScope(auth, 'event:history', taskId)) return c.json({ error: 'Forbidden' }, 403)

    const task = await engine.getTask(taskId)
    if (!task) return c.json({ error: 'Task not found' }, 404)

    const sinceIndex = c.req.query('since.index')
    const sinceTimestamp = c.req.query('since.timestamp')
    const sinceId = c.req.query('since.id')

    const events = await engine.getEvents(taskId, {
      since: {
        id: sinceId,
        index: sinceIndex ? Number(sinceIndex) : undefined,
        timestamp: sinceTimestamp ? Number(sinceTimestamp) : undefined,
      },
    })
    return c.json(events)
  })

  return router
}
```

**Step 4: 在 TaskEngine 增加 getEvents 方法**（回 Phase 1 补充）

在 `packages/core/src/engine.ts` 的 TaskEngine 类中添加：
```typescript
async getEvents(taskId: string, opts?: EventQueryOptions): Promise<TaskEvent[]> {
  return this.opts.shortTerm.getEvents(taskId, opts)
}
```

**Step 5: 运行测试，确认通过**

```bash
pnpm --filter @taskcast/server vitest run tests/tasks.test.ts
```

Expected: PASS。

**Step 6: Commit**

```bash
git add packages/server/src/routes/ packages/server/tests/tasks.test.ts packages/core/src/engine.ts
git commit -m "feat(server): add REST task/event routes with scope-based auth"
```

---

## Task 18: SSE 订阅端点

**Files:**
- Create: `packages/server/src/routes/sse.ts`
- Create: `packages/server/tests/sse.test.ts`

**Step 1: 写失败测试**

`packages/server/tests/sse.test.ts`:
```typescript
import { describe, it, expect, beforeEach } from 'vitest'
import { Hono } from 'hono'
import { TaskEngine, MemoryBroadcastProvider, MemoryShortTermStore } from '@taskcast/core'
import { createSSERouter } from '../src/routes/sse.js'

function makeApp() {
  const store = new MemoryShortTermStore()
  const broadcast = new MemoryBroadcastProvider()
  const engine = new TaskEngine({ shortTerm: store, broadcast })
  const app = new Hono()
  app.use('*', async (c, next) => {
    c.set('auth', { taskIds: '*', scope: ['*'] })
    await next()
  })
  app.route('/tasks', createSSERouter(engine))
  return { app, engine, broadcast }
}

async function collectSSEEvents(
  res: Response,
  count: number,
): Promise<Array<{ event: string; data: string }>> {
  const reader = res.body!.getReader()
  const decoder = new TextDecoder()
  const collected: Array<{ event: string; data: string }> = []
  let buffer = ''

  while (collected.length < count) {
    const { done, value } = await reader.read()
    if (done) break
    buffer += decoder.decode(value, { stream: true })

    const blocks = buffer.split('\n\n')
    buffer = blocks.pop() ?? ''

    for (const block of blocks) {
      if (!block.trim()) continue
      const lines = block.split('\n')
      const eventLine = lines.find((l) => l.startsWith('event:'))
      const dataLine = lines.find((l) => l.startsWith('data:'))
      if (eventLine && dataLine) {
        collected.push({
          event: eventLine.replace('event:', '').trim(),
          data: dataLine.replace('data:', '').trim(),
        })
      }
    }
  }

  reader.cancel()
  return collected
}

describe('GET /tasks/:taskId/events (SSE)', () => {
  it('returns 404 for unknown task', async () => {
    const { app } = makeApp()
    const res = await app.request('/tasks/nonexistent/events')
    expect(res.status).toBe(404)
  })

  it('replays history and delivers taskcast.done for completed task', async () => {
    const { app, engine } = makeApp()
    const task = await engine.createTask({})
    await engine.transitionTask(task.id, 'running')
    await engine.publishEvent(task.id, { type: 'llm.delta', level: 'info', data: { text: 'hi' } })
    await engine.transitionTask(task.id, 'completed', { result: { answer: 42 } })

    const res = await app.request(`/tasks/${task.id}/events`)
    expect(res.headers.get('content-type')).toContain('text/event-stream')

    // Collect: taskcast:status(running) + llm.delta + taskcast:status(completed) + taskcast.done
    const events = await collectSSEEvents(res, 4)
    const types = events.map((e) => e.event)
    expect(types).toContain('taskcast.event')
    expect(types).toContain('taskcast.done')
  })

  it('filters events by type wildcard', async () => {
    const { app, engine } = makeApp()
    const task = await engine.createTask({})
    await engine.transitionTask(task.id, 'running')
    await engine.publishEvent(task.id, { type: 'llm.delta', level: 'info', data: null })
    await engine.publishEvent(task.id, { type: 'tool.call', level: 'info', data: null })
    await engine.transitionTask(task.id, 'completed')

    const res = await app.request(`/tasks/${task.id}/events?types=llm.*&includeStatus=false`)
    const events = await collectSSEEvents(res, 2) // llm.delta + taskcast.done
    const eventTypes = events.filter(e => e.event === 'taskcast.event')
      .map(e => JSON.parse(e.data).type)
    expect(eventTypes).toEqual(['llm.delta'])
  })
})
```

**Step 2: 运行测试，确认失败**

```bash
pnpm --filter @taskcast/server vitest run tests/sse.test.ts
```

Expected: FAIL。

**Step 3: 实现 SSE 路由**

`packages/server/src/routes/sse.ts`:
```typescript
import { Hono } from 'hono'
import { streamSSE } from 'hono/streaming'
import { applyFilteredIndex } from '@taskcast/core'
import { checkScope } from '../auth.js'
import type { TaskEngine, TaskEvent, SubscribeFilter, SSEEnvelope } from '@taskcast/core'

function parseFilter(query: Record<string, string | string[]>): SubscribeFilter {
  const get = (k: string) => (Array.isArray(query[k]) ? query[k][0] : query[k]) as string | undefined

  return {
    since: {
      id: get('since.id'),
      index: get('since.index') !== undefined ? Number(get('since.index')) : undefined,
      timestamp: get('since.timestamp') !== undefined ? Number(get('since.timestamp')) : undefined,
    },
    types: get('types')?.split(',').filter(Boolean),
    levels: get('levels')?.split(',').filter(Boolean) as SubscribeFilter['levels'],
    includeStatus: get('includeStatus') !== 'false',
    wrap: get('wrap') !== 'false', // default true
  }
}

function toEnvelope(event: TaskEvent, filteredIndex: number, rawIndex: number): SSEEnvelope {
  return {
    filteredIndex,
    rawIndex,
    eventId: event.id,
    taskId: event.taskId,
    type: event.type,
    timestamp: event.timestamp,
    level: event.level,
    data: event.data,
    seriesId: event.seriesId,
    seriesMode: event.seriesMode,
  }
}

export function createSSERouter(engine: TaskEngine) {
  const router = new Hono()

  router.get('/:taskId/events', async (c) => {
    const { taskId } = c.req.param()
    const auth = c.get('auth')
    if (!checkScope(auth, 'event:subscribe', taskId)) return c.json({ error: 'Forbidden' }, 403)

    const task = await engine.getTask(taskId)
    if (!task) return c.json({ error: 'Task not found' }, 404)

    const filter = parseFilter(c.req.query() as Record<string, string>)
    const wrap = filter.wrap ?? true

    return streamSSE(c, async (stream) => {
      const TERMINAL = new Set(['completed', 'failed', 'timeout', 'cancelled'])

      const sendEvent = async (event: TaskEvent, filteredIndex: number) => {
        const payload = wrap
          ? toEnvelope(event, filteredIndex, event.index)
          : event
        await stream.writeSSE({
          event: 'taskcast.event',
          data: JSON.stringify(payload),
          id: event.id,
        })
      }

      const sendDone = async (reason: string) => {
        await stream.writeSSE({
          event: 'taskcast.done',
          data: JSON.stringify({ reason }),
        })
      }

      // Replay history
      const history = await engine.getEvents(taskId)
      const filtered = applyFilteredIndex(history, filter)
      for (const { event, filteredIndex } of filtered) {
        await sendEvent(event, filteredIndex)
      }

      // If task is already terminal, send done and close
      if (TERMINAL.has(task.status)) {
        await sendDone(task.status)
        return
      }

      // Subscribe to live events
      let nextFilteredIndex = filtered.length > 0
        ? (filtered[filtered.length - 1]!.filteredIndex + 1)
        : 0

      await new Promise<void>((resolve) => {
        const unsub = engine.subscribe(taskId, async (event) => {
          const { matchesFilter } = require('@taskcast/core')
          if (!matchesFilter(event, filter)) return

          await sendEvent(event, nextFilteredIndex++)

          if (event.type === 'taskcast:status') {
            const status = (event.data as { status: string }).status
            if (TERMINAL.has(status)) {
              await sendDone(status)
              unsub()
              resolve()
            }
          }
        })

        stream.onAbort(() => {
          unsub()
          resolve()
        })
      })
    })
  })

  return router
}
```

**Step 4: 在 TaskEngine 增加 subscribe 方法**（回 Phase 1 补充）

在 `packages/core/src/engine.ts` 的 TaskEngine 类中添加：
```typescript
subscribe(taskId: string, handler: (event: TaskEvent) => void): () => void {
  return this.opts.broadcast.subscribe(taskId, handler)
}
```

修复 SSE 路由中的 require 调用改为 import（在文件顶部增加）：
```typescript
import { applyFilteredIndex, matchesFilter } from '@taskcast/core'
```
同时从 `createSSERouter` 内部删除动态 `require`，直接使用导入的 `matchesFilter`。

**Step 5: 运行测试，确认通过**

```bash
pnpm --filter @taskcast/server vitest run tests/sse.test.ts
```

Expected: PASS。

**Step 6: Commit**

```bash
git add packages/server/src/routes/sse.ts packages/server/tests/sse.test.ts packages/core/src/engine.ts
git commit -m "feat(server): add SSE subscription endpoint with history replay and live streaming"
```

---

## Task 19: Webhook 投递引擎

**Files:**
- Create: `packages/server/src/webhook.ts`
- Create: `packages/server/tests/webhook.test.ts`

**Step 1: 写失败测试**

`packages/server/tests/webhook.test.ts`:
```typescript
import { describe, it, expect, vi, beforeEach } from 'vitest'
import { WebhookDelivery } from '../src/webhook.js'
import type { TaskEvent, WebhookConfig } from '@taskcast/core'

const makeEvent = (): TaskEvent => ({
  id: 'evt-1',
  taskId: 'task-1',
  index: 0,
  timestamp: 1700000000000,
  type: 'llm.delta',
  level: 'info',
  data: { text: 'hello' },
})

describe('WebhookDelivery', () => {
  it('sends POST request to webhook URL', async () => {
    const fetch = vi.fn().mockResolvedValue(new Response('ok', { status: 200 }))
    const delivery = new WebhookDelivery({ fetch })

    const config: WebhookConfig = {
      url: 'https://example.com/hook',
      retry: { retries: 0, backoff: 'fixed', initialDelayMs: 0, maxDelayMs: 0, timeoutMs: 5000 },
    }
    await delivery.send(makeEvent(), config)
    expect(fetch).toHaveBeenCalledOnce()
    const [url, opts] = fetch.mock.calls[0]!
    expect(url).toBe('https://example.com/hook')
    expect(opts.method).toBe('POST')
    expect(opts.headers['Content-Type']).toBe('application/json')
    expect(opts.headers['X-Taskcast-Event']).toBe('llm.delta')
    expect(opts.headers['X-Taskcast-Timestamp']).toBeTruthy()
  })

  it('includes HMAC signature when secret is configured', async () => {
    const fetch = vi.fn().mockResolvedValue(new Response('ok', { status: 200 }))
    const delivery = new WebhookDelivery({ fetch })

    const config: WebhookConfig = {
      url: 'https://example.com/hook',
      secret: 'test-secret',
      retry: { retries: 0, backoff: 'fixed', initialDelayMs: 0, maxDelayMs: 0, timeoutMs: 5000 },
    }
    await delivery.send(makeEvent(), config)
    const [, opts] = fetch.mock.calls[0]!
    expect(opts.headers['X-Taskcast-Signature']).toMatch(/^sha256=/)
  })

  it('retries on non-2xx response', async () => {
    const fetch = vi
      .fn()
      .mockResolvedValueOnce(new Response('error', { status: 500 }))
      .mockResolvedValueOnce(new Response('ok', { status: 200 }))
    const delivery = new WebhookDelivery({ fetch })

    const config: WebhookConfig = {
      url: 'https://example.com/hook',
      retry: { retries: 3, backoff: 'fixed', initialDelayMs: 0, maxDelayMs: 0, timeoutMs: 5000 },
    }
    await delivery.send(makeEvent(), config)
    expect(fetch).toHaveBeenCalledTimes(2)
  })

  it('throws after exhausting retries', async () => {
    const fetch = vi.fn().mockResolvedValue(new Response('error', { status: 500 }))
    const delivery = new WebhookDelivery({ fetch })

    const config: WebhookConfig = {
      url: 'https://example.com/hook',
      retry: { retries: 2, backoff: 'fixed', initialDelayMs: 0, maxDelayMs: 0, timeoutMs: 5000 },
    }
    await expect(delivery.send(makeEvent(), config)).rejects.toThrow(/webhook delivery failed/i)
    expect(fetch).toHaveBeenCalledTimes(3) // 1 original + 2 retries
  })
})
```

**Step 2: 运行测试，确认失败**

```bash
pnpm --filter @taskcast/server vitest run tests/webhook.test.ts
```

Expected: FAIL。

**Step 3: 实现 Webhook 投递**

`packages/server/src/webhook.ts`:
```typescript
import { createHmac } from 'crypto'
import { matchesFilter } from '@taskcast/core'
import type { TaskEvent, WebhookConfig, RetryConfig } from '@taskcast/core'

interface WebhookDeliveryOptions {
  fetch?: typeof globalThis.fetch
}

const DEFAULT_RETRY: RetryConfig = {
  retries: 3,
  backoff: 'exponential',
  initialDelayMs: 1000,
  maxDelayMs: 30000,
  timeoutMs: 5000,
}

export class WebhookDelivery {
  private fetch: typeof globalThis.fetch

  constructor(opts: WebhookDeliveryOptions = {}) {
    this.fetch = opts.fetch ?? globalThis.fetch
  }

  async send(event: TaskEvent, config: WebhookConfig): Promise<void> {
    // Check filter
    if (config.filter && !matchesFilter(event, config.filter)) return

    const retry = { ...DEFAULT_RETRY, ...config.retry }
    const body = JSON.stringify(event)
    const timestamp = String(Math.floor(Date.now() / 1000))
    const signature = config.secret ? this._sign(body, config.secret) : undefined

    const headers: Record<string, string> = {
      'Content-Type': 'application/json',
      'X-Taskcast-Event': event.type,
      'X-Taskcast-Timestamp': timestamp,
      ...(signature ? { 'X-Taskcast-Signature': signature } : {}),
    }

    let lastError: Error | null = null
    for (let attempt = 0; attempt <= retry.retries; attempt++) {
      if (attempt > 0) {
        await this._sleep(this._backoffMs(retry, attempt))
      }
      try {
        const controller = new AbortController()
        const timeout = setTimeout(() => controller.abort(), retry.timeoutMs)
        const res = await this.fetch(config.url, {
          method: 'POST',
          headers,
          body,
          signal: controller.signal,
        })
        clearTimeout(timeout)
        if (res.ok) return
        lastError = new Error(`HTTP ${res.status}`)
      } catch (err) {
        lastError = err instanceof Error ? err : new Error(String(err))
      }
    }

    throw new Error(`Webhook delivery failed after ${retry.retries + 1} attempts: ${lastError?.message}`)
  }

  private _sign(body: string, secret: string): string {
    const hmac = createHmac('sha256', secret)
    hmac.update(body)
    return `sha256=${hmac.digest('hex')}`
  }

  private _backoffMs(retry: RetryConfig, attempt: number): number {
    if (retry.backoff === 'fixed') return retry.initialDelayMs
    if (retry.backoff === 'linear') return retry.initialDelayMs * attempt
    // exponential
    return Math.min(retry.initialDelayMs * Math.pow(2, attempt - 1), retry.maxDelayMs)
  }

  private _sleep(ms: number): Promise<void> {
    return new Promise((resolve) => setTimeout(resolve, ms))
  }
}
```

**Step 4: 运行测试，确认通过**

```bash
pnpm --filter @taskcast/server vitest run tests/webhook.test.ts
```

Expected: PASS。

**Step 5: 创建 server 主入口**

`packages/server/src/index.ts`:
```typescript
export { createAuthMiddleware, checkScope } from './auth.js'
export type { AuthConfig, AuthContext, JWTConfig } from './auth.js'
export { createTasksRouter } from './routes/tasks.js'
export { createSSERouter } from './routes/sse.js'
export { WebhookDelivery } from './webhook.js'

import { Hono } from 'hono'
import { createAuthMiddleware } from './auth.js'
import { createTasksRouter } from './routes/tasks.js'
import { createSSERouter } from './routes/sse.js'
import type { AuthConfig } from './auth.js'
import type { TaskEngine } from '@taskcast/core'

export interface TaskcastServerOptions {
  engine: TaskEngine
  auth?: AuthConfig
}

/**
 * Creates a Hono app with all taskcast routes mounted.
 * Can be used standalone or mounted into an existing Hono app.
 */
export function createTaskcastApp(opts: TaskcastServerOptions): Hono {
  const app = new Hono()
  app.use('*', createAuthMiddleware(opts.auth ?? { mode: 'none' }))
  app.route('/tasks', createTasksRouter(opts.engine))
  app.route('/tasks', createSSERouter(opts.engine))
  return app
}
```

**Step 6: Commit**

```bash
git add packages/server/src/ packages/server/tests/webhook.test.ts
git commit -m "feat(server): add webhook delivery engine with HMAC signing and exponential backoff"
```

---

## Phase 3 完成检查

```bash
# 运行所有 server 测试
pnpm --filter @taskcast/server vitest run

# TypeScript 检查
pnpm --filter @taskcast/server exec tsc --noEmit
```

**下一步：** 继续 [Phase 4: SDKs + CLI](./2026-02-28-taskcast-04-sdks-cli.md)
