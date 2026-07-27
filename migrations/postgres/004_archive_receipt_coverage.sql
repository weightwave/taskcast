-- The first archive batch has no predecessor. This forward migration repairs
-- the original receipt schema without changing the checksum of migration 003.
ALTER TABLE taskcast_archive_batches
  ALTER COLUMN previous_digest DROP NOT NULL;

-- Compact coverage is bounded metadata only. Full event/series payloads remain
-- in their canonical tables and are never duplicated into every receipt.
ALTER TABLE taskcast_archive_batches
  ADD COLUMN IF NOT EXISTS series_coverage JSONB NOT NULL DEFAULT '[]'::jsonb;
