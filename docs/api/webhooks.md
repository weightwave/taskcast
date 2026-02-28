# Webhooks

Taskcast supports pushing task events to external systems via HTTP callbacks. Both global and task-level configuration are supported, with signature verification and automatic retries.

## Configuration

### Global Webhooks

Set in the configuration file; applies to all tasks:

```yaml
webhook:
  defaultRetry:
    retries: 3
    backoff: exponential
    initialDelayMs: 1000
    maxDelayMs: 30000
    timeoutMs: 5000
```

### Task-Level Webhooks

Configured when creating a task; applies only to that task:

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

## Webhook Configuration Fields

```typescript
interface WebhookConfig {
  url: string              // Callback URL
  filter?: SubscribeFilter // Event filter (same rules as SSE filtering)
  secret?: string          // HMAC-SHA256 signing secret
  wrap?: boolean           // Whether to wrap in envelope (default: true)
  retry?: RetryConfig      // Retry configuration
}

interface RetryConfig {
  retries: number          // Maximum retry count (default: 3)
  backoff: 'fixed' | 'exponential' | 'linear'
  initialDelayMs: number   // Initial delay (default: 1000ms)
  maxDelayMs: number       // Maximum delay (default: 30000ms)
  timeoutMs: number        // Request timeout (default: 5000ms)
}
```

## HTTP Request Format

Each event matching the filter triggers an HTTP POST request:

```
POST https://example.com/hooks/task-events
Content-Type: application/json
X-Taskcast-Event: llm.delta
X-Taskcast-Timestamp: 1700000000
X-Taskcast-Signature: sha256=abc123...
```

The **request body** is the JSON representation of the event (wrapped in an SSEEnvelope if `wrap=true`).

## Request Headers

| Header | Description |
|--------|-------------|
| `Content-Type` | Always `application/json` |
| `X-Taskcast-Event` | Event type, e.g. `llm.delta` |
| `X-Taskcast-Timestamp` | Event timestamp (Unix seconds) |
| `X-Taskcast-Signature` | HMAC-SHA256 signature (present only when `secret` is configured) |

## Signature Verification

When a `secret` is configured, Taskcast signs the request body using HMAC-SHA256:

```
signature = HMAC-SHA256(secret, requestBody)
X-Taskcast-Signature: sha256=<hex-encoded signature>
```

### Verification Example (Node.js)

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

// In your webhook handler
app.post('/hooks/task-events', (req, res) => {
  const signature = req.headers['x-taskcast-signature']
  const body = JSON.stringify(req.body)

  if (!verifyWebhook(body, signature, 'my-webhook-secret')) {
    return res.status(401).send('Invalid signature')
  }

  // Process the event...
  console.log(req.body)
  res.status(200).send('OK')
})
```

## Retry Strategy

When a webhook request fails (non-2xx response or network error), Taskcast automatically retries according to the configuration.

### Backoff Strategies

| Strategy | Formula | Example (initialDelayMs=1000) |
|----------|---------|-------------------------------|
| `fixed` | Same delay every time | 1s, 1s, 1s |
| `linear` | `initialDelayMs * attempt` | 1s, 2s, 3s |
| `exponential` | `initialDelayMs * 2^(attempt-1)` | 1s, 2s, 4s |

All delays are capped at `maxDelayMs`.

### Failure Handling

- If all retries are exhausted, Taskcast triggers the `onWebhookFailed` hook (can be used for Sentry alerting)
- Webhook failures do not affect normal event publishing or SSE streaming
- Delivery is not guaranteed to be exactly-once; receivers should implement idempotency

## Event Filtering

Webhook `filter` supports the same filtering rules as SSE subscriptions:

```json
{
  "filter": {
    "types": ["llm.*", "tool.call"],
    "levels": ["info", "warn", "error"],
    "includeStatus": true
  }
}
```

See [SSE Subscriptions](./sse.md) for details on filtering rules.

## Required Permission

Creating a task with webhooks requires the `webhook:create` permission:

```json
{
  "taskIds": "*",
  "scope": ["task:create", "webhook:create"]
}
```