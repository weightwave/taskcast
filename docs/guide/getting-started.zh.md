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

> **注意：** 在 `accumulate` 模式下，默认拼接的字段为 `delta`。可通过 `seriesAccField` 自定义拼接字段名。

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

const engine = new TaskEngine({
  broadcast: new MemoryBroadcastProvider(),
  shortTermStore: new MemoryShortTermStore(),
})

const app = new Hono()
app.route('/taskcast', createTaskcastApp({ engine }))

// 你的 API 端点 —— 直接创建并处理任务
app.post('/api/chat', async (c) => {
  const { prompt } = await c.req.json()
  const task = await engine.createTask({
    type: 'llm.chat',
    params: { prompt },
    ttl: 600, // 10 分钟超时
  })

  // 后台处理 —— 这个服务本身就是 Worker
  processChat(task.id, prompt)
  return c.json({ taskId: task.id })
})

async function processChat(taskId: string, prompt: string) {
  await engine.transitionTask(taskId, 'running')

  for await (const chunk of callLLM(prompt)) {
    await engine.publishEvent(taskId, {
      type: 'llm.delta',
      level: 'info',
      data: { delta: chunk },
      seriesId: 'response',
      seriesMode: 'accumulate',
    })
  }

  await engine.transitionTask(taskId, 'completed', {
    result: { output: '完整响应文本' },
  })
}
```

客户端通过 `GET /taskcast/tasks/{taskId}/events`（SSE）订阅流式结果。

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
  token: process.env.TASKCAST_TOKEN,
})

// 创建任务 —— 由 Worker 领取
const task = await taskcast.createTask({
  type: 'llm.chat',
  params: { prompt: '给我讲个故事' },
  assignMode: 'pull', // 或 'ws-offer' / 'ws-race'
})

// 将 taskId 返回给客户端，用于 SSE 订阅
return { taskId: task.id }
```

**步骤 3a —— Worker 长轮询领取任务：**

```typescript
const TASKCAST_URL = 'http://taskcast-service:3721'
const WORKER_ID = 'worker-1'

async function workerLoop() {
  while (true) {
    // 长轮询等待任务分配
    const res = await fetch(
      `${TASKCAST_URL}/workers/pull?workerId=${WORKER_ID}&timeout=30000`,
      { headers: { Authorization: `Bearer ${WORKER_TOKEN}` } },
    )

    if (res.status === 204) continue // 无可用任务，重试

    const task = await res.json()
    await processAndComplete(task.id, task.params)
  }
}

async function processAndComplete(taskId: string, params: Record<string, unknown>) {
  // 转换为运行状态
  await fetch(`${TASKCAST_URL}/tasks/${taskId}/status`, {
    method: 'PATCH',
    headers: { 'Content-Type': 'application/json', Authorization: `Bearer ${WORKER_TOKEN}` },
    body: JSON.stringify({ status: 'running' }),
  })

  // 推送流式结果
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

  // 完成任务
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
  // 注册 Worker，设置匹配规则和并发容量
  ws.send(JSON.stringify({
    type: 'register',
    matchRule: { types: ['llm.*'] },
    capacity: 5,
  }))
})

ws.addEventListener('message', async (event) => {
  const msg = JSON.parse(event.data)

  if (msg.type === 'offer') {
    // 接受分配的任务
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
  baseUrl: 'http://taskcast-service:3721',
  token: 'user-jwt-token',
})

await client.subscribe(taskId, {
  filter: { types: ['llm.*'] },
  onEvent: (envelope) => {
    document.getElementById('output')!.textContent += envelope.data.delta
  },
  onDone: (reason) => {
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