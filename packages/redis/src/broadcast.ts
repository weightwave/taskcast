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
  private patternGeneration = 0
  private patternSubscription: Promise<void> | undefined
  private disposed = false

  private readonly dispatch = (channel: string, message: string): void => {
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

  private readonly dispatchPattern = (
    _pattern: string,
    channel: string,
    message: string,
  ): void => {
    this.dispatch(channel, message)
  }

  private readonly markPatternUnavailable = (): void => {
    this.markPatternSubscriptionUnavailable()
  }

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

    this.sub.on('message', this.dispatch)
    this.sub.on('pmessage', this.dispatchPattern)

    if (subscriptionMode === 'pattern') {
      this.sub.on('close', this.markPatternUnavailable)
      this.sub.on('reconnecting', this.markPatternUnavailable)
      this.sub.on('end', this.markPatternUnavailable)
    }
  }

  async startPatternSubscription(): Promise<void> {
    if ((this.options.subscriptionMode ?? 'channels') !== 'pattern') {
      throw new Error('Redis pattern subscription mode is not enabled')
    }
    if (this.patternSubscribed) return

    const generation = this.patternGeneration
    const pending = this.patternSubscription ??=
      this.sub.psubscribe(`${this.channelPrefix}*`).then(() => {
        if (generation === this.patternGeneration) {
          this.patternSubscribed = true
        }
      })
    try {
      await pending
    } finally {
      if (this.patternSubscription === pending) {
        this.patternSubscription = undefined
      }
    }
  }

  markPatternSubscriptionUnavailable(): void {
    this.patternSubscribed = false
    this.patternGeneration += 1
  }

  isPatternSubscribed(): boolean {
    return this.patternSubscribed
  }

  dispose(): void {
    if (this.disposed) return
    this.disposed = true
    this.markPatternSubscriptionUnavailable()
    this.handlers.clear()
    this.sub.off('message', this.dispatch)
    this.sub.off('pmessage', this.dispatchPattern)
    if ((this.options.subscriptionMode ?? 'channels') === 'pattern') {
      this.sub.off('close', this.markPatternUnavailable)
      this.sub.off('reconnecting', this.markPatternUnavailable)
      this.sub.off('end', this.markPatternUnavailable)
    }
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
