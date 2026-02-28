import type postgres from 'postgres'
import type { Task, TaskEvent, LongTermStore, EventQueryOptions } from '@taskcast/core'

function makeTableNames(prefix: string) {
  return {
    tasks: `${prefix}_tasks`,
    events: `${prefix}_events`,
  }
}

export class PostgresLongTermStore implements LongTermStore {
  private tables: ReturnType<typeof makeTableNames>

  constructor(
    private sql: ReturnType<typeof postgres>,
    { prefix }: { prefix?: string } = {},
  ) {
    const resolvedPrefix = prefix ?? process.env['TASKCAST_PG_PREFIX'] ?? 'taskcast'
    this.tables = makeTableNames(resolvedPrefix)
  }

  async saveTask(task: Task): Promise<void> {
    const t = this.tables.tasks
    await this.sql`
      INSERT INTO ${this.sql(t)} (
        id, type, status, params, result, error, metadata,
        auth_config, webhooks, cleanup, created_at, updated_at, completed_at, ttl
      ) VALUES (
        ${task.id}, ${task.type ?? null}, ${task.status},
        ${task.params ? this.sql.json(task.params) : null},
        ${task.result ? this.sql.json(task.result) : null},
        ${task.error ? this.sql.json(task.error) : null},
        ${task.metadata ? this.sql.json(task.metadata) : null},
        ${task.authConfig ? this.sql.json(task.authConfig) : null},
        ${task.webhooks ? this.sql.json(task.webhooks) : null},
        ${task.cleanup ? this.sql.json(task.cleanup) : null},
        ${task.createdAt}, ${task.updatedAt},
        ${task.completedAt ?? null}, ${task.ttl ?? null}
      )
      ON CONFLICT (id) DO UPDATE SET
        status = EXCLUDED.status,
        result = EXCLUDED.result,
        error = EXCLUDED.error,
        metadata = EXCLUDED.metadata,
        updated_at = EXCLUDED.updated_at,
        completed_at = EXCLUDED.completed_at
    `
  }

  async getTask(taskId: string): Promise<Task | null> {
    const t = this.tables.tasks
    const rows = await this.sql`
      SELECT * FROM ${this.sql(t)} WHERE id = ${taskId}
    `
    const row = rows[0]
    if (!row) return null
    return this._rowToTask(row)
  }

  async saveEvent(event: TaskEvent): Promise<void> {
    const t = this.tables.events
    await this.sql`
      INSERT INTO ${this.sql(t)} (
        id, task_id, idx, timestamp, type, level, data, series_id, series_mode
      ) VALUES (
        ${event.id}, ${event.taskId}, ${event.index}, ${event.timestamp},
        ${event.type}, ${event.level},
        ${event.data ? this.sql.json(event.data as Record<string, unknown>) : null},
        ${event.seriesId ?? null}, ${event.seriesMode ?? null}
      )
      ON CONFLICT (id) DO NOTHING
    `
  }

  async getEvents(taskId: string, opts?: EventQueryOptions): Promise<TaskEvent[]> {
    const t = this.tables.events
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

  private _rowToTask(row: postgres.Row): Task {
    return {
      id: row['id'] as string,
      type: (row['type'] as string | null) ?? undefined,
      status: row['status'] as Task['status'],
      params: (row['params'] as Record<string, unknown> | null) ?? undefined,
      result: (row['result'] as Record<string, unknown> | null) ?? undefined,
      error: (row['error'] as Task['error'] | null) ?? undefined,
      metadata: (row['metadata'] as Record<string, unknown> | null) ?? undefined,
      authConfig: (row['auth_config'] as Task['authConfig'] | null) ?? undefined,
      webhooks: (row['webhooks'] as Task['webhooks'] | null) ?? undefined,
      cleanup: (row['cleanup'] as Task['cleanup'] | null) ?? undefined,
      createdAt: row['created_at'] as number,
      updatedAt: row['updated_at'] as number,
      completedAt: (row['completed_at'] as number | null) ?? undefined,
      ttl: (row['ttl'] as number | null) ?? undefined,
    }
  }

  private _rowToEvent(row: postgres.Row): TaskEvent {
    return {
      id: row['id'] as string,
      taskId: row['task_id'] as string,
      index: row['idx'] as number,
      timestamp: row['timestamp'] as number,
      type: row['type'] as string,
      level: row['level'] as TaskEvent['level'],
      data: (row['data'] as unknown) ?? null,
      seriesId: (row['series_id'] as string | null) ?? undefined,
      seriesMode: (row['series_mode'] as TaskEvent['seriesMode'] | null) ?? undefined,
    }
  }
}
