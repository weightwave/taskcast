# @taskcast/server

Hono HTTP server for [Taskcast](https://github.com/weightwave/taskcast) — REST API, SSE streaming, JWT auth, and webhook delivery.

## Install

```bash
pnpm add @taskcast/server @taskcast/core
```

## Usage

```typescript
import { TaskEngine, MemoryBroadcastProvider, MemoryShortTermStore } from '@taskcast/core'
import { createTaskcastApp } from '@taskcast/server'

const engine = new TaskEngine({
  broadcast: new MemoryBroadcastProvider(),
  shortTermStore: new MemoryShortTermStore(),
})

const app = createTaskcastApp({ engine })

// Mount to your existing Hono app or serve directly
export default app
```

### With JWT Auth

```typescript
const app = createTaskcastApp({
  engine,
  auth: {
    mode: 'jwt',
    jwt: {
      algorithm: 'HS256',
      secret: process.env.JWT_SECRET,
    },
  },
})
```

## API Endpoints

| Method | Path | Description |
|--------|------|-------------|
| `POST` | `/tasks` | Create a task |
| `GET` | `/tasks/:taskId` | Get task status and metadata |
| `PATCH` | `/tasks/:taskId/status` | Transition task status |
| `DELETE` | `/tasks/:taskId` | Delete a task |
| `POST` | `/tasks/:taskId/events` | Publish event(s) |
| `GET` | `/tasks/:taskId/events` | Subscribe via SSE |
| `GET` | `/tasks/:taskId/events/history` | Query event history |

## Part of Taskcast

This is the HTTP server package. See the [Taskcast monorepo](https://github.com/weightwave/taskcast) for the full project.

## License

[MIT](https://github.com/weightwave/taskcast/blob/main/LICENSE)
