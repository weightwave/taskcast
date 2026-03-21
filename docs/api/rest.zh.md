# REST API

所有端点的基础路径为服务根路径（默认 `http://localhost:3721`）。

请求和响应均使用 JSON 格式，字段名为 camelCase。

## 任务管理

### 创建任务

```
POST /tasks
```

**请求体：**

```json
{
  "id": "custom-id",
  "type": "llm.chat",
  "params": { "prompt": "Hello" },
  "metadata": { "userId": "u1" },
  "ttl": 3600,
  "webhooks": [
    {
      "url": "https://example.com/hook",
      "secret": "hmac-secret",
      "filter": { "types": ["llm.*"] }
    }
  ]
}
```

所有字段均为可选。如果不提供 `id`，会自动生成 ULID。

**响应：** `201 Created`

```json
{
  "id": "01HXXXXXXXXXXXXXXXXXXX",
  "type": "llm.chat",
  "status": "pending",
  "params": { "prompt": "Hello" },
  "metadata": { "userId": "u1" },
  "createdAt": 1700000000000,
  "updatedAt": 1700000000000,
  "ttl": 3600
}
```

**所需权限：** `task:create`

---

### 查询任务

```
GET /tasks/:taskId
```

**响应：** `200 OK`

```json
{
  "id": "01HXXXXXXXXXXXXXXXXXXX",
  "type": "llm.chat",
  "status": "running",
  "params": { "prompt": "Hello" },
  "createdAt": 1700000000000,
  "updatedAt": 1700000000100
}
```

任务不存在时返回 `404`。

**所需权限：** `event:subscribe`（需对该 taskId 有访问权限）

---

### 更新任务状态

```
PATCH /tasks/:taskId/status
```

**请求体：**

```json
{
  "status": "completed",
  "result": { "output": "Hello!" }
}
```

不同目标状态的请求体：

| 目标状态 | 附加字段 |
|----------|----------|
| `running` | 无 |
| `completed` | `result`（可选） |
| `failed` | `error: { code?, message, details? }`（可选） |
| `timeout` | `error`（可选） |
| `cancelled` | 无 |

**响应：** `200 OK` — 返回更新后的 Task 对象

**错误：**
- `400` — 非法状态转换（如 `completed → running`）
- `404` — 任务不存在
- `409` — 并发冲突（任务已被其他请求转换到终态）

**所需权限：** `task:manage`

---

### 删除任务（planned）

```
DELETE /tasks/:taskId
```

> **注意：** 此端点已规划但尚未实现。

**响应：** `204 No Content`

**所需权限：** `task:manage`

---

## 事件管理

### 发布事件

```
POST /tasks/:taskId/events
```

**单条事件：**

```json
{
  "type": "llm.delta",
  "level": "info",
  "data": { "delta": "Hello" },
  "seriesId": "response",
  "seriesMode": "accumulate"
}
```

> **注意：** 在 `accumulate` 模式下，默认拼接的字段为 `delta`。可通过 `seriesAccField` 自定义拼接字段名。

**批量事件：**

```json
[
  { "type": "tool.call", "level": "info", "data": { "name": "search", "args": {} } },
  { "type": "tool.result", "level": "info", "data": { "output": "..." } }
]
```

**字段说明：**

| 字段 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `type` | string | 是 | 事件类型，支持通配符过滤 |
| `level` | string | 否 | `debug`/`info`/`warn`/`error`，默认 `info` |
| `data` | any | 是 | 事件数据，任意 JSON |
| `seriesId` | string | 否 | 序列 ID，用于分组 |
| `seriesMode` | string | 否 | `keep-all`/`accumulate`/`latest` |
| `seriesAccField` | string | 否 | `accumulate` 模式下拼接的字段名（默认为 `delta`） |
| `clientId` | string | 否 | 客户端标识，用于序号排序。必须与 `clientSeq` 同时提供 |
| `clientSeq` | integer | 否 | 单调递增的客户端序号（≥ 0）。必须与 `clientId` 同时提供 |
| `seqMode` | string | 否 | `hold`（默认）或 `fast-fail`。控制乱序事件的处理方式 |

> **序号排序：** 当提供 `clientId` 和 `clientSeq` 时，服务器保证在同一 `(taskId, clientId)` 内按 `clientSeq` 顺序写入事件。乱序请求会被 hold 等待间隙填补（默认超时 30 秒），或在 `fast-fail` 模式下立即拒绝。不同 `clientId` 之间相互独立。两个字段都不传时完全跳过排序（向后兼容）。

**响应：** `201 Created` — 返回创建的事件（单条）或事件数组（批量）。提供 `clientId`/`clientSeq` 时会包含在响应中。

**错误：**
- `400` — 参数无效（任务不在 `running` 状态、`clientId`/`clientSeq` 未同时提供、`clientSeq` 为负数）
- `404` — 任务不存在
- `408` — 序号等待超时，间隙未在超时时间内填补
- `409` — 序号冲突：`seq_stale`（序号已消费）、`seq_duplicate`（重复序号）、`seq_gap`（fast-fail 模式，存在间隙）

**所需权限：** `event:publish`

---

### 查询序号状态

```
GET /tasks/:taskId/seq/:clientId
```

返回指定客户端的当前期望序号。

**响应：** `200 OK`

```json
{
  "clientId": "worker-1",
  "expectedSeq": 5
}
```

**错误：**
- `404` — 该客户端无序号状态（从未使用此 `clientId` 发布过事件，或任务已到达终态）

**所需权限：** `event:publish`

---

### 查询历史事件

```
GET /tasks/:taskId/events/history
```

**查询参数：**

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `since.id` | string | — | 从指定事件 ID 之后 |
| `since.index` | number | — | 从过滤后第 N 条之后 |
| `since.timestamp` | number | — | 从指定时间戳（ms）之后 |
| `types` | string | — | 逗号分隔的类型过滤（支持通配符） |
| `levels` | string | — | 逗号分隔的级别过滤 |
| `limit` | number | — | 返回事件的最大数量 |
| `seriesFormat` | string | `delta` | `accumulate` 序列的输出格式：`delta`（原样返回）或 `accumulated`（折叠为快照） |

**关于 `seriesFormat`：** 当请求 `accumulated` 时，同一 `accumulate` 序列的所有事件会折叠为一条快照事件（`seriesSnapshot: true`）。对于热任务（数据在短期存储中），快照反映最新的累积值。对于冷任务（数据在长期存储中），事件已按累积形式存储，因此 `delta` 和 `accumulated` 返回相同结果。

**关于 `limit`：** limit 在存储层生效，在序列折叠之前应用。当与 `seriesFormat=accumulated` 组合使用时，最终结果可能少于 limit 条，因为多条序列事件被折叠为一条。

**响应：** `200 OK`

```json
[
  {
    "id": "01HXXX001",
    "taskId": "01HXXX",
    "index": 0,
    "timestamp": 1700000000000,
    "type": "llm.delta",
    "level": "info",
    "data": { "delta": "Hello" }
  }
]
```

**所需权限：** `event:history`

---

## 错误响应格式

所有错误响应使用统一格式：

```json
{
  "error": {
    "code": "INVALID_STATUS_TRANSITION",
    "message": "Cannot transition from 'completed' to 'running'"
  }
}
```

## HTTP 状态码

| 状态码 | 含义 |
|--------|------|
| `200` | 成功 |
| `201` | 创建成功 |
| `204` | 删除成功（无内容） |
| `400` | 请求参数错误或非法操作 |
| `401` | 未认证 |
| `403` | 权限不足 |
| `404` | 资源不存在 |
| `409` | 并发冲突 |