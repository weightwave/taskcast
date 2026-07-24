# Dependency Reconnection and Readiness Design

**Date:** 2026-07-24

## Context

Taskcast 1.5.4's Rust Redis adapters keep raw
`redis::aio::MultiplexedConnection` values for short-term storage and
publishing, plus a raw `PubSub` connection for subscriptions. After the Redis
server or network path restarts, the old command connections remain unusable.
The PubSub listener also exits when its stream closes and never reconnects or
re-subscribes. A fresh connection works immediately, so the persistent failure
is in Taskcast's connection lifecycle rather than in Redis data or commands.

The equivalent TypeScript runtime uses ioredis, which reconnects and
re-subscribes by default. However, its defaults also queue commands while
offline and resend unfulfilled commands after reconnecting. That behavior does
not match the desired failure semantics for writes whose execution status is
unknown.

PostgreSQL is not affected by the same stale-connection defect. Rust uses
`sqlx::PgPool`, and TypeScript uses postgres.js; both replace broken pooled
connections. An in-flight SQL statement can still fail, and neither Taskcast
runtime should replay it.

Production currently uses TCP readiness and liveness probes. `/health` always
returns `ok: true`, while `/health/detail` reports configured adapter names but
does not test connectivity. Consequently, a Taskcast Pod with unavailable
dependencies can remain Ready and continue receiving traffic.

Both server implementations must retain equivalent HTTP and operational
behavior. This design therefore applies to the Rust and TypeScript runtimes,
while using each client's native connection-management facilities.

## Goals

- Recover Redis command connectivity in place after a runtime disconnect.
- Restore Redis pattern subscriptions after a disconnect without restarting
  Taskcast.
- Use one general-purpose Redis command connection per Taskcast instance for
  short-term storage, publishing, and readiness checks.
- Prevent concurrent callers in one process from creating competing Redis
  command reconnections.
- Apply bounded exponential backoff with jitter so independent Pods do not
  reconnect in lockstep.
- Never automatically replay a failed Taskcast business command or SQL
  statement.
- Fail startup when, and only when, a dependency selected by the resolved
  configuration cannot be initialized.
- Keep a running process alive during dependency outages while marking it Not
  Ready.
- Expose sanitized dependency state through health endpoints and structured
  logs.
- Preserve the existing public low-level adapter constructors so the change
  can ship without an avoidable Rust or TypeScript API break.

## Non-goals

- Exactly-once delivery across a network failure.
- Adding application-level retries for Redis commands, PostgreSQL statements,
  HTTP requests, webhooks, or client SDK operations.
- Adding a distributed lock or leader election for reconnecting across Pods.
- Replacing Redis PubSub with streams, queues, or another messaging system.
- Changing Redis or PostgreSQL schemas.
- Replacing the existing Taskcast logging system or adding a metrics backend.
- Requiring Redis or PostgreSQL when the resolved configuration does not
  instantiate that adapter.

## Selected Approach

Use native connection managers and pools, with a dedicated Taskcast supervisor
only where the library does not cover the lifecycle.

1. Rust uses one cloneable Redis `ConnectionManager` for ordinary commands.
2. Rust uses a separate supervised RESP2 PubSub connection because redis-rs
   0.27.6 does not reconnect and re-subscribe the current raw `PubSub`.
3. TypeScript uses one ioredis command client and one subscriber client.
4. PostgreSQL continues to use `PgPool` or postgres.js pooling.
5. A server-owned dependency-health registry connects these infrastructure
   components to readiness, diagnostics, and state-transition logging.

The command manager and subscriber are separate because a RESP2 connection in
subscriber mode cannot carry ordinary commands. Store and `PUBLISH` operations
can safely share the command manager: Taskcast does not use blocking Redis
commands, and the underlying connection is multiplexed.

## Configuration Activation

Dependency behavior is based on the resolved adapter selection, not merely on
the presence of an environment variable.

- Redis is active only when the resolved short-term/broadcast storage mode is
  Redis. An explicitly selected memory or SQLite mode does not connect to
  Redis, even if `TASKCAST_REDIS_URL` exists.
- PostgreSQL is active only when a PostgreSQL long-term store is actually
  instantiated by the resolved configuration.
- An active dependency must pass its initial connection check before the HTTP
  server binds.
- An inactive dependency has no client, health probe, retry loop, or readiness
  requirement.

This rule applies identically to configuration-file and environment-variable
resolution.

## Component Design

### Managed Redis command path

The Rust Redis package gains an additive managed construction path that accepts
or creates `redis::aio::ConnectionManager`. The existing constructors that
accept raw connections remain available for embedded callers that deliberately
own connection lifecycle. The Taskcast CLI uses only the managed path.

The managed factory creates one `ConnectionManager` and clones it into:

- `RedisShortTermStore`;
- the publishing side of `RedisBroadcastProvider`; and
- the Redis readiness checker.

Clones share the manager's `ArcSwap` connection future. When concurrent
commands detect the same dropped connection, compare-and-swap selects one
reconnect future and all clones observe that result. No Taskcast mutex or
distributed lock is added.

The TypeScript CLI similarly creates one ioredis command client and passes the
same client to the short-term store and publishing provider. Existing adapter
constructors remain valid for callers that supply distinct clients.

### Redis PubSub supervisor

Rust adds a focused PubSub supervisor with four responsibilities:

1. establish the dedicated PubSub connection;
2. issue the single wildcard `PSUBSCRIBE`;
3. dispatch received messages to the existing in-process subscriber map; and
4. reconnect and re-subscribe when the stream closes or subscription fails.

The subscriber map lives outside an individual PubSub connection and survives
replacements. The supervisor owns a shutdown signal so graceful server
shutdown stops retrying and releases the connection. Initial connection and
subscription are awaited during startup. Runtime retries continue indefinitely.

TypeScript keeps ioredis `autoResubscribe: true`. Its subscriber client reports
`ready`, `reconnecting`, `end`, and `error` events into the shared health
registry. It uses the same equal-jitter policy family as the command client,
with the PubSub-specific cap defined below. Subscription restoration is
allowed because it restores connection state rather than replaying a Taskcast
business operation.

### PostgreSQL pool

Rust constructs the existing `PgPool` through explicit `PgPoolOptions`.
TypeScript constructs postgres.js with matching pool and timeout values.

- Maximum connections default to 10 per Taskcast instance.
- `TASKCAST_POSTGRES_MAX_CONNECTIONS` can override the default with a positive
  integer in both runtimes.
- Pool acquire/connect timeout is 5 seconds.
- Minimum connections remain zero, so an outage does not cause every Pod to
  rebuild a full idle pool.
- The normal library liveness checks and broken-connection disposal remain
  enabled.

At startup, PostgreSQL executes `SELECT 1` before optional migrations. A
connection failure, authentication failure, or migration failure aborts
startup. At runtime, the pool replaces unusable connections on demand.

### Dependency health registry

The HTTP server state owns a small registry independent of core task logic.
Each actual dependency has one entry with:

- `configured`;
- `state`: `starting`, `healthy`, `reconnecting`, or `unhealthy`;
- `lastTransitionAt`;
- sanitized `lastErrorKind`;
- `consecutiveFailures`; and
- optional `reconnectAttempts` for the PubSub supervisor, whose attempts
  Taskcast directly controls.

The Redis command manager and PostgreSQL libraries do not expose their
internal reconnect-attempt counters, so the API does not invent those values.
Their entries report consecutive observed failures instead.

Business operations mark a dependency unhealthy immediately when they receive
a classified connectivity error. A successful operation or active readiness
probe marks it healthy. Domain, validation, authorization, Redis command, and
SQL constraint errors do not change dependency health.

Adapter status in `/health/detail` is derived from the dependencies it needs:
the Redis short-term store follows `redisCommand`, the Redis broadcast adapter
is healthy only when both `redisCommand` and `redisPubSub` are healthy, and the
PostgreSQL long-term store follows `postgres`.

## Startup Behavior

Startup is fail-fast only for active dependencies.

For Redis, both the command connection and initial pattern subscription must
succeed within a 15-second overall deadline before the HTTP server binds. Rust
uses the retry schedule below around initial manager construction, then
performs `PING` and completes `PSUBSCRIBE`. TypeScript explicitly waits for
`ready`, performs `PING`, and completes `PSUBSCRIBE` under the same deadline.
The deadline includes connection attempts, retry waits, and validation. If it
expires, all partially created clients are closed and startup returns an
error.

For PostgreSQL, `SELECT 1` must complete within the 5-second pool
acquire/connect timeout. Optional automatic migrations run only after that
check. A configured dependency is never silently downgraded to memory storage.

Memory-only and SQLite modes retain their current startup behavior.

## Redis Reconnect and Backoff Policy

### Rust command manager

Taskcast remains on redis-rs 0.27.6 for this change and enables its
`connection-manager` feature. A larger redis-rs upgrade is separate work.

The pinned release has a configuration mismatch: `exponent_base` is not used
when constructing Backon, while `factor` is passed as Backon's exponential
multiplier. The 0.27.6 default factor of 100 would therefore produce unsuitable
delays. Taskcast must not use the manager defaults.

The explicit configuration is:

- factor: 2;
- retries after the immediate connection attempt: 2;
- nominal maximum delay: 2 seconds;
- per-attempt connection timeout: 2 seconds;
- command response timeout: 10 seconds; and
- built-in jitter enabled by `ConnectionManager`.

With the pinned Backon implementation, retry waits are approximately 1-2
seconds and 2-4 seconds after jitter. If the round is exhausted, waiting
commands receive the connection error. The next command or readiness `PING`
starts another manager-controlled round when the stored error is an I/O error.
This produces long-outage recovery without holding one HTTP request
indefinitely.

The command that detects a dropped connection receives its error. The manager
does not replay it.

### TypeScript command client

The ioredis command client reconnects indefinitely with an equal-jitter
exponential strategy capped at 5 seconds. It uses:

- `enableOfflineQueue: false`;
- `autoResendUnfulfilledCommands: false`; and
- `maxRetriesPerRequest: 0`.

Commands issued while the connection is unavailable fail instead of waiting
for a later connection. Commands whose response was lost are rejected and
never resent.

### PubSub reconnect

The Rust PubSub supervisor retries indefinitely using equal jitter:

```text
cap = min(10 seconds, 500 milliseconds * 2^attempt)
delay = cap / 2 + random(0, cap / 2)
```

The attempt counter resets after a connection is established and
`PSUBSCRIBE` succeeds. TypeScript uses an equivalent jittered policy while
retaining ioredis automatic re-subscription.

Retries are independent across Pods. Jitter decorrelates them; a distributed
lock is neither necessary nor desirable because every Pod needs its own
subscriber and command connection.

## Command and Error Semantics

Taskcast performs no application-level retry.

- The Redis command that observes a disconnect fails.
- Commands submitted while TypeScript ioredis is offline fail.
- A Redis command waiting on a Rust reconnect future fails if that reconnect
  round fails.
- A PostgreSQL statement interrupted by a disconnect fails.
- Taskcast does not infer whether a write executed before its response was
  lost.

Connectivity errors from an active Redis or PostgreSQL adapter map to HTTP
`503 Service Unavailable` in both servers. The response body keeps the
runtime's existing error-envelope schema and gains no reconnect-specific
fields; only the HTTP status classification changes. Validation,
authentication, authorization, not-found, conflict, Redis data/command, and
SQL constraint failures keep their existing mappings. If the two runtimes
currently disagree on the connectivity-error envelope, implementation first
converges them on the existing public server error schema rather than
introducing a new one.

The existing always-on HTTP 5xx logger will record these 503 responses. It
does not perform retries.

## Health Endpoints

### Liveness

`GET /health` remains unauthenticated and performs no external I/O. It returns
200 when the Taskcast HTTP process is responsive. Redis or PostgreSQL outages
never make liveness fail.

### Readiness

`GET /health/ready` is a new unauthenticated endpoint. It runs all checks for
active dependencies concurrently under one 2-second overall deadline:

- Redis command connection: `PING` through the shared command manager;
- Redis PubSub: read the supervisor's current subscribed state; and
- PostgreSQL: `SELECT 1` through the pool.

Inactive dependencies do not participate. Every active dependency must be
healthy for a 200 response. A failure or timeout returns 503 and identifies
only the low-cardinality dependency name and error kind.

The Redis readiness `PING` also ensures that a quiet instance continues to
drive new command reconnect rounds after a long outage.

### Detailed health

`GET /health/detail` remains unauthenticated and backward compatible. Existing
adapter provider/status fields remain, and a new `dependencies` object exposes
the registry fields. It never returns connection URLs, hostnames, ports,
usernames, passwords, raw exceptions, SQL, Redis commands, task payloads, or
authorization data.

An illustrative degraded response is:

```json
{
  "ok": false,
  "adapters": {
    "broadcast": { "provider": "redis", "status": "error" },
    "shortTermStore": { "provider": "redis", "status": "error" },
    "longTermStore": { "provider": "postgres", "status": "ok" }
  },
  "dependencies": {
    "redisCommand": {
      "configured": true,
      "state": "reconnecting",
      "lastErrorKind": "connection_reset",
      "consecutiveFailures": 2
    },
    "redisPubSub": {
      "configured": true,
      "state": "healthy",
      "reconnectAttempts": 0
    },
    "postgres": {
      "configured": true,
      "state": "healthy",
      "consecutiveFailures": 0
    }
  }
}
```

Timestamps are included in real responses but omitted from the example for
brevity.

## Kubernetes Probe Design

The Coffice deployment changes from TCP-only probes to:

- startup probe: `GET /health`, every 2 seconds, failure threshold 30;
- liveness probe: `GET /health`, every 20 seconds, failure threshold 3; and
- readiness probe: `GET /health/ready`, timeout 2 seconds, every 5 seconds,
  failure threshold 2, success threshold 1.

The startup probe allows up to roughly 60 seconds for initial dependency
checks and migrations while preventing liveness from killing a valid startup.
Runtime dependency failures leave the process alive. Two consecutive readiness
failures remove the Pod from service after approximately 10 seconds, avoiding
probe flapping on a single short transient.

The Taskcast endpoint ships before or with the matching Coffice GitOps probe
change. The probe edit is committed in the Coffice GitOps repository rather
than bundled into the Taskcast source commit. Staging is updated and
fault-tested before production.

## Structured Operational Logs

Both runtimes emit one JSON record to stderr when a dependency changes state.
The stable schema is:

```json
{
  "timestamp": "2026-07-24T08:21:32.387Z",
  "level": "warn",
  "event": "dependency_state_change",
  "dependency": "redisPubSub",
  "from": "healthy",
  "to": "reconnecting",
  "attempt": 1,
  "nextRetryMs": 734,
  "errorKind": "connection_reset"
}
```

Fields that do not apply are omitted. Recovery records use level `info` and
include `downtimeMs`. A persistent outage emits at most one summary per
dependency every 60 seconds. Readiness probes and individual retry failures do
not each produce another state-change record.

Logs never contain connection strings, credentials, raw request content, task
payloads, SQL, Redis arguments, authorization headers, or cookies.

Coffice LoongCollector already ingests stdout/stderr for the `taskcast`
container in production and staging and expands JSON fields into the
environment Logstore. No new Taskcast log transport is required.

## Compatibility

- REST, SSE, task, and event schemas do not change.
- `/health` retains its existing lightweight liveness behavior.
- `/health/ready` is additive.
- `/health/detail` retains existing fields and adds sanitized dependency data.
- Existing low-level Redis adapter constructors remain available.
- The Node.js and Rust managed CLI paths gain equivalent behavior in the same
  release.
- No data migration is required.
- The only optional new runtime setting is
  `TASKCAST_POSTGRES_MAX_CONNECTIONS`; its default preserves the libraries'
  current limit of 10.

## Testing

Implementation follows red-green TDD in both runtimes.

### Unit and focused tests

- Configuration activates checks only for adapters actually selected.
- Memory and SQLite modes ignore an unrelated Redis URL.
- PostgreSQL maximum-connection parsing accepts positive integers and rejects
  zero, negative, non-numeric, and overflowing values.
- Backoff helpers stay within their specified jitter ranges and caps.
- Dependency state transitions update timestamps and counters correctly.
- Duplicate observations of the same state do not emit duplicate transition
  logs.
- Logs and health responses redact credentials and exclude raw errors.
- Connectivity errors map to 503; non-connectivity errors retain existing
  status mappings.
- TypeScript command-client options disable offline queuing and unfulfilled
  command replay.
- PubSub supervisor shutdown cancels pending sleep and reconnect work.

### Container and fault-injection tests

- An unavailable active Redis dependency makes startup fail within the bounded
  deadline.
- Redis restart leaves the Taskcast process running, changes readiness from
  200 to 503, then restores 200 and successful operations after recovery.
- A long Redis outage exceeds one command-manager retry round; a later
  readiness check starts another round and recovery still succeeds.
- Many concurrent Redis operations after disconnect create only one command
  reconnect flow per Taskcast instance. A counting TCP proxy or Redis client
  inspection verifies the connection bound.
- A deterministic one-shot RESP proxy accepts a side-effecting command, closes
  after processing it but before delivering the response, and verifies that
  neither runtime sends that business command again.
- Two Taskcast instances receive cross-instance events before a Redis restart
  and again after re-subscription, with one effective wildcard subscription
  per instance.
- PostgreSQL restart causes the current SQL operation and readiness to fail,
  does not stop Taskcast, and recovers through the existing pool.
- Startup fails for an active unreachable PostgreSQL dependency but succeeds
  when PostgreSQL is not configured.
- All health endpoints enforce the 2-second readiness deadline and remain
  unauthenticated.

The focused Redis, PostgreSQL, server, and CLI suites run first. The complete
TypeScript and Rust test, typecheck, lint, formatting, coverage, and
`git diff --check` gates run before release.

## Release and Acceptance

1. Ship matching Rust and TypeScript behavior to staging.
2. Keep or introduce the new HTTP probes only after the deployed image exposes
   `/health/ready`.
3. Perform controlled Redis and PostgreSQL restarts in staging.
4. Verify that the Pod UID and Taskcast process remain unchanged.
5. Verify readiness transitions, restored operations, cross-instance PubSub,
   and SLS `dependency_state_change` records.
6. Deploy the pinned image digest and probe changes to production.

Acceptance requires all of the following:

- no manual Taskcast restart after a dependency restart;
- the request in progress during a disconnect may fail, but no failed business
  operation is automatically replayed;
- subsequent Redis and PostgreSQL operations recover;
- cross-instance Redis PubSub resumes;
- the process remains Live while it is Not Ready;
- configured dependency failures are visible in health responses and SLS; and
- memory/SQLite deployments remain independent of Redis and optional
  PostgreSQL.

## Alternatives Rejected

### Separate command managers for Store and Publisher

This is closer to the current Rust layout but creates an extra connection and
an independent reconnect loop per Pod. Taskcast has no blocking Redis commands
that justify the isolation, so the additional reconnect competition has little
benefit.

### Exit the process on every runtime dependency failure

This delegates recovery to Kubernetes or Railway but turns a transient network
event into whole-process churn. Multiple Pods can restart together, creating a
larger reconnect spike and longer unavailability.

### Custom resilient wrappers with command retries

This duplicates library connection management and reintroduces ambiguous-write
risks. It would require idempotency keys or per-command retry classification
that Taskcast does not currently need.

### Upgrade redis-rs as part of this fix

Newer redis-rs code corrects the retry-configuration model and adds more RESP3
subscription support, but a dependency upgrade broadens the compatibility and
testing surface. The targeted fix uses the pinned version safely; a later
upgrade can remove the documented configuration workaround.
