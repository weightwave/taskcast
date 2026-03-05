<div align="center">

# Taskcast

**极简心智模型，开箱即用的 LLM 流式输出、Agent 及异步工作负载追踪服务。**

[![npm version](https://img.shields.io/npm/v/@taskcast/core?label=%40taskcast%2Fcore&color=blue)](https://www.npmjs.com/package/@taskcast/core)
[![Docker Node](https://img.shields.io/docker/v/mwr1998/taskcast?label=docker%20node&logo=docker&logoColor=white&color=2496ED)](https://hub.docker.com/r/mwr1998/taskcast)
[![Docker Rust](https://img.shields.io/docker/v/mwr1998/taskcast-rs?label=docker%20rust&logo=docker&logoColor=white&color=2496ED)](https://hub.docker.com/r/mwr1998/taskcast-rs)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](./LICENSE)
[![TypeScript](https://img.shields.io/badge/TypeScript-5.7-blue?logo=typescript&logoColor=white)](https://www.typescriptlang.org/)
[![Node.js](https://img.shields.io/badge/Node.js-%E2%89%A518-green?logo=node.js&logoColor=white)](https://nodejs.org/)
[![Coverage](https://img.shields.io/badge/coverage-95%25-brightgreen?logo=vitest&logoColor=white)]()

[快速上手](./docs/guide/getting-started.zh.md) | [核心概念](./docs/guide/concepts.zh.md) | [REST API](./docs/api/rest.zh.md) | [SSE](./docs/api/sse.zh.md) | [部署指南](./docs/guide/deployment.zh.md)

[English](./README.md) | [中文](./README.zh.md)

</div>

---

创建任务、发布事件、订阅 —— 这就是全部的心智模型。但 Taskcast 开箱即用地提供了**持久化状态**、**可恢复订阅**、**多客户端扇出**、**可选的 Worker 管理**，以及从单个 SQLite 文件到 Redis + PostgreSQL 的可插拔存储栈。专为大模型流式输出和 Agent 工作流设计。

## 核心亮点

- **可恢复的 SSE 流** — 通过事件 ID、过滤后索引或时间戳从任意位置重连，刷新页面不丢数据。
- **多客户端扇出** — 多个浏览器标签页、设备或服务可以同时订阅同一个任务的实时流。
- **序列消息合并** — 内置支持流式文本累加（`accumulate`，默认累加字段兼容 ChatCompletion delta 格式）、取最新值替换（`latest`）和全量保留（`keep-all`）。
- **三层存储架构** — 广播层（Redis Pub/Sub | 内存）+ 短期存储层（Redis | SQLite | 内存）+ 长期存储层（PostgreSQL | SQLite），每层可插拔、独立可选。
- **Worker 管理**（可选） — 内置任务分配，支持 Pull（长轮询）和 WebSocket（offer/race）模式。容量追踪、匹配规则、断连自动重分配。
- **Rust 服务端** — 可直接替换的原生 Rust 二进制（`taskcast-rs`），极致性能与最低资源占用。相同 API，相同行为，零 Node.js 依赖。Docker 镜像开箱即用。
- **灵活的认证** — 无认证、JWT 或自定义中间件，权限粒度细化到单个任务。
- **SDK-First 架构** — 核心零 HTTP 依赖，可嵌入你现有的服务器，也可用 `npx taskcast` 独立运行。

## 架构

```mermaid
graph TB
    subgraph 客户端
        Browser["浏览器 / React 应用<br/>@taskcast/client · @taskcast/react"]
        Backend["你的后端<br/>@taskcast/server-sdk"]
    end

    Workers["Workers（可选）<br/>长轮询 | WebSocket"]

    subgraph 服务端["@taskcast/server · 认证 · Webhooks"]
        REST["REST API"]
        SSE["SSE 流式推送"]
    end

    subgraph 核心["@taskcast/core"]
        Engine["任务引擎<br/>状态机 · 过滤器 · 序列合并"]
    end

    subgraph 存储["存储（可插拔）"]
        Broadcast["广播层 — Redis Pub/Sub | 内存"]
        ShortTerm["短期存储 — Redis | SQLite | 内存"]
        LongTerm["长期存储 — PostgreSQL | SQLite（可选）"]
    end

    Browser -->|SSE| SSE
    Backend -->|HTTP| REST
    Workers -.->|pull / ws| REST
    REST --> Engine
    SSE --> Engine
    Engine --> Broadcast
    Engine --> ShortTerm
    Engine -.->|异步| LongTerm
```

### 部署模式

**嵌入模式** — 将核心引擎导入并将 Hono 路由挂载到你的服务器中：

```
你的服务器 → @taskcast/core + 适配器 → @taskcast/server（Hono 路由）
```

**远程模式（推荐）** — 作为独立微服务运行，通过 RESTful API 连接。清晰的服务边界，独立扩缩容。支持 Docker 部署。

```
你的服务器 → @taskcast/server-sdk（REST）→ taskcast 服务 ← @taskcast/client（浏览器）
```

## 快速开始

### 独立服务器

```bash
npx taskcast
```

默认在 `3721` 端口启动。通过配置文件或环境变量进行配置：

```bash
npx taskcast -p 8080 -c taskcast.config.yaml
```

### 嵌入模式

```bash
pnpm add @taskcast/core @taskcast/server
```

```typescript
import { TaskEngine, MemoryBroadcastProvider, MemoryShortTermStore } from '@taskcast/core'
import { createTaskcastApp } from '@taskcast/server'

const engine = new TaskEngine({
  broadcast: new MemoryBroadcastProvider(),
  shortTermStore: new MemoryShortTermStore(),
})

const app = createTaskcastApp({ engine })
// 挂载到你现有的 Hono 应用或直接启动
export default app
```

## 使用示例

### 创建和追踪任务（服务端）

```typescript
// 创建任务
const task = await engine.createTask({
  type: 'llm.chat',
  params: { prompt: '给我讲个故事' },
  ttl: 3600,
})

// 转换到运行状态
await engine.transitionTask(task.id, 'running')

// 发布流式事件
await engine.publishEvent(task.id, {
  type: 'llm.delta',
  level: 'info',
  data: { delta: '从前有座山...' },
  seriesId: 'response',
  seriesMode: 'accumulate',
})

// 完成任务
await engine.transitionTask(task.id, 'completed', {
  result: { output: '从前有座山... 完。' },
})
```

### 浏览器端订阅

```typescript
import { TaskcastClient } from '@taskcast/client'

const client = new TaskcastClient({
  baseUrl: 'http://localhost:3721',
  token: 'your-jwt-token', // 可选
})

await client.subscribe(taskId, {
  filter: {
    types: ['llm.*'],
    since: { index: 0 },
  },
  onEvent: (envelope) => {
    console.log(envelope.data) // { text: "从前有座山..." }
  },
  onDone: (reason) => {
    console.log(`任务 ${reason}`) // "任务 completed"
  },
})
```

### React 集成

```typescript
import { useTaskEvents } from '@taskcast/react'

function TaskStream({ taskId }: { taskId: string }) {
  const { events, isDone, doneReason, error } = useTaskEvents(taskId, {
    baseUrl: 'http://localhost:3721',
    filter: { types: ['llm.*'] },
  })

  return (
    <div>
      {events.map((e) => (
        <span key={e.eventId}>{e.data.text}</span>
      ))}
      {isDone && <p>已完成：{doneReason}</p>}
      {error && <p>错误：{error.message}</p>}
    </div>
  )
}
```

## 包一览

| 包 | 说明 | 安装 |
|---|------|------|
| [`@taskcast/core`](./packages/core) | 任务引擎 — 状态机、过滤、序列合并，零 HTTP 依赖 | `pnpm add @taskcast/core` |
| [`@taskcast/server`](./packages/server) | Hono HTTP 服务器 — REST、SSE、认证、Webhook | `pnpm add @taskcast/server` |
| [`@taskcast/server-sdk`](./packages/server-sdk) | 远程模式 HTTP 客户端 SDK | `pnpm add @taskcast/server-sdk` |
| [`@taskcast/client`](./packages/client) | 浏览器 SSE 订阅客户端 | `pnpm add @taskcast/client` |
| [`@taskcast/react`](./packages/react) | React Hooks（`useTaskEvents`） | `pnpm add @taskcast/react` |
| [`@taskcast/cli`](./packages/cli) | 独立服务器 CLI | `npx taskcast` |
| [`@taskcast/sqlite`](./packages/sqlite) | SQLite 适配器（短期 + 长期存储层） | `pnpm add @taskcast/sqlite` |
| [`@taskcast/redis`](./packages/redis) | Redis 适配器（广播层 + 短期存储层） | `pnpm add @taskcast/redis` |
| [`@taskcast/postgres`](./packages/postgres) | PostgreSQL 适配器（长期存储层） | `pnpm add @taskcast/postgres` |
| [`@taskcast/sentry`](./packages/sentry) | Sentry 错误监控 Hooks | `pnpm add @taskcast/sentry` |

## Rust 服务端

原生 Rust 二进制（`taskcast-rs`）可直接替换 Node.js 服务端。基于 Axum + Tokio + sqlx 构建，HTTP 行为完全一致 —— 相同的路径、相同的 JSON 格式、相同的 SSE 事件、相同的状态码。适用于追求极致吞吐或最小资源占用的场景。

预编译二进制覆盖 Linux（amd64/arm64）、macOS（amd64/arm64）和 Windows，随每个 [GitHub Release](https://github.com/weightwave/taskcast/releases) 发布。多架构 Docker 镜像同步可用：

```bash
docker run -p 3721:3721 mwr1998/taskcast-rs
```

## 配置

### 配置文件

Taskcast 按以下优先级在当前目录搜索配置文件：

`taskcast.config.ts` > `.js` > `.mjs` > `.yaml` / `.yml` > `.json`

```yaml
# taskcast.config.yaml
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

webhook:
  defaultRetry:
    retries: 3
    backoff: exponential
    initialDelayMs: 1000

cleanup:
  rules:
    - match:
        taskTypes: ["llm.*"]
      trigger:
        afterMs: 3600000
      target: events
    - trigger:
        afterMs: 604800000
      target: all
```

### 环境变量

| 变量 | 说明 | 默认值 |
|------|------|--------|
| `TASKCAST_PORT` | 服务端口 | `3721` |
| `TASKCAST_AUTH_MODE` | `none` \| `jwt` \| `custom` | `none` |
| `TASKCAST_JWT_SECRET` | JWT HMAC 密钥 | — |
| `TASKCAST_JWT_PUBLIC_KEY_FILE` | JWT 公钥文件路径 | — |
| `TASKCAST_REDIS_URL` | Redis 连接 URL | — |
| `TASKCAST_POSTGRES_URL` | PostgreSQL 连接 URL | — |
| `TASKCAST_LOG_LEVEL` | `debug` \| `info` \| `warn` \| `error` | `info` |
| `SENTRY_DSN` | Sentry 错误追踪 DSN | — |

## API 概览

### REST 端点

| 方法 | 路径 | 说明 |
|------|------|------|
| `POST` | `/tasks` | 创建任务 |
| `GET` | `/tasks/:taskId` | 查询任务状态与元数据 |
| `PATCH` | `/tasks/:taskId/status` | 更新任务状态 |
| `DELETE` | `/tasks/:taskId` | 删除任务 |
| `POST` | `/tasks/:taskId/events` | 发布事件 |
| `GET` | `/tasks/:taskId/events` | SSE 订阅 |
| `GET` | `/tasks/:taskId/events/history` | 查询历史事件 |
| `POST` | `/workers/register` | 注册 Worker |
| `GET` | `/workers/pull` | 长轮询获取任务分配 |
| `WS` | `/workers/ws` | WebSocket Worker 连接 |

### SSE 查询参数

| 参数 | 说明 | 示例 |
|------|------|------|
| `since.id` | 从指定事件 ID 之后恢复 | `since.id=01HXXX` |
| `since.index` | 从过滤后索引之后恢复 | `since.index=5` |
| `since.timestamp` | 从指定时间戳之后恢复 | `since.timestamp=1700000` |
| `types` | 过滤事件类型（支持通配符） | `types=llm.*,tool.call` |
| `levels` | 过滤事件等级 | `levels=info,warn` |
| `includeStatus` | 是否包含状态事件 | `includeStatus=true` |
| `wrap` | 是否包裹 envelope | `wrap=true` |

### 任务状态生命周期

```mermaid
stateDiagram-v2
    [*] --> pending : 创建
    pending --> assigned : Worker 认领
    pending --> running : 开始执行
    pending --> cancelled : 取消
    assigned --> running : 开始执行
    assigned --> cancelled : 取消
    running --> paused : 暂停
    running --> completed : 成功完成
    running --> failed : 执行失败
    running --> timeout : 超时
    running --> cancelled : 取消
    paused --> running : 恢复
    paused --> cancelled : 取消
```

### 权限范围

| Scope | 说明 |
|-------|------|
| `task:create` | 创建任务 |
| `task:manage` | 更改任务状态、删除任务 |
| `event:publish` | 向任务发布事件 |
| `event:subscribe` | 订阅任务 SSE 流 |
| `event:history` | 查询事件历史 |
| `webhook:create` | 创建 Webhook 配置 |
| `*` | 完全访问权限 |

## 开发

```bash
# 安装依赖
pnpm install

# 构建所有包
pnpm build

# 运行测试
pnpm test

# 监听模式运行测试
pnpm test:watch

# 运行测试并生成覆盖率报告
pnpm test:coverage

# 类型检查
pnpm lint
```

## 贡献

欢迎贡献！请随时提交 Issue 和 Pull Request。

1. Fork 本仓库
2. 创建你的特性分支（`git checkout -b feat/amazing-feature`）
3. 提交你的更改（`git commit -m 'feat: add amazing feature'`）
4. 推送到分支（`git push origin feat/amazing-feature`）
5. 发起 Pull Request

## 许可证

[MIT](./LICENSE)