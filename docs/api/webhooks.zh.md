# Webhook

Taskcast 支持将任务事件通过 HTTP 回调推送到外部系统。支持全局和任务级配置，包含签名验证和自动重试。

## 配置

### 全局 Webhook

在配置文件中设置，对所有任务生效：

```yaml
webhook:
  defaultRetry:
    retries: 3
    backoff: exponential
    initialDelayMs: 1000
    maxDelayMs: 30000
    timeoutMs: 5000
```

### 任务级 Webhook

在创建任务时配置，仅对该任务生效：

```json
{
  "type": "llm.chat",
  "webhooks": [
    {
      "url": "https://example.com/hooks/task-events",
      "secret": "my-webhook-secret",
      "filter": {
        "types": ["llm.*"],
        "levels": ["info", "warn", "error"]
      },
      "wrap": true,
      "retry": {
        "retries": 5,
        "backoff": "exponential",
        "initialDelayMs": 2000,
        "maxDelayMs": 60000,
        "timeoutMs": 10000
      }
    }
  ]
}
```

## Webhook 配置项

```typescript
interface WebhookConfig {
  url: string              // 回调 URL
  filter?: SubscribeFilter // 事件过滤（同 SSE 过滤规则）
  secret?: string          // HMAC-SHA256 签名密钥
  wrap?: boolean           // 是否包裹 envelope（默认 true）
  retry?: RetryConfig      // 重试配置
}

interface RetryConfig {
  retries: number          // 最大重试次数（默认 3）
  backoff: 'fixed' | 'exponential' | 'linear'
  initialDelayMs: number   // 初始延迟（默认 1000ms）
  maxDelayMs: number       // 最大延迟（默认 30000ms）
  timeoutMs: number        // 请求超时（默认 5000ms）
}
```

## HTTP 请求格式

每个匹配过滤条件的事件会触发一个 HTTP POST 请求：

```
POST https://example.com/hooks/task-events
Content-Type: application/json
X-Taskcast-Event: llm.delta
X-Taskcast-Timestamp: 1700000000
X-Taskcast-Signature: sha256=abc123...
```

**请求体** 是事件的 JSON 表示（如果 `wrap=true`，则包裹在 SSEEnvelope 中）。

## 请求头

| 头 | 说明 |
|----|------|
| `Content-Type` | 始终为 `application/json` |
| `X-Taskcast-Event` | 事件类型，如 `llm.delta` |
| `X-Taskcast-Timestamp` | 事件时间戳（Unix 秒） |
| `X-Taskcast-Signature` | HMAC-SHA256 签名（仅在配置了 `secret` 时存在） |

## 签名验证

如果配置了 `secret`，Taskcast 会使用 HMAC-SHA256 对请求体进行签名：

```
签名 = HMAC-SHA256(secret, requestBody)
X-Taskcast-Signature: sha256=<hex编码的签名>
```

### 验证示例（Node.js）

```typescript
import { createHmac, timingSafeEqual } from 'crypto'

function verifyWebhook(body: string, signature: string, secret: string): boolean {
  const expected = 'sha256=' + createHmac('sha256', secret)
    .update(body)
    .digest('hex')

  return timingSafeEqual(
    Buffer.from(signature),
    Buffer.from(expected),
  )
}

// 在你的 webhook 处理器中
app.post('/hooks/task-events', (req, res) => {
  const signature = req.headers['x-taskcast-signature']
  const body = JSON.stringify(req.body)

  if (!verifyWebhook(body, signature, 'my-webhook-secret')) {
    return res.status(401).send('Invalid signature')
  }

  // 处理事件...
  console.log(req.body)
  res.status(200).send('OK')
})
```

## 重试策略

当 webhook 请求失败时（非 2xx 响应或网络错误），Taskcast 会根据配置自动重试。

### 退避策略

| 策略 | 计算公式 | 示例（initialDelayMs=1000） |
|------|----------|----------------------------|
| `fixed` | 每次等待相同时间 | 1s, 1s, 1s |
| `linear` | `initialDelayMs * attempt` | 1s, 2s, 3s |
| `exponential` | `initialDelayMs * 2^(attempt-1)` | 1s, 2s, 4s |

所有延迟都不超过 `maxDelayMs`。

### 失败处理

- 如果所有重试都失败，Taskcast 会触发 `onWebhookFailed` hook（可用于 Sentry 告警）
- Webhook 失败不会影响事件的正常发布和 SSE 推送
- 不保证 exactly-once 投递，接收方应做好幂等处理

## 事件过滤

Webhook 的 `filter` 支持与 SSE 订阅相同的过滤规则：

```json
{
  "filter": {
    "types": ["llm.*", "tool.call"],
    "levels": ["info", "warn", "error"],
    "includeStatus": true
  }
}
```

详见 [SSE 订阅](./sse.md) 中的过滤规则说明。

## 所需权限

创建带 webhook 的任务需要 `webhook:create` 权限：

```json
{
  "taskIds": "*",
  "scope": ["task:create", "webhook:create"]
}
```