CREATE TABLE IF NOT EXISTS taskcast_tasks (
  id TEXT PRIMARY KEY,
  type TEXT,
  status TEXT NOT NULL,
  params TEXT,
  result TEXT,
  error TEXT,
  metadata TEXT,
  auth_config TEXT,
  webhooks TEXT,
  cleanup TEXT,
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL,
  completed_at INTEGER,
  ttl INTEGER
);

CREATE TABLE IF NOT EXISTS taskcast_events (
  id TEXT PRIMARY KEY,
  task_id TEXT NOT NULL REFERENCES taskcast_tasks(id) ON DELETE CASCADE,
  idx INTEGER NOT NULL,
  timestamp INTEGER NOT NULL,
  type TEXT NOT NULL,
  level TEXT NOT NULL,
  data TEXT,
  series_id TEXT,
  series_mode TEXT,
  UNIQUE(task_id, idx)
);

CREATE TABLE IF NOT EXISTS taskcast_series_latest (
  task_id TEXT NOT NULL,
  series_id TEXT NOT NULL,
  event_json TEXT NOT NULL,
  PRIMARY KEY (task_id, series_id)
);

CREATE TABLE IF NOT EXISTS taskcast_index_counters (
  task_id TEXT PRIMARY KEY,
  counter INTEGER NOT NULL DEFAULT -1
);

CREATE INDEX IF NOT EXISTS idx_events_task_idx ON taskcast_events(task_id, idx);
CREATE INDEX IF NOT EXISTS idx_events_task_ts ON taskcast_events(task_id, timestamp);
