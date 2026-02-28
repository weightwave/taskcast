export { RedisBroadcastProvider } from './broadcast.js'
export { RedisShortTermStore } from './short-term.js'

import type { Redis } from 'ioredis'
import { RedisBroadcastProvider } from './broadcast.js'
import { RedisShortTermStore } from './short-term.js'

export interface RedisAdapterOptions {
  /**
   * Key/channel prefix used for all Redis keys. Defaults to 'taskcast'.
   * Can also be set via TASKCAST_REDIS_PREFIX environment variable.
   */
  prefix?: string
}

/**
 * Convenience factory: creates Redis adapters configured for both
 * broadcast and short-term store.
 *
 * NOTE: Requires three separate Redis connections:
 * - pubClient: for publishing events
 * - subClient: for subscribing to events (subscribe mode connection)
 * - storeClient: for task/event storage operations
 */
export function createRedisAdapters(
  pubClient: Redis,
  subClient: Redis,
  storeClient: Redis,
  options: RedisAdapterOptions = {},
) {
  return {
    broadcast: new RedisBroadcastProvider(pubClient, subClient, options),
    shortTerm: new RedisShortTermStore(storeClient, options),
  }
}
