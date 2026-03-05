import type Database from 'better-sqlite3'
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
import { rowToTask, rowToEvent, rowToWorker, rowToWorkerAssignment } from './row-mappers.js'

// ─── SqliteShortTermStore ─────────────────────────────────────────────────

export class SqliteShortTermStore implements ShortTermStore {
  constructor(private db: Database.Database) {}

  async saveTask(task: Task): Promise<void> {
    const stmt = this.db.prepare(`
      INSERT INTO taskcast_tasks (id, type, status, params, result, error, metadata, auth_config, webhooks, cleanup, created_at, updated_at, completed_at, ttl, tags, assign_mode, cost, assigned_worker, disconnect_policy)
      VALUES (@id, @type, @status, @params, @result, @error, @metadata, @auth_config, @webhooks, @cleanup, @created_at, @updated_at, @completed_at, @ttl, @tags, @assign_mode, @cost, @assigned_worker, @disconnect_policy)
      ON CONFLICT (id) DO UPDATE SET
        type = excluded.type,
        status = excluded.status,
        params = excluded.params,
        result = excluded.result,
        error = excluded.error,
        metadata = excluded.metadata,
        auth_config = excluded.auth_config,
        webhooks = excluded.webhooks,
        cleanup = excluded.cleanup,
        updated_at = excluded.updated_at,
        completed_at = excluded.completed_at,
        ttl = excluded.ttl,
        tags = excluded.tags,
        assign_mode = excluded.assign_mode,
        cost = excluded.cost,
        assigned_worker = excluded.assigned_worker,
        disconnect_policy = excluded.disconnect_policy
    `)

    stmt.run({
      id: task.id,
      type: task.type ?? null,
      status: task.status,
      params: task.params ? JSON.stringify(task.params) : null,
      result: task.result ? JSON.stringify(task.result) : null,
      error: task.error ? JSON.stringify(task.error) : null,
      metadata: task.metadata ? JSON.stringify(task.metadata) : null,
      auth_config: task.authConfig ? JSON.stringify(task.authConfig) : null,
      webhooks: task.webhooks ? JSON.stringify(task.webhooks) : null,
      cleanup: task.cleanup ? JSON.stringify(task.cleanup) : null,
      created_at: task.createdAt,
      updated_at: task.updatedAt,
      completed_at: task.completedAt ?? null,
      ttl: task.ttl ?? null,
      tags: task.tags ? JSON.stringify(task.tags) : null,
      assign_mode: task.assignMode ?? null,
      cost: task.cost ?? null,
      assigned_worker: task.assignedWorker ?? null,
      disconnect_policy: task.disconnectPolicy ?? null,
    })
  }

  async getTask(taskId: string): Promise<Task | null> {
    const row = this.db.prepare('SELECT * FROM taskcast_tasks WHERE id = ?').get(taskId) as
      | Record<string, unknown>
      | undefined

    return row ? rowToTask(row) : null
  }

  async nextIndex(taskId: string): Promise<number> {
    const row = this.db
      .prepare(
        `INSERT INTO taskcast_index_counters (task_id, counter)
         VALUES (?, 0)
         ON CONFLICT (task_id) DO UPDATE SET counter = counter + 1
         RETURNING counter`,
      )
      .get(taskId) as { counter: number }

    return row.counter
  }

  async appendEvent(taskId: string, event: TaskEvent): Promise<void> {
    this.db
      .prepare(
        `INSERT INTO taskcast_events (id, task_id, idx, timestamp, type, level, data, series_id, series_mode)
         VALUES (@id, @task_id, @idx, @timestamp, @type, @level, @data, @series_id, @series_mode)`,
      )
      .run({
        id: event.id,
        task_id: event.taskId,
        idx: event.index,
        timestamp: event.timestamp,
        type: event.type,
        level: event.level,
        data: event.data != null ? JSON.stringify(event.data) : null,
        series_id: event.seriesId ?? null,
        series_mode: event.seriesMode ?? null,
      })
  }

  async getEvents(taskId: string, opts?: EventQueryOptions): Promise<TaskEvent[]> {
    const since = opts?.since
    const limit = opts?.limit

    let sql: string
    const params: unknown[] = [taskId]

    if (since?.id) {
      // Find the idx of the since event, then return everything after it.
      // If the id is not found, the subquery returns NULL and the WHERE
      // condition `idx > NULL` is never true — we need to fall back to
      // returning all events.
      sql = `
        SELECT * FROM taskcast_events
        WHERE task_id = ?
          AND idx > COALESCE(
            (SELECT idx FROM taskcast_events WHERE task_id = ? AND id = ?),
            -1
          )
        ORDER BY idx ASC
      `
      params.push(taskId, since.id)
    } else if (since?.index !== undefined) {
      sql = `
        SELECT * FROM taskcast_events
        WHERE task_id = ? AND idx > ?
        ORDER BY idx ASC
      `
      params.push(since.index)
    } else if (since?.timestamp !== undefined) {
      sql = `
        SELECT * FROM taskcast_events
        WHERE task_id = ? AND timestamp > ?
        ORDER BY idx ASC
      `
      params.push(since.timestamp)
    } else {
      sql = `
        SELECT * FROM taskcast_events
        WHERE task_id = ?
        ORDER BY idx ASC
      `
    }

    if (limit) {
      sql += ' LIMIT ?'
      params.push(limit)
    }

    const rows = this.db.prepare(sql).all(...params) as Record<string, unknown>[]
    return rows.map(rowToEvent)
  }

  async setTTL(_taskId: string, _ttlSeconds: number): Promise<void> {
    // No-op: SQLite does not support key-level TTL.
  }

  async getSeriesLatest(taskId: string, seriesId: string): Promise<TaskEvent | null> {
    const row = this.db
      .prepare('SELECT event_json FROM taskcast_series_latest WHERE task_id = ? AND series_id = ?')
      .get(taskId, seriesId) as { event_json: string } | undefined

    return row ? (JSON.parse(row.event_json) as TaskEvent) : null
  }

  async setSeriesLatest(taskId: string, seriesId: string, event: TaskEvent): Promise<void> {
    this.db
      .prepare(
        `INSERT INTO taskcast_series_latest (task_id, series_id, event_json)
         VALUES (?, ?, ?)
         ON CONFLICT (task_id, series_id) DO UPDATE SET event_json = excluded.event_json`,
      )
      .run(taskId, seriesId, JSON.stringify(event))
  }

  async replaceLastSeriesEvent(
    taskId: string,
    seriesId: string,
    event: TaskEvent,
  ): Promise<void> {
    const prev = await this.getSeriesLatest(taskId, seriesId)

    if (prev) {
      // Replace content fields only, preserving the original id and idx so
      // the event stays at its original position in idx-based ordering.
      // This mirrors Redis lset behaviour where the list position is preserved.
      this.db
        .prepare(
          `UPDATE taskcast_events
           SET data = @data, type = @type, level = @level,
               series_id = @series_id, series_mode = @series_mode
           WHERE id = @prev_id`,
        )
        .run({
          prev_id: prev.id,
          type: event.type,
          level: event.level,
          data: event.data != null ? JSON.stringify(event.data) : null,
          series_id: event.seriesId ?? null,
          series_mode: event.seriesMode ?? null,
        })
    } else {
      await this.appendEvent(taskId, event)
    }

    await this.setSeriesLatest(taskId, seriesId, event)
  }

  // ─── Task query ──────────────────────────────────────────────────────────

  async listTasks(filter: TaskFilter): Promise<Task[]> {
    let sql = 'SELECT * FROM taskcast_tasks WHERE 1=1'
    const params: unknown[] = []

    if (filter.status?.length) {
      sql += ` AND status IN (${filter.status.map(() => '?').join(', ')})`
      params.push(...filter.status)
    }
    if (filter.types?.length) {
      sql += ` AND type IN (${filter.types.map(() => '?').join(', ')})`
      params.push(...filter.types)
    }
    if (filter.assignMode?.length) {
      sql += ` AND assign_mode IN (${filter.assignMode.map(() => '?').join(', ')})`
      params.push(...filter.assignMode)
    }
    if (filter.excludeTaskIds?.length) {
      sql += ` AND id NOT IN (${filter.excludeTaskIds.map(() => '?').join(', ')})`
      params.push(...filter.excludeTaskIds)
    }
    if (filter.limit !== undefined) {
      sql += ' LIMIT ?'
      params.push(filter.limit)
    }

    const rows = this.db.prepare(sql).all(...params) as Record<string, unknown>[]
    let tasks = rows.map(rowToTask)

    // Tag filtering is done in-memory because tags are stored as JSON text
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

    return tasks
  }

  // ─── Worker state ────────────────────────────────────────────────────────

  async saveWorker(worker: Worker): Promise<void> {
    this.db
      .prepare(
        `INSERT INTO taskcast_workers (id, status, match_rule, capacity, used_slots, weight, connection_mode, connected_at, last_heartbeat_at, metadata)
         VALUES (@id, @status, @match_rule, @capacity, @used_slots, @weight, @connection_mode, @connected_at, @last_heartbeat_at, @metadata)
         ON CONFLICT (id) DO UPDATE SET
           status = excluded.status,
           match_rule = excluded.match_rule,
           capacity = excluded.capacity,
           used_slots = excluded.used_slots,
           weight = excluded.weight,
           connection_mode = excluded.connection_mode,
           connected_at = excluded.connected_at,
           last_heartbeat_at = excluded.last_heartbeat_at,
           metadata = excluded.metadata`,
      )
      .run({
        id: worker.id,
        status: worker.status,
        match_rule: JSON.stringify(worker.matchRule),
        capacity: worker.capacity,
        used_slots: worker.usedSlots,
        weight: worker.weight,
        connection_mode: worker.connectionMode,
        connected_at: worker.connectedAt,
        last_heartbeat_at: worker.lastHeartbeatAt,
        metadata: worker.metadata ? JSON.stringify(worker.metadata) : null,
      })
  }

  async getWorker(workerId: string): Promise<Worker | null> {
    const row = this.db
      .prepare('SELECT * FROM taskcast_workers WHERE id = ?')
      .get(workerId) as Record<string, unknown> | undefined

    return row ? rowToWorker(row) : null
  }

  async listWorkers(filter?: WorkerFilter): Promise<Worker[]> {
    let sql = 'SELECT * FROM taskcast_workers WHERE 1=1'
    const params: unknown[] = []

    if (filter?.status?.length) {
      sql += ` AND status IN (${filter.status.map(() => '?').join(', ')})`
      params.push(...filter.status)
    }
    if (filter?.connectionMode?.length) {
      sql += ` AND connection_mode IN (${filter.connectionMode.map(() => '?').join(', ')})`
      params.push(...filter.connectionMode)
    }

    const rows = this.db.prepare(sql).all(...params) as Record<string, unknown>[]
    return rows.map(rowToWorker)
  }

  async deleteWorker(workerId: string): Promise<void> {
    this.db.prepare('DELETE FROM taskcast_workers WHERE id = ?').run(workerId)
  }

  // ─── Atomic claim ────────────────────────────────────────────────────────

  async claimTask(taskId: string, workerId: string, cost: number): Promise<boolean> {
    // SQLite is single-writer, so a transaction provides atomicity.
    const claim = this.db.transaction(() => {
      const workerRow = this.db
        .prepare('SELECT * FROM taskcast_workers WHERE id = ?')
        .get(workerId) as Record<string, unknown> | undefined
      if (!workerRow) return false

      const worker = rowToWorker(workerRow)
      if (worker.usedSlots + cost > worker.capacity) return false

      const taskRow = this.db
        .prepare('SELECT * FROM taskcast_tasks WHERE id = ?')
        .get(taskId) as Record<string, unknown> | undefined
      if (!taskRow) return false

      const task = rowToTask(taskRow)
      if (task.status !== 'pending' && task.status !== 'assigned') return false

      // Update task
      this.db
        .prepare(
          `UPDATE taskcast_tasks
           SET status = 'assigned', assigned_worker = ?, cost = ?, updated_at = ?
           WHERE id = ?`,
        )
        .run(workerId, cost, Date.now(), taskId)

      // Update worker used slots
      this.db
        .prepare('UPDATE taskcast_workers SET used_slots = ? WHERE id = ?')
        .run(worker.usedSlots + cost, workerId)

      return true
    })

    return claim()
  }

  // ─── Worker assignments ──────────────────────────────────────────────────

  async addAssignment(assignment: WorkerAssignment): Promise<void> {
    this.db
      .prepare(
        `INSERT INTO taskcast_worker_assignments (task_id, worker_id, cost, assigned_at, status)
         VALUES (@task_id, @worker_id, @cost, @assigned_at, @status)
         ON CONFLICT (task_id) DO UPDATE SET
           worker_id = excluded.worker_id,
           cost = excluded.cost,
           assigned_at = excluded.assigned_at,
           status = excluded.status`,
      )
      .run({
        task_id: assignment.taskId,
        worker_id: assignment.workerId,
        cost: assignment.cost,
        assigned_at: assignment.assignedAt,
        status: assignment.status,
      })
  }

  async removeAssignment(taskId: string): Promise<void> {
    this.db.prepare('DELETE FROM taskcast_worker_assignments WHERE task_id = ?').run(taskId)
  }

  async getWorkerAssignments(workerId: string): Promise<WorkerAssignment[]> {
    const rows = this.db
      .prepare('SELECT * FROM taskcast_worker_assignments WHERE worker_id = ?')
      .all(workerId) as Record<string, unknown>[]

    return rows.map(rowToWorkerAssignment)
  }

  async getTaskAssignment(taskId: string): Promise<WorkerAssignment | null> {
    const row = this.db
      .prepare('SELECT * FROM taskcast_worker_assignments WHERE task_id = ?')
      .get(taskId) as Record<string, unknown> | undefined

    return row ? rowToWorkerAssignment(row) : null
  }
}
