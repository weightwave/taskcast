import type {
  Task,
  TaskEvent,
  TaskStatus,
  BroadcastProvider,
  ShortTermStore,
  EventQueryOptions,
  TaskFilter,
  Worker,
  WorkerFilter,
  WorkerAssignment,
} from './types.js'

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
  private workers = new Map<string, Worker>()
  private assignments = new Map<string, WorkerAssignment>()

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

  // Task query
  async listTasks(filter: TaskFilter): Promise<Task[]> {
    let tasks = Array.from(this.tasks.values())

    if (filter.status?.length) {
      tasks = tasks.filter((t) => filter.status!.includes(t.status))
    }
    if (filter.types?.length) {
      tasks = tasks.filter((t) => t.type !== undefined && filter.types!.includes(t.type))
    }
    if (filter.tags) {
      const { all, any, none } = filter.tags
      tasks = tasks.filter((t) => {
        const taskTags = t.tags ?? []
        if (all && !all.every((tag) => taskTags.includes(tag))) return false
        if (any && !any.some((tag) => taskTags.includes(tag))) return false
        if (none && none.some((tag) => taskTags.includes(tag))) return false
        return true
      })
    }
    if (filter.assignMode?.length) {
      tasks = tasks.filter((t) => t.assignMode !== undefined && filter.assignMode!.includes(t.assignMode))
    }
    if (filter.excludeTaskIds?.length) {
      const excluded = new Set(filter.excludeTaskIds)
      tasks = tasks.filter((t) => !excluded.has(t.id))
    }
    if (filter.limit !== undefined) {
      tasks = tasks.slice(0, filter.limit)
    }

    return tasks
  }

  // Worker state
  async saveWorker(worker: Worker): Promise<void> {
    this.workers.set(worker.id, { ...worker })
  }

  async getWorker(workerId: string): Promise<Worker | null> {
    return this.workers.get(workerId) ?? null
  }

  async listWorkers(filter?: WorkerFilter): Promise<Worker[]> {
    let workers = Array.from(this.workers.values())

    if (filter?.status?.length) {
      workers = workers.filter((w) => filter.status!.includes(w.status))
    }
    if (filter?.connectionMode?.length) {
      workers = workers.filter((w) => filter.connectionMode!.includes(w.connectionMode))
    }

    return workers
  }

  async deleteWorker(workerId: string): Promise<void> {
    this.workers.delete(workerId)
  }

  // Atomic claim — single-threaded JS makes this safe without locking.
  // The Redis adapter uses a Lua script for the same guarantee across processes.
  async claimTask(taskId: string, workerId: string, cost: number): Promise<boolean> {
    const worker = this.workers.get(workerId)
    if (!worker || worker.usedSlots + cost > worker.capacity) return false

    const task = this.tasks.get(taskId)
    if (!task || (task.status !== 'pending' && task.status !== 'assigned')) return false

    task.status = 'assigned'
    task.assignedWorker = workerId
    task.cost = cost
    task.updatedAt = Date.now()

    worker.usedSlots += cost
    return true
  }

  // Worker assignments
  async addAssignment(assignment: WorkerAssignment): Promise<void> {
    this.assignments.set(assignment.taskId, { ...assignment })
  }

  async removeAssignment(taskId: string): Promise<void> {
    this.assignments.delete(taskId)
  }

  async getWorkerAssignments(workerId: string): Promise<WorkerAssignment[]> {
    return Array.from(this.assignments.values()).filter((a) => a.workerId === workerId)
  }

  async getTaskAssignment(taskId: string): Promise<WorkerAssignment | null> {
    return this.assignments.get(taskId) ?? null
  }

  // TTL management — no-op in memory adapter (setTTL is also a no-op)
  async clearTTL(_taskId: string): Promise<void> {
    // no-op in memory adapter
  }

  // Task query by status
  async listByStatus(statuses: TaskStatus[]): Promise<Task[]> {
    return Array.from(this.tasks.values()).filter((t) => statuses.includes(t.status))
  }
}
