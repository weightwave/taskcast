---
"@taskcast/server-sdk": minor
---

Add `subscribe` method to `TaskcastServerClient` for real-time SSE event streaming. Connects to the server's SSE endpoint and delivers `TaskEvent` objects to a callback handler. Returns a synchronous unsubscribe function.
