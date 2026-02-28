# Deployment Guide

## Deployment Modes

### Embedded Mode

Embed the Taskcast engine directly into your existing server. This is the right choice when you already have a Node.js/Bun backend and want to manage tasks within the same process.

```typescript
import { TaskEngine, MemoryBroadcastProvider, MemoryShortTermStore } from '@taskcast/core'
import { createTaskcastApp } from '@taskcast/server'
import { createRedisAdapters } from '@taskcast/redis'
import { createPostgresAdapter } from '@taskcast/postgres'
import Redis from 'ioredis'

// Create Redis adapters
const pubClient = new Redis(process.env.REDIS_URL)
const subClient = new Redis(process.env.REDIS_URL)
const storeClient = new Redis(process.env.REDIS_URL)
const { broadcast, shortTerm } = createRedisAdapters(pubClient, subClient, storeClient)

// Create PostgreSQL adapter (optional)
const longTerm = await createPostgresAdapter({
  url: process.env.DATABASE_URL!,
})

// Create the engine
const engine = new TaskEngine({
  broadcast,
  shortTermStore: shortTerm,
  longTermStore: longTerm, // optional
})

// Create the HTTP application
const app = createTaskcastApp({
  engine,
  auth: {
    mode: 'jwt',
    jwt: {
      algorithm: 'HS256',
      secret: process.env.JWT_SECRET!,
    },
  },
})

// Mount onto your Hono application
import { Hono } from 'hono'
const mainApp = new Hono()
mainApp.route('/taskcast', app)

export default mainApp
```

### Remote Mode

Run Taskcast as a standalone service, with your backend communicating with it over HTTP via the SDK. This is the right choice for microservice architectures or when you want an independently deployable task service.

**Start the service:**

```bash
npx taskcast -c taskcast.config.yaml
```

**Backend integration (producer):**

```typescript
import { TaskcastServerClient } from '@taskcast/server-sdk'

const taskcast = new TaskcastServerClient({
  baseUrl: 'http://taskcast-service:3721',
  token: process.env.TASKCAST_TOKEN,
})

// Create a task
const task = await taskcast.createTask({
  type: 'llm.chat',
  params: { prompt: 'Hello' },
})

// Publish an event
await taskcast.publishEvent(task.id, {
  type: 'llm.delta',
  level: 'info',
  data: { text: 'Hello!' },
})

// Complete the task
await taskcast.transitionTask(task.id, 'completed', {
  result: { output: 'Hello!' },
})
```

**Frontend integration (consumer):**

```typescript
import { TaskcastClient } from '@taskcast/client'

const client = new TaskcastClient({
  baseUrl: 'http://taskcast-service:3721',
  token: userJwtToken,
})

await client.subscribe(taskId, {
  onEvent: (e) => console.log(e),
  onDone: (reason) => console.log('Done:', reason),
})
```

## Configuration

### Configuration Files

Taskcast searches for configuration files in the following order (the first one found is used):

1. `taskcast.config.ts` — Full feature support (functions, custom adapters, middleware)
2. `taskcast.config.js` / `.mjs`
3. `taskcast.config.yaml` / `.yml`
4. `taskcast.config.json`

You can also specify a config file explicitly with the `-c` flag:

```bash
npx taskcast -c /path/to/config.yaml
```

### TypeScript Configuration (recommended for complex setups)

```typescript
// taskcast.config.ts
import type { TaskcastConfig } from '@taskcast/core'

export default {
  port: 3721,
  logLevel: 'info',

  auth: {
    mode: 'jwt',
    jwt: {
      algorithm: 'RS256',
      publicKeyFile: '/run/secrets/jwt.pub',
    },
  },

  adapters: {
    broadcast: { provider: 'redis', url: process.env.REDIS_URL },
    shortTerm: { provider: 'redis', url: process.env.REDIS_URL },
    longTerm: { provider: 'postgres', url: process.env.DATABASE_URL },
  },

  sentry: {
    dsn: process.env.SENTRY_DSN,
    captureTaskFailures: true,
    captureTaskTimeouts: true,
    captureUnhandledErrors: true,
  },

  webhook: {
    defaultRetry: {
      retries: 3,
      backoff: 'exponential',
      initialDelayMs: 1000,
      maxDelayMs: 30000,
      timeoutMs: 5000,
    },
  },

  cleanup: {
    rules: [
      {
        match: { taskTypes: ['llm.*'] },
        trigger: { afterMs: 3600_000 },
        target: 'events',
        eventFilter: { levels: ['debug'] },
      },
      {
        trigger: { afterMs: 86400_000 * 7 },
        target: 'all',
      },
    ],
  },
} satisfies TaskcastConfig
```

### YAML Configuration (recommended for simple setups)

```yaml
# taskcast.config.yaml
port: 3721
logLevel: info

auth:
  mode: jwt
  jwt:
    algorithm: RS256
    publicKeyFile: /run/secrets/jwt.pub

adapters:
  broadcast:
    provider: redis
    url: ${REDIS_URL}
  shortTerm:
    provider: redis
    url: ${REDIS_URL}
  longTerm:
    provider: postgres
    url: ${DATABASE_URL}

sentry:
  dsn: ${SENTRY_DSN}
  captureTaskFailures: true
  captureTaskTimeouts: true

cleanup:
  rules:
    - match:
        taskTypes: ["llm.*"]
      trigger:
        afterMs: 3600000
      target: events
      eventFilter:
        levels: [debug]
    - trigger:
        afterMs: 604800000
      target: all
```

> **Note:** YAML/JSON configuration supports `${ENV_VAR}` environment variable interpolation, but does not support custom middleware or custom adapter instances.

### Environment Variables

All configuration options can be overridden via environment variables:

| Variable | Description | Default |
|----------|-------------|---------|
| `TASKCAST_PORT` | Service port | `3721` |
| `TASKCAST_LOG_LEVEL` | Log level | `info` |
| `TASKCAST_AUTH_MODE` | Auth mode: `none`, `jwt`, or `custom` | `none` |
| `TASKCAST_JWT_SECRET` | JWT HMAC secret (HS256) | — |
| `TASKCAST_JWT_ALGORITHM` | JWT algorithm | `HS256` |
| `TASKCAST_JWT_PUBLIC_KEY_FILE` | Path to JWT public key file (RS256/ES*) | — |
| `TASKCAST_JWT_ISSUER` | JWT issuer | — |
| `TASKCAST_JWT_AUDIENCE` | JWT audience | — |
| `TASKCAST_REDIS_URL` | Redis connection URL | — |
| `TASKCAST_POSTGRES_URL` | PostgreSQL connection URL | — |
| `SENTRY_DSN` | Sentry DSN | — |

**Precedence:** CLI flags > environment variables > config file > defaults

### TS/JS vs YAML/JSON Feature Comparison

| Feature | TS/JS | YAML/JSON |
|---------|-------|-----------|
| Basic config (port, auth, adapters) | Yes | Yes |
| Environment variable interpolation `${VAR}` | Yes | Yes |
| File path references | Yes | Yes |
| Custom auth middleware | Yes | No |
| Custom adapter instances | Yes | No — built-in providers only |
| Sentry custom hooks | Yes | No |

## Production Recommendations

### Minimal Production Configuration

```yaml
# taskcast.config.yaml
port: 3721

auth:
  mode: jwt
  jwt:
    algorithm: HS256
    secret: ${JWT_SECRET}

adapters:
  broadcast:
    provider: redis
    url: ${REDIS_URL}
  shortTerm:
    provider: redis
    url: ${REDIS_URL}
```

This configuration uses Redis for broadcast and short-term storage (Redis is required for multi-instance deployments), JWT authentication, and no long-term storage.

### Full Production Configuration

On top of the minimal configuration, add:

- **PostgreSQL long-term storage** — if you need to persist task history permanently
- **Sentry monitoring** — to capture task failures, timeouts, and unhandled errors
- **Cleanup rules** — to automatically purge stale data and prevent storage bloat
- **Webhooks** — to push task events to external systems

### Multi-Instance Deployment

When deploying multiple Taskcast instances, **Redis is required** as the broadcast and short-term storage layer to ensure:

- SSE subscribers on any instance receive all events
- Task state is consistent across all instances
- Resume-from-checkpoint works correctly regardless of which instance handles the request

### Sentry Integration

```bash
pnpm add @taskcast/sentry @sentry/node
```

In CLI mode, simply set the `SENTRY_DSN` environment variable and Sentry integration is enabled automatically. In embedded mode:

```typescript
import * as Sentry from '@sentry/node'
import { createSentryHooks } from '@taskcast/sentry'

Sentry.init({ dsn: process.env.SENTRY_DSN })

const hooks = createSentryHooks({
  captureTaskFailures: true,
  captureTaskTimeouts: true,
  captureUnhandledErrors: true,
})

const engine = new TaskEngine({
  broadcast,
  shortTermStore: shortTerm,
  hooks,
})
```

## Next Steps

- [REST API](../api/rest.md) — Complete API reference
- [SSE Subscriptions](../api/sse.md) — SSE protocol in detail
- [Authentication & Authorization](../api/authentication.md) — Authentication system in detail