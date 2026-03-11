---
"@taskcast/core": minor
"@taskcast/server": minor
"@taskcast/client": minor
"@taskcast/react": minor
"@taskcast/redis": minor
"@taskcast/sqlite": minor
"@taskcast/cli": minor
"@taskcast/server-sdk": minor
---

Add `seriesFormat` SSE parameter for consumer-controlled accumulate output

**Breaking change:** `accumulate` series mode now stores deltas in ShortTermStore and accumulated values in LongTermStore. SSE subscribers receive deltas by default (`seriesFormat=delta`). Existing consumers that expected accumulated values must add `seriesFormat=accumulated` to their SSE subscription.

New features:
- `seriesFormat` query parameter on SSE endpoint: `delta` (default) or `accumulated`
- Late-join snapshot collapse: subscribers connecting mid-stream receive a single accumulated snapshot per series
- `seriesSnapshot` field on SSEEnvelope to distinguish snapshots from regular events
- Atomic `accumulateSeries` on storage adapters (Redis Lua script, SQLite transaction)
- `SeriesResult` type: `processSeries` returns both delta and accumulated events
