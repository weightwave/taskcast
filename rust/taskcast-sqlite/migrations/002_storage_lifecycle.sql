CREATE TABLE IF NOT EXISTS taskcast_storage_locks (
  task_id TEXT PRIMARY KEY,
  lock_token TEXT NOT NULL,
  generation TEXT NOT NULL,
  storage_epoch INTEGER NOT NULL,
  expires_at INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS taskcast_durable_assignments (
  task_id TEXT PRIMARY KEY,
  assignment_id TEXT NOT NULL,
  worker_id TEXT NOT NULL,
  cost INTEGER NOT NULL,
  assigned_at INTEGER NOT NULL,
  status TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS taskcast_terminal_outbox (
  projection_id TEXT PRIMARY KEY,
  task_id TEXT NOT NULL,
  event_id TEXT NOT NULL,
  assignment_id TEXT,
  payload TEXT NOT NULL,
  claim_token TEXT,
  claim_until INTEGER,
  projected_at INTEGER,
  created_at INTEGER NOT NULL,
  UNIQUE(task_id, event_id)
);

CREATE INDEX IF NOT EXISTS idx_tasks_storage_state_last_event
  ON taskcast_tasks(storage_state, last_event_at);

CREATE INDEX IF NOT EXISTS idx_tasks_execution_deadline
  ON taskcast_tasks(execution_deadline_at)
  WHERE execution_deadline_at IS NOT NULL
    AND status NOT IN ('completed', 'failed', 'timeout', 'cancelled');

CREATE INDEX IF NOT EXISTS idx_terminal_outbox_pending
  ON taskcast_terminal_outbox(projected_at, claim_until);
