# @taskcast/react

React hook for [Taskcast](https://github.com/weightwave/taskcast) SSE event subscriptions.

## Install

```bash
pnpm add @taskcast/react @taskcast/client @taskcast/core react
```

## Usage

```typescript
import { useTaskEvents } from '@taskcast/react'

function TaskStream({ taskId }: { taskId: string }) {
  const { events, isDone, doneReason, error } = useTaskEvents(taskId, {
    baseUrl: 'http://localhost:3721',
    filter: { types: ['llm.*'] },
  })

  return (
    <div>
      {events.map((e) => (
        <span key={e.eventId}>{e.data.text}</span>
      ))}
      {isDone && <p>Done: {doneReason}</p>}
      {error && <p>Error: {error.message}</p>}
    </div>
  )
}
```

## API

### `useTaskEvents(taskId, options)`

Returns:

| Field | Type | Description |
|-------|------|-------------|
| `events` | `TaskEventEnvelope[]` | Accumulated events |
| `isDone` | `boolean` | Whether the task has reached a terminal state |
| `doneReason` | `string \| undefined` | Terminal status (`completed`, `failed`, etc.) |
| `error` | `Error \| undefined` | Connection or parse errors |

## Part of Taskcast

This is the React integration package. See the [Taskcast monorepo](https://github.com/weightwave/taskcast) for the full project.

## License

[MIT](https://github.com/weightwave/taskcast/blob/main/LICENSE)
