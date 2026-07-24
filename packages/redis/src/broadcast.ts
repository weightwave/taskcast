import type { Redis } from 'ioredis'
import type { BroadcastProvider, TaskEvent } from '@taskcast/core'
import {
  observeRedisCommand,
  type RedisOperationOptions,
} from './connectivity.js'

type SubscriptionMode = 'channels' | 'pattern'

export class RedisBroadcastProvider implements BroadcastProvider {
  private handlers = new Map<string, Set<(event: TaskEvent) => void>>()
  private channelPrefix: string
  private patternSubscribed = false

  constructor(
    private pub: Redis,
    private sub: Redis,
    private options: {
      prefix?: string
      subscriptionMode?: SubscriptionMode
    } & RedisOperationOptions = {},
  ) {
    const { prefix, subscriptionMode = 'channels' } = options
    const resolvedPrefix =
      prefix ?? process.env['TASKCAST_REDIS_PREFIX'] ?? 'taskcast'
    this.channelPrefix = `${resolvedPrefix}:task:`

    const dispatch = (channel: string, message: string) => {
      const taskId = channel.startsWith(this.channelPrefix)
        ? channel.slice(this.channelPrefix.length)
        : channel
      const handlers = this.handlers.get(taskId)
      if (!handlers) return
      try {
        const event = JSON.parse(message) as TaskEvent
        for (const handler of handlers) handler(event)
      } catch {
        // Malformed messages are ignored.
      }
    }

    this.sub.on('message', dispatch)
    this.sub.on(
      'pmessage',
      (_pattern: string, channel: string, message: string) =>
        dispatch(channel, message),
    )

    if (subscriptionMode === 'pattern') {
      this.sub.on('ready', () => {
        this.patternSubscribed = true
      })
      this.sub.on('reconnecting', () => {
        this.patternSubscribed = false
      })
      this.sub.on('end', () => {
        this.patternSubscribed = false
      })
    }
  }

  async startPatternSubscription(): Promise<void> {
    if ((this.options.subscriptionMode ?? 'channels') !== 'pattern') {
      throw new Error('Redis pattern subscription mode is not enabled')
    }
    await this.sub.psubscribe(`${this.channelPrefix}*`)
    this.patternSubscribed = true
  }

  isPatternSubscribed(): boolean {
    return this.patternSubscribed
  }

  async publish(channel: string, event: TaskEvent): Promise<void> {
    await observeRedisCommand(this.options, () =>
      this.pub.publish(this.channelPrefix + channel, JSON.stringify(event)),
    )
  }

  subscribe(channel: string, handler: (event: TaskEvent) => void): () => void {
    if (!this.handlers.has(channel)) {
      this.handlers.set(channel, new Set())
      if ((this.options.subscriptionMode ?? 'channels') === 'channels') {
        this.sub.subscribe(this.channelPrefix + channel)
      }
    }
    this.handlers.get(channel)!.add(handler)

    return () => {
      const set = this.handlers.get(channel)
      if (!set) return
      set.delete(handler)
      if (set.size === 0) {
        this.handlers.delete(channel)
        if ((this.options.subscriptionMode ?? 'channels') === 'channels') {
          this.sub.unsubscribe(this.channelPrefix + channel)
        }
      }
    }
  }
}
