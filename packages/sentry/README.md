# @taskcast/sentry

Sentry error monitoring hooks for [Taskcast](https://github.com/weightwave/taskcast). Automatically captures task failures and timeouts.

## Install

```bash
pnpm add @taskcast/sentry @sentry/node
```

## Usage

```typescript
import { TaskEngine, MemoryBroadcastProvider, MemoryShortTermStore } from '@taskcast/core'
import { createSentryHooks } from '@taskcast/sentry'

const engine = new TaskEngine({
  broadcast: new MemoryBroadcastProvider(),
  shortTermStore: new MemoryShortTermStore(),
  hooks: createSentryHooks({
    captureTaskFailures: true,
    captureTaskTimeouts: true,
  }),
})
```

## Features

- Captures task failures as Sentry exceptions
- Captures task timeouts as Sentry exceptions
- `@sentry/node` is an optional peer dependency — hooks are no-ops if Sentry is not installed

## Part of Taskcast

This is the Sentry integration package. See the [Taskcast monorepo](https://github.com/weightwave/taskcast) for the full project.

## License

[MIT](https://github.com/weightwave/taskcast/blob/main/LICENSE)
