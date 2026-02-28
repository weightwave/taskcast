CREATE TABLE IF NOT EXISTS taskcast_tasks (
  id TEXT PRIMARY KEY,
  type TEXT,
  status TEXT NOT NULL,
  params JSONB,
  result JSONB,
  error JSONB,
  metadata JSONB,
  auth_config JSONB,
  webhooks JSONB,
  cleanup JSONB,
  created_at BIGINT NOT NULL,
  updated_at BIGINT NOT NULL,
  completed_at BIGINT,
  ttl INTEGER
);

CREATE TABLE IF NOT EXISTS taskcast_events (
  id TEXT PRIMARY KEY,
  task_id TEXT NOT NULL REFERENCES taskcast_tasks(id) ON DELETE CASCADE,
  idx INTEGER NOT NULL,
  timestamp BIGINT NOT NULL,
  type TEXT NOT NULL,
  level TEXT NOT NULL,
  data JSONB,
  series_id TEXT,
  series_mode TEXT,
  UNIQUE(task_id, idx)
);

CREATE INDEX IF NOT EXISTS taskcast_events_task_id_idx ON taskcast_events(task_id, idx);
CREATE INDEX IF NOT EXISTS taskcast_events_task_id_timestamp ON taskcast_events(task_id, timestamp);
