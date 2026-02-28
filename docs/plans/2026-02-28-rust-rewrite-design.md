# Taskcast Rust Server Rewrite — Design Document

**Date**: 2026-02-28
**Status**: Approved
**Scope**: Server-side only (cli + server + core + postgres + redis)

## Motivation

- **Memory**: Reduce runtime memory footprint (Node.js baseline ~50-80MB → Rust ~5-10MB)
- **Performance**: Lower latency and higher throughput for event streaming
- **Technical exploration**: Learn Rust with a real production codebase

## Scope

### In scope (Rust rewrite)
- `packages/core` → `rust/taskcast-core`
- `packages/server` → `rust/taskcast-server`
- `packages/cli` → `rust/taskcast-cli`
- `packages/postgres` → `rust/taskcast-postgres`
- `packages/redis` → `rust/taskcast-redis`

### Out of scope (keep TypeScript)
- `packages/client` — Browser SSE client
- `packages/react` — React hook
- `packages/server-sdk` — Node.js HTTP client
- `packages/sentry` — Sentry hooks (TS-only concern, not needed in Rust binary)

## Architecture

### Cargo Workspace Layout

```
rust/
├── Cargo.toml                    # workspace definition
├── taskcast-core/
│   ├── Cargo.toml
│   └── src/
│       ├── lib.rs
│       ├── types.rs              # Task, TaskEvent, TaskStatus, enums
│       ├── state_machine.rs      # transition validation
│       ├── engine.rs             # TaskEngine orchestration
│       ├── filter.rs             # event filtering (type wildcard, level, since)
│       ├── series.rs             # series processing (keep-all, accumulate, latest)
│       ├── cleanup.rs            # cleanup rule matching
│       ├── config.rs             # config parsing (YAML/JSON + env var interpolation)
│       └── adapters.rs           # trait definitions
│
├── taskcast-server/
│   ├── Cargo.toml
│   └── src/
│       ├── lib.rs
│       ├── app.rs                # Axum Router assembly
│       ├── auth.rs               # JWT middleware (jsonwebtoken crate)
│       ├── routes/
│       │   ├── tasks.rs          # REST: POST/GET/PATCH + events
│       │   └── sse.rs            # SSE streaming (axum::response::Sse)
│       ├── webhook.rs            # webhook delivery + HMAC + retry
│       └── error.rs              # unified error handling
│
├── taskcast-postgres/
│   ├── Cargo.toml
│   └── src/
│       ├── lib.rs
│       └── store.rs              # sqlx LongTermStore implementation
│
├── taskcast-redis/
│   ├── Cargo.toml
│   └── src/
│       ├── lib.rs
│       ├── broadcast.rs          # redis pub/sub BroadcastProvider
│       └── short_term.rs         # redis ShortTermStore
│
└── taskcast-cli/
    ├── Cargo.toml
    └── src/
        └── main.rs               # clap CLI + server bootstrap
```

## Type Mapping (TS → Rust)

| TypeScript | Rust | Notes |
|---|---|---|
| `TaskStatus` enum string | `enum TaskStatus` | `#[serde(rename_all = "camelCase")]` |
| `Task` interface | `struct Task` | optional fields → `Option<T>` |
| `TaskEvent` interface | `struct TaskEvent` | `data: serde_json::Value` |
| `TaskError` | `struct TaskError` | code + message + details |
| `BroadcastProvider` | `trait BroadcastProvider` | async trait |
| `ShortTermStore` | `trait ShortTermStore` | async trait |
| `LongTermStore` | `trait LongTermStore` | async trait |
| `TaskcastHooks` | `trait TaskcastHooks` | optional callback trait |
| Zod schemas | serde + custom validation | validation at deserialization |

## Technology Choices

| Feature | Rust Crate | Replaces (TS) |
|---|---|---|
| HTTP framework | **axum** + **tokio** | hono + @hono/node-server |
| SSE | **axum::response::Sse** | manual implementation |
| JWT | **jsonwebtoken** | jose |
| PostgreSQL | **sqlx** (async, compile-time) | postgres |
| Redis | **redis** (tokio feature) | ioredis |
| CLI args | **clap** (derive) | commander |
| YAML parsing | **serde_yaml** | js-yaml |
| JSON | **serde_json** | built-in |
| ULID | **ulid** | ulidx |
| HMAC signing | **hmac** + **sha2** | crypto (Node built-in) |
| HTTP client | **reqwest** | fetch |
| Env vars | **dotenvy** | process.env |

## Core Trait Definitions

```rust
use async_trait::async_trait;

#[async_trait]
pub trait BroadcastProvider: Send + Sync {
    async fn publish(&self, task_id: &str, event: &TaskEvent) -> Result<()>;
    async fn subscribe(&self, task_id: &str) -> Result<broadcast::Receiver<TaskEvent>>;
}

#[async_trait]
pub trait ShortTermStore: Send + Sync {
    async fn save_task(&self, task: &Task) -> Result<()>;
    async fn get_task(&self, task_id: &str) -> Result<Option<Task>>;
    async fn append_event(&self, task_id: &str, event: &TaskEvent) -> Result<()>;
    async fn get_events(&self, task_id: &str, since: Option<&SinceCursor>) -> Result<Vec<TaskEvent>>;
    async fn set_ttl(&self, task_id: &str, ttl_secs: u64) -> Result<()>;
    async fn get_series_latest(&self, task_id: &str, series_id: &str) -> Result<Option<TaskEvent>>;
    async fn set_series_latest(&self, task_id: &str, series_id: &str, event: &TaskEvent) -> Result<()>;
    async fn replace_last_series_event(&self, task_id: &str, series_id: &str, event: &TaskEvent) -> Result<()>;
}

#[async_trait]
pub trait LongTermStore: Send + Sync {
    async fn save_task(&self, task: &Task) -> Result<()>;
    async fn get_task(&self, task_id: &str) -> Result<Option<Task>>;
    async fn save_event(&self, task_id: &str, event: &TaskEvent) -> Result<()>;
    async fn get_events(&self, task_id: &str, opts: &GetEventsOpts) -> Result<Vec<TaskEvent>>;
}
```

## SSE Implementation

Axum natively supports SSE via `axum::response::Sse`:

```rust
async fn sse_handler(
    State(engine): State<Arc<TaskEngine>>,
    Path(task_id): Path<String>,
    Query(filter): Query<SseFilter>,
    auth: AuthContext,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    // 1. Replay historical events (filtered)
    // 2. Subscribe to new events via broadcast channel
    // 3. Merge history + live into unified Stream
    // 4. Apply filters (type, level, since)
    // 5. Send taskcast.done and close on terminal status
}
```

## Authentication

```rust
async fn auth_middleware(
    State(auth_config): State<AuthConfig>,
    mut req: Request,
    next: Next,
) -> Response {
    let auth_context = match auth_config.mode {
        AuthMode::None => AuthContext::open(),
        AuthMode::Jwt(ref jwt_config) => validate_jwt(&req, jwt_config)?,
        // Custom auth not needed for CLI binary
    };
    req.extensions_mut().insert(auth_context);
    next.run(req).await
}
```

## API Compatibility

The Rust server MUST produce identical HTTP behavior:

- **Paths**: `/tasks`, `/tasks/:taskId`, `/tasks/:taskId/status`, `/tasks/:taskId/events`, `/tasks/:taskId/events/history`
- **JSON format**: camelCase field names via `#[serde(rename_all = "camelCase")]`
- **SSE events**: `taskcast.event` and `taskcast.done` event types
- **Auth headers**: `Authorization: Bearer <token>`
- **Error responses**: same JSON error shape
- **Status codes**: identical HTTP status codes for each endpoint

## Testing Strategy

### Rust Unit Tests (per crate)
- `taskcast-core`: state machine transitions, filter matching, series processing, engine logic
- `taskcast-server`: route handlers (axum::test), auth middleware, webhook HMAC
- `taskcast-postgres`: sqlx tests with testcontainers-rs
- `taskcast-redis`: redis tests with testcontainers-rs

### TypeScript Integration Tests (reuse existing)
- Start Rust binary → call API via `server-sdk` → verify identical behavior
- SSE streaming tests via `client` package against Rust server
- Existing `engine-full.test.ts` and `concurrent.test.ts` can target Rust server
- This ensures 100% API compatibility with the TypeScript implementation

## Database Compatibility

The Rust server uses the same PostgreSQL schema (`migrations/001_initial.sql`):
- `taskcast_tasks` table
- `taskcast_events` table
- Same indexes and constraints
- sqlx migrations can reuse the same SQL files

Redis key format is also identical:
- `{prefix}:task:{id}` for tasks
- `{prefix}:events:{id}` for event lists
- `{prefix}:series:{taskId}:{seriesId}` for series tracking
- `{prefix}:seriesIds:{taskId}` for series ID sets

## Estimated Complexity

| Crate | Estimated LOC | Difficulty |
|---|---|---|
| taskcast-core | ~800 | Medium (state machine + engine logic) |
| taskcast-server | ~600 | Medium-High (SSE + auth + webhook) |
| taskcast-postgres | ~200 | Low (straightforward sqlx queries) |
| taskcast-redis | ~300 | Medium (pub/sub + series ops) |
| taskcast-cli | ~100 | Low (clap + server bootstrap) |
| **Total** | **~2000** | |