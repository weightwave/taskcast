import type { Task, TaskEvent, BroadcastProvider, ShortTermStore, EventQueryOptions } from './types.js'

export class MemoryBroadcastProvider implements BroadcastProvider {
  private listeners = new Map<string, Set<(event: TaskEvent) => void>>()

  async publish(channel: string, event: TaskEvent): Promise<void> {
    const handlers = this.listeners.get(channel)
    if (!handlers) return
    for (const handler of handlers) {
      handler(event)
    }
  }

  subscribe(channel: string, handler: (event: TaskEvent) => void): () => void {
    if (!this.listeners.has(channel)) {
      this.listeners.set(channel, new Set())
    }
    this.listeners.get(channel)!.add(handler)
    return () => {
      this.listeners.get(channel)?.delete(handler)
    }
  }
}

export class MemoryShortTermStore implements ShortTermStore {
  private tasks = new Map<string, Task>()
  private events = new Map<string, TaskEvent[]>()
  private seriesLatest = new Map<string, TaskEvent>()
  private indexCounters = new Map<string, number>()

  async saveTask(task: Task): Promise<void> {
    this.tasks.set(task.id, { ...task })
  }

  async getTask(taskId: string): Promise<Task | null> {
    return this.tasks.get(taskId) ?? null
  }

  async nextIndex(taskId: string): Promise<number> {
    const current = this.indexCounters.get(taskId) ?? -1
    const next = current + 1
    this.indexCounters.set(taskId, next)
    return next
  }

  async appendEvent(taskId: string, event: TaskEvent): Promise<void> {
    if (!this.events.has(taskId)) this.events.set(taskId, [])
    this.events.get(taskId)!.push({ ...event })
  }

  async getEvents(taskId: string, opts?: EventQueryOptions): Promise<TaskEvent[]> {
    const all = this.events.get(taskId) ?? []
    let result = all

    if (opts?.since?.id) {
      const idx = result.findIndex((e) => e.id === opts.since!.id)
      result = idx >= 0 ? result.slice(idx + 1) : result
    } else if (opts?.since?.index !== undefined) {
      result = result.filter((e) => e.index > opts.since!.index!)
    } else if (opts?.since?.timestamp !== undefined) {
      result = result.filter((e) => e.timestamp > opts.since!.timestamp!)
    }

    if (opts?.limit) result = result.slice(0, opts.limit)
    return result
  }

  async setTTL(_taskId: string, _ttlSeconds: number): Promise<void> {
    // no-op in memory adapter
  }

  async getSeriesLatest(taskId: string, seriesId: string): Promise<TaskEvent | null> {
    return this.seriesLatest.get(`${taskId}:${seriesId}`) ?? null
  }

  async setSeriesLatest(taskId: string, seriesId: string, event: TaskEvent): Promise<void> {
    this.seriesLatest.set(`${taskId}:${seriesId}`, { ...event })
  }

  async replaceLastSeriesEvent(taskId: string, seriesId: string, event: TaskEvent): Promise<void> {
    const key = `${taskId}:${seriesId}`
    const prev = this.seriesLatest.get(key)
    if (prev) {
      const taskEvents = this.events.get(taskId)
      if (taskEvents) {
        // Find the last index manually (findLastIndex requires ES2023+)
        let idx = -1
        for (let i = taskEvents.length - 1; i >= 0; i--) {
          if (taskEvents[i]?.id === prev.id) {
            idx = i
            break
          }
        }
        if (idx >= 0) taskEvents[idx] = { ...event }
      }
    } else {
      await this.appendEvent(taskId, event)
    }
    this.seriesLatest.set(key, { ...event })
  }
}
