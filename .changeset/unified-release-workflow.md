---
"@taskcast/core": patch
"@taskcast/server": patch
"@taskcast/server-sdk": patch
"@taskcast/client": patch
"@taskcast/react": patch
"@taskcast/cli": patch
"@taskcast/redis": patch
"@taskcast/postgres": patch
"@taskcast/sentry": patch
---

Unified release workflow: npm publish, Rust binary builds (5 platforms), and Docker image push now share a single version number and run in one workflow.
