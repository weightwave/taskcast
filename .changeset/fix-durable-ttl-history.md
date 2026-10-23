---
'@taskcast/postgres': patch
---

Fix overdue tasks failing to transition to timeout after latest-series compaction, event archival, or history expiration. Node and Rust PostgreSQL adapters now validate the terminal event index against the complete durable history watermark while preserving claim ownership and concurrent terminalization checks.
