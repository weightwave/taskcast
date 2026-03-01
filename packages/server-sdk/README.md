# @taskcast/server-sdk

HTTP client SDK for [Taskcast](https://github.com/weightwave/taskcast) remote server mode. Use this to interact with a standalone Taskcast server from your backend.

## Install

```bash
pnpm add @taskcast/server-sdk
```

## Usage

```typescript
import { TaskcastServerClient } from '@taskcast/server-sdk'

const client = new TaskcastServerClient({
  baseUrl: 'http://localhost:3721',
  token: 'your-jwt-token', // optional
})

// Create a task
const task = await client.createTask({
  type: 'llm.chat',
  params: { prompt: 'Hello' },
})

// Transition status
await client.transitionTask(task.id, 'running')

// Publish events
await client.publishEvent(task.id, {
  type: 'llm.delta',
  level: 'info',
  data: { text: 'response chunk' },
})

// Complete
await client.transitionTask(task.id, 'completed', {
  result: { output: 'done' },
})
```

## Part of Taskcast

This is the server-side HTTP client. See the [Taskcast monorepo](https://github.com/weightwave/taskcast) for the full project.

## License

[MIT](https://github.com/weightwave/taskcast/blob/main/LICENSE)
