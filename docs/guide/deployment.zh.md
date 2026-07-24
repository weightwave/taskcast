# 部署指南

## 部署模式

### 嵌入模式

将 Taskcast 引擎嵌入到你现有的服务器中。适用于已有 Node.js/Bun 后端、希望在同进程中管理任务的场景。

```typescript
import { TaskEngine, MemoryBroadcastProvider, MemoryShortTermStore } from '@taskcast/core'
import { createTaskcastApp } from '@taskcast/server'
import { createRedisAdapters } from '@taskcast/redis'
import { createPostgresAdapter } from '@taskcast/postgres'
import Redis from 'ioredis'

// 创建 Redis 适配器
const pubClient = new Redis(process.env.REDIS_URL)
const subClient = new Redis(process.env.REDIS_URL)
const storeClient = new Redis(process.env.REDIS_URL)
const { broadcast, shortTermStore } = createRedisAdapters(pubClient, subClient, storeClient)

// 创建 PostgreSQL 适配器（可选）
const longTermStore = await createPostgresAdapter({
  url: process.env.DATABASE_URL!,
})

// 创建引擎
const engine = new TaskEngine({
  broadcast,
  shortTermStore,
  longTermStore, // 可选
})

// 创建 HTTP 应用
const app = createTaskcastApp({
  engine,
  auth: {
    mode: 'jwt',
    jwt: {
      algorithm: 'HS256',
      secret: process.env.JWT_SECRET!,
    },
  },
})

// 挂载到你的 Hono 应用
import { Hono } from 'hono'
const mainApp = new Hono()
mainApp.route('/taskcast', app)

export default mainApp
```

### 远程模式

Taskcast 作为独立服务运行，你的后端通过 HTTP SDK 与之通信。适用于微服务架构或希望独立部署的场景。

**启动服务：**

```bash
npx @taskcast/cli -c taskcast.config.yaml
```

**后端集成（生产者）：**

```typescript
import { TaskcastServerClient } from '@taskcast/server-sdk'

const taskcast = new TaskcastServerClient({
  baseUrl: 'http://taskcast-service:3721',
  token: process.env.TASKCAST_TOKEN,
})

// 创建任务
const task = await taskcast.createTask({
  type: 'llm.chat',
  params: { prompt: 'Hello' },
})

// 发布事件
await taskcast.publishEvent(task.id, {
  type: 'llm.delta',
  level: 'info',
  data: { delta: 'Hello!' },
})

// 完成任务
await taskcast.transitionTask(task.id, 'completed', {
  result: { output: 'Hello!' },
})
```

**前端集成（消费者）：**

```typescript
import { TaskcastClient } from '@taskcast/client'

const client = new TaskcastClient({
  baseUrl: 'http://taskcast-service:3721',
  token: userJwtToken,
})

await client.subscribe(taskId, {
  onEvent: (e) => console.log(e),
  onDone: (reason) => console.log('Done:', reason),
})
```

## 配置

### 配置文件

Taskcast 按以下顺序搜索配置文件（使用第一个找到的）：

1. `taskcast.config.ts` — 完整功能（支持函数、自定义适配器、中间件）
2. `taskcast.config.js` / `.mjs`
3. `taskcast.config.yaml` / `.yml`
4. `taskcast.config.json`

也可以通过 `-c` 参数指定：

```bash
npx @taskcast/cli -c /path/to/config.yaml
```

### TypeScript 配置（推荐用于复杂场景）

```typescript
// taskcast.config.ts
import type { TaskcastConfig } from '@taskcast/core'

export default {
  port: 3721,
  logLevel: 'info',

  auth: {
    mode: 'jwt',
    jwt: {
      algorithm: 'RS256',
      publicKeyFile: '/run/secrets/jwt.pub',
    },
  },

  trustedServices: [
    {
      name: 'backend',
      key: process.env.TASKCAST_SERVICE_KEY_BACKEND!,
      taskIds: '*',
      scope: ['*'],
    },
  ],

  adapters: {
    broadcast: { provider: 'redis', url: process.env.REDIS_URL },
    shortTerm: { provider: 'redis', url: process.env.REDIS_URL },
    longTerm: { provider: 'postgres', url: process.env.DATABASE_URL },
  },

  sentry: {
    dsn: process.env.SENTRY_DSN,
    captureTaskFailures: true,
    captureTaskTimeouts: true,
    captureUnhandledErrors: true,
  },

  webhook: {
    defaultRetry: {
      retries: 3,
      backoff: 'exponential',
      initialDelayMs: 1000,
      maxDelayMs: 30000,
      timeoutMs: 5000,
    },
  },

  cleanup: {
    rules: [
      {
        match: { taskTypes: ['llm.*'] },
        trigger: { afterMs: 3600_000 },
        target: 'events',
        eventFilter: { levels: ['debug'] },
      },
      {
        trigger: { afterMs: 86400_000 * 7 },
        target: 'all',
      },
    ],
  },
} satisfies TaskcastConfig
```

### YAML 配置（推荐用于简单场景）

```yaml
# taskcast.config.yaml
port: 3721
logLevel: info

auth:
  mode: jwt
  jwt:
    algorithm: RS256
    publicKeyFile: /run/secrets/jwt.pub

trustedServices:
  - name: backend
    key: ${TASKCAST_SERVICE_KEY_BACKEND}
    taskIds: "*"
    scope: ["*"]

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
        afterMs: 604800000
      target: all
```

> **注意：** YAML/JSON 配置支持 `${ENV_VAR}` 环境变量插值，但不支持自定义中间件和自定义适配器实例。

### 环境变量

所有配置项都可以通过环境变量覆盖：

| 变量 | 说明 | 默认值 |
|------|------|--------|
| `TASKCAST_PORT` | 服务端口 | `3721` |
| `TASKCAST_LOG_LEVEL` | 服务端最低日志级别（`debug`、`info`、`warn` 或 `error`）；非法值会阻止启动。HTTP 5xx 故障会以结构化 JSON 写入 stderr。 | `info` |
| `TASKCAST_AUTH_MODE` | 认证模式：`none`、`jwt`、`custom` | `none` |
| `TASKCAST_JWT_SECRET` | JWT HMAC 密钥（HS256） | — |
| `TASKCAST_JWT_ALGORITHM` | JWT 算法 | `HS256` |
| `TASKCAST_JWT_PUBLIC_KEY` | 内联 JWT 公钥 PEM（RS256/ES*） | — |
| `TASKCAST_JWT_PUBLIC_KEY_FILE` | JWT 公钥文件路径（RS256/ES*） | — |
| `TASKCAST_JWT_ISSUER` | JWT 签发者 | — |
| `TASKCAST_JWT_AUDIENCE` | JWT 受众 | — |
| `TASKCAST_SERVICE_KEY_*` | `trustedServices` 引用的服务到 Taskcast 预共享密钥 | — |
| `TASKCAST_REDIS_URL` | Redis 连接 URL | — |
| `TASKCAST_POSTGRES_URL` | PostgreSQL 连接 URL | — |
| `TASKCAST_POSTGRES_MAX_CONNECTIONS` | 每个 Taskcast 进程的 PostgreSQL 连接池最大连接数；仅接受正整数 | `10` |
| `TASKCAST_STORAGE` | 短期存储/广播存储模式：`memory`、`redis` 或 `sqlite` | `memory` |
| `SENTRY_DSN` | Sentry DSN | — |

**优先级：** CLI 参数 > 环境变量 > 配置文件 > 默认值

### 存储选择与 PostgreSQL 启用

对于 CLI 服务端，短期存储/广播存储按以下顺序解析：CLI `--storage`、
`TASKCAST_STORAGE`、已配置的短期存储/广播 provider、非空 Redis URL，然后是 `memory`。如果同时配置了短期存储
和广播 provider，它们必须是同一 provider。显式选择 `memory` 或 `sqlite` 时，不会仅因存在 Redis URL
就启用 Redis。

PostgreSQL 长期存储单独解析。SQLite 会使 PostgreSQL 保持未启用。当
`adapters.longTermStore.provider` 显式为 `postgres` 时，必须提供非空的
`TASKCAST_POSTGRES_URL` 或 `adapters.longTermStore.url`。未配置长期 provider 时，非空的
`TASKCAST_POSTGRES_URL` 会启用 PostgreSQL；配置其他长期 provider 则会使其保持未启用。

### 依赖健康与恢复

启动时会在 HTTP 绑定前检查每个已启用的 Redis 和 PostgreSQL 依赖，如果某个依赖无法连接，启动会失败。
`/health` 是存活性端点，不执行任何依赖 I/O。`/health/ready` 是就绪性端点：它会检查已启用的依赖，任一依赖不可用时返回 HTTP `503`。
`/health/detail` 仅提供经脱敏的依赖状态，绝不包含凭据。

依赖在此后恢复可用时，受管理的重连和连接池行为会使后续操作恢复；但被断连中断的业务请求可能失败，且绝不会被自动重放。
Redis PubSub 订阅在运行时报告 PubSub 就绪前会被恢复。已类型化的依赖连通性故障使用现有的
`{ "error": string }` 业务封装并返回 HTTP `503`。

`dependency_state_change` 和受节流的 `dependency_outage_summary` 事件会以结构化 JSON 记录写入 stderr。如果希望导出或保留这些记录，请配置自己的日志采集器；Taskcast 不会自动发送它们到日志服务。

### TS/JS vs YAML/JSON 功能对比

| 功能 | TS/JS | YAML/JSON |
|------|-------|-----------|
| 基础配置（端口、认证、适配器） | ✅ | ✅ |
| 环境变量插值 `${VAR}` | ✅ | ✅ |
| 文件路径引用 | ✅ | ✅ |
| 可信服务密钥（`trustedServices`） | ✅ | ✅ |
| 自定义 auth 中间件 | ✅ | ❌ |
| 自定义适配器实例 | ✅ | ❌ 仅支持内置 provider |
| Sentry 自定义 hooks | ✅ | ❌ |

## 生产环境建议

### 最小生产配置

```yaml
# taskcast.config.yaml
port: 3721

auth:
  mode: jwt
  jwt:
    algorithm: HS256
    secret: ${JWT_SECRET}

adapters:
  broadcast:
    provider: redis
    url: ${REDIS_URL}
  shortTerm:
    provider: redis
    url: ${REDIS_URL}
```

这个配置使用 Redis 做广播和短期存储（多实例部署必须用 Redis），JWT 认证，不配置长期存储。

### 可信服务密钥

公开部署时，面向用户/前端的客户端继续使用 JWT；后端等可信服务与 Taskcast 之间使用预共享服务密钥。可信服务请求时发送：

```http
X-Taskcast-Service-Key: <service-key>
```

如果密钥命中配置的服务，Taskcast 会直接授予该服务配置的 `scope` 和 `taskIds`。如果没有这个 header，则继续走普通 Bearer JWT 认证。

```yaml
auth:
  mode: jwt
  jwt:
    algorithm: RS256
    publicKey: ${TASKCAST_JWT_PUBLIC_KEY}
    issuer: your-auth-service
    audience: taskcast

trustedServices:
  - name: backend
    key: ${TASKCAST_SERVICE_KEY_BACKEND}
    taskIds: "*"
    scope: ["*"]
```

服务密钥应当按普通共享密钥轮换和保管，不要暴露给浏览器或移动端。

### 完整生产配置

在最小配置基础上添加：

- **PostgreSQL 长期存储** — 如果需要永久保存任务历史
- **Sentry 监控** — 捕获任务失败、超时、未处理错误
- **清理规则** — 自动清理过期数据，防止存储膨胀
- **Webhook** — 将任务事件推送到外部系统

### 多实例部署

当部署多个 Taskcast 实例时，**必须使用 Redis** 作为广播层和短期存储层，以确保：

- 不同实例上的 SSE 订阅者都能收到事件
- 任务状态在所有实例间一致
- 断点续传在任何实例上都能正常工作

### Sentry 集成

```bash
pnpm add @taskcast/sentry @sentry/node
```

在 CLI 模式下只需配置 `SENTRY_DSN` 环境变量即可自动启用。在嵌入模式下：

```typescript
import * as Sentry from '@sentry/node'
import { createSentryHooks } from '@taskcast/sentry'

Sentry.init({ dsn: process.env.SENTRY_DSN })

const hooks = createSentryHooks(Sentry, {
  captureTaskFailures: true,
  captureTaskTimeouts: true,
  captureUnhandledErrors: true,
})

const engine = new TaskEngine({
  broadcast,
  shortTermStore,
  hooks,
})
```

## 下一步

- [REST API](../api/rest.md) — 完整 API 参考
- [SSE 订阅](../api/sse.md) — SSE 协议详解
- [认证与权限](../api/authentication.md) — 认证系统详解
