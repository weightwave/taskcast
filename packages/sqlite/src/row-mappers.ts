import type {
  Task,
  TaskEvent,
  SeriesMode,
  Worker,
  WorkerAssignment,
  WorkerAuditEvent,
  AssignMode,
  DisconnectPolicy,
} from '@taskcast/core'

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
  if (row['tags'] != null) task.tags = JSON.parse(row['tags'] as string) as string[]
  if (row['assign_mode'] != null) task.assignMode = row['assign_mode'] as AssignMode
  if (row['cost'] != null) task.cost = row['cost'] as number
  if (row['assigned_worker'] != null) task.assignedWorker = row['assigned_worker'] as string
  if (row['disconnect_policy'] != null) task.disconnectPolicy = row['disconnect_policy'] as DisconnectPolicy

  return task
}

export function rowToWorker(row: Record<string, unknown>): Worker {
  const worker: Worker = {
    id: row['id'] as string,
    status: row['status'] as Worker['status'],
    matchRule: JSON.parse(row['match_rule'] as string),
    capacity: row['capacity'] as number,
    usedSlots: row['used_slots'] as number,
    weight: row['weight'] as number,
    connectionMode: row['connection_mode'] as Worker['connectionMode'],
    connectedAt: row['connected_at'] as number,
    lastHeartbeatAt: row['last_heartbeat_at'] as number,
  }
  if (row['metadata'] != null) worker.metadata = JSON.parse(row['metadata'] as string)
  return worker
}

export function rowToWorkerAssignment(row: Record<string, unknown>): WorkerAssignment {
  return {
    taskId: row['task_id'] as string,
    workerId: row['worker_id'] as string,
    cost: row['cost'] as number,
    assignedAt: row['assigned_at'] as number,
    status: row['status'] as WorkerAssignment['status'],
  }
}

export function rowToWorkerEvent(row: Record<string, unknown>): WorkerAuditEvent {
  const event: WorkerAuditEvent = {
    id: row['id'] as string,
    workerId: row['worker_id'] as string,
    timestamp: row['timestamp'] as number,
    action: row['action'] as WorkerAuditEvent['action'],
  }
  if (row['data'] != null) event.data = JSON.parse(row['data'] as string)
  return event
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
  if (row['series_acc_field'] != null) event.seriesAccField = row['series_acc_field'] as string

  return event
}
