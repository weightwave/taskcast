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

### 删除任务

```
DELETE /tasks/:taskId
```

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
  "data": { "text": "Hello" },
  "seriesId": "response",
  "seriesMode": "accumulate"
}
```

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

**响应：** `201 Created` — 返回创建的事件（单条）或事件数组（批量）

**错误：**
- `400` — 任务不在 `running` 状态时不能发布事件
- `404` — 任务不存在

**所需权限：** `event:publish`

---

### 查询历史事件

```
GET /tasks/:taskId/events/history
```

**查询参数：**

| 参数 | 类型 | 说明 |
|------|------|------|
| `since.id` | string | 从指定事件 ID 之后 |
| `since.index` | number | 从过滤后第 N 条之后 |
| `since.timestamp` | number | 从指定时间戳（ms）之后 |
| `types` | string | 逗号分隔的类型过滤（支持通配符） |
| `levels` | string | 逗号分隔的级别过滤 |

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
    "data": { "text": "Hello" }
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