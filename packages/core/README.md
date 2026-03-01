# @taskcast/core

Task engine, state machine, filtering, and series merging for [Taskcast](https://github.com/weightwave/taskcast). Zero HTTP dependencies.

## Install

```bash
pnpm add @taskcast/core
```

## Usage

```typescript
import {
  TaskEngine,
  MemoryBroadcastProvider,
  MemoryShortTermStore,
} from '@taskcast/core'

const engine = new TaskEngine({
  broadcast: new MemoryBroadcastProvider(),
  shortTermStore: new MemoryShortTermStore(),
})

// Create a task
const task = await engine.createTask({
  type: 'llm.chat',
  params: { prompt: 'Tell me a story' },
  ttl: 3600,
})

// Transition to running
await engine.transitionTask(task.id, 'running')

// Publish streaming events
await engine.publishEvent(task.id, {
  type: 'llm.delta',
  level: 'info',
  data: { text: 'Once upon a time...' },
  seriesId: 'response',
  seriesMode: 'accumulate',
})

// Complete the task
await engine.transitionTask(task.id, 'completed', {
  result: { output: 'The End.' },
})
```

## Key Concepts

- **State Machine** — Task lifecycle: `pending` -> `running` -> `completed` | `failed` | `timeout` | `cancelled`
- **Event Filtering** — Wildcard type matching (`llm.*`), level filtering, cursor-based resumption
- **Series Merging** — `keep-all` (full history), `accumulate` (text concatenation), `latest` (replace)
- **Storage Adapters** — Pluggable `BroadcastProvider`, `ShortTermStore`, and `LongTermStore` interfaces

## Part of Taskcast

This is the core engine package. See the [Taskcast monorepo](https://github.com/weightwave/taskcast) for the full project, including HTTP server, client SDKs, and storage adapters.

## License

[MIT](https://github.com/weightwave/taskcast/blob/main/LICENSE)
