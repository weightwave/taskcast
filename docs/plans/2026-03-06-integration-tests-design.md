# Integration Test Plan

## Overview

Supplement integration tests across the Taskcast monorepo. Covers all packages with meaningful integration gaps, plus fixing 7 existing test failures.

## Scope

- **Memory adapter integration tests** (no Docker) — core engine flows, server HTTP+SSE, client, server-sdk, CLI
- **Testcontainer integration tests** (Redis/Postgres) — server with real adapters, multi-instance scenarios
- **Fix 7 failing tests** — worker-release (4) and worker-manager-remaining-gaps (3)

## Directory Structure

```
packages/
  core/tests/integration/
    lifecycle.test.ts           # Full task lifecycle (memory adapters)
    ttl-timeout.test.ts         # TTL expiration handling
    cleanup.test.ts             # Cleanup rule integration
    multi-subscriber.test.ts    # Multiple subscribers with filtering
    concurrent.test.ts          # (existing, supplement memory tests)
    engine-full.test.ts         # (existing, Redis+Postgres)

  server/tests/integration/
    task-lifecycle.test.ts      # HTTP full round-trip
    sse-streaming.test.ts       # SSE real streaming
    concurrent-transitions.test.ts  # Concurrent PATCH race
    webhook-delivery.test.ts    # Engine event -> real webhook delivery
    auth-scope.test.ts          # taskIds + scope enforcement
    worker-flow.test.ts         # Worker assign, claim, capacity release
    redis-adapters.test.ts      # Server with Redis testcontainer

  client/tests/integration/
    sse-client.test.ts          # Client against real SSE endpoint

  server-sdk/tests/integration/
    sdk-client.test.ts          # SDK against real HTTP server

  cli/tests/
    config.test.ts              # Config loading, env precedence
    startup.test.ts             # Adapter init, port resolution, /health
```

## Shared Test Infrastructure

### `packages/server/tests/helpers/test-server.ts`

```ts
export async function startTestServer(opts?: Partial<TaskcastOptions>) {
  // Creates real TaskEngine + Hono app with memory adapters
  // Returns { app, engine, baseUrl, close }
}
```

### `packages/server/tests/helpers/test-client.ts`

```ts
export function createTestClient(baseUrl: string) {
  // Wraps fetch for POST/GET/PATCH/SSE convenience
}
```

Client and server-sdk integration tests reuse `startTestServer`.

## Test Specifications

### 1. Core — lifecycle.test.ts

| Scenario | Assertions |
|----------|-----------|
| pending -> running -> completed full flow | State correct at each step, completedAt set, hooks fire in order |
| Series accumulate across lifecycle | 10 accumulate events -> merged result correct -> history consistent after completion |
| Series mixed modes on same task | accumulate + latest + keep-all each behave correctly |
| Result/error persistence | transition with result -> getTask returns result; with error -> error field complete |
| LongTermStore async write | Event immediately in ShortTermStore -> eventually in LongTermStore |
| LongTermStore fallback query | ShortTermStore empty -> auto-queries LongTermStore |

### 2. Core — ttl-timeout.test.ts

| Scenario | Assertions |
|----------|-----------|
| TTL expires -> auto timeout | Status becomes timeout, onTaskTimeout hook fires |
| Complete before TTL | TTL expires after completion -> status stays completed |
| TTL + concurrent transition race | TTL about to fire + manual complete -> exactly one wins |

### 3. Core — cleanup.test.ts

| Scenario | Assertions |
|----------|-----------|
| Type pattern matching | `llm.*` matches `llm.chat`, not `tool.call` |
| Status + maxAge cleanup | Completed tasks beyond maxAge deleted |
| Event filter cleanup | Only matching events deleted, others preserved |
| Multiple rules | Rules execute in order, no conflicts |

### 4. Core — multi-subscriber.test.ts

| Scenario | Assertions |
|----------|-----------|
| 5 subscribers with different type filters | Each receives correct subset, filteredIndex independent |
| Late subscriber replay | Task has 100 events -> new subscriber replays correctly -> live stream also works |
| Sequential unsubscribe | Unsubscribed client stops receiving, others unaffected |
| Subscribe to terminal task | Receives history replay -> auto-terminates (no hang) |

### 5. Server — task-lifecycle.test.ts

| Scenario | Assertions |
|----------|-----------|
| POST create -> POST events -> PATCH complete -> GET query | Each HTTP response correct, final GET consistent |
| Batch events -> GET history | Order preserved, series merging works, filter params work |
| Publish to terminal task | 409/422 response, event not written |
| JSON round-trip | camelCase fields, correct types throughout |

### 6. Server — sse-streaming.test.ts

| Scenario | Assertions |
|----------|-----------|
| Subscribe pending -> transition to running -> events flow | SSE stays open, events arrive after transition |
| Running task: history replay + live | History first, then live, order correct, index continuous |
| Terminal task -> replay then close | Receives `taskcast.done` with correct reason |
| 10 concurrent SSE clients | All receive identical event set |
| Filter params | types/levels/since -> SSE only pushes matching events |
| wrap vs no-wrap mode | wrap=true has status envelope, wrap=false has raw events |

### 7. Server — concurrent-transitions.test.ts

| Scenario | Assertions |
|----------|-----------|
| 10 concurrent PATCH complete | Exactly 1 returns 200, rest return 409 |
| Concurrent create + transition | State consistent after race |
| SSE observes only winning transition | Only the successful transition appears in SSE stream |

### 8. Server — webhook-delivery.test.ts

| Scenario | Assertions |
|----------|-----------|
| Create webhook -> publish event -> delivery received | Local HTTP server receives webhook, payload correct |
| HMAC signature | Receiver verifies HMAC matches |
| Webhook filter | Only matching type events delivered |
| Retry on failure | Receiver returns 500 first -> retry succeeds -> one valid delivery |

### 9. Server — auth-scope.test.ts

| Scenario | Assertions |
|----------|-----------|
| taskIds restriction | Token for task-1 -> access task-2 -> 403 |
| Insufficient scope | Only `event:subscribe` -> attempt `task:create` -> 403 |
| No token in jwt mode | 401 |
| Multi-scope combo | `task:create` + `event:publish` -> create and publish succeeds |

### 10. Server — worker-flow.test.ts

| Scenario | Assertions |
|----------|-----------|
| Register -> claim -> run -> complete -> capacity release | usedSlots 0->1->0, status idle->busy->idle |
| Concurrent claim race | Multiple workers claim same task -> exactly one succeeds |
| Pull long-polling | GET /workers/pull waits -> returns on task available -> timeout returns empty |
| Decline + blacklist | Declined worker not re-assigned same task |
| Terminal capacity release (fixes existing failures) | completed/failed/cancelled all release capacity correctly |

### 11. Server — redis-adapters.test.ts (testcontainer)

| Scenario | Assertions |
|----------|-----------|
| Redis broadcast + HTTP full round-trip | Create -> publish -> SSE receives via Redis pub/sub |
| Multi-instance shared Redis | Instance A publishes -> Instance B's SSE client receives |
| Redis reconnect | Redis restart -> server degrades but doesn't crash |

### 12. Client — sse-client.test.ts

| Scenario | Assertions |
|----------|-----------|
| Connect -> receive events -> receive done | onEvent fires, onDone fires with correct reason |
| Filter passthrough | types/levels set -> only matching events received |
| Since reconnect | Receive 5 events -> disconnect -> reconnect with since -> no duplicates |
| Auth token | With token + jwt mode -> success; without -> 401 |
| Network error | Server not running -> fetch throws -> caller can catch |
| High-volume stream | 500 events rapid-fire -> all received in order, none lost |

### 13. Server-SDK — sdk-client.test.ts

| Scenario | Assertions |
|----------|-----------|
| createTask -> getTask consistency | Created task matches queried task |
| transitionTask flow | pending -> running -> completed at each step |
| publishEvent + getHistory | 3 events -> history returns in order |
| Batch publish | 10 events -> all queryable, order correct |
| Since pagination | history with since -> returns incremental |
| Auth token | With token -> success; without -> 401 |
| Error: not found | getTask nonexistent -> throws with 404 |
| Error: conflict | Double complete -> throws with 409 |

### 14. CLI — config.test.ts

| Scenario | Assertions |
|----------|-----------|
| YAML config loading | Temp config file -> loadConfigFile returns correct structure |
| Env override | TASKCAST_STORAGE=redis overrides config's memory |
| Port precedence | CLI flag > env TASKCAST_PORT > config > default 3721 |
| Invalid YAML | Malformed file -> readable error |
| Missing config file | Nonexistent path -> reasonable error handling |

### 15. CLI — startup.test.ts

| Scenario | Assertions |
|----------|-----------|
| Memory mode startup | Memory adapters -> /health responds `{ ok: true }` |
| SQLite mode startup | Temp SQLite -> create task -> data persists |
| Auth jwt mode | JWT secret configured -> unauthenticated request rejected |
| Workers enabled | workersEnabled=true -> WorkerManager init -> /workers endpoint available |
| Default config creation | No global config + non-TTY -> createDefaultGlobalConfig writes file |

## Failing Test Fixes

### worker-release.test.ts (4 failures)

Capacity release on terminal transitions (completed/failed/cancelled) fails because the release logic is not implemented in the current codebase. Fix: implement capacity release in task transition handler, or update tests to match current behavior with TODO markers.

### worker-manager-remaining-gaps.test.ts (3 failures)

Tests call `manager.releaseTask()` which doesn't exist. Fix: either implement the method on WorkerManager or rename test calls to the correct existing method.

## Priority Order

1. **Fix 7 failing tests** — unblock CI
2. **Shared test infrastructure** — startTestServer, createTestClient
3. **Server integration tests** — highest value, covers Engine + HTTP
4. **Core integration tests** — lifecycle, TTL, cleanup, multi-subscriber
5. **Client + Server-SDK integration tests** — real endpoint validation
6. **CLI tests** — config and startup
7. **Redis testcontainer tests** — multi-instance, reconnect
