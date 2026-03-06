# Swagger / OpenAPI Support Design

**Date:** 2026-03-05
**Status:** Approved

## Goal

Add OpenAPI 3.1 spec generation and Swagger UI to both the TypeScript (Hono) and Rust (Axum) server implementations. Serves two purposes:

1. **Developer debugging** — interactive UI for testing API requests locally
2. **External documentation** — publishable API reference for integrators

## Architecture

Both implementations generate their OpenAPI spec from code (not a shared static file). This keeps spec and implementation permanently in sync.

```
TypeScript (Hono)                    Rust (Axum)
─────────────────                    ──────────────
@hono/zod-openapi                    utoipa + utoipa-axum
  ↓                                    ↓
Zod schemas → OpenAPI spec           #[utoipa] macros → OpenAPI spec
  ↓                                    ↓
Scalar UI middleware                 utoipa-swagger-ui
  ↓                                    ↓
GET /docs → Interactive UI            GET /docs → Interactive UI
GET /openapi.json → JSON spec        GET /openapi.json → JSON spec
```

## Endpoint Convention

Both implementations expose the same paths:

| Path | Purpose |
|------|---------|
| `GET /openapi.json` | OpenAPI 3.1 JSON spec |
| `GET /docs` | Swagger/Scalar interactive UI |

## TypeScript Implementation

### Libraries

- `@hono/zod-openapi` — Hono's official OpenAPI integration, reuses existing Zod schemas
- `@scalar/hono-api-reference` — modern API docs UI (replaces classic Swagger UI)

### Migration Steps

1. Replace `new Hono()` with `new OpenAPIHono()` in app factory
2. Convert route registrations from `app.post('/tasks', handler)` to `createRoute()` + `app.openapi(route, handler)` pattern
3. Define request/response Zod schemas for each route (most already exist in route handlers)
4. Mount Scalar UI at `/docs` and spec at `/openapi.json`

### Route Coverage

All REST routes will be documented:

| Route | Method | Description |
|-------|--------|-------------|
| `/health` | GET | Health check |
| `/tasks` | POST | Create task |
| `/tasks/:taskId` | GET | Get task |
| `/tasks/:taskId/status` | PATCH | Update task status |
| `/tasks/:taskId/events` | POST | Publish events |
| `/tasks/:taskId/events` | GET | SSE event stream (noted as streaming) |
| `/tasks/:taskId/events/history` | GET | Query event history |
| `/workers` | GET | List workers |
| `/workers/:workerId` | GET | Get worker |
| `/workers/:workerId` | DELETE | Delete worker |
| `/workers/pull` | GET | Long-poll task assignment |
| `/workers/tasks/:taskId/decline` | POST | Worker decline task |

### Auth Documentation

- Declare `BearerAuth` security scheme (JWT)
- Each route annotated with required permission scope(s)
- Routes that require auth marked with security requirement

## Rust Implementation

### Libraries

- `utoipa` — OpenAPI spec generation via derive macros
- `utoipa-axum` — Axum integration for route-level annotations
- `utoipa-swagger-ui` — Swagger UI serving middleware (or `utoipa-scalar` if available)

### Migration Steps

1. Add `#[derive(ToSchema)]` to all request/response types (Task, TaskEvent, Worker, etc.)
2. Add `#[derive(IntoParams)]` to query parameter structs
3. Add `#[utoipa::path(...)]` macro to all handler functions with params, request body, responses
4. Create `#[derive(OpenApi)]` struct aggregating all paths and schemas
5. Mount Swagger UI at `/docs` and spec at `/openapi.json`

### Type Annotations

Key types needing `ToSchema`:
- `Task`, `TaskStatus`, `TaskError`, `AssignMode`, `DisconnectPolicy`
- `TaskEvent`, `Level`, `SeriesMode`
- `Worker`, `WorkerStatus`, `WorkerMatchRule`
- `CreateTaskInput`, `PublishEventInput`, `TransitionInput`
- All response envelope types

## Out of Scope

- **WebSocket endpoints** — OpenAPI has limited WebSocket support; not worth forcing
- **Automated spec compatibility testing** between TS and Rust (future enhancement)
- **Code generation from spec** — we generate spec from code, not the reverse

## Testing

- Verify `/openapi.json` returns valid OpenAPI 3.1 JSON (schema validation test)
- Verify `/docs` returns HTML (basic smoke test)
- Both TS and Rust should pass the same validation
