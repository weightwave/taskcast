-- Opt-in enrollment only. Existing cleanup JSON is deliberately not backfilled.
ALTER TABLE taskcast_tasks ADD COLUMN IF NOT EXISTS cleanup_policy_version INTEGER;
ALTER TABLE taskcast_tasks ADD COLUMN IF NOT EXISTS cleanup_resolved_at BIGINT;
ALTER TABLE taskcast_tasks ADD COLUMN IF NOT EXISTS cleanup_due_at BIGINT;
ALTER TABLE taskcast_tasks ADD COLUMN IF NOT EXISTS cleanup_next_attempt_at BIGINT;
ALTER TABLE taskcast_tasks ADD COLUMN IF NOT EXISTS cleanup_claim_token TEXT;
ALTER TABLE taskcast_tasks ADD COLUMN IF NOT EXISTS cleanup_claim_until BIGINT;
ALTER TABLE taskcast_tasks ADD COLUMN IF NOT EXISTS cleanup_target TEXT;
ALTER TABLE taskcast_tasks ADD COLUMN IF NOT EXISTS cleanup_in_progress BOOLEAN NOT NULL DEFAULT false;
ALTER TABLE taskcast_tasks ADD COLUMN IF NOT EXISTS history_expired_at BIGINT;
ALTER TABLE taskcast_tasks ADD COLUMN IF NOT EXISTS history_expired_through_index BIGINT;

CREATE INDEX IF NOT EXISTS idx_taskcast_tasks_cleanup_due
  ON taskcast_tasks (cleanup_due_at, id)
  WHERE cleanup_policy_version = 1 AND cleanup_due_at IS NOT NULL;
