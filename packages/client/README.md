# @taskcast/client

Browser SSE subscription client for [Taskcast](https://github.com/weightwave/taskcast). Subscribe to task event streams with automatic reconnection and cursor-based resumption.

## Install

```bash
pnpm add @taskcast/client
```

## Usage

```typescript
import { TaskcastClient } from '@taskcast/client'

const client = new TaskcastClient({
  baseUrl: 'http://localhost:3721',
  token: 'your-jwt-token', // optional
})

const taskId = 'task-123' // replace with your actual task ID

await client.subscribe(taskId, {
  filter: {
    types: ['llm.*'],
    since: { index: 0 },
  },
  onEvent: (envelope) => {
    console.log(envelope.data) // { text: "Once upon a time..." }
  },
  onDone: (reason) => {
    console.log(`Task ${reason}`) // "Task completed"
  },
})
```

## Features

- SSE-based real-time event streaming
- Wildcard type filtering (`llm.*`, `tool.call`)
- Cursor-based resumption (by event ID, index, or timestamp)
- Automatic reconnection on disconnect

## Part of Taskcast

This is the browser client package. See the [Taskcast monorepo](https://github.com/weightwave/taskcast) for the full project. For React integration, see [`@taskcast/react`](https://www.npmjs.com/package/@taskcast/react).

## License

[MIT](https://github.com/weightwave/taskcast/blob/main/LICENSE)
