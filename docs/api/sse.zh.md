# SSE 订阅

Taskcast 的 SSE 订阅端点提供实时事件流，支持历史重放、断点续传和事件过滤。

## 端点

```
GET /tasks/:taskId/events
```

**所需权限：** `event:subscribe`

## 订阅行为

SSE 连接的行为取决于任务当前状态：

| 任务状态 | 行为 |
|----------|------|
| `pending` | 挂起等待。任务变为 `running` 后自动重放历史 + 推送实时事件 |
| `running` | 重放历史事件（按过滤条件），然后推送实时事件。任务到达终态后自动断开 |
| 终态 | 重放历史事件（默认），然后发送关闭信号 |
| 不存在 | 返回 `404` |

## 查询参数

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `since.id` | string | — | 从指定事件 ID 之后恢复（跨过滤器精确续传） |
| `since.index` | number | — | 从过滤后第 N 条之后恢复（同过滤器重连） |
| `since.timestamp` | number | — | 从指定时间戳（ms）之后恢复 |
| `types` | string | — | 逗号分隔的类型过滤，支持通配符。如 `llm.*,tool.call` |
| `levels` | string | — | 逗号分隔的级别过滤。如 `info,warn,error` |
| `includeStatus` | boolean | `true` | 是否包含 `taskcast:status` 内置状态事件 |
| `wrap` | boolean | `true` | 是否将事件包裹在 SSEEnvelope 中 |
| `seriesFormat` | string | `delta` | `accumulate` 序列事件的格式：`delta`（原始增量数据）或 `accumulated`（累积总量）。详见[序列格式](#序列格式)。 |
| `limit` | number | — | 连接时重放的历史事件最大数量。不影响重放后推送的实时事件。 |

### 示例

```
# 订阅所有事件
GET /tasks/01HXXX/events

# 只订阅 LLM 相关事件
GET /tasks/01HXXX/events?types=llm.*

# 从第 5 条开始续传，只看 info 及以上
GET /tasks/01HXXX/events?since.index=5&levels=info,warn,error

# 不需要状态事件，不包裹 envelope
GET /tasks/01HXXX/events?includeStatus=false&wrap=false

# 接收累积值而非增量
GET /tasks/01HXXX/events?seriesFormat=accumulated

# 只重放最近 50 条历史事件，然后推送实时事件
GET /tasks/01HXXX/events?limit=50
```

## 事件流格式

### 普通事件（wrap=true，默认）

```
event: taskcast.event
id: 01HXXX001
data: {"filteredIndex":0,"rawIndex":0,"eventId":"01HXXX001","taskId":"01HXXX","type":"llm.delta","timestamp":1700000000000,"level":"info","data":{"delta":"Hello"},"seriesId":"response","seriesMode":"accumulate"}

event: taskcast.event
id: 01HXXX002
data: {"filteredIndex":1,"rawIndex":1,"eventId":"01HXXX002","taskId":"01HXXX","type":"llm.delta","timestamp":1700000000100,"level":"info","data":{"delta":" world!"}}
```

### 普通事件（wrap=false）

```
event: taskcast.event
id: 01HXXX001
data: {"delta":"Hello"}
```

### 状态变更事件

```
event: taskcast.status
data: {"taskId":"01HXXX","status":"completed","result":{"output":"Hello world!"}}
```

### 关闭信号

当任务到达终态，连接关闭前会发送：

```
event: taskcast.done
data: {"reason":"completed"}
```

`reason` 对应任务的终态：`completed`、`failed`、`timeout`、`cancelled`。

## SSEEnvelope 结构

当 `wrap=true` 时，每个事件被包裹在 envelope 中：

```typescript
interface SSEEnvelope {
  filteredIndex: number  // 过滤后的序号（0, 1, 2...），用于 since.index 断点续传
  rawIndex: number       // 原始全局序号，供调试
  eventId: string        // 事件 ULID
  taskId: string
  type: string           // 事件类型
  timestamp: number      // ms 时间戳
  level: string
  data: unknown          // 事件数据
  seriesId?: string
  seriesMode?: string
  seriesSnapshot?: boolean  // 为 true 时表示此事件是迟到加入的快照（非增量 delta）
  clientId?: string      // 生产者客户端 ID（仅当发布方启用了 client seq 排序时出现）
  clientSeq?: number     // 生产者序列号（仅当发布方启用了 client seq 排序时出现）
}
```

**`filteredIndex` 的作用：** 当使用过滤条件时，`rawIndex` 可能不连续（被过滤掉的事件跳过了），而 `filteredIndex` 始终从 0 开始连续递增。客户端用 `filteredIndex` 配合 `since.index` 实现断点续传。

**`clientId`/`clientSeq` 的作用：** 这两个字段由发布端使用，用于 [写入时排序](./rest.zh.md#发布事件)。它们会透传到 SSE 流中，方便下游消费者在需要时将事件与生产客户端关联。它们**不影响**订阅行为 —— SSE 的过滤、断点续传与排序与这两个字段无关。

## 断点续传

### 场景：页面刷新后恢复

```javascript
// 记录最后收到的 filteredIndex
let lastIndex = -1

client.subscribe(taskId, {
  filter: { types: ['llm.*'] },
  onEvent: (envelope) => {
    lastIndex = envelope.filteredIndex
    // 处理事件...
  },
})

// 刷新后，用同样的过滤条件 + since.index 恢复
client.subscribe(taskId, {
  filter: {
    types: ['llm.*'],
    since: { index: lastIndex }, // 从上次的位置继续
  },
  onEvent: (envelope) => {
    // 只会收到 lastIndex 之后的事件
  },
})
```

### 场景：跨过滤器恢复

如果你改变了过滤条件（比如从只看 `llm.*` 变成看所有事件），`since.index` 就不准了。此时使用 `since.id`：

```javascript
client.subscribe(taskId, {
  filter: {
    // 新的过滤条件
    since: { id: lastEventId }, // 从某个事件之后继续，无论过滤条件如何变化
  },
})
```

## 序列格式

`seriesFormat` 查询参数控制 `accumulate` 序列事件的交付格式。

### `seriesFormat=delta`（默认）

每条事件携带原始增量数据。这是流式 UI 的自然格式——逐个拼接到达的片段。

### `seriesFormat=accumulated`

每条事件携带累积总量——截至该时刻的完整累积值。适用于只需显示当前结果、无需跟踪各增量的简单展示场景。

### 迟到加入行为

当订阅者连接到已有 `accumulate` 序列事件的任务时（即"迟到加入"），服务器会将每个 accumulate 序列的所有历史事件**折叠**为单条快照事件。无论 `seriesFormat` 设置如何，此行为都会生效。

快照事件在 envelope 中带有 `seriesSnapshot: true`，包含完整的累积值。

| 场景 | 行为 |
|------|------|
| 从头订阅 | 按所选格式推送实时事件（delta 或 accumulated） |
| 迟到加入（序列活跃） | 每个序列一条快照 + 后续事件按所选格式 |
| 迟到加入（序列不活跃） | 每个序列一条快照 |
| 终态任务重放 | 每个序列一条快照 |
| 带 `since` 游标重连 | **不折叠** — 从断点开始按所选格式推送 |

非序列事件不受 `seriesFormat` 影响，始终原样交付。

### 存储语义

在 `accumulate` 模式下，短期存储保存**增量事件**，长期存储保存**累积事件**。REST 历史端点（`GET /tasks/:taskId/events/history`）按存储原样返回数据。

## 认证

SSE 端点通过 `Authorization` 请求头进行认证：

```
GET /tasks/01HXXX/events
Authorization: Bearer <jwt-token>
```

JWT payload 中的 `taskIds` 字段控制可访问的任务 ID，`scope` 中需包含 `event:subscribe`。

## 连接管理

- 当任务到达终态时，服务端会发送 `taskcast.done` 事件并关闭连接
- 客户端断开连接时，服务端会自动清理订阅资源
- 长时间空闲的连接不会被主动关闭（由客户端的 SSE 机制维护心跳）