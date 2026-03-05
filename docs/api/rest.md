# REST API

All endpoints share the service root path (default `http://localhost:3721`).

Requests and responses use JSON with camelCase field names.

## Task Management

### Create Task

```
POST /tasks
```

**Request body:**

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

All fields are optional. If `id` is not provided, a ULID is generated automatically.

**Response:** `201 Created`

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

**Required permission:** `task:create`

---

### Get Task

```
GET /tasks/:taskId
```

**Response:** `200 OK`

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

Returns `404` if the task does not exist.

**Required permission:** `event:subscribe` (must have access to the given taskId)

---

### Update Task Status

```
PATCH /tasks/:taskId/status
```

**Request body:**

```json
{
  "status": "completed",
  "result": { "output": "Hello!" }
}
```

Additional fields by target status:

| Target status | Additional fields |
|---------------|-------------------|
| `running` | None |
| `completed` | `result` (optional) |
| `failed` | `error: { code?, message, details? }` (optional) |
| `timeout` | `error` (optional) |
| `cancelled` | None |

**Response:** `200 OK` — returns the updated Task object

**Errors:**
- `400` — Invalid status transition (e.g. `completed → running`)
- `404` — Task not found
- `409` — Concurrent conflict (task has already been transitioned to a terminal state by another request)

**Required permission:** `task:manage`

---

### Delete Task (planned)

```
DELETE /tasks/:taskId
```

> **Note:** This endpoint is planned but not yet implemented.

**Response:** `204 No Content`

**Required permission:** `task:manage`

---

## Event Management

### Publish Events

```
POST /tasks/:taskId/events
```

**Single event:**

```json
{
  "type": "llm.delta",
  "level": "info",
  "data": { "delta": "Hello" },
  "seriesId": "response",
  "seriesMode": "accumulate"
}
```

> **Note:** In `accumulate` mode, the field concatenated defaults to `delta`. This can be customized via `seriesAccField`.

**Batch events:**

```json
[
  { "type": "tool.call", "level": "info", "data": { "name": "search", "args": {} } },
  { "type": "tool.result", "level": "info", "data": { "output": "..." } }
]
```

**Field reference:**

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `type` | string | Yes | Event type, supports wildcard filtering |
| `level` | string | No | `debug`/`info`/`warn`/`error`, defaults to `info` |
| `data` | any | Yes | Event payload, arbitrary JSON |
| `seriesId` | string | No | Series ID for grouping |
| `seriesMode` | string | No | `keep-all`/`accumulate`/`latest` |
| `seriesAccField` | string | No | Field name to concatenate in `accumulate` mode (defaults to `delta`) |

**Response:** `201 Created` — returns the created event (single) or event array (batch)

**Errors:**
- `400` — Cannot publish events when the task is not in `running` status
- `404` — Task not found

**Required permission:** `event:publish`

---

### Query Event History

```
GET /tasks/:taskId/events/history
```

**Query parameters:**

| Parameter | Type | Description |
|-----------|------|-------------|
| `since.id` | string | After the specified event ID |
| `since.index` | number | After the Nth filtered event |
| `since.timestamp` | number | After the specified timestamp (ms) |
| `types` | string | Comma-separated type filter (supports wildcards) |
| `levels` | string | Comma-separated level filter |

**Response:** `200 OK`

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

**Required permission:** `event:history`

---

## Error Response Format

All error responses use a consistent format:

```json
{
  "error": {
    "code": "INVALID_STATUS_TRANSITION",
    "message": "Cannot transition from 'completed' to 'running'"
  }
}
```

## HTTP Status Codes

| Status code | Meaning |
|-------------|---------|
| `200` | Success |
| `201` | Created |
| `204` | Deleted (no content) |
| `400` | Bad request or invalid operation |
| `401` | Unauthenticated |
| `403` | Forbidden (insufficient permissions) |
| `404` | Resource not found |
| `409` | Concurrent conflict |
