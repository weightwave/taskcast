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
  ttl INTEGER,
  tags TEXT,
  assign_mode TEXT,
  cost INTEGER,
  assigned_worker TEXT,
  disconnect_policy TEXT
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
  series_acc_field TEXT,
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

CREATE TABLE IF NOT EXISTS taskcast_workers (
  id TEXT PRIMARY KEY,
  status TEXT NOT NULL,
  match_rule TEXT NOT NULL,
  capacity INTEGER NOT NULL,
  used_slots INTEGER NOT NULL DEFAULT 0,
  weight INTEGER NOT NULL DEFAULT 1,
  connection_mode TEXT NOT NULL,
  connected_at INTEGER NOT NULL,
  last_heartbeat_at INTEGER NOT NULL,
  metadata TEXT
);

CREATE TABLE IF NOT EXISTS taskcast_worker_assignments (
  task_id TEXT PRIMARY KEY,
  worker_id TEXT NOT NULL,
  cost INTEGER NOT NULL,
  assigned_at INTEGER NOT NULL,
  status TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS taskcast_worker_events (
  id TEXT PRIMARY KEY,
  worker_id TEXT NOT NULL,
  timestamp INTEGER NOT NULL,
  action TEXT NOT NULL,
  data TEXT
);

CREATE INDEX IF NOT EXISTS idx_events_task_idx ON taskcast_events(task_id, idx);
CREATE INDEX IF NOT EXISTS idx_events_task_ts ON taskcast_events(task_id, timestamp);
CREATE INDEX IF NOT EXISTS idx_worker_assignments_worker ON taskcast_worker_assignments(worker_id);
CREATE INDEX IF NOT EXISTS idx_worker_events_worker ON taskcast_worker_events(worker_id);
CREATE INDEX IF NOT EXISTS idx_worker_events_ts ON taskcast_worker_events(worker_id, timestamp);
