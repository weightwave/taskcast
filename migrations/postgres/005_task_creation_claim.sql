-- Explicit task IDs are claimed durably before their hot Redis state is
-- created. Claims are leased so a pristine row left by a crashed creator can
-- be taken over, while the retained token makes completion idempotent.
ALTER TABLE taskcast_tasks ADD COLUMN IF NOT EXISTS creation_token TEXT;
ALTER TABLE taskcast_tasks ADD COLUMN IF NOT EXISTS creation_claimed_at BIGINT;
ALTER TABLE taskcast_tasks ADD COLUMN IF NOT EXISTS creation_claim_expires_at BIGINT;
ALTER TABLE taskcast_tasks ADD COLUMN IF NOT EXISTS creation_completed_at BIGINT;

CREATE INDEX IF NOT EXISTS idx_taskcast_tasks_creation_token
  ON taskcast_tasks (creation_token)
  WHERE creation_token IS NOT NULL;

CREATE INDEX IF NOT EXISTS idx_taskcast_tasks_creation_claim_expiry
  ON taskcast_tasks (creation_claim_expires_at)
  WHERE creation_token IS NOT NULL AND creation_completed_at IS NULL;
