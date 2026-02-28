# 核心概念

## 任务（Task）

任务是 Taskcast 的核心实体，代表一个需要追踪的长周期异步操作。

### 任务生命周期

```
pending → running → completed
                  → failed
                  → timeout
                  → cancelled
pending → cancelled
```

**关键规则：**

- 状态只能向前转换，不能回退
- 终态（completed/failed/timeout/cancelled）一旦到达就不可改变
- 并发安全——如果多个请求同时尝试转换到终态，只有一个会成功，其余会收到错误
- 设置了 `ttl` 的任务在超时后会自动转为 `timeout` 状态

### 任务属性

```typescript
{
  id: string          // ULID（自动生成）或用户指定
  type: string        // 任务类型，如 "llm.chat"、"agent.run"，用于过滤和清理规则匹配
  status: TaskStatus  // 当前状态
  params: object      // 任务输入参数（创建时写入，之后只读）
  result: object      // 成功完成时的结果（仅 completed 状态）
  error: TaskError    // 失败信息（仅 failed/timeout 状态）
  metadata: object    // 自定义元数据
  ttl: number         // 超时秒数，超时后自动转为 timeout
}
```

## 事件（TaskEvent）

事件是发布到任务上的不可变消息。每个事件都有：

- **type** — 用户自定义类型字符串，支持通配符过滤。例如 `llm.delta`、`tool.call`、`agent.thought`
- **level** — 日志级别：`debug`、`info`、`warn`、`error`
- **data** — 任意 JSON 数据
- **index** — 在该任务内单调递增的序号

### 内置事件

当任务状态发生变化时，Taskcast 会自动注入 `type: "taskcast:status"` 的内置事件，客户端可以选择是否接收这些事件。

## 序列消息（Series）

序列消息是 Taskcast 的特色功能，专为流式场景设计。同一个 `seriesId` 的事件会被分组处理：

### keep-all（默认）

所有事件独立存储，不做任何合并。适用于需要完整历史的场景。

```
事件1: { seriesId: "s1", data: { text: "你" } }
事件2: { seriesId: "s1", data: { text: "好" } }
存储: [事件1, 事件2]  ← 都保留
```

### accumulate

文本累加模式。新事件的 `data.text` 会追加到系列已有的文本后面。存储的是累加后的完整文本，广播的是原始增量。

```
事件1: { seriesId: "s1", data: { text: "你" }, seriesMode: "accumulate" }
事件2: { seriesId: "s1", data: { text: "好" }, seriesMode: "accumulate" }
存储: 累加结果 → data.text = "你好"
广播: 每次发送原始增量
```

**这是 LLM 流式输出最常用的模式。** 客户端刷新重连时，可以直接获得累加到当前的完整文本，而不需要重放所有增量。

### latest

只保留最新值。适用于进度条、状态指示器等只关心当前值的场景。

```
事件1: { seriesId: "progress", data: { percent: 30 }, seriesMode: "latest" }
事件2: { seriesId: "progress", data: { percent: 60 }, seriesMode: "latest" }
存储: 只保留事件2
```

## 三层存储

Taskcast 将存储抽象为三个独立的层，每层职责不同，可以根据需求选择不同的实现：

### 广播层（BroadcastProvider）

**职责：** 实时消息扇出，无持久化保证。

当一个事件被发布时，广播层负责将它推送给所有在线的 SSE 订阅者。这是一个 fire-and-forget 的操作。

| 实现 | 适用场景 |
|------|----------|
| 内存（默认） | 单进程开发/测试 |
| Redis Pub/Sub | 多进程/多实例生产部署 |

### 短期存储层（ShortTermStore）

**职责：** 事件缓冲 + 任务状态缓存。

这是保证数据可靠性的核心层。所有事件在广播前会先同步写入短期存储，确保即使客户端重连也能获取历史事件。支持 TTL 自动过期。

| 实现 | 适用场景 |
|------|----------|
| 内存（默认） | 单进程开发/测试 |
| Redis | 生产环境，支持多进程共享和持久化 |

### 长期存储层（LongTermStore）— 可选

**职责：** 永久归档。

对于需要长期保存任务历史的场景，可以配置长期存储层。事件会异步写入（不阻塞主流程），适合事后审计、分析等需求。

| 实现 | 适用场景 |
|------|----------|
| PostgreSQL | 需要永久保存和复杂查询 |
| 不配置 | 短生命周期任务，不需要长期存储 |

### 写入流程

```
发布事件
  → 序列合并（根据 seriesMode 处理）
  → 写入短期存储（同步，保证有序）
  → 广播给订阅者（同步，实时推送）
  → 写入长期存储（异步，不阻塞）
```

## 事件过滤

SSE 订阅和 Webhook 都支持事件过滤，通过 `SubscribeFilter` 配置：

### 类型过滤（types）

支持通配符匹配：

```
"llm.*"       → 匹配 llm.delta, llm.done, llm.error
"tool.*"      → 匹配 tool.call, tool.result
"*"           → 匹配所有类型
"llm.delta"   → 精确匹配
```

### 级别过滤（levels）

按日志级别过滤：`debug`、`info`、`warn`、`error`。

### 断点续传（since）

三种方式指定从哪里恢复：

| 方式 | 用途 | 示例 |
|------|------|------|
| `since.id` | 从某个事件 ID 之后 | 跨过滤器精确续传 |
| `since.index` | 从过滤后第 N 条之后 | 同过滤器重连 |
| `since.timestamp` | 从某个时间戳之后 | 基于时间的恢复 |

## 清理规则

Taskcast 支持配置清理规则，自动清理已完成任务的数据：

```yaml
cleanup:
  rules:
    # 1小时后清理 LLM 任务的 debug 级别事件
    - match:
        taskTypes: ["llm.*"]
      trigger:
        afterMs: 3600000
      target: events
      eventFilter:
        levels: [debug]

    # 7天后清理所有已完成任务
    - trigger:
        afterMs: 604800000
      target: all
```

**清理目标（target）：**
- `events` — 只删除事件，保留任务记录
- `task` — 只删除任务记录
- `all` — 删除任务和所有事件

## 下一步

- [部署指南](./deployment.md) — 生产环境配置
- [REST API](../api/rest.md) — 完整 API 参考
- [认证与权限](../api/authentication.md) — JWT 认证配置