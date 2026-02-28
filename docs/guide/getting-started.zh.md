# Getting Started

本指南将帮助你在 5 分钟内运行 Taskcast 并完成第一个任务的创建、流式推送和订阅。

## 安装

### 方式一：独立服务器（推荐快速体验）

无需安装，直接用 npx 启动：

```bash
npx taskcast
```

服务默认运行在 `http://localhost:3721`。

### 方式二：嵌入到你的项目

```bash
pnpm add @taskcast/core @taskcast/server
```

## 第一个任务

### 1. 启动服务

```bash
npx taskcast
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
    "data": { "text": "你好" },
    "seriesId": "response",
    "seriesMode": "accumulate"
  }'

# 发送第二条（会累加到同一系列）
curl -X POST http://localhost:3721/tasks/{taskId}/events \
  -H "Content-Type: application/json" \
  -d '{
    "type": "llm.delta",
    "level": "info",
    "data": { "text": "世界！" },
    "seriesId": "response",
    "seriesMode": "accumulate"
  }'
```

订阅终端会实时收到这些事件。

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

## 在代码中使用

### 嵌入模式（推荐用于生产）

```typescript
import { TaskEngine, MemoryBroadcastProvider, MemoryShortTermStore } from '@taskcast/core'
import { createTaskcastApp } from '@taskcast/server'

// 创建引擎
const engine = new TaskEngine({
  broadcast: new MemoryBroadcastProvider(),
  shortTermStore: new MemoryShortTermStore(),
})

// 创建 HTTP 应用
const app = createTaskcastApp({ engine })

// 在你的 LLM 调用中使用
async function handleChat(prompt: string) {
  const task = await engine.createTask({
    type: 'llm.chat',
    params: { prompt },
    ttl: 600, // 10 分钟超时
  })

  await engine.transitionTask(task.id, 'running')

  // 模拟流式输出
  for (const chunk of ['Hello, ', 'world!']) {
    await engine.publishEvent(task.id, {
      type: 'llm.delta',
      level: 'info',
      data: { text: chunk },
      seriesId: 'response',
      seriesMode: 'accumulate',
    })
  }

  await engine.transitionTask(task.id, 'completed', {
    result: { output: 'Hello, world!' },
  })

  return task.id
}
```

### 浏览器端订阅

```bash
pnpm add @taskcast/client
```

```typescript
import { TaskcastClient } from '@taskcast/client'

const client = new TaskcastClient({
  baseUrl: 'http://localhost:3721',
})

await client.subscribe('task-id', {
  filter: {
    types: ['llm.*'],
    since: { index: 0 },
  },
  onEvent: (envelope) => {
    // 每收到一个事件都会调用
    document.getElementById('output')!.textContent += envelope.data.text
  },
  onDone: (reason) => {
    console.log('任务完成:', reason)
  },
  onError: (err) => {
    console.error('订阅错误:', err)
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

  if (error) return <div>错误: {error.message}</div>

  return (
    <div>
      {events.map((e) => (
        <span key={e.eventId}>{e.data.text}</span>
      ))}
      {isDone && <p>已完成: {doneReason}</p>}
    </div>
  )
}
```

## 下一步

- [核心概念](./concepts.md) — 深入了解任务生命周期、序列消息、三层存储
- [部署指南](./deployment.md) — 生产环境配置、Redis/PostgreSQL 接入
- [REST API](../api/rest.md) — 完整 API 参考
- [SSE 订阅](../api/sse.md) — SSE 协议详解