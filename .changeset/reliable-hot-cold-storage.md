---
"@taskcast/core": minor
"@taskcast/server": minor
"@taskcast/redis": minor
"@taskcast/postgres": minor
"@taskcast/cli": minor
---

Add a fenced hot-to-cold storage lifecycle with a guarded release endpoint,
PostgreSQL-canonical history, bounded Redis rehydration, and durable task TTL
terminalization. Emit payload-free release, rehydration, history-source,
watermark, TTL, and unusually old/large hot-task observations.
