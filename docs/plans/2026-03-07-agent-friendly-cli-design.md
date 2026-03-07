# Agent-Friendly CLI & Skill Design

**Date:** 2026-03-07
**Status:** Draft
**Goal:** Make Taskcast more friendly for AI agents and vibe coding workflows by adding CLI client commands, node management, debugging tools, and an updated skill.

## Motivation

Taskcast's current CLI is server-only (`start`, `migrate`, `playground`, `ui`). External AI agents and developers using vibe coding have no CLI way to inspect tasks, stream logs, or manage connections to Taskcast instances. They must manually construct HTTP calls or read docs to debug issues.

This design adds:
1. **Client CLI commands** — inspect tasks, stream logs, check connectivity
2. **Node management** — save/switch between local and remote Taskcast instances with auth
3. **Server verbose mode** — detailed request/event logging for debugging
4. **Skill updates** — debugging guidance and agent workflow patterns

Both TypeScript and Rust CLIs must be updated simultaneously.

## Non-Goals

- MCP Server (deferred to future work)
- New server-side features beyond `/health/detail` and global event SSE
- Dashboard changes

---

## 1. CLI Architecture

### File Structure

**TypeScript** (`packages/cli/src/`):
```
src/
├── index.ts              # Entry point, register all commands
├── commands/
│   ├── start.ts          # taskcast start (existing, refactored)
│   ├── migrate.ts        # taskcast migrate (existing)
│   ├── playground.ts     # taskcast playground (existing)
│   ├── ui.ts             # taskcast ui (existing)
│   ├── node.ts           # taskcast node add/remove/use/list
│   ├── tasks.ts          # taskcast tasks list/inspect
│   ├── logs.ts           # taskcast logs <taskId> / taskcast tail
│   └── ping.ts           # taskcast ping / taskcast doctor
├── node-config.ts        # ~/.taskcast/nodes.json read/write
└── client.ts             # Create HTTP client from current node config
```

**Rust** (`rust/taskcast-cli/src/`):
```
src/
├── main.rs               # Entry point + clap definitions
├── commands/
│   ├── start.rs          # Existing
│   ├── migrate.rs        # Existing
│   ├── playground.rs     # Existing
│   ├── node.rs           # node add/remove/use/list
│   ├── tasks.rs          # tasks list/inspect
│   ├── logs.rs           # logs / tail
│   └── ping.rs           # ping / doctor
├── node_config.rs        # ~/.taskcast/nodes.json read/write
└── client.rs             # HTTP client (reqwest)
```

### Dependency Separation

Client commands (node, tasks, logs, ping) must NOT load server dependencies:

- **TypeScript:** Client commands only import `@taskcast/server-sdk`. No ioredis, postgres, or @taskcast/core imports.
- **Rust:** Client commands only use `reqwest` + `serde`. No sqlx, redis, or taskcast-core dependencies. Use Cargo feature flags if needed to keep binary size down.

---

## 2. Node Management

### Commands

```bash
taskcast node add <name> --url <url> [--token <token>] [--token-type jwt|admin]
taskcast node remove <name>
taskcast node use <name>           # Set as default
taskcast node list                 # List all, mark current
```

### Storage

File: `~/.taskcast/nodes.json`

```json
{
  "current": "local",
  "nodes": {
    "local": {
      "url": "http://localhost:3721"
    },
    "prod": {
      "url": "https://taskcast.example.com",
      "token": "eyJhbG...",
      "tokenType": "jwt"
    },
    "staging": {
      "url": "https://staging.tc.io",
      "token": "admin_xxx",
      "tokenType": "admin"
    }
  }
}
```

### Auth Behavior

- `tokenType: "jwt"` — Use token directly as `Authorization: Bearer <token>` header.
- `tokenType: "admin"` — On each client command, first call `POST /admin/token` with the admin token to exchange for a JWT, then use the JWT for subsequent requests. Cache the JWT in memory for the command's lifetime.
- No token — Connect without auth (for `auth: none` servers).

### Defaults

- All client commands accept `--node <name>` to override the current default.
- If no nodes are configured, default to `http://localhost:3721` with no auth.

---

## 3. Client Commands

### `taskcast tasks list`

```bash
taskcast tasks list [--status running] [--type "llm.*"] [--limit 20] [--node prod]
```

Output: structured table (task ID, type, status, created time). Agent-parseable.

```
ID                          TYPE        STATUS     CREATED
01JXXXXXXXXXXXXXXXXXX       llm.chat    running    2026-03-07 14:30:01
01JYYYYYYYYYYYYYYYYYY       agent.step  completed  2026-03-07 14:28:55
```

### `taskcast tasks inspect`

```bash
taskcast tasks inspect <taskId> [--node prod]
```

Output: full task details + recent events summary.

```
Task: 01JXXXXXXXXXXXXXXXXXX
  Type:    llm.chat
  Status:  running
  Params:  {"prompt": "Hello"}
  Created: 2026-03-07 14:30:01
  TTL:     3600s

Recent Events (last 5):
  #0  llm.delta    info   series:response  2026-03-07 14:30:02
  #1  llm.delta    info   series:response  2026-03-07 14:30:02
  #2  llm.delta    info   series:response  2026-03-07 14:30:03
```

### `taskcast logs`

```bash
taskcast logs <taskId> [--types "llm.*"] [--levels info,warn]
```

SSE subscription to a single task. Prints events as they arrive:

```
[14:30:02] llm.delta    info  {"delta": "Hello "}
[14:30:02] llm.delta    info  {"delta": "world!"}
[14:30:03] [DONE] completed
```

Exits when task reaches terminal status.

### `taskcast tail`

```bash
taskcast tail [--types "llm.*"] [--levels info,warn]
```

Global event stream across all tasks. Requires a new server-side endpoint (see section 5).

```
[14:30:02] 01JXX..  llm.delta    info  {"delta": "Hello "}
[14:30:03] 01JYY..  agent.step   info  {"step": 3}
```

Runs until interrupted (Ctrl+C).

### `taskcast ping`

```bash
taskcast ping [--node prod]
```

Quick connectivity check via `GET /health`.

```
OK — taskcast v0.3.1 at https://taskcast.example.com (12ms)
```

On failure:
```
FAIL — cannot reach https://taskcast.example.com: ECONNREFUSED
```

### `taskcast doctor`

```bash
taskcast doctor [--node prod]
```

Deep health check. Calls `GET /health/detail` (new endpoint, see section 5).

```
Server:    OK  taskcast v0.3.1 at http://localhost:3721
Auth:      OK  jwt mode, token valid (expires 2026-03-08)
Broadcast: OK  redis (redis://localhost:6379)
ShortTerm: OK  redis (redis://localhost:6379)
LongTerm:  OK  postgres (postgresql://localhost:5432/taskcast)
```

On issues:
```
Server:    OK  taskcast v0.3.1 at http://localhost:3721
Auth:      WARN  no token configured for this node
Broadcast: OK  memory
ShortTerm: OK  memory
LongTerm:  SKIP  not configured
```

---

## 4. Server Verbose Mode

### `taskcast start --verbose`

Add `--verbose` / `-v` flag to the `start` command. When enabled, log every HTTP request and event publication to stdout:

```
[2026-03-07 14:32:01] POST   /tasks                    → 201  12ms  (task: 01JXXXXX)
[2026-03-07 14:32:02] PATCH  /tasks/01JXXXXX/status    → 200   3ms  (pending → running)
[2026-03-07 14:32:02] POST   /tasks/01JXXXXX/events    → 200   2ms  (type: llm.delta, series: response)
[2026-03-07 14:32:03] GET    /tasks/01JXXXXX/events    → SSE   0ms  (subscriber connected)
[2026-03-07 14:32:05] PATCH  /tasks/01JXXXXX/status    → 200   4ms  (running → completed)
[2026-03-07 14:32:05]        SSE closed for 01JXXXXX          (reason: completed, subscribers: 0)
```

Implementation:
- **TypeScript:** Hono middleware that logs method, path, status, duration, and context-specific details.
- **Rust:** Axum Tower middleware (or tracing layer) with the same output format.

Both implementations must produce identical log format.

---

## 5. New Server Endpoints

### `GET /health/detail`

Returns detailed health info for `taskcast doctor`. Requires admin token or no auth.

```json
{
  "version": "0.3.1",
  "uptime": 3600,
  "auth": { "mode": "jwt" },
  "adapters": {
    "broadcast": { "provider": "redis", "status": "ok" },
    "shortTermStore": { "provider": "redis", "status": "ok" },
    "longTermStore": { "provider": "postgres", "status": "ok" }
  }
}
```

Each adapter reports its provider type and connectivity status. On error:

```json
{
  "adapters": {
    "broadcast": { "provider": "redis", "status": "error", "error": "ECONNREFUSED" }
  }
}
```

### `GET /events` (Global SSE)

Fan-out SSE stream for all tasks. Used by `taskcast tail`.

Query params: same as task-level SSE (`types`, `levels`) plus optional `taskTypes` filter.

Each SSE event includes `taskId` in the envelope so the client can distinguish sources.

Must be implemented in both TypeScript and Rust servers.

---

## 6. Skill Updates

Update `docs/skill/taskcast.md` with three new sections appended after the existing content:

### Debugging Section

Quick checks with CLI commands, common error messages and their causes, state machine reference, SSE behavior gotchas.

### Agent Workflow Patterns

Code snippets for three patterns:
1. **Agent as producer** — create task, transition to running, stream events, complete
2. **Agent as orchestrator** — create multiple subtasks, poll/subscribe status
3. **Error recovery** — catch errors, transition to failed with context

### Node Management Quick Reference

How to add nodes, switch between them, use different auth types.

---

## 7. Testing Strategy

### TypeScript Tests (`packages/cli/tests/`)

| Layer | Scope | Approach |
|-------|-------|----------|
| **Unit** | `node-config.ts` — read/write/validate nodes.json | Temp directory, pure file IO |
| **Unit** | `client.ts` — token type handling, admin token exchange | Mock HTTP responses |
| **Unit** | Command output formatting (table, log lines) | Call format functions directly |
| **Integration** | Each client command end-to-end | Start in-memory Taskcast server in test, run command, assert stdout |
| **Integration** | `--verbose` log output | Start server with verbose, send requests, check stdout contains expected log lines |
| **Error** | Node not found, connection refused, token expired, 403 | Mock various HTTP error responses |

### Rust Tests (`rust/taskcast-cli/`)

| Layer | Scope | Approach |
|-------|-------|----------|
| **Unit** | `node_config.rs` — read/write/validate | Temp directory |
| **Unit** | `client.rs` — token handling, request building | Mock HTTP (wiremock or similar) |
| **Integration** | CLI commands via `assert_cmd` + `predicates` | Start test server, run binary with args, assert output |
| **Integration** | Verbose mode logging | Same approach as TypeScript |
| **Error** | Same error cases as TypeScript | Mock HTTP errors |

### Server Endpoint Tests

| Endpoint | TypeScript | Rust |
|----------|-----------|------|
| `GET /health/detail` | Unit test in `packages/server/tests/` | Unit test in `taskcast-server/tests/` |
| `GET /events` (global SSE) | Unit + integration test | Unit + integration test |

---

## 8. Implementation Order

1. **Server endpoints first** — `/health/detail` and `GET /events` (both TS + Rust)
2. **Node config** — `node-config.ts` / `node_config.rs` + `client.ts` / `client.rs`
3. **Refactor existing commands** — Move `start`, `migrate`, `playground`, `ui` into `commands/` files
4. **Client commands** — `ping`, `doctor`, `tasks list`, `tasks inspect`, `logs`, `tail`
5. **Node management commands** — `node add/remove/use/list`
6. **Verbose mode** — `--verbose` middleware for `start`
7. **Skill update** — Append debugging, agent patterns, and node management sections
8. **Tests** — Unit and integration tests throughout (TDD where practical)

Each step must update both TypeScript and Rust implementations before moving to the next.
