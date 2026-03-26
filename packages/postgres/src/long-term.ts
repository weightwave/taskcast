import type postgres from 'postgres'
import type {
  Task,
  TaskEvent,
  LongTermStore,
  EventQueryOptions,
  TaskError,
  TaskAuthConfig,
  WebhookConfig,
  CleanupRule,
  SeriesMode,
  WorkerAuditEvent,
  AssignMode,
  DisconnectPolicy,
} from '@taskcast/core'

const TASKS = 'taskcast_tasks'
const EVENTS = 'taskcast_events'
const WORKER_EVENTS = 'taskcast_worker_events'

export class PostgresLongTermStore implements LongTermStore {
  constructor(private sql: ReturnType<typeof postgres>) {}

  async saveTask(task: Task): Promise<void> {
    const t = TASKS
    await this.sql`
      INSERT INTO ${this.sql(t)} (
        id, type, status, params, result, error, metadata,
        auth_config, webhooks, cleanup, created_at, updated_at, completed_at, ttl,
        tags, assign_mode, cost, assigned_worker, disconnect_policy
      ) VALUES (
        ${task.id}, ${task.type ?? null}, ${task.status},
        ${task.params ? this.sql.json(task.params as never) : null},
        ${task.result ? this.sql.json(task.result as never) : null},
        ${task.error ? this.sql.json(task.error as never) : null},
        ${task.metadata ? this.sql.json(task.metadata as never) : null},
        ${task.authConfig ? this.sql.json(task.authConfig as never) : null},
        ${task.webhooks ? this.sql.json(task.webhooks as never) : null},
        ${task.cleanup ? this.sql.json(task.cleanup as never) : null},
        ${task.createdAt}, ${task.updatedAt},
        ${task.completedAt ?? null}, ${task.ttl ?? null},
        ${task.tags ? this.sql.json(task.tags as never) : null},
        ${task.assignMode ?? null},
        ${task.cost ?? null},
        ${task.assignedWorker ?? null},
        ${task.disconnectPolicy ?? null}
      )
      ON CONFLICT (id) DO UPDATE SET
        status = EXCLUDED.status,
        result = EXCLUDED.result,
        error = EXCLUDED.error,
        metadata = EXCLUDED.metadata,
        updated_at = EXCLUDED.updated_at,
        completed_at = EXCLUDED.completed_at,
        tags = EXCLUDED.tags,
        assign_mode = EXCLUDED.assign_mode,
        cost = EXCLUDED.cost,
        assigned_worker = EXCLUDED.assigned_worker,
        disconnect_policy = EXCLUDED.disconnect_policy
    `
  }

  async getTask(taskId: string): Promise<Task | null> {
    const t = TASKS
    const rows = await this.sql`
      SELECT * FROM ${this.sql(t)} WHERE id = ${taskId}
    `
    const row = rows[0]
    if (!row) return null
    return this._rowToTask(row)
  }

  async saveEvent(event: TaskEvent): Promise<void> {
    const t = EVENTS
    await this.sql`
      INSERT INTO ${this.sql(t)} (
        id, task_id, idx, timestamp, type, level, data, series_id, series_mode, series_acc_field, client_id, client_seq
      ) VALUES (
        ${event.id}, ${event.taskId}, ${event.index}, ${event.timestamp},
        ${event.type}, ${event.level},
        ${event.data ? this.sql.json(event.data as never) : null},
        ${event.seriesId ?? null}, ${event.seriesMode ?? null},
        ${event.seriesAccField ?? null},
        ${event.clientId ?? null}, ${event.clientSeq ?? null}
      )
      ON CONFLICT (id) DO NOTHING
    `
  }

  async getEvents(taskId: string, opts?: EventQueryOptions): Promise<TaskEvent[]> {
    const t = EVENTS
    const since = opts?.since

    let rows: postgres.RowList<postgres.Row[]>
    if (since?.index !== undefined) {
      rows = await this.sql`
        SELECT * FROM ${this.sql(t)}
        WHERE task_id = ${taskId} AND idx > ${since.index}
        ORDER BY idx ASC
        ${opts?.limit ? this.sql`LIMIT ${opts.limit}` : this.sql``}
      `
    } else if (since?.timestamp !== undefined) {
      rows = await this.sql`
        SELECT * FROM ${this.sql(t)}
        WHERE task_id = ${taskId} AND timestamp > ${since.timestamp}
        ORDER BY idx ASC
        ${opts?.limit ? this.sql`LIMIT ${opts.limit}` : this.sql``}
      `
    } else if (since?.id) {
      const anchor = await this.sql`
        SELECT idx FROM ${this.sql(t)} WHERE id = ${since.id}
      `
      const anchorIdx = (anchor[0]?.['idx'] as number | undefined) ?? -1
      rows = await this.sql`
        SELECT * FROM ${this.sql(t)}
        WHERE task_id = ${taskId} AND idx > ${anchorIdx}
        ORDER BY idx ASC
        ${opts?.limit ? this.sql`LIMIT ${opts.limit}` : this.sql``}
      `
    } else {
      rows = await this.sql`
        SELECT * FROM ${this.sql(t)}
        WHERE task_id = ${taskId}
        ORDER BY idx ASC
        ${opts?.limit ? this.sql`LIMIT ${opts.limit}` : this.sql``}
      `
    }

    return rows.map((r) => this._rowToEvent(r))
  }

  async saveWorkerEvent(event: WorkerAuditEvent): Promise<void> {
    const t = WORKER_EVENTS
    await this.sql`
      INSERT INTO ${this.sql(t)} (
        id, worker_id, timestamp, action, data
      ) VALUES (
        ${event.id}, ${event.workerId}, ${event.timestamp},
        ${event.action},
        ${event.data ? this.sql.json(event.data as never) : null}
      )
      ON CONFLICT (id) DO NOTHING
    `
  }

  async getWorkerEvents(workerId: string, opts?: EventQueryOptions): Promise<WorkerAuditEvent[]> {
    const t = WORKER_EVENTS
    const since = opts?.since

    let rows: postgres.RowList<postgres.Row[]>
    if (since?.timestamp !== undefined) {
      rows = await this.sql`
        SELECT * FROM ${this.sql(t)}
        WHERE worker_id = ${workerId} AND timestamp > ${since.timestamp}
        ORDER BY timestamp ASC
        ${opts?.limit ? this.sql`LIMIT ${opts.limit}` : this.sql``}
      `
    } else if (since?.id) {
      const anchor = await this.sql`
        SELECT timestamp FROM ${this.sql(t)} WHERE id = ${since.id}
      `
      const anchorTs = (anchor[0]?.['timestamp'] as number | undefined) ?? 0
      rows = await this.sql`
        SELECT * FROM ${this.sql(t)}
        WHERE worker_id = ${workerId} AND timestamp > ${anchorTs}
        ORDER BY timestamp ASC
        ${opts?.limit ? this.sql`LIMIT ${opts.limit}` : this.sql``}
      `
    } else {
      rows = await this.sql`
        SELECT * FROM ${this.sql(t)}
        WHERE worker_id = ${workerId}
        ORDER BY timestamp ASC
        ${opts?.limit ? this.sql`LIMIT ${opts.limit}` : this.sql``}
      `
    }

    return rows.map((r) => this._rowToWorkerEvent(r))
  }

  private _rowToTask(row: postgres.Row): Task {
    // Build using mutable assignment to satisfy exactOptionalPropertyTypes
    // Note: PostgreSQL BIGINT comes back as string from postgres.js, so we use Number() for numeric columns
    const task: Task = {
      id: row['id'] as string,
      status: row['status'] as Task['status'],
      createdAt: Number(row['created_at']),
      updatedAt: Number(row['updated_at']),
    }
    if (row['type'] != null) task.type = row['type'] as string
    if (row['params'] != null) task.params = row['params'] as Record<string, unknown>
    if (row['result'] != null) task.result = row['result'] as Record<string, unknown>
    if (row['error'] != null) task.error = row['error'] as TaskError
    if (row['metadata'] != null) task.metadata = row['metadata'] as Record<string, unknown>
    if (row['auth_config'] != null) task.authConfig = row['auth_config'] as TaskAuthConfig
    if (row['webhooks'] != null) task.webhooks = row['webhooks'] as WebhookConfig[]
    if (row['cleanup'] != null) task.cleanup = row['cleanup'] as { rules: CleanupRule[] }
    if (row['completed_at'] != null) task.completedAt = Number(row['completed_at'])
    if (row['ttl'] != null) task.ttl = Number(row['ttl'])
    if (row['tags'] != null) task.tags = row['tags'] as string[]
    if (row['assign_mode'] != null) task.assignMode = row['assign_mode'] as AssignMode
    if (row['cost'] != null) task.cost = Number(row['cost'])
    if (row['assigned_worker'] != null) task.assignedWorker = row['assigned_worker'] as string
    if (row['disconnect_policy'] != null) task.disconnectPolicy = row['disconnect_policy'] as DisconnectPolicy
    return task
  }

  private _rowToEvent(row: postgres.Row): TaskEvent {
    // Build using mutable assignment to satisfy exactOptionalPropertyTypes
    const event: TaskEvent = {
      id: row['id'] as string,
      taskId: row['task_id'] as string,
      index: Number(row['idx']),
      timestamp: Number(row['timestamp']),
      type: row['type'] as string,
      level: row['level'] as TaskEvent['level'],
      data: (row['data'] as unknown) ?? null,
    }
    if (row['series_id'] != null) event.seriesId = row['series_id'] as string
    if (row['series_mode'] != null) event.seriesMode = row['series_mode'] as SeriesMode
    if (row['series_acc_field'] != null) event.seriesAccField = row['series_acc_field'] as string
    if (row['client_id'] != null) event.clientId = row['client_id'] as string
    if (row['client_seq'] != null) event.clientSeq = Number(row['client_seq'])
    return event
  }

  private _rowToWorkerEvent(row: postgres.Row): WorkerAuditEvent {
    const event: WorkerAuditEvent = {
      id: row['id'] as string,
      workerId: row['worker_id'] as string,
      timestamp: Number(row['timestamp']),
      action: row['action'] as WorkerAuditEvent['action'],
    }
    if (row['data'] != null) event.data = row['data'] as Record<string, unknown>
    return event
  }
}
