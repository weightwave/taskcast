import type { Redis } from 'ioredis'
import type { BroadcastProvider, TaskEvent } from '@taskcast/core'

export class RedisBroadcastProvider implements BroadcastProvider {
  // 每个 channel 的本地 handlers，在接收到 Redis 消息后转发
  private handlers = new Map<string, Set<(event: TaskEvent) => void>>()
  private channelPrefix: string

  constructor(
    private pub: Redis,
    private sub: Redis,
    { prefix }: { prefix?: string } = {},
  ) {
    const resolvedPrefix = prefix ?? process.env['TASKCAST_REDIS_PREFIX'] ?? 'taskcast'
    this.channelPrefix = `${resolvedPrefix}:task:`

    this.sub.on('message', (channel: string, message: string) => {
      const taskId = channel.startsWith(this.channelPrefix)
        ? channel.slice(this.channelPrefix.length)
        : channel
      const handlers = this.handlers.get(taskId)
      if (!handlers) return
      try {
        const event = JSON.parse(message) as TaskEvent
        for (const handler of handlers) handler(event)
      } catch {
        // malformed message, ignore
      }
    })
  }

  async publish(channel: string, event: TaskEvent): Promise<void> {
    await this.pub.publish(this.channelPrefix + channel, JSON.stringify(event))
  }

  subscribe(channel: string, handler: (event: TaskEvent) => void): () => void {
    if (!this.handlers.has(channel)) {
      this.handlers.set(channel, new Set())
      this.sub.subscribe(this.channelPrefix + channel)
    }
    this.handlers.get(channel)!.add(handler)

    return () => {
      const set = this.handlers.get(channel)
      if (!set) return
      set.delete(handler)
      if (set.size === 0) {
        this.handlers.delete(channel)
        this.sub.unsubscribe(this.channelPrefix + channel)
      }
    }
  }
}
