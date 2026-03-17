---
"@taskcast/core": patch
---

Fix seriesMode "latest" producing duplicate events in history. The engine now skips appendEvent when processSeries has already stored the event via replaceLastSeriesEvent.
