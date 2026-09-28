---
"@taskcast/core": minor
"@taskcast/server": minor
"@taskcast/client": minor
"@taskcast/cli": minor
"@taskcast/postgres": minor
---

Add opt-in terminal task retention with snapshotted per-task policies, bounded PostgreSQL cleanup, Redis release coordination, and fenced late-write protection in Node and Rust. Expired history is explicit in REST and SSE, and archive restore creates a fresh unenrolled generation. Existing tasks and deployments remain unchanged unless cleanup is enabled for newly created tasks.
