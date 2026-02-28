import type { Redis } from 'ioredis'
import type { Task, TaskEvent, ShortTermStore, EventQueryOptions } from '@taskcast/core'

function makeKeys(prefix: string) {
  return {
    task: (id: string) => `${prefix}:task:${id}`,
    events: (id: string) => `${prefix}:events:${id}`,
    seriesLatest: (taskId: string, seriesId: string) => `${prefix}:series:${taskId}:${seriesId}`,
    seriesIds: (taskId: string) => `${prefix}:seriesIds:${taskId}`,
  }
}

export class RedisShortTermStore implements ShortTermStore {
  private KEY: ReturnType<typeof makeKeys>

  constructor(
    private redis: Redis,
    { prefix }: { prefix?: string } = {},
  ) {
    const resolvedPrefix = prefix ?? process.env['TASKCAST_REDIS_PREFIX'] ?? 'taskcast'
    this.KEY = makeKeys(resolvedPrefix)
  }

  async saveTask(task: Task): Promise<void> {
    await this.redis.set(this.KEY.task(task.id), JSON.stringify(task))
  }

  async getTask(taskId: string): Promise<Task | null> {
    const raw = await this.redis.get(this.KEY.task(taskId))
    return raw ? (JSON.parse(raw) as Task) : null
  }

  async appendEvent(taskId: string, event: TaskEvent): Promise<void> {
    await this.redis.rpush(this.KEY.events(taskId), JSON.stringify(event))
  }

  async getEvents(taskId: string, opts?: EventQueryOptions): Promise<TaskEvent[]> {
    const raw = await this.redis.lrange(this.KEY.events(taskId), 0, -1)
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
    await this.redis.expire(this.KEY.task(taskId), ttlSeconds)
    await this.redis.expire(this.KEY.events(taskId), ttlSeconds)

    const seriesIds = await this.redis.smembers(this.KEY.seriesIds(taskId))
    const pipeline = this.redis.pipeline()
    for (const sid of seriesIds) {
      pipeline.expire(this.KEY.seriesLatest(taskId, sid), ttlSeconds)
    }
    pipeline.expire(this.KEY.seriesIds(taskId), ttlSeconds)
    await pipeline.exec()
  }

  async getSeriesLatest(taskId: string, seriesId: string): Promise<TaskEvent | null> {
    const raw = await this.redis.get(this.KEY.seriesLatest(taskId, seriesId))
    return raw ? (JSON.parse(raw) as TaskEvent) : null
  }

  async setSeriesLatest(taskId: string, seriesId: string, event: TaskEvent): Promise<void> {
    await this.redis.set(this.KEY.seriesLatest(taskId, seriesId), JSON.stringify(event))
    await this.redis.sadd(this.KEY.seriesIds(taskId), seriesId)
  }

  async replaceLastSeriesEvent(taskId: string, seriesId: string, event: TaskEvent): Promise<void> {
    const prev = await this.getSeriesLatest(taskId, seriesId)
    if (prev) {
      // Find and replace the previous event in the list
      const raw = await this.redis.lrange(this.KEY.events(taskId), 0, -1)
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
        await this.redis.lset(this.KEY.events(taskId), idx, JSON.stringify(event))
      }
    } else {
      await this.appendEvent(taskId, event)
    }
    await this.setSeriesLatest(taskId, seriesId, event)
  }
}
