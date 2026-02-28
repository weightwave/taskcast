export { RedisBroadcastProvider } from './broadcast.js'
export { RedisShortTermStore } from './short-term.js'

import type { Redis } from 'ioredis'
import { RedisBroadcastProvider } from './broadcast.js'
import { RedisShortTermStore } from './short-term.js'

export interface RedisAdapterOptions {
  url?: string
  client?: Redis
}

/**
 * Convenience factory: creates a Redis instance configured for both
 * broadcast and short-term store.
 *
 * NOTE: Requires two separate Redis connections (pub and sub cannot share
 * a connection in subscribe mode).
 */
export function createRedisAdapters(pubClient: Redis, subClient: Redis, storeClient: Redis) {
  return {
    broadcast: new RedisBroadcastProvider(pubClient, subClient),
    shortTerm: new RedisShortTermStore(storeClient),
  }
}
