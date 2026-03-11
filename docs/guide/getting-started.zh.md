# Getting Started

本指南将帮助你在 5 分钟内运行 Taskcast 并完成第一个任务的创建、流式推送和订阅。

## 安装

### 方式一：独立服务器（推荐快速体验）

**Node.js (npx) —— 无需安装：**

```bash
npx @taskcast/cli
```

**原生 Rust 二进制 —— 极致性能，零 Node.js 依赖：**

```bash
# Homebrew（macOS / Linux）
brew tap weightwave/tap
brew install taskcast
taskcast-rs

# 或从 GitHub Releases 下载预编译二进制
# https://github.com/weightwave/taskcast/releases

# 或通过 Docker 运行
docker run -p 3721:3721 mwr1998/taskcast-rs
```

两个版本行为完全一致。服务默认运行在 `http://localhost:3721`。

### 方式二：嵌入到你的项目

```bash
pnpm add @taskcast/core @taskcast/server
```

## 第一个任务

### 1. 启动服务

```bash
npx @taskcast/cli
```

输出类似：

```
Taskcast server listening on http://localhost:3721
  Auth: none
  Broadcast: memory
  ShortTerm: memory
  LongTerm: not configured
```

### 2. 创建任务

```bash
curl -X POST http://localhost:3721/tasks \
  -H "Content-Type: application/json" \
  -d '{
    "type": "llm.chat",
    "params": { "prompt": "Hello, world!" }
  }'
```

返回：

```json
{
  "id": "01HXXXXXXXXXXXXXXXXXXX",
  "type": "llm.chat",
  "status": "pending",
  "params": { "prompt": "Hello, world!" },
  "createdAt": 1700000000000,
  "updatedAt": 1700000000000
}
```

记住返回的 `id`，后续步骤需要用到。

### 3. 订阅任务事件（在另一个终端）

```bash
curl -N http://localhost:3721/tasks/{taskId}/events
```

此时连接会挂起等待，因为任务还是 `pending` 状态。

### 4. 开始任务

```bash
curl -X PATCH http://localhost:3721/tasks/{taskId}/status \
  -H "Content-Type: application/json" \
  -d '{ "status": "running" }'
```

订阅终端会收到状态变更事件。

### 5. 发送流式消息

```bash
# 发送第一条消息
curl -X POST http://localhost:3721/tasks/{taskId}/events \
  -H "Content-Type: application/json" \
  -d '{
    "type": "llm.delta",
    "level": "info",
    "data": { "delta": "你好" },
    "seriesId": "response",
    "seriesMode": "accumulate"
  }'

# 发送第二条（会累加到同一系列）
curl -X POST http://localhost:3721/tasks/{taskId}/events \
  -H "Content-Type: application/json" \
  -d '{
    "type": "llm.delta",
    "level": "info",
    "data": { "delta": "世界！" },
    "seriesId": "response",
    "seriesMode": "accumulate"
  }'
```

订阅终端会实时收到这些事件。

> **注意：** 在 `accumulate` 模式下，默认拼接的字段为 `delta`。可通过 `seriesAccField` 自定义拼接字段名。默认情况下，SSE 订阅者收到的是原始增量（`seriesFormat=delta`）。添加 `?seriesFormat=accumulated` 可改为接收累加值。中途加入的订阅者始终会先收到当前累加值的快照。

### 6. 完成任务

```bash
curl -X PATCH http://localhost:3721/tasks/{taskId}/status \
  -H "Content-Type: application/json" \
  -d '{
    "status": "completed",
    "result": { "output": "你好世界！" }
  }'
```

订阅连接会收到完成事件后自动关闭。

## 使用示例

### 模式一：后端 + Worker 一体（自管理）

后端直接创建任务、处理任务并推送流式结果 —— 全部在同一个进程内完成，无需独立 Worker。适合 API 服务自身承担计算的简单部署场景。

```typescript
import { TaskEngine, MemoryBroadcastProvider, MemoryShortTermStore } from '@taskcast/core'
import { createTaskcastApp } from '@taskcast/server'
import { Hono } from 'hono'

// 创建任务引擎，使用内存适配器（生产环境可替换为 Redis/SQLite）
const engine = new TaskEngine({
  broadcast: new MemoryBroadcastProvider(),   // 实时事件广播层 —— 将事件扇出给所有 SSE 订阅者
  shortTermStore: new MemoryShortTermStore(), // 任务状态 + 事件缓冲层（同步写入保证顺序）
})

const app = new Hono()
// 挂载 Taskcast HTTP 路由 —— 提供 REST API + SSE 端点，路径前缀为 /taskcast
app.route('/taskcast', createTaskcastApp({ engine }))

// 你的 API 端点 —— 直接创建并处理任务
app.post('/api/chat', async (c) => {
  const { prompt } = await c.req.json()

  // 创建任务，初始状态为 "pending"。客户端可以立即开始订阅 ——
  // SSE 会保持连接等待，任务转为 "running" 后自动开始推送事件。
  const task = await engine.createTask({
    type: 'llm.chat',       // 任务类型，用于事件过滤（支持通配符如 "llm.*"）
    params: { prompt },      // 任意参数，原样传递给消费者
    ttl: 600,                // 10 分钟未完成则自动超时
  })

  // 后台处理 —— 这个服务本身就是 Worker。
  // 客户端立即拿到 taskId，通过 SSE 订阅结果。
  processChat(task.id, prompt)
  return c.json({ taskId: task.id })
})

async function processChat(taskId: string, prompt: string) {
  // pending → running：等待中的 SSE 订阅者将开始接收事件
  await engine.transitionTask(taskId, 'running')

  for await (const chunk of callLLM(prompt)) {
    // 发布流式事件。seriesMode: 'accumulate' 表示引擎会追踪累加值。
    // SSE 订阅者通过 seriesFormat 参数选择格式：
    // 'delta'（默认）发原始 chunk，'accumulated' 发累加值。
    // 后加入的订阅者会先收到当前累加值的快照。
    await engine.publishEvent(taskId, {
      type: 'llm.delta',
      level: 'info',
      data: { delta: chunk },
      seriesId: 'response',       // 将事件归入命名的系列
      seriesMode: 'accumulate',   // 'accumulate' | 'latest' | 'keep-all'
    })
  }

  // running → completed：SSE 连接收到完成事件后自动关闭。
  // 终态转换具有并发安全性 —— 只允许一次终态转换。
  await engine.transitionTask(taskId, 'completed', {
    result: { output: '完整响应文本' },
  })
}
```

客户端通过 `GET /taskcast/tasks/{taskId}/events`（SSE）订阅流式结果。任务为 `pending` 时连接会保持等待；转为 `running` 后自动推流；若任务已 `completed`，客户端会收到完整的历史回放后断开。

### 模式二：后端 + Worker 分离

后端通过 HTTP SDK 创建任务，独立的 Worker 进程连接到 Taskcast 服务领取并处理任务。适合需要独立扩缩 Worker 的场景。

**步骤 1 —— 启动独立的 Taskcast 服务：**

```bash
npx @taskcast/cli
# 或：taskcast-rs
```

**步骤 2 —— 后端创建任务（任务生产者）：**

```typescript
import { TaskcastServerClient } from '@taskcast/server-sdk'

const taskcast = new TaskcastServerClient({
  baseUrl: 'http://taskcast-service:3721',
  token: process.env.TASKCAST_TOKEN, // 携带 task:create + event:subscribe 权限的 JWT
})

// 创建任务 —— 初始状态为 "pending"，等待 Worker 领取。
// assignMode 决定引擎如何将任务分发给 Worker。
const task = await taskcast.createTask({
  type: 'llm.chat',
  params: { prompt: '给我讲个故事' },
  assignMode: 'pull',    // 'pull' = Worker 长轮询领取；'ws-offer' = 服务端推送给 WS Worker；
                         // 'ws-race' = 推送给多个 WS Worker，先接受者获得任务
})

// 将 taskId 返回给客户端 —— 客户端通过 SSE 订阅流式结果
return { taskId: task.id }
```

**步骤 3a —— Worker 长轮询领取任务：**

```typescript
const TASKCAST_URL = 'http://taskcast-service:3721'
const WORKER_ID = 'worker-1'

async function workerLoop() {
  while (true) {
    // 长轮询：服务端会保持连接直到有匹配的任务可用，或超时返回。
    // 匹配成功时，任务会被原子性地分配给该 Worker（pending → assigned），
    // 其他 Worker 无法再领取同一任务。
    const res = await fetch(
      `${TASKCAST_URL}/workers/pull?workerId=${WORKER_ID}&timeout=30000`,
      { headers: { Authorization: `Bearer ${WORKER_TOKEN}` } },
    )

    if (res.status === 204) continue // 超时，没有匹配的任务 —— 重试

    const task = await res.json() // { id, type, params, ... }
    await processAndComplete(task.id, task.params)
  }
}

async function processAndComplete(taskId: string, params: Record<string, unknown>) {
  // assigned → running：通知所有订阅者任务已开始处理
  await fetch(`${TASKCAST_URL}/tasks/${taskId}/status`, {
    method: 'PATCH',
    headers: { 'Content-Type': 'application/json', Authorization: `Bearer ${WORKER_TOKEN}` },
    body: JSON.stringify({ status: 'running' }),
  })

  // 发布流式事件 —— 每个事件会实时广播给所有 SSE 订阅者。
  // seriesMode: 'accumulate' 追踪累加值；后加入的订阅者会先收到快照。
  // 订阅者通过 seriesFormat 参数选择格式：'delta'（默认）或 'accumulated'。
  for await (const chunk of callLLM(params.prompt as string)) {
    await fetch(`${TASKCAST_URL}/tasks/${taskId}/events`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json', Authorization: `Bearer ${WORKER_TOKEN}` },
      body: JSON.stringify({
        type: 'llm.delta', level: 'info',
        data: { delta: chunk },
        seriesId: 'response', seriesMode: 'accumulate',
      }),
    })
  }

  // running → completed：SSE 订阅者收到终态事件后自动断开。
  // Worker 的并发容量槽位会被自动释放，可以领取下一个任务。
  await fetch(`${TASKCAST_URL}/tasks/${taskId}/status`, {
    method: 'PATCH',
    headers: { 'Content-Type': 'application/json', Authorization: `Bearer ${WORKER_TOKEN}` },
    body: JSON.stringify({ status: 'completed', result: { output: '完整文本' } }),
  })
}
```

**步骤 3b —— 或使用 WebSocket Worker：**

```typescript
const ws = new WebSocket('ws://taskcast-service:3721/workers/ws')

ws.addEventListener('open', () => {
  // 注册 Worker：matchRule 过滤要接收的任务类型；capacity 限制最大并发数。
  ws.send(JSON.stringify({
    type: 'register',
    matchRule: { types: ['llm.*'] }, // 只接收 type 匹配 "llm.*" 的任务
    capacity: 5,                     // 最多同时处理 5 个任务
  }))
})

ws.addEventListener('message', async (event) => {
  const msg = JSON.parse(event.data)

  if (msg.type === 'offer') {
    // 服务端推送任务给该 Worker（ws-offer 模式：独占推送；
    // ws-race 模式：同时推送给多个 Worker，先 accept 者获得任务）。
    // msg.task 包含 { id, type, params, tags, cost }。
    ws.send(JSON.stringify({ type: 'accept', taskId: msg.task.id }))
    await processAndComplete(msg.task.id, msg.task.params)
  }
})
```

**步骤 4 —— 客户端订阅（浏览器）：**

```bash
pnpm add @taskcast/client
```

```typescript
import { TaskcastClient } from '@taskcast/client'

const client = new TaskcastClient({
  baseUrl: 'http://taskcast-service:3721', // 直接使用外部地址，或部署在 API Gateway 后，鉴权交给 Gateway
  token: 'user-jwt-token',
})

// 订阅任务的 SSE 事件流。
// filter 支持通配符匹配，只接收感兴趣的事件类型。
await client.subscribe(taskId, {
  filter: { types: ['llm.*'] },  // 只接收 type 匹配 "llm.*" 的事件
  onEvent: (envelope) => {
    // envelope 包含完整的事件信封：{ eventId, type, level, data, seriesId, ... }
    document.getElementById('output')!.textContent += envelope.data.delta
  },
  onDone: (reason) => {
    // reason: 'completed' | 'failed' | 'timeout' | 'cancelled'
    console.log('任务完成：', reason)
  },
})
```

### React 集成

```bash
pnpm add @taskcast/react
```

```typescript
import { useTaskEvents } from '@taskcast/react'

function ChatStream({ taskId }: { taskId: string }) {
  const { events, isDone, doneReason, error } = useTaskEvents(taskId, {
    baseUrl: 'http://localhost:3721',
    filter: { types: ['llm.*'] },
  })

  if (error) return <div>错误：{error.message}</div>

  return (
    <div>
      {events.map((e) => (
        <span key={e.eventId}>{e.data.delta}</span>
      ))}
      {isDone && <p>已完成：{doneReason}</p>}
    </div>
  )
}
```

## 下一步

- [核心概念](./concepts.md) — 深入了解任务生命周期、序列消息、三层存储
- [部署指南](./deployment.md) — 生产环境配置、Redis/PostgreSQL 接入
- [REST API](../api/rest.md) — 完整 API 参考
- [SSE 订阅](../api/sse.md) — SSE 协议详解