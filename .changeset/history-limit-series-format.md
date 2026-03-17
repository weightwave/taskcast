---
"@taskcast/core": minor
"@taskcast/server": minor
---

Add `limit` and `seriesFormat` query parameters to the history endpoint, and `limit` to the SSE endpoint. History endpoint now supports hot/cold task routing (ShortTermStore → LongTermStore fallback) and series collapse via `seriesFormat=accumulated`. SSE handler refactored to use shared `collapseAccumulateSeries` function.
