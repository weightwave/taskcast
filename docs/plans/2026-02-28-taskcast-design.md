# Taskcast Design Document

**Date:** 2026-02-28
**Status:** Approved

## Implementation Plans

| Phase | 内容 | 文档 |
|---|---|---|
| 1 | Monorepo 搭建 + `@taskcast/core` 全部内部逻辑 | [01-monorepo-core](./2026-02-28-taskcast-01-monorepo-core.md) |
| 2 | `@taskcast/redis` + `@taskcast/postgres` 存储适配器 | [02-adapters](./2026-02-28-taskcast-02-adapters.md) |
| 3 | `@taskcast/server` HTTP + SSE + Auth + Webhook | [03-server](./2026-02-28-taskcast-03-server.md) |
| 4 | `@taskcast/server-sdk` + `client` + `react` + `cli` + 配置加载 | [04-sdks-cli](./2026-02-28-taskcast-04-sdks-cli.md) |
| 5 | `@taskcast/sentry` + 集成测试 + 并发测试 | [05-sentry-tests](./2026-02-28-taskcast-05-sentry-tests.md) |

## Overview

Taskcast 是一个统一的长周期任务追踪与管理服务，专为 LLM 流式输出、流式 Agent 等场景设计。解决单页面 SSE 刷新丢失、多客户端无法订阅同一任务等问题。

**核心能力：**
- 任务状态管理（生命周期、持久化）
- SSE 实时订阅（断点续传、过滤、重放）
- Webhook 回调（全局/任务级，含签名验证）
- 多层存储抽象（广播/短期/长期）
- 可选认证（无认证/JWT/自定义中间件）

---

## Architecture

**方案：SDK-First**

核心逻辑无 HTTP/基础设施依赖，框架层是薄封装，适配器可替换。

### 部署形态

```
嵌入模式：
  你的服务器 → import @taskcast/core + adapters
               mount @taskcast/server (Hono router) 到自己的路由

远程模式：
  你的服务器 → @taskcast/server-sdk（纯 HTTP 客户端）→ standalone taskcast server
  浏览器     → @taskcast/client / @taskcast/react → standalone taskcast server SSE
```

### Monorepo 包结构

```
packages/
├── core/          @taskcast/core         纯引擎，无 HTTP 依赖
├── server/        @taskcast/server       Hono HTTP 服务器（SSE + REST 路由）
├── server-sdk/    @taskcast/server-sdk   生产者 HTTP 客户端（远程模式）
├── client/        @taskcast/client       浏览器 SSE 订阅客户端
├── react/         @taskcast/react        React hooks 封装
├── cli/           @taskcast/cli          npx taskcast 独立服务器入口
├── redis/         @taskcast/redis        Redis 适配器（广播层 + 短期存储层）
├── postgres/      @taskcast/postgres     Postgres 适配器（长期存储层）
└── sentry/        @taskcast/sentry       可选 Sentry 集成
```

**技术选型：**
- 运行时：Node.js / Bun 兼容
- HTTP 框架：Hono（跨运行时，天然支持 SSE）
- 包管理：pnpm monorepo
- 测试框架：Vitest

---

## Section 1: Core Data Model

### Task

```typescript
interface Task {
  id: string                          // 用户指定或自动生成 ULID
  type?: string                       // 任务类型，如 "llm.chat"，用于清理规则匹配
  status: TaskStatus
  params?: Record<string, unknown>    // 任务输入参数（创建时写入，只读）
  result?: Record<string, unknown>    // 完成时的最终结果
  error?: {                           // 失败/超时时的错误信息
    code?: string
    message: string
    details?: Record<string, unknown>
  }
  metadata?: Record<string, unknown>
  createdAt: number                   // ms timestamp
  updatedAt: number
  completedAt?: number
  ttl?: number                        // 秒，过期自动变 timeout
  authConfig?: TaskAuthConfig         // 任务级权限配置
  webhooks?: WebhookConfig[]          // 任务级 webhook
  cleanup?: { rules: CleanupRule[] }  // 任务级清理规则（覆盖全局）
}

type TaskStatus =
  | 'pending'
  | 'running'
  | 'completed'
  | 'failed'
  | 'timeout'
  | 'cancelled'
```

### TaskEvent（消息）

```typescript
interface TaskEvent {
  id: string           // ULID，全局唯一
  taskId: string
  index: number        // 单任务内单调递增（原始全局序号）
  timestamp: number    // ms
  type: string         // 用户自定义，支持 wildcard 过滤，如 "llm.delta"
  level: 'debug' | 'info' | 'warn' | 'error'
  data: unknown        // 任意 JSON

  // 流式序列（可选）
  seriesId?: string
  seriesMode?: 'keep-all' | 'accumulate' | 'latest'
}
```

任务状态变化时自动注入 `type: "taskcast:status"` 内置事件。

### SSE Envelope（wrap=true 时，默认）

```typescript
interface SSEEnvelope {
  filteredIndex: number    // 过滤后序号（0,1,2...），用于断点续传
  rawIndex: number         // 原始全局序号，供调试
  eventId: string
  taskId: string
  type: string
  timestamp: number
  level: string
  data: unknown
  seriesId?: string
  seriesMode?: string
}
```

### 订阅过滤

```typescript
interface SubscribeFilter {
  since?: {
    id?: string        // 从某 event ULID 之后（跨 filter 精确续传）
    index?: number     // 从过滤后第 N 条之后（同 filter 重连）
    timestamp?: number
  }
  types?: string[]     // 支持 wildcard，如 ["llm.*", "tool.call"]
  levels?: Level[]
  includeStatus?: boolean   // 是否含 taskcast:status 事件（默认 true）
  wrap?: boolean            // 是否加 envelope（默认 true）
}
```

---

## Section 2: Storage Architecture

### 三层抽象接口

```typescript
// 广播层：实时 fan-out，无持久化保证
interface BroadcastProvider {
  publish(channel: string, event: TaskEvent): Promise<void>
  subscribe(channel: string, handler: (event: TaskEvent) => void): () => void
}

// 短期层：有序事件缓冲 + 任务状态，带 TTL
interface ShortTermStore {
  saveTask(task: Task): Promise<void>
  getTask(taskId: string): Promise<Task | null>
  appendEvent(taskId: string, event: TaskEvent): Promise<void>
  getEvents(taskId: string, opts: EventQueryOptions): Promise<TaskEvent[]>
  setTTL(taskId: string, ttl: number): Promise<void>
}

// 长期层：永久归档（可选）
interface LongTermStore {
  saveTask(task: Task): Promise<void>
  getTask(taskId: string): Promise<Task | null>
  saveEvent(event: TaskEvent): Promise<void>
  getEvents(taskId: string, opts: EventQueryOptions): Promise<TaskEvent[]>
}
```

### 写入流程

```
发消息
  → 序列合并处理（seriesMode）
  → 短期层 appendEvent（同步，保证有序）
  → 广播层 publish（同步，实时推送）
  → 长期层 saveEvent（异步，不阻塞主流程）
```

### 序列消息（seriesId）合并规则

在写入短期层前处理：
- `keep-all`：直接追加，广播原始 delta
- `accumulate`：读取该 series 最新累积值，拼接 `data.text`，存合并结果，广播 delta
- `latest`：替换该 series 上一条记录，广播最新值

### 适配器

| 适配器 | 实现层 |
|---|---|
| `@taskcast/redis` | BroadcastProvider + ShortTermStore |
| `@taskcast/postgres` | LongTermStore |
| 内置内存适配器 | BroadcastProvider + ShortTermStore（测试/开发用） |

长期层可选，不配置时仅靠短期层（适合短生命周期任务）。

### 生命周期 Hooks（含 Sentry 接入点）

```typescript
interface TaskcastHooks {
  onTaskFailed?(task: Task, error: TaskError): void
  onTaskTimeout?(task: Task): void
  onUnhandledError?(err: unknown, context: ErrorContext): void
  onEventDropped?(event: TaskEvent, reason: string): void
  onWebhookFailed?(config: WebhookConfig, err: unknown): void
  onSSEConnect?(taskId: string, clientId: string): void
  onSSEDisconnect?(taskId: string, clientId: string, duration: number): void
}
```

---

## Section 3: HTTP API

### REST 端点

```
POST   /tasks                         创建任务
GET    /tasks/:taskId                 查询任务状态（含 params/result/error）
PATCH  /tasks/:taskId/status          更新任务状态
DELETE /tasks/:taskId                 删除任务

POST   /tasks/:taskId/events          发布消息（单条或批量）
GET    /tasks/:taskId/events          SSE 订阅
GET    /tasks/:taskId/events/history  查询历史消息（REST）
```

### SSE 订阅参数

```
GET /tasks/:taskId/events
  ?since.id=01HXXX           从某 event id 之后
  &since.index=5             从过滤后第 N 条之后
  &since.timestamp=1700000   从某时间戳之后
  &types=llm.*,tool.call     wildcard 类型过滤（逗号分隔）
  &levels=info,warn,error    等级过滤
  &includeStatus=true        是否含 taskcast:status 事件
  &wrap=true                 是否加 envelope（默认 true）
```

### SSE 订阅行为

```
任务 pending  → 挂起等待，变 running 后自动 replay 历史 + 订阅实时
任务 running  → replay 历史（按 filter），再订阅实时，终态后自动断开
任务终态      → replay 历史（默认），或直接返回 result/error
任务不存在    → 404
```

### SSE Wire 格式

```
// 普通事件（wrap=true，默认）
event: taskcast.event
id: 01HXXX
data: {"filteredIndex":3,"rawIndex":7,"eventId":"01HXXX","taskId":"xxx","type":"llm.delta",...}

// 内置状态事件
event: taskcast.status
data: {"taskId":"xxx","status":"completed","result":{...}}

// 终态关闭信号
event: taskcast.done
data: {"reason":"completed"}
```

### 发布消息

```typescript
// 单条
POST /tasks/:taskId/events
{ type: "llm.delta", level: "info", data: { text: "hello" },
  seriesId: "msg-1", seriesMode: "accumulate" }

// 批量
POST /tasks/:taskId/events
[{ type: "tool.call", ... }, { type: "tool.result", ... }]
```

### Webhook 回调

**配置层级：** 全局级（所有任务）和任务级（单个任务）。

```typescript
interface WebhookConfig {
  url: string
  filter?: SubscribeFilter
  secret?: string                // HMAC-SHA256 签名密钥
  wrap?: boolean                 // 默认 true
  retry?: RetryConfig
}

interface RetryConfig {
  retries: number                // 默认 3
  backoff: 'fixed' | 'exponential' | 'linear'
  initialDelayMs: number         // 默认 1000
  maxDelayMs: number             // 默认 30000
  timeoutMs: number              // 默认 5000
}
```

**HTTP 请求头：**

```
X-Taskcast-Signature: sha256=<hmac-sha256(secret, body)>
X-Taskcast-Timestamp: 1700000000
X-Taskcast-Event: llm.delta
```

### 权限 Scope

```typescript
type PermissionScope =
  | 'task:create'
  | 'task:manage'       // 改状态、删除
  | 'event:publish'
  | 'event:subscribe'
  | 'event:history'
  | 'webhook:create'
  | '*'
```

### 任务清理规则

```typescript
interface CleanupRule {
  name?: string
  match?: {
    taskTypes?: string[]       // wildcard，如 "llm.*"
    status?: TaskStatus[]      // 哪些终态触发（默认所有终态）
  }
  trigger: {
    afterMs?: number           // 进入终态后延迟执行（默认立即）
  }
  target: 'all' | 'events' | 'task'
  eventFilter?: {
    types?: string[]
    levels?: Level[]
    olderThanMs?: number
    seriesMode?: SeriesMode[]
  }
}
```

---

## Section 4: Authentication

### 配置

```typescript
createTaskcast({
  auth: {
    mode: 'none' | 'jwt' | 'custom',
    jwt: {
      algorithm: 'HS256' | 'RS256' | 'ES256' | 'ES384' | ...,
      secret?: string,
      publicKey?: string,
      publicKeyFile?: string,
      issuer?: string,
      audience?: string,
    },
    middleware?: (req: Request) => Promise<AuthContext | null>,
  }
})
```

### JWT Payload

```typescript
interface TaskcastJWTPayload {
  sub?: string
  taskIds: string[] | '*'
  scope: PermissionScope[]
  exp?: number
}
```

### 任务级权限（authConfig）

```typescript
interface TaskAuthConfig {
  rules: Array<{
    match: { scope: PermissionScope[] }
    require: {
      claims?: Record<string, unknown>
      sub?: string[]
    }
  }>
}
```

---

## Section 5: Configuration Loading

**优先级：** CLI 参数 > 环境变量 > 配置文件 > 默认值

### 支持格式（按优先级查找）

```
taskcast.config.ts   完整功能（函数、自定义适配器、中间件）
taskcast.config.js
taskcast.config.mjs
taskcast.config.yaml / .yml
taskcast.config.json
```

### 环境变量

```bash
TASKCAST_PORT=3721
TASKCAST_AUTH_MODE=jwt
TASKCAST_JWT_SECRET=xxx
TASKCAST_JWT_PUBLIC_KEY_FILE=/run/secrets/jwt.pub
TASKCAST_REDIS_URL=redis://localhost:6379
TASKCAST_POSTGRES_URL=postgres://...
TASKCAST_LOG_LEVEL=info
SENTRY_DSN=https://xxx@sentry.io/123
```

### YAML 示例

```yaml
port: 3721
logLevel: info

auth:
  mode: jwt
  jwt:
    algorithm: RS256
    publicKeyFile: /run/secrets/jwt.pub

adapters:
  broadcast:
    provider: redis
    url: ${REDIS_URL}
  shortTerm:
    provider: redis
    url: ${REDIS_URL}
  longTerm:
    provider: postgres
    url: ${DATABASE_URL}

sentry:
  dsn: ${SENTRY_DSN}
  captureTaskFailures: true
  captureTaskTimeouts: true
  captureUnhandledErrors: true
  captureDroppedEvents: true
  captureStorageErrors: true
  captureBroadcastErrors: true
  traceSSEConnections: false
  traceEventPublish: false

webhook:
  defaultRetry:
    retries: 3
    backoff: exponential
    initialDelayMs: 1000
    maxDelayMs: 30000
    timeoutMs: 5000

cleanup:
  rules:
    - match:
        taskTypes: ["llm.*"]
      trigger:
        afterMs: 3600000
      target: events
      eventFilter:
        levels: [debug]
    - trigger:
        afterMs: 86400000
      target: events
    - trigger:
        afterMs: 604800000
      target: all
```

### JSON/YAML 功能限制

| 功能 | TS/JS | JSON/YAML |
|---|---|---|
| 自定义 auth 中间件 | ✅ | ❌ |
| 自定义适配器实例 | ✅ | ❌ 只支持内置 provider 名称 |
| Sentry 自定义 hooks | ✅ | ❌ |
| Sentry DSN 配置 | ✅ | ✅ 内置集成 |
| 环境变量插值 `${VAR}` | ✅ | ✅ |
| 文件路径引用 | ✅ | ✅ |

---

## Section 6: Testing Strategy

### 分层

```
packages/core/tests/
  unit/           纯逻辑，无 IO，使用内存适配器
  integration/    需要真实 Redis/Postgres（testcontainers）

packages/server/tests/
  integration/    HTTP + SSE 端到端
  concurrent/     并发压力测试
```

### 单元测试覆盖点

| 测试对象 | 覆盖场景 |
|---|---|
| 任务状态机 | 合法/非法状态转换，并发转换竞争 |
| 消息序列合并 | accumulate 文本拼接、latest 替换、keep-all、边界（空/超长/乱序） |
| 事件过滤器 | wildcard 匹配、等级过滤、filteredIndex 计算、since 偏移 |
| 清理规则引擎 | 多规则叠加、taskType 匹配、时间触发 |
| JWT 验证 | 各算法、过期、scope 不足、taskId 不匹配、任务级 authConfig |
| Webhook 签名 | HMAC 生成/验证、时间戳防重放 |

### 并发测试示例

```typescript
// 100 个客户端同时订阅同一任务
test('100 concurrent SSE subscribers receive all events in order', async () => {
  const subs = await Promise.all(
    Array.from({ length: 100 }, () => subscribe(taskId))
  )
  await publishBurst(taskId, 1000)
  for (const sub of subs) {
    expect(sub.received).toHaveLength(1000)
    expect(sub.received.map(e => e.filteredIndex)).toEqual([...Array(1000).keys()])
  }
})

// 并发状态转换安全性
test('concurrent status transitions are safe', async () => {
  const task = await createTask()
  const results = await Promise.allSettled(
    Array.from({ length: 10 }, () => completeTask(task.id))
  )
  expect(results.filter(r => r.status === 'fulfilled')).toHaveLength(1)
})
```

### 工具选型

| 用途 | 工具 |
|---|---|
| 测试框架 | Vitest |
| 容器化依赖 | testcontainers-node |
| SSE 测试客户端 | 内置 eventsource polyfill |
| 覆盖率 | @vitest/coverage-v8，目标 >85% |
