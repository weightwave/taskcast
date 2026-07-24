# Task 8 Implementation: Deterministic Disconnect Recovery

## Scope

Task 8 remained test-only. No production source was changed.

Changed files:

- `packages/redis/tests/helpers/tcp-fault-proxy.ts`
- `packages/redis/tests/managed.test.ts`
- `packages/postgres/tests/health.test.ts`
- `rust/taskcast-redis/tests/support/mod.rs`
- `rust/taskcast-redis/tests/reconnect.rs`
- `rust/taskcast-postgres/tests/health.rs`

The progress ledger was not modified.

## Proxy design

Both test proxies now bind an ephemeral IPv4 loopback port and retain the
allocated endpoint for the life of the proxy.

- `open` forwards bytes bidirectionally.
- `blackhole` accepts/retains connections but does not forward bytes.
- `refuse` closes existing sockets and rejects later connection attempts.
  The TypeScript proxy closes the listener to produce a real
  `ECONNREFUSED`, then rebinds the same allocated port on `open`. This avoids
  the Windows/ioredis race in which accept-then-reset could emit `ready` after
  the stream was already destroyed.
- `dropNextResponse` / `drop_next_response` forwards a matched request,
  consumes the first upstream response bytes, and closes before writing the
  response downstream.
- Accepted connection and matched-command counters are monotonic.
- A matcher is scoped to connections that were already established when it
  was armed and is removed immediately after the first match. Replacement
  connections and their authentication/startup frames are never inspected.
- `stop` closes the listener and all downstream/upstream sockets/tasks.
  Rust `Drop` also aborts listener and connection tasks.

Request bytes are retained only after a matcher is armed, separately per
connection, with a 64 KiB cap. Nothing logs raw forwarded bytes. Matchers are
armed only after authentication/startup, so credentials are neither parsed nor
retained. Redis uses an exact RESP array parser for the first pending command
and has focused assertions for fragmentation, target-first coalescing, and
rejection of a leading unrelated command. That ordering ensures the response
being dropped belongs to the command that matched. PostgreSQL tests use unique
safe statement markers after the pool is ready.

## Redis regressions

TypeScript and Rust both send protocol-level:

`INCR taskcast:test:no-replay`

through the managed command path after initializing the key directly to zero.
The proxy drops the response. The caller errors, direct upstream state is
exactly `1`, and the matcher count is exactly one. The upstream value detects
an ambiguous-command replay; the match count proves only one fault was
injected.

Long-outage tests establish command and PubSub paths, deliver a pre-outage
message, blackhole one in-flight command, refuse long enough to observe more
than one retry round through counters/observer attempts, and then reopen.
Without rebuilding the managed adapters they prove:

- command readiness recovers;
- a later short-term-store write/read succeeds;
- PubSub recovers by delivering a newly published post-outage message.

The 50-caller tests are separate so concurrency pressure and long-outage
recovery have independent failure evidence.

### Architecture-derived connection bounds

There are two independently coordinated paths: one command manager and one
PubSub supervisor.

- TypeScript long-outage and 50-caller bound: `<= 4` accepted connections.
  Each coordinated path may have one transition-race connection and one
  successful recovery connection.
- Rust 50-caller bound: `<= 4` for the same two-path race-plus-recovery model.
- Rust long-outage bound: `<= 10`. The test deliberately observes a
  blackholed round plus two refused rounds, then allows one transition race and
  one successful recovery for each of the two coordinated paths.

These are fixed manager/supervisor bounds and do not scale with the 50 callers.

## PostgreSQL regressions

The TypeScript test uses one `postgres.js` pool with `max: 1`; the Rust test
uses one `PgPool` with `max_connections(1)`. Each store is created before the
fault and is never rebuilt.

Both tests:

1. migrate and pass `SELECT 1`;
2. create a disposable execution-count probe and send one uniquely marked
   increment statement;
3. drop its response and require the caller to fail;
4. require the marked-statement count to remain exactly one;
5. refuse connections and require readiness to fail;
6. reopen and poll readiness to success with a deadline;
7. query the probe through the original pool and require its execution count
   to be exactly `1`;
8. save and read a Taskcast task through the original store/pool.

## Determinism and cleanup

Correctness uses bounded deadlines or event/counter/status polling. The only
short delays are the 25 ms backoff inside polling loops. No fixed sleep is used
as proof. TypeScript operations are converted to tagged settled outcomes
before applying their deadlines, so a timeout fails the test and cannot
masquerade as the expected operation rejection.

New TypeScript integration tests use `try/finally`, including nested cleanup
so a failed pool close cannot skip proxy/container cleanup. Rust relies on
RAII for containers/pools and explicit `Drop` guards for proxy listener and
connection tasks; successful paths also shut down PubSub/pools/proxies
explicitly.

## RED and diagnosis evidence

- Baseline at `64a817a5493329d29b823a0df6bb5511a709dc2f`:
  - TypeScript Redis focused: 7/7
  - TypeScript PostgreSQL focused: 28/28
  - Rust Redis focused: 4/4
  - Rust PostgreSQL focused: 3/3
- TypeScript proxy RED: the no-replay test failed with
  `redisCommandMatcher is not a function`.
- Rust proxy RED: compilation failed because `redis_command_matches`,
  `drop_next_response`, and `matched_commands` did not exist.
- A TypeScript long-outage RED exposed an accept-then-reset proxy race:
  command status was `ready` while the stream was non-writable, whereas
  PubSub recovered. Chronology showed the refused socket emitting healthy
  without a later reconnect. Refusal was changed to close/rebind the listener,
  producing real connection refusal.
- A later stability run exposed a test-stage race caused by closing sockets on
  entry to blackhole and treating unrelated accepts as command progress.
  TypeScript blackhole now keeps established sockets and dynamically discards
  traffic, making the interrupted command deterministic.
- No production bug or production change was required.

## Verification

All Rust commands used:

`CARGO_TARGET_DIR=D:\Projects\weightwave\taskcast\rust\target`

Final focused suites were each run three consecutive times after the last
behavioral changes:

- `pnpm --filter @taskcast/redis test -- tests/managed.test.ts`
  - 3 x 11/11 passed
- `pnpm --filter @taskcast/postgres test -- tests/health.test.ts`
  - 3 x 29/29 passed
- `cargo test -p taskcast-redis --test reconnect`
  - 3 x 8/8 passed
- `cargo test -p taskcast-postgres --test health`
  - 3 x 4/4 passed

Additional successful verification:

- `pnpm --filter @taskcast/redis test`: 85/85 passed.
- `pnpm --filter @taskcast/postgres test`: 106/106 passed.
- `cargo test -p taskcast-redis`: all 77 tests passed (18 unit, 5 concurrent,
  8 reconnect, and 46 store tests; no warnings).
- `pnpm --filter @taskcast/redis build`
- `pnpm --filter @taskcast/postgres build`
- direct TypeScript check of the three changed test files with `tsc --noEmit`
- direct `rustfmt --edition 2021 --check` of the three changed Rust files
- `git diff --check`

## Environment incidents and limitations

No Docker service or user resource was restarted, stopped, or deleted.

1. During Node repetition, Testcontainers created a Ryuk container whose
   exposed `8080/tcp` had no host mapping. Standard commands then failed with
   `No host port found for host IP` / `Expected Reaper to map exposed port
   8080`. Read-only diagnosis confirmed the Docker daemon was healthy and the
   Ryuk mapping itself was absent. Node verification therefore used
   `TESTCONTAINERS_RYUK_DISABLED=true`; every test-owned container was still
   stopped by its suite/finally cleanup.
2. `cargo test -p taskcast-postgres` passed its 25 unit tests and 4 health
   tests, but all 27 pre-existing `store_tests` failed in their common setup
   while Docker tried to pull uncached `postgres:11-alpine` and returned
   `unexpected EOF`. Cached PostgreSQL images were 16, 16-alpine, and 18.
   The failure occurred before store assertions and is unrelated to Task 8.
3. One final Rust Redis package run had a single transient Testcontainers
   `PortNotExposed { port: Tcp(6379) }` during setup of an existing store test.
   Testcontainers had already cleaned that container when inspected. The
   exact failed test then passed alone, and the next complete Rust Redis
   package run passed all 77 tests.

Unresolved product issues: none. The two remaining limitations are external
Docker/Testcontainers environment failures described above.
