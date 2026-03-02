# @taskcast/redis

Redis adapters for [Taskcast](https://github.com/weightwave/taskcast) — real-time broadcast via pub/sub and short-term event/task storage.

## Install

```bash
pnpm add @taskcast/redis ioredis
```

## Usage

```typescript
import { TaskEngine } from '@taskcast/core'
import { createRedisAdapters } from '@taskcast/redis'
import Redis from 'ioredis'

const pubClient = new Redis(process.env.REDIS_URL)
const subClient = new Redis(process.env.REDIS_URL)
const storeClient = new Redis(process.env.REDIS_URL)

const { broadcast, shortTermStore } = createRedisAdapters(pubClient, subClient, storeClient)

const engine = new TaskEngine({
  broadcast,
  shortTermStore,
})
```

## Adapters

- **`RedisBroadcastProvider`** — Real-time event fan-out via Redis pub/sub
- **`RedisShortTermStore`** — Event buffer and task state storage via Redis

Both adapters are created together with `createRedisAdapters()`, which requires three separate Redis connections (pub, sub, store).

## Part of Taskcast

This is the Redis adapter package. See the [Taskcast monorepo](https://github.com/weightwave/taskcast) for the full project.

## License

[MIT](https://github.com/weightwave/taskcast/blob/main/LICENSE)
