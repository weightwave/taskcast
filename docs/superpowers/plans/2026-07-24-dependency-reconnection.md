# Dependency Reconnection Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make both Taskcast server runtimes recover Redis and PostgreSQL connectivity in place, restore Redis PubSub subscriptions, expose dependency-aware readiness, and never replay an ambiguous business operation.

**Architecture:** Add a small cross-runtime dependency observation/error contract in core, with the mutable registry and health probes owned by each HTTP server. The managed CLI paths create one Redis command client/manager, one PubSub client/supervisor, and the existing PostgreSQL pools with explicit limits; low-level adapter constructors remain available. Native clients own connection replacement, while Taskcast owns PubSub supervision, readiness state, safe logs, and connectivity-error-to-503 classification.

**Tech Stack:** TypeScript, Hono, ioredis 5.x, postgres.js 3.x, Vitest, Rust, Axum, Tokio, redis-rs 0.27.6, sqlx 0.8.6, testcontainers, pnpm, Cargo, Changesets.

## Global Constraints

- Update the Node.js/TypeScript and Rust server implementations in the same change.
- Keep redis-rs pinned to 0.27.6; enable `connection-manager` without upgrading the crate.
- Create exactly one general Redis command connection per managed Taskcast instance and share it between short-term storage, publishing, and readiness.
- Keep Redis PubSub on a separate connection.
- Configure Redis command reconnection with factor 2, two retries after the immediate attempt, a nominal 2-second maximum delay, 2-second connection timeout, 10-second response timeout, and jitter.
- Use a 15-second overall Redis startup deadline for command connection, `PING`, and initial `PSUBSCRIBE`.
- Use equal-jitter Redis PubSub reconnection with a 500-millisecond base and 10-second cap.
- Configure TypeScript command reconnection with a 5-second cap, `enableOfflineQueue: false`, `autoResendUnfulfilledCommands: false`, and `maxRetriesPerRequest: 0`.
- Do not add application-level Redis command retries, SQL retries, HTTP retries, or ambiguous-write replay.
- Fail startup only for dependencies activated by the resolved adapter configuration.
- Keep `/health` I/O-free and Live during dependency outages.
- Add unauthenticated `/health/ready`; check active dependencies concurrently under one 2-second overall deadline.
- Keep `/health/detail` backward compatible and add only sanitized dependency fields.
- Default PostgreSQL pools to 10 connections, minimum 0, and 5-second acquire/connect deadlines; accept `TASKCAST_POSTGRES_MAX_CONNECTIONS` only as a positive integer.
- Emit structured state-transition logs without URLs, hosts, ports, credentials, raw errors, SQL, Redis arguments, authorization data, or payloads.
- Add regression tests before production code and observe each new test fail for the expected missing behavior.
- Preserve existing raw Redis adapter constructors and all REST/SSE/task/event schemas.
- Make no Coffice Kubernetes manifest change in this repository; execute the separate GitOps rollout plan after a release image exists.

---

## File Map

### New TypeScript files

- `packages/core/src/dependency.ts` — dependency names, low-cardinality error kinds, observations, observer interface, and typed unavailability error.
- `packages/core/tests/unit/dependency.test.ts` — error-chain and serialization contract tests.
- `packages/server/src/dependency-health.ts` — registry, readiness checks, state snapshots, transition logs, and outage-summary throttling.
- `packages/server/tests/dependency-health.test.ts` — registry, readiness deadline, endpoint, sanitization, and public-route tests.
- `packages/redis/src/managed.ts` — managed ioredis client construction, startup deadline, command options, event observation, checks, and shutdown.
- `packages/redis/src/backoff.ts` — deterministic equal-jitter calculation.
- `packages/redis/tests/managed.test.ts` — command-client options, startup, sharing, backoff, shutdown, and reconnect tests.
- `packages/redis/tests/helpers/tcp-fault-proxy.ts` — controllable TCP forwarding, connection counting, outage, and drop-response modes.
- `packages/postgres/src/health.ts` — postgres.js connectivity classification and operation observation.
- `packages/postgres/tests/health.test.ts` — classifier and observer tests.
- `packages/cli/tests/integration/dependency-startup.test.ts` — active/inactive dependency startup regressions.

### New Rust files

- `rust/taskcast-core/src/dependency.rs` — Rust equivalent of the dependency contract and typed error.
- `rust/taskcast-core/tests/dependency.rs` — error-source-chain and serde contract tests.
- `rust/taskcast-server/src/dependency_health.rs` — registry, async checks, snapshots, logs, and readiness evaluation.
- `rust/taskcast-server/tests/dependency_health.rs` — registry and endpoint parity tests.
- `rust/taskcast-redis/src/connection.rs` — raw/managed Redis command connection abstraction and connectivity classification.
- `rust/taskcast-redis/src/pubsub.rs` — supervised pattern subscription and equal-jitter retry loop.
- `rust/taskcast-redis/tests/reconnect.rs` — manager recovery, sharing, PubSub recovery, and shutdown tests.
- `rust/taskcast-redis/tests/support/mod.rs` — controllable TCP fault proxy for Redis integration tests.
- `rust/taskcast-postgres/src/health.rs` — sqlx connectivity classification and observation helper.
- `rust/taskcast-postgres/tests/health.rs` — classifier, pool limit, and recovery tests.
- `rust/taskcast-cli/tests/dependency_startup.rs` — active/inactive dependency startup regressions.

### Modified files

- `packages/core/src/index.ts`, `rust/taskcast-core/src/lib.rs` — export the shared dependency contract.
- `packages/server/src/index.ts`, `packages/server/src/schemas.ts` — install the registry, health endpoints, root link, and 503 helper.
- `rust/taskcast-server/src/app.rs`, `rust/taskcast-server/src/error.rs`, `rust/taskcast-server/src/lib.rs` — Rust health wiring and typed 503 mapping.
- `packages/redis/src/index.ts`, `packages/redis/src/broadcast.ts`, `packages/redis/src/short-term.ts` — managed factory, wildcard subscriber mode, and typed connectivity errors.
- `rust/taskcast-redis/Cargo.toml`, `rust/taskcast-redis/src/lib.rs`, `rust/taskcast-redis/src/broadcast.rs`, `rust/taskcast-redis/src/short_term.rs` — connection manager, supervisor, managed factory, and preserved raw constructors.
- `packages/postgres/src/index.ts`, `packages/postgres/src/long-term.ts` — observed operations and health exports.
- `rust/taskcast-postgres/src/lib.rs`, `rust/taskcast-postgres/src/store.rs` — observed operations and pool access.
- `packages/cli/src/commands/start.ts`, `packages/cli/tests/unit/start-command.test.ts` — exact adapter activation, managed clients, PostgreSQL configuration, startup checks, and cleanup.
- `rust/taskcast-cli/src/commands/start.rs`, `rust/taskcast-cli/src/helpers.rs`, `rust/taskcast-cli/tests/start_env_tests.rs` — Rust-equivalent activation and managed construction.
- `packages/server/src/routes/tasks.ts`, `packages/server/src/routes/workers.ts` — preserve existing error envelopes while returning 503 for typed dependency failures.
- `packages/server/tests/health-detail.test.ts`, `packages/server/tests/tasks.test.ts`, `packages/server/tests/workers.test.ts`, `rust/taskcast-server/tests/health_detail.rs`, `rust/taskcast-server/tests/server_tests.rs` — parity regressions.
- `README.md`, `README.zh.md`, `packages/cli/README.md`, `docs/guide/deployment.md`, `docs/guide/deployment.zh.md` — configuration, health, and operational behavior.
- `rust/Cargo.lock`, `pnpm-lock.yaml` — dependency feature/lock updates generated by package managers.
- `.changeset/calm-stores-reconnect.md` — fixed-version patch release note.

---

### Task 1: Shared Dependency Observation and Error Contract

**Files:**

- Create: `packages/core/src/dependency.ts`
- Create: `packages/core/tests/unit/dependency.test.ts`
- Modify: `packages/core/src/index.ts`
- Create: `rust/taskcast-core/src/dependency.rs`
- Create: `rust/taskcast-core/tests/dependency.rs`
- Modify: `rust/taskcast-core/src/lib.rs`

**Interfaces:**

- Produces equivalent TypeScript and Rust definitions:
  - `DependencyName`: `redisCommand`, `redisPubSub`, `postgres`
  - `DependencyState`: `starting`, `healthy`, `reconnecting`, `unhealthy`
  - `DependencyErrorKind`: `connection_refused`, `connection_reset`, `timeout`, `dns`, `authentication`, `connection_closed`, `unavailable`
  - `DependencyObservation`
  - `DependencyObserver`
  - `DependencyUnavailableError`
  - `findDependencyUnavailableError` / `find_dependency_unavailable`
- Consumed by every later task.

- [ ] **Step 1: Write failing TypeScript contract tests**

Create `packages/core/tests/unit/dependency.test.ts`:

```ts
import { describe, expect, it, vi } from 'vitest'
import {
  DependencyUnavailableError,
  findDependencyUnavailableError,
  type DependencyObservation,
  type DependencyObserver,
} from '../../src/index.js'

describe('dependency contract', () => {
  it('finds a typed dependency error through a cause chain', () => {
    const cause = new Error('redis://user:secret@redis:6379 socket closed')
    const unavailable = new DependencyUnavailableError(
      'redisCommand',
      'connection_reset',
      cause,
    )
    const outer = new Error('store failed', { cause: unavailable })

    expect(findDependencyUnavailableError(outer)).toBe(unavailable)
    expect(unavailable.message).toBe(
      'redisCommand unavailable (connection_reset)',
    )
    expect(unavailable.cause).toBe(cause)
  })

  it('returns undefined for cycles and ordinary errors', () => {
    const cyclic = new Error('ordinary') as Error & { cause?: unknown }
    cyclic.cause = cyclic
    expect(findDependencyUnavailableError(cyclic)).toBeUndefined()
    expect(findDependencyUnavailableError('not an error')).toBeUndefined()
  })

  it('keeps observations low-cardinality', () => {
    const observe = vi.fn()
    const observer: DependencyObserver = { observe }
    const observation: DependencyObservation = {
      dependency: 'redisPubSub',
      state: 'reconnecting',
      errorKind: 'connection_closed',
      attempt: 3,
      nextRetryMs: 1750,
    }

    observer.observe(observation)

    expect(observe).toHaveBeenCalledWith(observation)
    expect(JSON.stringify(observation)).not.toContain('redis://')
  })
})
```

- [ ] **Step 2: Write failing Rust contract tests**

Create `rust/taskcast-core/tests/dependency.rs`:

```rust
use std::error::Error;
use std::io;

use taskcast_core::{
    find_dependency_unavailable, DependencyErrorKind, DependencyName,
    DependencyObservation, DependencyState, DependencyUnavailableError,
};

#[test]
fn finds_dependency_error_through_source_chain() {
    let unavailable = DependencyUnavailableError::new(
        DependencyName::RedisCommand,
        DependencyErrorKind::ConnectionReset,
        io::Error::new(io::ErrorKind::ConnectionReset, "secret raw error"),
    );
    let outer = io::Error::new(io::ErrorKind::Other, unavailable);
    let found = find_dependency_unavailable(&outer as &(dyn Error + 'static)).unwrap();

    assert_eq!(found.dependency(), DependencyName::RedisCommand);
    assert_eq!(found.kind(), DependencyErrorKind::ConnectionReset);
    assert_eq!(
        found.to_string(),
        "redisCommand unavailable (connection_reset)"
    );
}

#[test]
fn serializes_observation_with_the_public_names() {
    let observation = DependencyObservation {
        dependency: DependencyName::RedisPubSub,
        state: DependencyState::Reconnecting,
        error_kind: Some(DependencyErrorKind::ConnectionClosed),
        attempt: Some(3),
        next_retry_ms: Some(1_750),
    };
    let json = serde_json::to_value(observation).unwrap();

    assert_eq!(json["dependency"], "redisPubSub");
    assert_eq!(json["state"], "reconnecting");
    assert_eq!(json["errorKind"], "connection_closed");
    assert_eq!(json["nextRetryMs"], 1_750);
}
```

- [ ] **Step 3: Run both tests and verify RED**

Run:

```bash
pnpm --filter @taskcast/core test -- tests/unit/dependency.test.ts
cd rust
cargo test -p taskcast-core --test dependency
```

Expected: both commands fail to compile because the dependency contract does not exist.

- [ ] **Step 4: Implement and export the TypeScript contract**

Create `packages/core/src/dependency.ts` with these exact public shapes:

```ts
export type DependencyName = 'redisCommand' | 'redisPubSub' | 'postgres'
export type DependencyState =
  | 'starting'
  | 'healthy'
  | 'reconnecting'
  | 'unhealthy'
export type DependencyErrorKind =
  | 'connection_refused'
  | 'connection_reset'
  | 'timeout'
  | 'dns'
  | 'authentication'
  | 'connection_closed'
  | 'unavailable'

export interface DependencyObservation {
  dependency: DependencyName
  state: Exclude<DependencyState, 'starting'>
  errorKind?: DependencyErrorKind
  attempt?: number
  nextRetryMs?: number
}

export interface DependencyObserver {
  observe(observation: DependencyObservation): void
}

export class DependencyUnavailableError extends Error {
  override readonly cause: unknown

  constructor(
    public readonly dependency: DependencyName,
    public readonly kind: DependencyErrorKind,
    cause?: unknown,
  ) {
    super(`${dependency} unavailable (${kind})`)
    this.name = 'DependencyUnavailableError'
    this.cause = cause
  }
}

export function findDependencyUnavailableError(
  value: unknown,
): DependencyUnavailableError | undefined {
  const seen = new Set<unknown>()
  let current = value
  while (current instanceof Error && !seen.has(current)) {
    if (current instanceof DependencyUnavailableError) return current
    seen.add(current)
    current = (current as Error & { cause?: unknown }).cause
  }
  return undefined
}
```

Export it from `packages/core/src/index.ts`:

```ts
export * from './dependency.js'
```

- [ ] **Step 5: Implement and export the Rust contract**

Create `rust/taskcast-core/src/dependency.rs` with serde names matching the TypeScript strings. Use a manual `Display` for the exact safe message and retain the raw source only through `Error::source()`:

```rust
use std::error::Error;
use std::fmt;

use serde::{Deserialize, Serialize};

pub type BoxError = Box<dyn Error + Send + Sync>;

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum DependencyName {
    RedisCommand,
    RedisPubSub,
    Postgres,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum DependencyState {
    Starting,
    Healthy,
    Reconnecting,
    Unhealthy,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DependencyErrorKind {
    ConnectionRefused,
    ConnectionReset,
    Timeout,
    Dns,
    Authentication,
    ConnectionClosed,
    Unavailable,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DependencyObservation {
    pub dependency: DependencyName,
    pub state: DependencyState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_kind: Option<DependencyErrorKind>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attempt: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_retry_ms: Option<u64>,
}

pub trait DependencyObserver: Send + Sync + 'static {
    fn observe(&self, observation: DependencyObservation);
}

#[derive(Debug)]
pub struct DependencyUnavailableError {
    dependency: DependencyName,
    kind: DependencyErrorKind,
    source: BoxError,
}

impl DependencyUnavailableError {
    pub fn new(
        dependency: DependencyName,
        kind: DependencyErrorKind,
        source: impl Error + Send + Sync + 'static,
    ) -> Self {
        Self { dependency, kind, source: Box::new(source) }
    }

    pub fn dependency(&self) -> DependencyName { self.dependency }
    pub fn kind(&self) -> DependencyErrorKind { self.kind }
}

impl fmt::Display for DependencyUnavailableError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let dependency = serde_json::to_value(self.dependency)
            .expect("DependencyName serializes");
        let kind = serde_json::to_value(self.kind)
            .expect("DependencyErrorKind serializes");
        write!(
            f,
            "{} unavailable ({})",
            dependency.as_str().unwrap(),
            kind.as_str().unwrap()
        )
    }
}

impl Error for DependencyUnavailableError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(self.source.as_ref())
    }
}

pub fn find_dependency_unavailable<'a>(
    mut error: &'a (dyn Error + 'static),
) -> Option<&'a DependencyUnavailableError> {
    loop {
        if let Some(found) = error.downcast_ref::<DependencyUnavailableError>() {
            return Some(found);
        }
        error = error.source()?;
    }
}
```

Add `pub mod dependency;` and `pub use dependency::*;` to
`rust/taskcast-core/src/lib.rs`.

- [ ] **Step 6: Run both contract suites and commit**

Run:

```bash
pnpm --filter @taskcast/core test -- tests/unit/dependency.test.ts
pnpm --filter @taskcast/core build
cd rust
cargo fmt --all
cargo test -p taskcast-core --test dependency
```

Expected: all commands pass.

Commit:

```bash
git add packages/core/src/dependency.ts packages/core/src/index.ts \
  packages/core/tests/unit/dependency.test.ts \
  rust/taskcast-core/src/dependency.rs rust/taskcast-core/src/lib.rs \
  rust/taskcast-core/tests/dependency.rs
git commit -m "feat(core): define dependency failure contract"
```

---

### Task 2: Dependency Registry and Health Endpoints

**Files:**

- Create: `packages/server/src/dependency-health.ts`
- Create: `packages/server/tests/dependency-health.test.ts`
- Modify: `packages/server/src/index.ts`
- Modify: `packages/server/src/schemas.ts`
- Modify: `packages/server/tests/health-detail.test.ts`
- Create: `rust/taskcast-server/src/dependency_health.rs`
- Create: `rust/taskcast-server/tests/dependency_health.rs`
- Modify: `rust/taskcast-server/src/app.rs`
- Modify: `rust/taskcast-server/src/lib.rs`
- Modify: `rust/taskcast-server/tests/health_detail.rs`

**Interfaces:**

- Produces TypeScript `DependencyHealthRegistry`, `DependencyCheck`, `DependencyHealthLogger`, and `ReadinessResult`.
- Produces Rust `DependencyHealthRegistry`, `DependencyCheck`, `DependencyHealthLogger`, and `RuntimeHealth`.
- Both registries implement `DependencyObserver`.
- Adds `TaskcastServerOptions.dependencyHealth?: DependencyHealthRegistry`.
- Adds Rust `create_app_with_runtime_health_and_routes(...)`; existing Rust constructors call it with `RuntimeHealth::default()` so their public signatures stay intact.
- Adds root link `healthReady: "/health/ready"`.

- [ ] **Step 1: Write failing registry and endpoint tests in both runtimes**

The TypeScript test must cover:

```ts
const now = { value: 1_000 }
const records: unknown[] = []
const health = new DependencyHealthRegistry({
  now: () => now.value,
  logger: (record) => records.push(record),
})
health.register('redisCommand', async () => {})
health.register('redisPubSub', async () => {
  throw new DependencyUnavailableError(
    'redisPubSub',
    'connection_closed',
    new Error('must not leak'),
  )
})

const result = await health.checkReadiness(2_000)
expect(result.ok).toBe(false)
expect(result.dependencies.redisPubSub).toEqual({
  state: 'unhealthy',
  errorKind: 'connection_closed',
})
expect(JSON.stringify(result)).not.toContain('must not leak')
```

Also assert:

- inactive dependencies are absent and do no work;
- checks run concurrently;
- the whole call returns within the injected 2-second deadline;
- transition duplicates do not emit duplicate logs;
- a continuing outage emits at most one summary per 60 seconds;
- recovery emits `downtimeMs`;
- `/health` stays 200;
- `/health/ready` returns 503 for a failed active check and 200 after recovery;
- `/health/detail` retains existing adapter fields and derives Redis broadcast status from both Redis entries;
- all three routes bypass JWT authentication.

Create the Rust test with equivalent assertions using an injected millisecond
clock and a collecting logger. Use `axum_test::TestServer` for endpoint parity.

- [ ] **Step 2: Run the focused server tests and verify RED**

Run:

```bash
pnpm --filter @taskcast/server test -- \
  tests/dependency-health.test.ts tests/health-detail.test.ts
cd rust
cargo test -p taskcast-server --test dependency_health
cargo test -p taskcast-server --test health_detail
```

Expected: compilation fails because the registry and readiness route do not exist.

- [ ] **Step 3: Implement the TypeScript registry**

Create `packages/server/src/dependency-health.ts` with these public signatures:

```ts
import {
  DependencyUnavailableError,
  type DependencyErrorKind,
  type DependencyName,
  type DependencyObservation,
  type DependencyObserver,
  type DependencyState,
} from '@taskcast/core'

export type DependencyCheck = () => Promise<void>
export type DependencyHealthLogger = (record: Record<string, unknown>) => void

export interface DependencySnapshot {
  configured: true
  state: DependencyState
  lastTransitionAt: string
  lastErrorKind?: DependencyErrorKind
  consecutiveFailures: number
  reconnectAttempts?: number
}

export interface ReadinessResult {
  ok: boolean
  dependencies: Partial<Record<
    DependencyName,
    { state: DependencyState; errorKind?: DependencyErrorKind }
  >>
}

export class DependencyHealthRegistry implements DependencyObserver {
  constructor(options?: {
    now?: () => number
    logger?: DependencyHealthLogger
  })
  register(name: DependencyName, check: DependencyCheck): void
  observe(observation: DependencyObservation): void
  snapshot(): Partial<Record<DependencyName, DependencySnapshot>>
  async checkReadiness(timeoutMs = 2_000): Promise<ReadinessResult>
}
```

Implementation rules:

- `register` creates a `starting` entry and rejects duplicate names.
- Every check runs immediately in a single `Promise.allSettled` group.
- Race that group against one timer; do not run checks sequentially.
- A successful check records `healthy`.
- A thrown `DependencyUnavailableError` uses its kind; any other check error becomes `unavailable`.
- The timer marks unfinished checks `timeout` and returns without awaiting their eventual result.
- Log `dependency_state_change` only when state changes.
- Log `dependency_outage_summary` only after 60,000 milliseconds in the same degraded state.
- Transition records contain `timestamp`, `level`, `event`, `dependency`,
  `from`, and `to`; degraded transitions use `warn`, recovery uses `info`.
- Add `attempt`, `nextRetryMs`, `errorKind`, and `downtimeMs` only when they
  apply; never serialize the thrown object.
- Expose `reconnectAttempts` only for `redisPubSub`, because Taskcast owns that
  retry loop; use `consecutiveFailures` for command Redis and PostgreSQL.
- Default logger is `console.error(JSON.stringify(record))`.

- [ ] **Step 4: Implement the Rust registry**

Create `rust/taskcast-server/src/dependency_health.rs` with equivalent names:

```rust
pub type DependencyCheck = Arc<
    dyn Fn() -> Pin<Box<dyn Future<Output = Result<(), DependencyUnavailableError>> + Send>>
        + Send
        + Sync,
>;

pub trait DependencyHealthLogger: Send + Sync + 'static {
    fn log(&self, record: &serde_json::Value);
}

#[derive(Clone)]
pub struct DependencyHealthRegistry { /* Arc<RwLock<...>>, checks, clock, logger */ }

impl DependencyHealthRegistry {
    pub fn new() -> Self;
    pub fn with_logger(logger: Arc<dyn DependencyHealthLogger>) -> Self;
    pub fn register(&self, name: DependencyName, check: DependencyCheck) -> Result<(), String>;
    pub fn snapshot(&self) -> serde_json::Value;
    pub async fn check_readiness(&self, timeout: Duration) -> ReadinessResult;
}

impl DependencyObserver for DependencyHealthRegistry {
    fn observe(&self, observation: DependencyObservation);
}

#[derive(Clone, Default)]
pub struct RuntimeHealth {
    pub registry: Option<Arc<DependencyHealthRegistry>>,
}
```

Use `futures::future::join_all` inside one `tokio::time::timeout`. Keep
`record_at(observation, now_ms)` crate-visible so tests can verify the 60-second
summary without sleeping. The default logger writes exactly one JSON object per
line to stderr.

- [ ] **Step 5: Wire all health routes without breaking existing constructors**

In TypeScript:

```ts
export interface TaskcastServerOptions {
  // existing fields
  dependencyHealth?: DependencyHealthRegistry
}
```

Add:

```ts
app.get('/health/ready', async (c) => {
  const result = opts.dependencyHealth
    ? await opts.dependencyHealth.checkReadiness(2_000)
    : { ok: true, dependencies: {} }
  return c.json(result, result.ok ? 200 : 503)
})
```

Make `/health/detail` merge `dependencies: registry.snapshot()` and set its
top-level `ok` plus adapter statuses from the rules in the design. Keep existing
fields byte-for-byte compatible when no registry is supplied.

In Rust, add `runtime_health: RuntimeHealth` to `AppState`. Introduce
`create_app_with_runtime_health_and_routes(...)`, move the existing constructor
body into it, and have all existing constructor variants pass
`RuntimeHealth::default()`. Register:

```rust
.route("/health/ready", get(health_ready).with_state(app_state.clone()))
.route("/health/detail", get(health_detail).with_state(app_state))
```

Return `(StatusCode::SERVICE_UNAVAILABLE, Json(result))` only when readiness is
false. Keep `/health` independent of `RuntimeHealth`.

- [ ] **Step 6: Run focused tests and commit**

Run:

```bash
pnpm --filter @taskcast/server test -- \
  tests/dependency-health.test.ts tests/health-detail.test.ts
pnpm --filter @taskcast/server build
cd rust
cargo fmt --all
cargo test -p taskcast-server --test dependency_health
cargo test -p taskcast-server --test health_detail
```

Expected: all commands pass.

Commit:

```bash
git add packages/server/src/dependency-health.ts packages/server/src/index.ts \
  packages/server/src/schemas.ts packages/server/tests/dependency-health.test.ts \
  packages/server/tests/health-detail.test.ts \
  rust/taskcast-server/src/dependency_health.rs rust/taskcast-server/src/app.rs \
  rust/taskcast-server/src/lib.rs rust/taskcast-server/tests/dependency_health.rs \
  rust/taskcast-server/tests/health_detail.rs
git commit -m "feat(server): add dependency readiness health"
```

---

### Task 3: One Managed Redis Command Connection per Runtime

**Files:**

- Create: `packages/redis/src/backoff.ts`
- Create: `packages/redis/src/managed.ts`
- Create: `packages/redis/tests/managed.test.ts`
- Modify: `packages/redis/src/index.ts`
- Modify: `packages/redis/src/broadcast.ts`
- Modify: `packages/redis/src/short-term.ts`
- Modify: `rust/taskcast-redis/Cargo.toml`
- Create: `rust/taskcast-redis/src/connection.rs`
- Modify: `rust/taskcast-redis/src/lib.rs`
- Modify: `rust/taskcast-redis/src/broadcast.rs`
- Modify: `rust/taskcast-redis/src/short_term.rs`
- Modify: `rust/Cargo.lock`

**Interfaces:**

- TypeScript produces `createManagedRedisCommandClient(url, options)` and
  `ManagedRedisCommand`.
- Rust produces `create_connection_manager(client)` and the raw/managed
  `RedisCommandConnection` abstraction; the managed enum variant carries the
  optional observer alongside the manager.
- Both return a command readiness check and a shutdown/close handle.
- Existing `createRedisAdapters(...)`, `create_redis_adapters(...)`, and raw constructors remain callable.

- [ ] **Step 1: Write failing managed-command tests**

TypeScript assertions:

```ts
const managed = await createManagedRedisCommandClient(redisUrl, {
  observer,
  startupTimeoutMs: 15_000,
  random: () => 0,
})
expect(managed.client.options).toMatchObject({
  enableOfflineQueue: false,
  autoResendUnfulfilledCommands: false,
  maxRetriesPerRequest: 0,
})
await managed.check()
await managed.close()
```

Test `equalJitterDelay(500, 5_000, attempt, random)` at the lower bound,
upper bound, and cap. Verify a command submitted while the proxy is offline
rejects instead of remaining pending for later replay.

Rust assertions:

- cloned handles from `create_connection_manager` share one
  `ConnectionManager` reconnect future;
- `PING` succeeds through the command check;
- 50 concurrent commands after one dropped socket create one effective manager
  reconnect flow, measured by the TCP proxy connection counter;
- the command that sees the disconnect returns an error;
- a later command succeeds;
- raw constructors still compile and pass their existing tests.

- [ ] **Step 2: Run focused Redis tests and verify RED**

Run:

```bash
pnpm --filter @taskcast/redis test -- tests/managed.test.ts
cd rust
cargo test -p taskcast-redis --test reconnect managed_command
```

Expected: compilation fails because managed factories and backoff helpers do not exist.

- [ ] **Step 3: Implement the TypeScript managed command client**

Create `packages/redis/tests/helpers/tcp-fault-proxy.ts` before the test. For
this task it needs `open`, `refuse`, accepted-connection counting, socket
closing, and `stop`; Task 8 extends it with response dropping.

Create the Rust counterpart at
`rust/taskcast-redis/tests/support/mod.rs` with the same controls and counters.
Keep both helpers test-only; neither package may export them.

Create `packages/redis/src/backoff.ts`:

```ts
export function equalJitterDelay(
  baseMs: number,
  capMs: number,
  attempt: number,
  random = Math.random,
): number {
  const cap = Math.min(capMs, baseMs * 2 ** Math.max(0, attempt))
  return Math.floor(cap / 2 + random() * (cap / 2))
}
```

Define in `packages/redis/src/managed.ts`:

```ts
export interface ManagedRedisOptions extends RedisAdapterOptions {
  observer?: DependencyObserver
  startupTimeoutMs?: number
  random?: () => number
}

export interface ManagedRedisCommand {
  client: Redis
  check(): Promise<void>
  close(): Promise<void>
}

export async function createManagedRedisCommandClient(
  url: string,
  options: ManagedRedisOptions = {},
): Promise<ManagedRedisCommand>
```

Use this deadline helper and always clear its timer:

```ts
async function withDeadline<T>(
  operation: () => Promise<T>,
  timeoutMs: number,
): Promise<T> {
  let timer: ReturnType<typeof setTimeout> | undefined
  try {
    return await Promise.race([
      operation(),
      new Promise<never>((_, reject) => {
        timer = setTimeout(
          () => reject(new Error('Redis startup timed out')),
          timeoutMs,
        )
      }),
    ])
  } finally {
    if (timer !== undefined) clearTimeout(timer)
  }
}
```

Construct exactly one command client with:

```ts
const commandClient = new Redis(url, {
  lazyConnect: true,
  enableOfflineQueue: false,
  autoResendUnfulfilledCommands: false,
  maxRetriesPerRequest: 0,
  retryStrategy: (times) =>
    equalJitterDelay(500, 5_000, times - 1, options.random),
})
```

Attach `ready`, `reconnecting`, `close`, `end`, and `error` listeners before
`connect()`. Map only OS/socket/DNS/timeout/authentication failures to the
low-cardinality kinds. Do not include `error.message` in observations.

The command startup sequence is:

```ts
await withDeadline(async () => {
  await commandClient.connect()
  await commandClient.ping()
}, options.startupTimeoutMs ?? 15_000)
```

Expose `check` as one `PING`. `close` must remove listeners and call
`disconnect(false)` without issuing `QUIT` during an outage. Task 4 passes this
same client to the store and publisher.

- [ ] **Step 4: Implement the Rust command manager**

Enable:

```toml
redis = { version = "0.27", features = ["tokio-comp", "aio", "connection-manager"] }
```

Construct the manager with:

```rust
let config = redis::aio::ConnectionManagerConfig::new()
    .set_exponent_base(2)
    .set_factor(2)
    .set_number_of_retries(2)
    .set_max_delay(2_000)
    .set_connection_timeout(Duration::from_secs(2))
    .set_response_timeout(Duration::from_secs(10));
let manager = redis::aio::ConnectionManager::new_with_config(client.clone(), config).await?;
```

Expose:

```rust
pub async fn create_connection_manager(
    client: redis::Client,
) -> redis::RedisResult<redis::aio::ConnectionManager>

pub async fn command_check(
    manager: &redis::aio::ConnectionManager,
) -> Result<(), DependencyUnavailableError>
```

Create `RedisCommandConnection` in `connection.rs`:

```rust
#[derive(Clone)]
pub enum RedisCommandConnection {
    Raw(redis::aio::MultiplexedConnection),
    Managed {
        manager: redis::aio::ConnectionManager,
        observer: Option<Arc<dyn DependencyObserver>>,
    },
}
```

Implement `redis::aio::ConnectionLike` by delegating all three trait methods.
Change the store and publisher fields to this enum. Keep `new(raw, prefix)` and
add crate-visible `new_managed(manager, prefix, observer)`.

Classify only `RedisError::is_io_error()`, `is_connection_dropped()`,
authentication errors, connection refusal/reset, DNS, and timeout as
`DependencyUnavailableError`. Every public adapter method returns the typed
wrapper for those errors and leaves Redis command/data errors unchanged.
Report `healthy` on a successful managed operation and `reconnecting` on a
classified failure; do not retry the method body. Wrap manager construction
and its initial `PING` in a 15-second `tokio::time::timeout`; Task 4 places that
future and initial PubSub subscription under the final shared startup deadline.

- [ ] **Step 5: Run focused Redis suites and commit**

Run:

```bash
pnpm --filter @taskcast/redis test -- tests/managed.test.ts tests/short-term.test.ts
pnpm --filter @taskcast/redis build
cd rust
cargo fmt --all
cargo test -p taskcast-redis --test reconnect managed_command
cargo test -p taskcast-redis --test short_term_tests
```

Expected: all commands pass; the fault test proves recovery without replay.

Commit:

```bash
git add packages/redis/src packages/redis/tests/managed.test.ts \
  packages/redis/tests/helpers/tcp-fault-proxy.ts \
  rust/taskcast-redis/Cargo.toml rust/taskcast-redis/src \
  rust/taskcast-redis/tests/reconnect.rs \
  rust/taskcast-redis/tests/support/mod.rs rust/Cargo.lock
git commit -m "fix(redis): manage command reconnections"
```

---

### Task 4: Supervised Redis Pattern Subscription

**Files:**

- Modify: `packages/redis/src/broadcast.ts`
- Modify: `packages/redis/src/managed.ts`
- Modify: `packages/redis/src/index.ts`
- Modify: `packages/redis/tests/broadcast.test.ts`
- Modify: `packages/redis/tests/managed.test.ts`
- Create: `rust/taskcast-redis/src/pubsub.rs`
- Modify: `rust/taskcast-redis/src/broadcast.rs`
- Modify: `rust/taskcast-redis/src/lib.rs`
- Modify: `rust/taskcast-redis/tests/concurrent.rs`
- Modify: `rust/taskcast-redis/tests/reconnect.rs`

**Interfaces:**

- TypeScript `RedisBroadcastProvider.startPatternSubscription(): Promise<void>`.
- Rust `RedisPubSubHandle::{is_subscribed, shutdown}`.
- TypeScript produces `createManagedRedisAdapters(url, options)` and
  `ManagedRedisAdapters`.
- Rust produces `create_managed_redis_adapters(client, prefix, observer)` and
  `ManagedRedisAdapters`.
- Final managed factories share the Task 3 command client/manager and await
  initial wildcard subscription before returning.

- [ ] **Step 1: Add failing PubSub lifecycle tests**

Both runtimes must prove:

- initial `PSUBSCRIBE <prefix>:task:*` completes before the managed factory returns;
- the store and publisher share the exact Task 3 command client/manager;
- two Taskcast adapter instances exchange events;
- after the proxy drops the subscriber socket and later resumes forwarding, the
  same instances exchange events again;
- local handler maps survive reconnect;
- only one pattern subscription exists per instance;
- equal-jitter delay stays between `cap / 2` and `cap`, capped at 10 seconds;
- shutdown during retry cancels pending sleep and produces no further connection;
- initial unreachable Redis fails within 15 seconds.

- [ ] **Step 2: Run PubSub tests and verify RED**

Run:

```bash
pnpm --filter @taskcast/redis test -- \
  tests/broadcast.test.ts tests/managed.test.ts
cd rust
cargo test -p taskcast-redis --test reconnect pubsub
cargo test -p taskcast-redis --test concurrent cross_instance
```

Expected: reconnect tests fail because the current listener exits permanently.

- [ ] **Step 3: Implement TypeScript wildcard mode**

Keep the existing constructor signature. Add an internal mode option used only
by the managed factory:

```ts
type SubscriptionMode = 'channels' | 'pattern'

constructor(
  private pub: Redis,
  private sub: Redis,
  { prefix, subscriptionMode = 'channels' }: {
    prefix?: string
    subscriptionMode?: SubscriptionMode
  } = {},
)
```

For `pattern` mode:

- listen to `pmessage`;
- `startPatternSubscription()` awaits one `psubscribe(`${prefix}:task:*`)`;
- `subscribe()` and its unsubscribe closure modify only the local handler map;
- rely on `autoResubscribe: true` for restoration;
- expose `isPatternSubscribed()` from observed `ready`/`reconnecting`/`end`
  state.

Create the subscriber with:

```ts
new Redis(url, {
  lazyConnect: true,
  autoResubscribe: true,
  enableOfflineQueue: false,
  maxRetriesPerRequest: 0,
  retryStrategy: (times) =>
    equalJitterDelay(500, 10_000, times - 1, options.random),
})
```

Subscription restoration is connection-state restoration, not business
command replay.

Compose the TypeScript result with the exact public shape:

```ts
export interface ManagedRedisAdapters {
  broadcast: RedisBroadcastProvider
  shortTermStore: RedisShortTermStore
  commandClient: Redis
  subscriberClient: Redis
  commandCheck(): Promise<void>
  pubSubCheck(): Promise<void>
  close(): Promise<void>
}
```

`createManagedRedisAdapters` calls Task 3's command constructor, passes
`command.client` to both adapters, starts the subscriber, and closes both
clients if any part of the startup deadline fails.

Compute `deadlineAt = Date.now() + (options.startupTimeoutMs ?? 15_000)` once.
Pass only `Math.max(1, deadlineAt - Date.now())` to command construction and
then to subscriber connect/`PSUBSCRIBE`. This makes 15 seconds one overall
budget rather than two consecutive 15-second budgets. In `catch`, disconnect
the subscriber if it exists, await `command.close()` if command construction
completed, and rethrow the safe startup error.

- [ ] **Step 4: Implement the Rust PubSub supervisor**

In `pubsub.rs`, store the handler map outside each connection attempt. The
supervisor loop must follow this exact state machine:

```text
connect -> PSUBSCRIBE -> healthy -> consume messages
   |           |              |
   +--error----+------EOF-----+
                 |
             reconnecting -> equal-jitter sleep -> connect
```

Use a `tokio::sync::watch` shutdown channel and a `watch` status channel.
`tokio::select!` must race shutdown against connect, `PSUBSCRIBE`, stream
consumption, and retry sleep. After a successful subscription reset the retry
attempt to zero. Before each retry emit:

```rust
DependencyObservation {
    dependency: DependencyName::RedisPubSub,
    state: DependencyState::Reconnecting,
    error_kind: Some(kind),
    attempt: Some(attempt),
    next_retry_ms: Some(delay.as_millis() as u64),
}
```

The delay helper receives an injectable `random_unit: f64` in unit tests:

```rust
pub(crate) fn equal_jitter_delay(attempt: u32, random_unit: f64) -> Duration {
    let cap_ms = (500_u64.saturating_mul(2_u64.saturating_pow(attempt)))
        .min(10_000);
    Duration::from_millis(
        cap_ms / 2 + ((cap_ms as f64 / 2.0) * random_unit) as u64
    )
}
```

Return an initialization `oneshot` result so the managed factory can await the
first successful `PSUBSCRIBE`. The raw constructor keeps its current owned
`PubSub` behavior for compatibility.

Compose:

```rust
pub struct ManagedRedisAdapters {
    pub adapters: RedisAdapters,
    pub command_manager: redis::aio::ConnectionManager,
    pub pubsub: RedisPubSubHandle,
}

pub async fn create_managed_redis_adapters(
    client: redis::Client,
    prefix: Option<&str>,
    observer: Option<Arc<dyn DependencyObserver>>,
) -> Result<ManagedRedisAdapters, Box<dyn std::error::Error + Send + Sync>>
```

Run command-manager construction, initial `PING`, and the supervisor's initial
`PSUBSCRIBE` under one 15-second timeout. On any error, signal supervisor
shutdown before returning.

- [ ] **Step 5: Run PubSub suites and commit**

Run:

```bash
pnpm --filter @taskcast/redis test
pnpm --filter @taskcast/redis build
cd rust
cargo fmt --all
cargo test -p taskcast-redis
```

Expected: all Redis tests pass, including cross-instance delivery before and
after the forced disconnect.

Commit:

```bash
git add packages/redis/src packages/redis/tests \
  rust/taskcast-redis/src rust/taskcast-redis/tests
git commit -m "fix(redis): restore pubsub subscriptions"
```

---

### Task 5: PostgreSQL Pool Policy and Runtime Observation

**Files:**

- Create: `packages/postgres/src/health.ts`
- Create: `packages/postgres/tests/health.test.ts`
- Modify: `packages/postgres/src/index.ts`
- Modify: `packages/postgres/src/long-term.ts`
- Create: `rust/taskcast-postgres/src/health.rs`
- Create: `rust/taskcast-postgres/tests/health.rs`
- Modify: `rust/taskcast-postgres/src/lib.rs`
- Modify: `rust/taskcast-postgres/src/store.rs`

**Interfaces:**

- Adds optional `DependencyObserver` to the store constructors without removing
  the current one-argument constructors.
- Adds `postgresCheck()`/`postgres_check()` helpers used by CLI readiness.
- Exposes connectivity classifiers only for tests and server error wrapping.

- [ ] **Step 1: Write failing classifier and observation tests**

Cover these positive connectivity cases:

- postgres.js `ECONNREFUSED`, `ECONNRESET`, `ETIMEDOUT`,
  `CONNECTION_CLOSED`, SQLSTATE class `08`, and shutdown code `57P01`;
- sqlx `Io`, `PoolTimedOut`, `PoolClosed`, `WorkerCrashed`, TLS failures, and
  database codes in class `08` or `57P01`.

Cover these negative cases:

- unique/foreign-key/constraint SQLSTATEs;
- syntax errors;
- archive conflicts and validation failures.

Assert that a classified public store-operation failure:

- emits one `postgres` unhealthy observation;
- throws/returns `DependencyUnavailableError`;
- is not retried;
- reports healthy after the next successful operation.

- [ ] **Step 2: Run focused PostgreSQL tests and verify RED**

Run:

```bash
pnpm --filter @taskcast/postgres test -- tests/health.test.ts
cd rust
cargo test -p taskcast-postgres --test health
```

Expected: tests fail because observation and typed classification do not exist.

- [ ] **Step 3: Implement TypeScript operation observation**

Export:

```ts
export function classifyPostgresConnectivity(
  error: unknown,
): DependencyErrorKind | undefined

export async function postgresCheck(
  sql: ReturnType<typeof postgres>,
): Promise<void> {
  await sql`SELECT 1`
}
```

Extend the constructor additively:

```ts
constructor(
  private sql: ReturnType<typeof postgres>,
  private observer?: DependencyObserver,
) {}
```

Use one helper around every public `LongTermStore` method:

```ts
private async observed<T>(operation: () => Promise<T>): Promise<T> {
  try {
    const result = await operation()
    this.observer?.observe({ dependency: 'postgres', state: 'healthy' })
    return result
  } catch (error) {
    const kind = classifyPostgresConnectivity(error)
    if (!kind) throw error
    this.observer?.observe({
      dependency: 'postgres',
      state: 'unhealthy',
      errorKind: kind,
    })
    throw new DependencyUnavailableError('postgres', kind, error)
  }
}
```

Move each existing method body unchanged inside `this.observed(async () => {
... })`; never rerun `operation`.

- [ ] **Step 4: Implement Rust operation observation**

Keep:

```rust
pub fn new(pool: PgPool) -> Self
```

Add:

```rust
pub fn new_observed(
    pool: PgPool,
    observer: Arc<dyn DependencyObserver>,
) -> Self

pub fn pool(&self) -> &PgPool

pub async fn postgres_check(pool: &PgPool) -> Result<(), DependencyUnavailableError>
```

Wrap each public trait method once around its full existing body. Inspect the
boxed error source chain for `sqlx::Error`, classify only the cases listed in
Step 1, observe, and return the typed dependency error. Do not change internal
transaction SQL or repeat the future.

- [ ] **Step 5: Run PostgreSQL suites and commit**

Run:

```bash
pnpm --filter @taskcast/postgres test
pnpm --filter @taskcast/postgres build
cd rust
cargo fmt --all
cargo test -p taskcast-postgres
```

Expected: all package tests pass and non-connectivity error behavior remains unchanged.

Commit:

```bash
git add packages/postgres/src packages/postgres/tests/health.test.ts \
  rust/taskcast-postgres/src rust/taskcast-postgres/tests/health.rs
git commit -m "fix(postgres): observe pooled connection failures"
```

---

### Task 6: Exact Adapter Activation and CLI Startup Wiring

**Files:**

- Modify: `packages/cli/src/commands/start.ts`
- Modify: `packages/cli/tests/unit/start-command.test.ts`
- Create: `packages/cli/tests/integration/dependency-startup.test.ts`
- Modify: `rust/taskcast-cli/src/helpers.rs`
- Modify: `rust/taskcast-cli/src/commands/start.rs`
- Modify: `rust/taskcast-cli/tests/start_env_tests.rs`
- Create: `rust/taskcast-cli/tests/dependency_startup.rs`

**Interfaces:**

- Produces identical `resolveStorageMode`/`resolve_storage_mode` behavior:
  CLI flag > `TASKCAST_STORAGE` > configured short-term/broadcast provider >
  Redis-URL auto-detection > memory.
- Explicit `memory` or `sqlite` ignores an unrelated Redis URL.
- Adds positive-integer PostgreSQL max parsing in both CLIs.
- Passes the registry and checks to the server.

- [ ] **Step 1: Write failing storage-resolution tests**

Use this table in both runtimes:

| CLI | Env | Config provider | Redis URL | Expected |
|---|---|---|---|---|
| `memory` | unset | `redis` | yes | `memory` |
| `sqlite` | `redis` | `redis` | yes | `sqlite` |
| unset | `memory` | `redis` | yes | `memory` |
| unset | unset | `redis` | yes | `redis` |
| unset | unset | unset | yes | `redis` |
| unset | unset | unset | no | `memory` |

Change the TypeScript Commander option to have no default and the Rust
`StartArgs.storage` field to `Option<String>`, so explicit `memory` is
distinguishable from no selection.

When neither CLI nor `TASKCAST_STORAGE` selects a mode, derive the configured
provider from `shortTermStore.provider` and `broadcast.provider`. If both are
present and differ, fail configuration instead of silently creating a mixed
pair that the current CLI does not support.

Add max-connection tests for absent/`10`/`1` success and
`0`/`-1`/`1.5`/`abc`/overflow failure.

Add startup tests:

- active unreachable Redis fails before binding;
- active unreachable PostgreSQL fails before binding;
- memory plus an unrelated Redis URL starts without opening Redis;
- SQLite plus PostgreSQL URL starts without opening PostgreSQL;
- a non-SQLite mode with an explicitly configured non-PostgreSQL long-term
  provider ignores an unrelated PostgreSQL URL;
- config-file Redis/PostgreSQL selections activate without requiring the
  equivalent environment variable.

- [ ] **Step 2: Run CLI tests and verify RED**

Run:

```bash
pnpm --filter @taskcast/cli test -- \
  tests/unit/start-command.test.ts tests/integration/dependency-startup.test.ts
cd rust
cargo test -p taskcast-cli --test start_env_tests
cargo test -p taskcast-cli --test dependency_startup
```

Expected: explicit-memory tests fail under current URL auto-detection and the
new managed startup APIs are absent.

- [ ] **Step 3: Implement TypeScript CLI wiring**

Resolve storage with this exact signature:

```ts
export function resolveStorageMode(options: {
  cli?: string
  env?: string
  configuredProvider?: string
  hasRedisUrl: boolean
}): 'memory' | 'redis' | 'sqlite'
```

Reject any value outside the three modes. Instantiate Redis only when the
returned mode is `redis`:

```ts
const dependencyHealth = new DependencyHealthRegistry()
const managedRedis = storageMode === 'redis'
  ? await createManagedRedisAdapters(redisUrl!, {
      observer: dependencyHealth,
      startupTimeoutMs: 15_000,
    })
  : undefined
```

Register command and PubSub checks from the managed result. Resolve PostgreSQL
identically in both runtimes:

```text
storage mode sqlite                                      -> inactive
explicit long-term provider other than postgres         -> inactive
explicit long-term provider postgres + resolved URL     -> active
no explicit long-term provider + non-empty env URL      -> active
all other combinations                                  -> inactive
```

An explicit PostgreSQL provider without a resolved URL is a configuration
error. Construct postgres.js only when this result is active:

```ts
export function parsePostgresMaxConnections(value?: string): number {
  if (value === undefined || value === '') return 10
  if (!/^[1-9]\d*$/.test(value)) {
    throw new Error(
      'TASKCAST_POSTGRES_MAX_CONNECTIONS must be a positive integer',
    )
  }
  const parsed = Number(value)
  if (!Number.isSafeInteger(parsed)) {
    throw new Error(
      'TASKCAST_POSTGRES_MAX_CONNECTIONS must be a positive integer',
    )
  }
  return parsed
}

const max = parsePostgresMaxConnections(
  process.env['TASKCAST_POSTGRES_MAX_CONNECTIONS'],
)
const sql = postgres(postgresUrl, {
  max,
  connect_timeout: 5,
})
await postgresCheck(sql)
dependencyHealth.register('postgres', () => postgresCheck(sql))
```

Run `SELECT 1` before auto-migration. Pass `dependencyHealth` to
`createTaskcastApp`. On startup failure close every partially created client.
On SIGINT/SIGTERM stop server services, close the HTTP listener, close managed
Redis, and call `sql.end({ timeout: 5 })`.

- [ ] **Step 4: Implement Rust CLI wiring**

Change the helper to:

```rust
pub fn resolve_storage_mode<'a>(
    cli: Option<&'a str>,
    env: Option<&'a str>,
    configured_provider: Option<&'a str>,
    has_redis_url: bool,
) -> Result<&'a str, String>
```

Use the same priority table. Parse PostgreSQL max with:

```rust
fn parse_postgres_max_connections(value: Option<&str>) -> Result<u32, String> {
    match value.filter(|value| !value.is_empty()) {
        None => Ok(10),
        Some(value) => value
            .parse::<u32>()
            .ok()
            .filter(|parsed| *parsed > 0)
            .ok_or_else(|| {
                "TASKCAST_POSTGRES_MAX_CONNECTIONS must be a positive integer"
                    .to_string()
            }),
    }
}
```

Build the pool:

```rust
let pool = sqlx::postgres::PgPoolOptions::new()
    .max_connections(max_connections)
    .min_connections(0)
    .acquire_timeout(Duration::from_secs(5))
    .connect(postgres_url)
    .await?;
tokio::time::timeout(
    Duration::from_secs(5),
    sqlx::query("SELECT 1").execute(&pool),
).await??;
```

Create one registry, pass it as the observer to managed Redis/PostgreSQL, and
register their checks. Call
`create_app_with_runtime_health_and_routes(...)` with a clone of the real
`file_config` instead of the current `None`. After Axum graceful shutdown,
await PubSub shutdown and close the PostgreSQL pool.

- [ ] **Step 5: Run CLI suites and commit**

Run:

```bash
pnpm --filter @taskcast/cli test
pnpm --filter @taskcast/cli build
cd rust
cargo fmt --all
cargo test -p taskcast-cli
```

Expected: all CLI tests pass; inactive dependencies make no network connection.

Commit:

```bash
git add packages/cli/src/commands/start.ts packages/cli/tests \
  rust/taskcast-cli/src/commands/start.rs rust/taskcast-cli/src/helpers.rs \
  rust/taskcast-cli/tests
git commit -m "fix(cli): wire managed runtime dependencies"
```

---

### Task 7: Map Typed Connectivity Failures to HTTP 503

**Files:**

- Modify: `packages/server/src/index.ts`
- Modify: `packages/server/src/routes/tasks.ts`
- Modify: `packages/server/src/routes/workers.ts`
- Modify: `packages/server/tests/tasks.test.ts`
- Modify: `packages/server/tests/workers.test.ts`
- Modify: `rust/taskcast-server/src/error.rs`
- Modify: `rust/taskcast-server/tests/server_tests.rs`

**Interfaces:**

- TypeScript produces `dependencyErrorResponse(c, error, fallbackStatus)`.
- Rust `AppError::Engine(EngineError::Store(...))` inspects the typed source chain.
- Both preserve `{ "error": string }`; no reconnect fields enter business responses.

- [ ] **Step 1: Write failing 503 parity tests**

For every generic catch path in task and worker routes, throw a
`DependencyUnavailableError` and assert:

```ts
expect(response.status).toBe(503)
expect(await response.json()).toEqual({
  error: 'redisCommand unavailable (connection_reset)',
})
```

Also assert ordinary store errors retain their current route-specific status
and body. In Rust, wrap the dependency error inside `EngineError::Store` and
assert the same status/body.

- [ ] **Step 2: Run server tests and verify RED**

Run:

```bash
pnpm --filter @taskcast/server test -- tests/tasks.test.ts tests/workers.test.ts
cd rust
cargo test -p taskcast-server --test server_tests dependency
```

Expected: typed dependency failures still use current 400/500 mappings.

- [ ] **Step 3: Implement TypeScript response selection**

Add:

```ts
export function dependencyErrorResponse(
  c: Context,
  error: unknown,
  fallbackStatus: ContentfulStatusCode,
): Response {
  const dependency = findDependencyUnavailableError(error)
  if (dependency) {
    return c.json({ error: dependency.message }, 503)
  }
  const message = error instanceof Error ? error.message : String(error)
  return c.json({ error: message }, fallbackStatus)
}
```

Use it only at existing generic catch fallbacks in `tasks.ts` and `workers.ts`;
keep validation, authorization, not-found, conflict, and archive-specific
branches before it.

- [ ] **Step 4: Implement Rust response selection**

In the `EngineError::Store(error)` branch:

```rust
if let Some(unavailable) =
    taskcast_core::find_dependency_unavailable(error.as_ref())
{
    (
        StatusCode::SERVICE_UNAVAILABLE,
        unavailable.to_string(),
        Some(HttpFailureDetail::new(
            HttpFailureKind::Store,
            unavailable.to_string(),
        )),
    )
} else {
    // existing 500 tuple unchanged
}
```

Do not expose `unavailable.source()` in the response or logger.

- [ ] **Step 5: Run server suites and commit**

Run:

```bash
pnpm --filter @taskcast/server test
pnpm --filter @taskcast/server build
cd rust
cargo fmt --all
cargo test -p taskcast-server
```

Expected: all tests pass with typed connectivity failures at 503.

Commit:

```bash
git add packages/server/src packages/server/tests \
  rust/taskcast-server/src/error.rs rust/taskcast-server/tests/server_tests.rs
git commit -m "fix(server): classify dependency outages as 503"
```

---

### Task 8: Deterministic Fault-Injection and No-Replay Regressions

**Files:**

- Modify: `packages/redis/tests/helpers/tcp-fault-proxy.ts`
- Modify: `packages/redis/tests/managed.test.ts`
- Modify: `rust/taskcast-redis/tests/support/mod.rs`
- Modify: `rust/taskcast-redis/tests/reconnect.rs`
- Modify: `packages/postgres/tests/health.test.ts`
- Modify: `rust/taskcast-postgres/tests/health.rs`

**Interfaces:**

- Test-only proxy supports `open`, `blackhole`, `refuse`, and
  `dropNextResponse(matcher)` modes.
- Exposes accepted upstream-connection and matched-command counters.

- [ ] **Step 1: Implement the test-only TCP proxies**

Extend the proxies created in Task 3. The final proxy must:

- continue binding an ephemeral loopback port;
- continue forwarding bytes bidirectionally in `open`;
- continue closing existing sockets and refusing new upstream connections in `refuse`;
- accept but never forward in `blackhole`;
- for `dropNextResponse`, forward the matched request, read the upstream
  response, close the downstream socket before writing that response, and
  increment the matcher count;
- close all sockets and listener in `stop`.

Do not parse credentials or print forwarded bytes.

- [ ] **Step 2: Add the no-replay Redis regression**

Through the managed command connection:

1. send an `INCR taskcast:test:no-replay`;
2. have the proxy deliver it upstream and drop its response;
3. assert the caller receives an error;
4. restore forwarding and query the upstream Redis directly;
5. assert the key equals `1`;
6. assert the proxy matched the side-effecting command exactly once.

Run this test in TypeScript and Rust.

- [ ] **Step 3: Add long-outage and concurrency regressions**

For both Redis runtimes:

- keep the proxy unavailable longer than one manager retry round;
- assert the current request fails;
- restore the proxy;
- invoke readiness/`PING`;
- assert later store and publish operations succeed;
- launch at least 50 concurrent operations after one drop and assert the
  accepted-connection counter stays at the manager/supervisor bound rather
  than growing per caller.

- [ ] **Step 4: Add PostgreSQL outage recovery regressions**

Using the same proxy model:

- establish the pool and pass `SELECT 1`;
- drop existing sockets and refuse new forwarding;
- assert the in-flight statement fails once and is not replayed;
- assert the readiness check fails;
- restore forwarding;
- assert a later `SELECT 1` and store operation succeed without rebuilding the
  Taskcast process.

- [ ] **Step 5: Run all fault suites and commit**

Run:

```bash
pnpm --filter @taskcast/redis test -- tests/managed.test.ts
pnpm --filter @taskcast/postgres test -- tests/health.test.ts
cd rust
cargo test -p taskcast-redis --test reconnect
cargo test -p taskcast-postgres --test health
```

Expected: every fault test passes repeatedly; no test relies on a fixed sleep
for correctness—use bounded polling with explicit deadlines.

Commit:

```bash
git add packages/redis/tests packages/postgres/tests/health.test.ts \
  rust/taskcast-redis/tests rust/taskcast-postgres/tests/health.rs
git commit -m "test: cover dependency disconnect recovery"
```

---

### Task 9: Documentation, Release Note, and Full Verification

**Files:**

- Create: `.changeset/calm-stores-reconnect.md`
- Modify: `README.md`
- Modify: `README.zh.md`
- Modify: `packages/cli/README.md`
- Modify: `docs/guide/deployment.md`
- Modify: `docs/guide/deployment.zh.md`

**Interfaces:**

- Documents effective adapter activation, PostgreSQL pool limit, health
  endpoints, no-replay semantics, and structured state logs.
- Produces the fixed-version patch release note.

- [ ] **Step 1: Update operator documentation**

Add:

```md
| `TASKCAST_POSTGRES_MAX_CONNECTIONS` | Maximum PostgreSQL pool connections per Taskcast process; positive integer only. | `10` |
```

Document:

- explicit `memory`/`sqlite` does not activate Redis merely because a URL is present;
- startup requires active Redis/PostgreSQL dependencies;
- `/health` is liveness and performs no dependency I/O;
- `/health/ready` is readiness and returns 503 when an active dependency is unavailable;
- `/health/detail` exposes sanitized dependency state;
- the request interrupted by a disconnect can fail and is never automatically replayed;
- `dependency_state_change` and throttled outage summaries are JSON stderr records.

Use equivalent Chinese text in the Chinese documents.

- [ ] **Step 2: Add the changeset**

Create `.changeset/calm-stores-reconnect.md`:

```md
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
```

- [ ] **Step 3: Run focused parity verification**

Run:

```bash
pnpm --filter @taskcast/core test -- tests/unit/dependency.test.ts
pnpm --filter @taskcast/server test -- \
  tests/dependency-health.test.ts tests/health-detail.test.ts
pnpm --filter @taskcast/redis test -- tests/managed.test.ts
pnpm --filter @taskcast/postgres test -- tests/health.test.ts
pnpm --filter @taskcast/cli test -- tests/integration/dependency-startup.test.ts
cd rust
cargo test -p taskcast-core --test dependency
cargo test -p taskcast-server --test dependency_health
cargo test -p taskcast-redis --test reconnect
cargo test -p taskcast-postgres --test health
cargo test -p taskcast-cli --test dependency_startup
```

Expected: all focused parity and fault-injection suites pass.

- [ ] **Step 4: Run full TypeScript verification**

From the repository root:

```bash
pnpm lint
pnpm build
pnpm test
pnpm test:coverage
```

Expected: every command exits 0 and coverage meets the repository threshold
without excluding new files.

- [ ] **Step 5: Run full Rust verification**

From `rust/`:

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

Expected: every command exits 0 with no format, lint, or test failure.

- [ ] **Step 6: Run repository hygiene and secret scans**

From the repository root:

```bash
git diff --check
rg -n "redis://[^[:space:]]+@|postgres(ql)?://[^[:space:]]+@" \
  packages rust README.md README.zh.md docs
git status --short
```

Expected: `git diff --check` exits 0; the URL scan finds only intentionally
redacted test fixtures; status lists only the documentation and changeset for
this final task.

- [ ] **Step 7: Commit documentation**

```bash
git add README.md README.zh.md packages/cli/README.md \
  docs/guide/deployment.md docs/guide/deployment.zh.md \
  .changeset/calm-stores-reconnect.md
git commit -m "docs: document dependency recovery"
```

- [ ] **Step 8: Perform final verification after the commit**

Run:

```bash
git status --short --branch
git diff --check HEAD~9 HEAD
git log --oneline --decorate -12
```

Expected: worktree clean; the branch contains the nine intended implementation
commits after the approved design commit.
