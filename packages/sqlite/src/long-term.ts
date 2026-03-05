import type Database from 'better-sqlite3'
import type { Task, TaskEvent, LongTermStore, EventQueryOptions, WorkerAuditEvent } from '@taskcast/core'
import { rowToTask, rowToEvent, rowToWorkerEvent } from './row-mappers.js'

// ─── SqliteLongTermStore ──────────────────────────────────────────────────

export class SqliteLongTermStore implements LongTermStore {
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

  async saveEvent(event: TaskEvent): Promise<void> {
    this.db
      .prepare(
        `INSERT INTO taskcast_events (id, task_id, idx, timestamp, type, level, data, series_id, series_mode)
         VALUES (@id, @task_id, @idx, @timestamp, @type, @level, @data, @series_id, @series_mode)
         ON CONFLICT (id) DO NOTHING`,
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

  // ─── Worker audit events ─────────────────────────────────────────────────

  async saveWorkerEvent(event: WorkerAuditEvent): Promise<void> {
    this.db
      .prepare(
        `INSERT INTO taskcast_worker_events (id, worker_id, timestamp, action, data)
         VALUES (@id, @worker_id, @timestamp, @action, @data)
         ON CONFLICT (id) DO NOTHING`,
      )
      .run({
        id: event.id,
        worker_id: event.workerId,
        timestamp: event.timestamp,
        action: event.action,
        data: event.data ? JSON.stringify(event.data) : null,
      })
  }

  async getWorkerEvents(workerId: string, opts?: EventQueryOptions): Promise<WorkerAuditEvent[]> {
    const since = opts?.since
    const limit = opts?.limit

    let sql: string
    const params: unknown[] = [workerId]

    if (since?.timestamp !== undefined) {
      sql = `
        SELECT * FROM taskcast_worker_events
        WHERE worker_id = ? AND timestamp > ?
        ORDER BY timestamp ASC
      `
      params.push(since.timestamp)
    } else if (since?.id) {
      sql = `
        SELECT * FROM taskcast_worker_events
        WHERE worker_id = ?
          AND timestamp > COALESCE(
            (SELECT timestamp FROM taskcast_worker_events WHERE id = ?),
            0
          )
        ORDER BY timestamp ASC
      `
      params.push(since.id)
    } else {
      sql = `
        SELECT * FROM taskcast_worker_events
        WHERE worker_id = ?
        ORDER BY timestamp ASC
      `
    }

    if (limit) {
      sql += ' LIMIT ?'
      params.push(limit)
    }

    const rows = this.db.prepare(sql).all(...params) as Record<string, unknown>[]
    return rows.map(rowToWorkerEvent)
  }
}
