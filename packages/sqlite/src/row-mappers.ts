import type { Task, TaskEvent, SeriesMode } from '@taskcast/core'

export function rowToTask(row: Record<string, unknown>): Task {
  const task: Task = {
    id: row['id'] as string,
    status: row['status'] as Task['status'],
    createdAt: row['created_at'] as number,
    updatedAt: row['updated_at'] as number,
  }

  if (row['type'] != null) task.type = row['type'] as string
  if (row['params'] != null) task.params = JSON.parse(row['params'] as string)
  if (row['result'] != null) task.result = JSON.parse(row['result'] as string)
  if (row['error'] != null) task.error = JSON.parse(row['error'] as string)
  if (row['metadata'] != null) task.metadata = JSON.parse(row['metadata'] as string)
  if (row['auth_config'] != null) task.authConfig = JSON.parse(row['auth_config'] as string)
  if (row['webhooks'] != null) task.webhooks = JSON.parse(row['webhooks'] as string)
  if (row['cleanup'] != null) task.cleanup = JSON.parse(row['cleanup'] as string)
  if (row['completed_at'] != null) task.completedAt = row['completed_at'] as number
  if (row['ttl'] != null) task.ttl = row['ttl'] as number

  return task
}

export function rowToEvent(row: Record<string, unknown>): TaskEvent {
  const event: TaskEvent = {
    id: row['id'] as string,
    taskId: row['task_id'] as string,
    index: row['idx'] as number,
    timestamp: row['timestamp'] as number,
    type: row['type'] as string,
    level: row['level'] as TaskEvent['level'],
    data: row['data'] != null ? JSON.parse(row['data'] as string) : null,
  }

  if (row['series_id'] != null) event.seriesId = row['series_id'] as string
  if (row['series_mode'] != null) event.seriesMode = row['series_mode'] as SeriesMode

  return event
}
