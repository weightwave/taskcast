/**
 * Auto-generated migration embeddings.
 * Do not edit manually — run: pnpm generate-migrations
 */

export interface EmbeddedMigration {
  filename: string
  sql: string
}

export const EMBEDDED_MIGRATIONS: readonly EmbeddedMigration[] = [
  {
    filename: "001_initial.sql",
    sql: "CREATE TABLE IF NOT EXISTS taskcast_tasks (\n  id TEXT PRIMARY KEY,\n  type TEXT,\n  status TEXT NOT NULL,\n  params JSONB,\n  result JSONB,\n  error JSONB,\n  metadata JSONB,\n  auth_config JSONB,\n  webhooks JSONB,\n  cleanup JSONB,\n  created_at BIGINT NOT NULL,\n  updated_at BIGINT NOT NULL,\n  completed_at BIGINT,\n  ttl INTEGER\n);\n\nCREATE TABLE IF NOT EXISTS taskcast_events (\n  id TEXT PRIMARY KEY,\n  task_id TEXT NOT NULL REFERENCES taskcast_tasks(id) ON DELETE CASCADE,\n  idx INTEGER NOT NULL,\n  timestamp BIGINT NOT NULL,\n  type TEXT NOT NULL,\n  level TEXT NOT NULL,\n  data JSONB,\n  series_id TEXT,\n  series_mode TEXT,\n  series_acc_field TEXT,\n  UNIQUE(task_id, idx)\n);\n\nCREATE INDEX IF NOT EXISTS taskcast_events_task_id_idx ON taskcast_events(task_id, idx);\nCREATE INDEX IF NOT EXISTS taskcast_events_task_id_timestamp ON taskcast_events(task_id, timestamp);\n",
  },
  {
    filename: "002_workers.sql",
    sql: "-- Worker audit events table\nCREATE TABLE IF NOT EXISTS taskcast_worker_events (\n  id TEXT PRIMARY KEY,\n  worker_id TEXT NOT NULL,\n  timestamp BIGINT NOT NULL,\n  action TEXT NOT NULL,\n  data JSONB,\n  created_at TIMESTAMPTZ DEFAULT now()\n);\n\nCREATE INDEX IF NOT EXISTS idx_taskcast_worker_events_worker_id\n  ON taskcast_worker_events (worker_id, timestamp DESC);\n\n-- New Task columns for worker assignment\nALTER TABLE taskcast_tasks ADD COLUMN IF NOT EXISTS tags JSONB;\nALTER TABLE taskcast_tasks ADD COLUMN IF NOT EXISTS assign_mode TEXT;\nALTER TABLE taskcast_tasks ADD COLUMN IF NOT EXISTS cost INTEGER;\nALTER TABLE taskcast_tasks ADD COLUMN IF NOT EXISTS assigned_worker TEXT;\nALTER TABLE taskcast_tasks ADD COLUMN IF NOT EXISTS disconnect_policy TEXT;\n\n-- Modified at Thu Apr  9 04:24:55 CST 2026\n",
  },
]
