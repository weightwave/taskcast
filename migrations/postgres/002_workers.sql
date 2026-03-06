-- Worker audit events table
CREATE TABLE IF NOT EXISTS taskcast_worker_events (
  id TEXT PRIMARY KEY,
  worker_id TEXT NOT NULL,
  timestamp BIGINT NOT NULL,
  action TEXT NOT NULL,
  data JSONB,
  created_at TIMESTAMPTZ DEFAULT now()
);

CREATE INDEX IF NOT EXISTS idx_taskcast_worker_events_worker_id
  ON taskcast_worker_events (worker_id, timestamp DESC);

-- New Task columns for worker assignment
ALTER TABLE taskcast_tasks ADD COLUMN IF NOT EXISTS tags JSONB;
ALTER TABLE taskcast_tasks ADD COLUMN IF NOT EXISTS assign_mode TEXT;
ALTER TABLE taskcast_tasks ADD COLUMN IF NOT EXISTS cost INTEGER;
ALTER TABLE taskcast_tasks ADD COLUMN IF NOT EXISTS assigned_worker TEXT;
ALTER TABLE taskcast_tasks ADD COLUMN IF NOT EXISTS disconnect_policy TEXT;
