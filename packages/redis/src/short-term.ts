import type { Redis } from 'ioredis'
import type { Task, TaskEvent, ShortTermStore, EventQueryOptions } from '@taskcast/core'

const KEY = {
  task: (id: string) => `taskcast:task:${id}`,
  events: (id: string) => `taskcast:events:${id}`,
  seriesLatest: (taskId: string, seriesId: string) =>
    `taskcast:series:${taskId}:${seriesId}`,
  seriesIds: (taskId: string) => `taskcast:seriesIds:${taskId}`,
}

export class RedisShortTermStore implements ShortTermStore {
  constructor(private redis: Redis) {}

  async saveTask(task: Task): Promise<void> {
    await this.redis.set(KEY.task(task.id), JSON.stringify(task))
  }

  async getTask(taskId: string): Promise<Task | null> {
    const raw = await this.redis.get(KEY.task(taskId))
    return raw ? (JSON.parse(raw) as Task) : null
  }

  async appendEvent(taskId: string, event: TaskEvent): Promise<void> {
    await this.redis.rpush(KEY.events(taskId), JSON.stringify(event))
  }

  async getEvents(taskId: string, opts?: EventQueryOptions): Promise<TaskEvent[]> {
    const raw = await this.redis.lrange(KEY.events(taskId), 0, -1)
    let events = raw.map((r) => JSON.parse(r) as TaskEvent)

    const since = opts?.since
    if (since?.id) {
      const idx = events.findIndex((e) => e.id === since.id)
      events = idx >= 0 ? events.slice(idx + 1) : events
    } else if (since?.index !== undefined) {
      events = events.filter((e) => e.index > since.index!)
    } else if (since?.timestamp !== undefined) {
      events = events.filter((e) => e.timestamp > since.timestamp!)
    }

    if (opts?.limit) events = events.slice(0, opts.limit)
    return events
  }

  async setTTL(taskId: string, ttlSeconds: number): Promise<void> {
    await this.redis.expire(KEY.task(taskId), ttlSeconds)
    await this.redis.expire(KEY.events(taskId), ttlSeconds)

    const seriesIds = await this.redis.smembers(KEY.seriesIds(taskId))
    const pipeline = this.redis.pipeline()
    for (const sid of seriesIds) {
      pipeline.expire(KEY.seriesLatest(taskId, sid), ttlSeconds)
    }
    pipeline.expire(KEY.seriesIds(taskId), ttlSeconds)
    await pipeline.exec()
  }

  async getSeriesLatest(taskId: string, seriesId: string): Promise<TaskEvent | null> {
    const raw = await this.redis.get(KEY.seriesLatest(taskId, seriesId))
    return raw ? (JSON.parse(raw) as TaskEvent) : null
  }

  async setSeriesLatest(taskId: string, seriesId: string, event: TaskEvent): Promise<void> {
    await this.redis.set(KEY.seriesLatest(taskId, seriesId), JSON.stringify(event))
    await this.redis.sadd(KEY.seriesIds(taskId), seriesId)
  }

  async replaceLastSeriesEvent(taskId: string, seriesId: string, event: TaskEvent): Promise<void> {
    const prev = await this.getSeriesLatest(taskId, seriesId)
    if (prev) {
      // Find and replace the previous event in the list
      const raw = await this.redis.lrange(KEY.events(taskId), 0, -1)
      let idx = -1
      for (let i = raw.length - 1; i >= 0; i--) {
        try {
          if ((JSON.parse(raw[i]!) as TaskEvent).id === prev.id) {
            idx = i
            break
          }
        } catch {
          // ignore parse errors
        }
      }
      if (idx >= 0) {
        await this.redis.lset(KEY.events(taskId), idx, JSON.stringify(event))
      }
    } else {
      await this.appendEvent(taskId, event)
    }
    await this.setSeriesLatest(taskId, seriesId, event)
  }
}
