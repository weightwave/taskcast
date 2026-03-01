# @taskcast/postgres

PostgreSQL long-term store adapter for [Taskcast](https://github.com/weightwave/taskcast). Provides permanent archival of tasks and events.

## Install

```bash
pnpm add @taskcast/postgres postgres
```

## Usage

```typescript
import { TaskEngine, MemoryBroadcastProvider, MemoryShortTermStore } from '@taskcast/core'
import { createPostgresAdapter } from '@taskcast/postgres'

const longTermStore = createPostgresAdapter({
  url: process.env.DATABASE_URL,
})

const engine = new TaskEngine({
  broadcast: new MemoryBroadcastProvider(),
  shortTermStore: new MemoryShortTermStore(),
  longTermStore,
})
```

## Features

- Permanent archival of tasks and events
- Async writes — non-blocking to the main event pipeline
- Built on the [`postgres`](https://www.npmjs.com/package/postgres) driver

## Part of Taskcast

This is the PostgreSQL adapter package. See the [Taskcast monorepo](https://github.com/weightwave/taskcast) for the full project.

## License

[MIT](https://github.com/weightwave/taskcast/blob/main/LICENSE)
