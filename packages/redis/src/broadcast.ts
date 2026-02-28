import type { Redis } from 'ioredis'
import type { BroadcastProvider, TaskEvent } from '@taskcast/core'

const CHANNEL_PREFIX = 'taskcast:task:'

export class RedisBroadcastProvider implements BroadcastProvider {
  // 每个 channel 的本地 handlers，在接收到 Redis 消息后转发
  private handlers = new Map<string, Set<(event: TaskEvent) => void>>()

  constructor(
    private pub: Redis,
    private sub: Redis,
  ) {
    this.sub.on('message', (channel: string, message: string) => {
      const taskId = channel.replace(CHANNEL_PREFIX, '')
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
    await this.pub.publish(CHANNEL_PREFIX + channel, JSON.stringify(event))
  }

  subscribe(channel: string, handler: (event: TaskEvent) => void): () => void {
    if (!this.handlers.has(channel)) {
      this.handlers.set(channel, new Set())
      this.sub.subscribe(CHANNEL_PREFIX + channel)
    }
    this.handlers.get(channel)!.add(handler)

    return () => {
      const set = this.handlers.get(channel)
      if (!set) return
      set.delete(handler)
      if (set.size === 0) {
        this.handlers.delete(channel)
        this.sub.unsubscribe(CHANNEL_PREFIX + channel)
      }
    }
  }
}
