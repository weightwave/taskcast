# Taskcast — Claude Code Instructions

## What Is This

Taskcast is a unified long-lifecycle task tracking service for LLM streaming, agents, and similar async workloads. pnpm monorepo, 9 packages, TypeScript + ESM.

## Commands

```bash
pnpm install          # Install all deps
pnpm build            # Build all packages (tsc -b)
pnpm test             # Run all tests (vitest)
pnpm test:coverage    # Coverage report
pnpm lint             # Type check (tsc -b)
```

Run a single package's tests:

```bash
cd packages/core && pnpm test
```

## Package Map

| Package | Path | Purpose |
|---------|------|---------|
| `@taskcast/core` | `packages/core` | Task engine, state machine, filtering, series merging. Zero HTTP deps. |
| `@taskcast/server` | `packages/server` | Hono HTTP server — REST + SSE + auth + webhooks |
| `@taskcast/server-sdk` | `packages/server-sdk` | HTTP client for remote server mode |
| `@taskcast/client` | `packages/client` | Browser SSE subscription client |
| `@taskcast/react` | `packages/react` | React hook `useTaskEvents` |
| `@taskcast/cli` | `packages/cli` | `npx taskcast` standalone server |
| `@taskcast/redis` | `packages/redis` | Redis broadcast + short-term store adapters |
| `@taskcast/postgres` | `packages/postgres` | PostgreSQL long-term store adapter |
| `@taskcast/sentry` | `packages/sentry` | Sentry error monitoring hooks |

## Key Files

- `packages/core/src/engine.ts` — TaskEngine orchestration
- `packages/core/src/types.ts` — all type definitions (Task, TaskEvent, interfaces)
- `packages/core/src/state-machine.ts` — status transition validation
- `packages/core/src/filter.ts` — event filtering (wildcard, level, since)
- `packages/core/src/series.ts` — series merging (accumulate/latest/keep-all)
- `packages/core/src/cleanup.ts` — cleanup rule matching
- `packages/core/src/config.ts` — config file loading + env var interpolation
- `packages/core/src/memory-adapters.ts` — in-memory adapters for testing
- `packages/server/src/index.ts` — createTaskcastApp factory
- `packages/server/src/routes/tasks.ts` — REST endpoints
- `packages/server/src/routes/sse.ts` — SSE streaming
- `packages/server/src/auth.ts` — JWT auth middleware
- `packages/server/src/webhook.ts` — webhook delivery + HMAC + retry
- `packages/cli/src/index.ts` — CLI entry point

## Design Principles

### SDK-First Architecture

Core logic (`@taskcast/core`) has **zero HTTP/infrastructure dependencies**. The HTTP layer (`@taskcast/server`) is a thin wrapper. Storage adapters are pluggable via interfaces. This means:

- The engine can be embedded into any server framework
- Storage backends can be swapped without changing business logic
- Testing is simple — use in-memory adapters for unit tests

### Three-Layer Storage

Each layer has a distinct responsibility and can be independently configured:

1. **BroadcastProvider** — Real-time event fan-out. Fire-and-forget. (Redis pub/sub or memory)
2. **ShortTermStore** — Event buffer + task state. Sync writes ensure ordering. (Redis or memory)
3. **LongTermStore** — Permanent archive. Async writes, non-blocking. (PostgreSQL, optional)

**Write path:** `publish → series merge → ShortTerm (sync) → Broadcast (sync) → LongTerm (async)`

### Concurrent Safety

- Task status transitions use optimistic concurrency — if two requests race to complete a task, only one succeeds
- The state machine validates all transitions at the engine level, not just at the API boundary
- Series message merging is atomic within the engine

## Coding Conventions

- **ESM only** — all packages use `"type": "module"` and `.js` extensions in imports
- **Workspace refs** — internal deps use `workspace:*`
- **No default exports** — everything is named exports
- **Zod validation** — input validation at boundaries uses Zod schemas
- **ULID IDs** — all generated IDs use ULID via `ulidx`
- **camelCase JSON** — all API responses use camelCase field names
- **Hono framework** — HTTP layer uses Hono, not Express

## Testing Philosophy

**Coverage target: 100% where practical. Minimum: 90%.**

- **Every bug must produce a regression test** — when you fix a bug, write a test that would have caught it first
- **Test bad cases thoroughly** — don't just test the happy path. Test invalid inputs, edge cases, race conditions, error states, boundary values, empty inputs, and overflows
- **Unit tests** — pure logic tests using in-memory adapters. No IO, no containers. Fast.
- **Integration tests** — use testcontainers for real Redis/Postgres. Test actual adapter behavior.
- **Concurrent tests** — verify safety under parallel access (e.g., 100 SSE subscribers, 10 concurrent status transitions)
- **Code that truly doesn't need testing** (trivial re-exports, type definitions) can be excluded, but everything else must be covered
- Always assert both the success case AND the rejection/error case

### Test File Structure

```
packages/<pkg>/tests/
  unit/           # Pure logic, no IO, memory adapters
  integration/    # Real Redis/Postgres via testcontainers
```

## Architecture Quick Ref

```
Task lifecycle: pending → running → completed|failed|timeout|cancelled
  - No backward transitions
  - Only one terminal transition allowed (concurrent-safe)
  - TTL triggers automatic timeout

Event filtering: wildcard type matching (e.g. "llm.*"), level filtering, since cursor
Series modes: keep-all | accumulate (text concat) | latest (replace)

SSE behavior:
  pending  → hold, auto-stream when running
  running  → replay history + stream live
  terminal → replay history, then close

Auth modes: none | jwt | custom
Permission scopes: task:create, task:manage, event:publish, event:subscribe, event:history, webhook:create, *
```

## Documentation Map

| Location | Content |
|----------|---------|
| `docs/plan.md` | Original project vision (Chinese) |
| `docs/plans/` | Design specs and implementation plans |
| `docs/plans/2026-02-28-taskcast-design.md` | **Full design document** — the source of truth |
| `docs/plans/2026-02-28-rust-rewrite-design.md` | Planned Rust rewrite design |
| `docs/guide/` | Human-readable guides (EN default, `.zh.md` for Chinese) |
| `docs/api/` | API reference (EN default, `.zh.md` for Chinese) |
| `docs/skill/taskcast.md` | Claude Code skill for external projects |

## Rust Rewrite (Planned)

Server-side packages (core, server, cli, redis, postgres) have a planned Rust rewrite using Axum + Tokio + sqlx. Client-side packages (client, react, server-sdk, sentry) stay TypeScript. See `docs/plans/2026-02-28-rust-rewrite-design.md`.

The Rust server must produce **identical HTTP behavior** — same paths, same JSON format, same SSE events, same status codes.

**IMPORTANT: When changing any server-side feature, both the Node.js (TypeScript) and Rust implementations MUST be updated simultaneously.** Do not merge a feature change that only modifies one side — the two implementations must stay in sync at all times.
