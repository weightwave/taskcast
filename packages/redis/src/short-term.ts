import type { Redis } from 'ioredis'
import type {
  Task,
  TaskEvent,
  TaskStatus,
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

  async accumulateSeries(taskId: string, seriesId: string, event: TaskEvent, field: string): Promise<TaskEvent> {
    // TODO(Task 8): Replace with Lua script for atomic JSON-aware accumulation
    const prev = await this.getSeriesLatest(taskId, seriesId)
    let accumulated = event
    if (prev !== null) {
      const prevData = (typeof prev.data === 'object' && prev.data !== null)
        ? prev.data as Record<string, unknown> : {}
      const newData = (typeof event.data === 'object' && event.data !== null)
        ? event.data as Record<string, unknown> : {}
      if (typeof prevData[field] === 'string' && typeof newData[field] === 'string') {
        accumulated = {
          ...event,
          data: { ...newData, [field]: prevData[field] + newData[field] },
        }
      }
    }
    await this.setSeriesLatest(taskId, seriesId, accumulated)
    return accumulated
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
    const staleIds: string[] = []
    if (results) {
      for (let i = 0; i < results.length; i++) {
        const entry = results[i]!
        const [err, raw] = entry
        if (!err && typeof raw === 'string') {
          tasks.push(JSON.parse(raw) as Task)
        } else if (!err) {
          staleIds.push(taskIds[i]!)
        }
      }
    }
    if (staleIds.length > 0) {
      await this.redis.srem(this.KEY.taskSet, ...staleIds)
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

  // Atomic claim — uses a Lua script so the read-check-write is a single Redis command.
  // This prevents two workers racing to claim the same task.
  private static CLAIM_LUA = `
    local taskJson = redis.call('GET', KEYS[1])
    if not taskJson then return 0 end

    local task = cjson.decode(taskJson)
    if task.status ~= 'pending' and task.status ~= 'assigned' then return 0 end

    local workerJson = redis.call('GET', KEYS[2])
    if not workerJson then return 0 end

    local worker = cjson.decode(workerJson)
    local cost = tonumber(ARGV[1])
    if worker.usedSlots + cost > worker.capacity then return 0 end

    worker.usedSlots = worker.usedSlots + cost
    redis.call('SET', KEYS[2], cjson.encode(worker))

    task.status = 'assigned'
    task.assignedWorker = ARGV[2]
    task.cost = cost
    task.updatedAt = tonumber(ARGV[3])
    redis.call('SET', KEYS[1], cjson.encode(task))

    return 1
  `

  async claimTask(taskId: string, workerId: string, cost: number): Promise<boolean> {
    const result = await this.redis.eval(
      RedisShortTermStore.CLAIM_LUA,
      2,
      this.KEY.task(taskId),
      this.KEY.worker(workerId),
      String(cost),
      workerId,
      String(Date.now()),
    )
    return result === 1
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

  // TTL management — remove expiry from task-related keys
  async clearTTL(taskId: string): Promise<void> {
    await this.redis.persist(this.KEY.task(taskId))
    await this.redis.persist(this.KEY.events(taskId))
    await this.redis.persist(this.KEY.idx(taskId))

    const seriesIds = await this.redis.smembers(this.KEY.seriesIds(taskId))
    const pipeline = this.redis.pipeline()
    for (const sid of seriesIds) {
      pipeline.persist(this.KEY.seriesLatest(taskId, sid))
    }
    pipeline.persist(this.KEY.seriesIds(taskId))
    await pipeline.exec()
  }

  // Task query by status
  async listByStatus(statuses: TaskStatus[]): Promise<Task[]> {
    return this.listTasks({ status: statuses })
  }
}
