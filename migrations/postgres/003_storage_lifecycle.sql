-- Durable task lifecycle metadata. Event index widening is intentionally
-- deferred to a separate online migration because changing INTEGER to BIGINT
-- may rewrite the production event table on supported PostgreSQL versions.
ALTER TABLE taskcast_tasks ADD COLUMN IF NOT EXISTS storage_state TEXT NOT NULL DEFAULT 'hot';
ALTER TABLE taskcast_tasks ADD COLUMN IF NOT EXISTS storage_epoch BIGINT NOT NULL DEFAULT 1;
ALTER TABLE taskcast_tasks ADD COLUMN IF NOT EXISTS active_release_generation TEXT;
ALTER TABLE taskcast_tasks ADD COLUMN IF NOT EXISTS archive_watermark BIGINT NOT NULL DEFAULT -1;
ALTER TABLE taskcast_tasks ADD COLUMN IF NOT EXISTS last_event_at BIGINT;
ALTER TABLE taskcast_tasks ADD COLUMN IF NOT EXISTS cold_at BIGINT;
ALTER TABLE taskcast_tasks ADD COLUMN IF NOT EXISTS execution_deadline_at BIGINT;
ALTER TABLE taskcast_tasks ADD COLUMN IF NOT EXISTS task_version BIGINT NOT NULL DEFAULT 0;
ALTER TABLE taskcast_tasks ADD COLUMN IF NOT EXISTS ttl_claim_token TEXT;
ALTER TABLE taskcast_tasks ADD COLUMN IF NOT EXISTS ttl_claim_until BIGINT;
ALTER TABLE taskcast_tasks ADD COLUMN IF NOT EXISTS release_requested_at BIGINT;
ALTER TABLE taskcast_tasks ADD COLUMN IF NOT EXISTS release_expected_index BIGINT;
ALTER TABLE taskcast_tasks ADD COLUMN IF NOT EXISTS release_inactive_since BIGINT;

DO $$
BEGIN
  IF NOT EXISTS (
    SELECT 1
    FROM pg_constraint
    WHERE conname = 'taskcast_tasks_storage_state_check'
      AND conrelid = 'taskcast_tasks'::regclass
  ) THEN
    ALTER TABLE taskcast_tasks
      ADD CONSTRAINT taskcast_tasks_storage_state_check
      CHECK (storage_state IN ('hot', 'releasing', 'cold'));
  END IF;
END
$$;

CREATE INDEX IF NOT EXISTS idx_taskcast_tasks_storage_activity
  ON taskcast_tasks (storage_state, last_event_at);

CREATE INDEX IF NOT EXISTS idx_taskcast_tasks_due_execution_deadline
  ON taskcast_tasks (execution_deadline_at)
  WHERE execution_deadline_at IS NOT NULL
    AND status NOT IN ('completed', 'failed', 'timeout', 'cancelled');

CREATE INDEX IF NOT EXISTS idx_taskcast_tasks_release_requested
  ON taskcast_tasks (release_requested_at)
  WHERE release_requested_at IS NOT NULL;

-- One row per archive attempt. The manifest commits to the complete source
-- coverage before any hot data can be deleted.
CREATE TABLE IF NOT EXISTS taskcast_archive_generations (
  task_id TEXT NOT NULL REFERENCES taskcast_tasks(id) ON DELETE CASCADE,
  generation TEXT NOT NULL,
  storage_epoch BIGINT NOT NULL,
  target_watermark BIGINT NOT NULL,
  manifest JSONB NOT NULL,
  status TEXT NOT NULL DEFAULT 'uploading',
  created_at BIGINT NOT NULL,
  updated_at BIGINT NOT NULL,
  finalized_at BIGINT,
  PRIMARY KEY (task_id, generation),
  CONSTRAINT taskcast_archive_generations_status_check
    CHECK (status IN ('uploading', 'finalized', 'abandoned'))
);

CREATE INDEX IF NOT EXISTS idx_taskcast_archive_generations_incomplete
  ON taskcast_archive_generations (updated_at)
  WHERE status = 'uploading';

-- Receipts only: canonical event payloads stay in taskcast_events, while each
-- batch row proves its numbered source coverage and chained digest.
CREATE TABLE IF NOT EXISTS taskcast_archive_batches (
  task_id TEXT NOT NULL,
  generation TEXT NOT NULL,
  ordinal INTEGER NOT NULL CHECK (ordinal >= 0),
  previous_digest TEXT NOT NULL,
  current_digest TEXT NOT NULL,
  source_first_index BIGINT,
  source_last_index BIGINT,
  source_index_digest TEXT NOT NULL,
  source_series_digest TEXT NOT NULL,
  entry_count INTEGER NOT NULL CHECK (entry_count >= 0),
  created_at BIGINT NOT NULL,
  PRIMARY KEY (task_id, generation, ordinal),
  FOREIGN KEY (task_id, generation)
    REFERENCES taskcast_archive_generations(task_id, generation)
    ON DELETE CASCADE
);

-- Canonical latest/accumulated series state is separate from the delta history.
CREATE TABLE IF NOT EXISTS taskcast_series_state (
  task_id TEXT NOT NULL REFERENCES taskcast_tasks(id) ON DELETE CASCADE,
  series_id TEXT NOT NULL,
  mode TEXT NOT NULL,
  event JSONB NOT NULL,
  through_index BIGINT NOT NULL,
  updated_at BIGINT NOT NULL,
  PRIMARY KEY (task_id, series_id),
  CONSTRAINT taskcast_series_state_mode_check
    CHECK (mode IN ('latest', 'accumulate'))
);

-- Durable ownership is authoritative while a task is cold and while Redis is
-- being rebuilt after a restart.
CREATE TABLE IF NOT EXISTS taskcast_durable_assignments (
  task_id TEXT PRIMARY KEY REFERENCES taskcast_tasks(id) ON DELETE CASCADE,
  assignment_id TEXT NOT NULL UNIQUE,
  worker_id TEXT NOT NULL,
  cost INTEGER NOT NULL,
  assigned_at BIGINT NOT NULL,
  status TEXT NOT NULL,
  updated_at BIGINT NOT NULL,
  CONSTRAINT taskcast_durable_assignments_status_check
    CHECK (status IN ('offered', 'assigned', 'running'))
);

CREATE INDEX IF NOT EXISTS idx_taskcast_durable_assignments_worker
  ON taskcast_durable_assignments (worker_id, status);

-- Terminal transitions and worker-credit settlement share one durable outbox.
-- A projection is complete only after projected_at is populated.
CREATE TABLE IF NOT EXISTS taskcast_terminal_outbox (
  projection_id TEXT PRIMARY KEY,
  task_id TEXT NOT NULL REFERENCES taskcast_tasks(id) ON DELETE CASCADE,
  event_id TEXT NOT NULL,
  assignment_id TEXT,
  payload JSONB NOT NULL,
  claim_token TEXT,
  claim_until BIGINT,
  projected_at BIGINT,
  created_at BIGINT NOT NULL,
  UNIQUE (task_id, event_id)
);

CREATE INDEX IF NOT EXISTS idx_taskcast_terminal_outbox_pending
  ON taskcast_terminal_outbox (claim_until, created_at)
  WHERE projected_at IS NULL;
