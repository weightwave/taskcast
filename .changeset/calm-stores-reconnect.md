---
"@taskcast/core": patch
"@taskcast/server": patch
"@taskcast/cli": patch
"@taskcast/redis": patch
"@taskcast/postgres": patch
---

Recover managed Redis and PostgreSQL connectivity without replaying failed
business operations, restore Redis PubSub subscriptions, and expose
dependency-aware readiness in both server runtimes.
