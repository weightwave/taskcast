---
"@taskcast/server": patch
"@taskcast/cli": patch
---

Log every HTTP 5xx response once as sanitized structured JSON in both the
TypeScript and Rust servers, and validate `TASKCAST_LOG_LEVEL` at startup.
