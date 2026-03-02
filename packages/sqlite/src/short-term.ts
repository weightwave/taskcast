import type Database from 'better-sqlite3'
import type { Task, TaskEvent, ShortTermStore, EventQueryOptions } from '@taskcast/core'
import { rowToTask, rowToEvent } from './row-mappers.js'

// ─── SqliteShortTermStore ─────────────────────────────────────────────────

export class SqliteShortTermStore implements ShortTermStore {
  constructor(private db: Database.Database) {}

  async saveTask(task: Task): Promise<void> {
    const stmt = this.db.prepare(`
      INSERT INTO taskcast_tasks (id, type, status, params, result, error, metadata, auth_config, webhooks, cleanup, created_at, updated_at, completed_at, ttl)
      VALUES (@id, @type, @status, @params, @result, @error, @metadata, @auth_config, @webhooks, @cleanup, @created_at, @updated_at, @completed_at, @ttl)
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
        ttl = excluded.ttl
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
}
