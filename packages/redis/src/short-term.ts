import type { Redis } from 'ioredis'
import type {
  Task,
  TaskEvent,
  ShortTermStore,
  EventQueryOptions,
  TaskFilter,
  Worker,
  WorkerFilter,
  WorkerAssignment,
} from '@taskcast/core'

function makeKeys(prefix: string) {
  return {
    task: (id: string) => `${prefix}:task:${id}`,
    taskSet: `${prefix}:tasks`,
    events: (id: string) => `${prefix}:events:${id}`,
    idx: (id: string) => `${prefix}:idx:${id}`,
    seriesLatest: (taskId: string, seriesId: string) => `${prefix}:series:${taskId}:${seriesId}`,
    seriesIds: (taskId: string) => `${prefix}:seriesIds:${taskId}`,
    worker: (id: string) => `${prefix}:worker:${id}`,
    workerSet: `${prefix}:workers`,
    assignment: (taskId: string) => `${prefix}:assignment:${taskId}`,
    workerAssignments: (workerId: string) => `${prefix}:workerAssignments:${workerId}`,
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
    await this.redis.sadd(this.KEY.taskSet, task.id)
  }

  async getTask(taskId: string): Promise<Task | null> {
    const raw = await this.redis.get(this.KEY.task(taskId))
    return raw ? (JSON.parse(raw) as Task) : null
  }

  async nextIndex(taskId: string): Promise<number> {
    // INCR is atomic — safe across multiple instances sharing the same Redis
    return (await this.redis.incr(this.KEY.idx(taskId))) - 1
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
    await this.redis.expire(this.KEY.idx(taskId), ttlSeconds)

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

  // Task query
  async listTasks(filter: TaskFilter): Promise<Task[]> {
    const taskIds = await this.redis.smembers(this.KEY.taskSet)
    if (taskIds.length === 0) return []

    const pipeline = this.redis.pipeline()
    for (const id of taskIds) {
      pipeline.get(this.KEY.task(id))
    }
    const results = await pipeline.exec()

    let tasks: Task[] = []
    if (results) {
      for (const [err, raw] of results) {
        if (!err && typeof raw === 'string') {
          tasks.push(JSON.parse(raw) as Task)
        }
      }
    }

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
    await this.redis.set(this.KEY.worker(worker.id), JSON.stringify(worker))
    await this.redis.sadd(this.KEY.workerSet, worker.id)
  }

  async getWorker(workerId: string): Promise<Worker | null> {
    const raw = await this.redis.get(this.KEY.worker(workerId))
    return raw ? (JSON.parse(raw) as Worker) : null
  }

  async listWorkers(filter?: WorkerFilter): Promise<Worker[]> {
    const workerIds = await this.redis.smembers(this.KEY.workerSet)
    if (workerIds.length === 0) return []

    const pipeline = this.redis.pipeline()
    for (const id of workerIds) {
      pipeline.get(this.KEY.worker(id))
    }
    const results = await pipeline.exec()

    let workers: Worker[] = []
    if (results) {
      for (const [err, raw] of results) {
        if (!err && typeof raw === 'string') {
          workers.push(JSON.parse(raw) as Worker)
        }
      }
    }

    if (filter?.status?.length) {
      workers = workers.filter((w) => filter.status!.includes(w.status))
    }
    if (filter?.connectionMode?.length) {
      workers = workers.filter((w) => filter.connectionMode!.includes(w.connectionMode))
    }

    return workers
  }

  async deleteWorker(workerId: string): Promise<void> {
    await this.redis.del(this.KEY.worker(workerId))
    await this.redis.srem(this.KEY.workerSet, workerId)
  }

  // Atomic claim
  async claimTask(taskId: string, workerId: string, cost: number): Promise<boolean> {
    const task = await this.getTask(taskId)
    if (!task || (task.status !== 'pending' && task.status !== 'assigned')) return false

    task.status = 'assigned'
    task.assignedWorker = workerId
    task.cost = cost
    task.updatedAt = Date.now()
    await this.saveTask(task)
    return true
  }

  // Worker assignments
  async addAssignment(assignment: WorkerAssignment): Promise<void> {
    await this.redis.set(this.KEY.assignment(assignment.taskId), JSON.stringify(assignment))
    await this.redis.sadd(this.KEY.workerAssignments(assignment.workerId), assignment.taskId)
  }

  async removeAssignment(taskId: string): Promise<void> {
    const raw = await this.redis.get(this.KEY.assignment(taskId))
    if (raw) {
      const assignment = JSON.parse(raw) as WorkerAssignment
      await this.redis.srem(this.KEY.workerAssignments(assignment.workerId), taskId)
    }
    await this.redis.del(this.KEY.assignment(taskId))
  }

  async getWorkerAssignments(workerId: string): Promise<WorkerAssignment[]> {
    const taskIds = await this.redis.smembers(this.KEY.workerAssignments(workerId))
    if (taskIds.length === 0) return []

    const pipeline = this.redis.pipeline()
    for (const id of taskIds) {
      pipeline.get(this.KEY.assignment(id))
    }
    const results = await pipeline.exec()

    const assignments: WorkerAssignment[] = []
    if (results) {
      for (const [err, raw] of results) {
        if (!err && typeof raw === 'string') {
          assignments.push(JSON.parse(raw) as WorkerAssignment)
        }
      }
    }
    return assignments
  }

  async getTaskAssignment(taskId: string): Promise<WorkerAssignment | null> {
    const raw = await this.redis.get(this.KEY.assignment(taskId))
    return raw ? (JSON.parse(raw) as WorkerAssignment) : null
  }
}
