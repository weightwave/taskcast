# Terminal retention implementation and verification

Implementation and the independent review fix pass are complete. The full TypeScript package coverage gates now pass without lowered thresholds or added exclusions. Taskcast draft PR #67 and WonderAgent PR #244 are pushed; final remote CI and changed-line coverage are still being verified. No merge, release, deployment or production cleanup has been performed.

## Scope and revisions

- Taskcast branch: `codex/terminal-retention`, base `20a09bb1933a5422673b9fe66622de534bfe591c`.
- Implementation commits: `7e0c539`, `e9cd808`, `ecbd68b`, `d17aaa7`, `2b17a30`, plus review fixes `f24d81d` and subsequent coverage/CI fixes recorded on PR #67.
- Companion WonderAgent branch: `codex/taskcast-retention-design`; notification compaction commits `e76ba329` and `98fb7820`. The original independent review passed. For remote integration, the six relevant files were extracted onto current origin/dev as `codex/search-notification-compaction`; unrelated local dev history and its summary-test fixture were excluded. On this new base, API 179 and desktop 69 targeted tests, both typechecks and affected lint pass.
- Taskcast cleanup remains disabled by default. New tasks snapshot type/status policies; task overrides replace defaults, and empty rules opt out. Old tasks and imported archives remain unenrolled.
- TypeScript and Rust implement the same bounded events/all cleanup, durable expiry marker, REST/SSE semantics and protected archive overwrite. Search 24-hour/7-day policies are documented examples, not deployment changes.

## Independent review and final fixes

The whole-branch reviewer assessed core requirements, actual usage and existing constraints, explicitly avoiding speculative mechanisms. Two Important findings were accepted and fixed; no deferred Minor findings were reported.

1. **Busy tasks could starve later due tasks.** Ordering only by the original deadline repeatedly selected the same busy tasks after their retry delay elapsed. Both PostgreSQL and memory adapters now order by the later of the policy deadline and retry deadline. Regression tests failed before the change and pass in both runtimes. The Rust memory regression uses a strictly later millisecond retry deadline to avoid an unrelated timestamp tie.
2. **Legacy writes could contaminate overwrite imports.** An imported task has a new creation token but no retention enrollment. Writes without captured context were still accepted. Both runtimes now require matching context whenever the stored task has a token, preserving compatibility for genuinely tokenless legacy rows. PostgreSQL overwrite regression tests verify rejection of both old-token and unscoped writes.
3. **Author-found WorkerManager regression.** Claim and decline persisted tasks without context. Both implementations now capture the creation token before mutation and pass it to persistence. The enrolled claim/decline regression failed before this fix and passes afterward. Existing adapter tests that intentionally mutate a claimed task now provide the token they claimed.
4. **Full-stack test migrations.** The core test fixture hard-coded migrations 001–005, missing 006. It now invokes the real migration runner. Additional real Redis/PostgreSQL core tests cover hot and expired archive overwrite, series/results preservation, fresh identity, stripped enrollment, and lease-loss retry. A memory regression verifies that stale cached terminal state cannot override durable expiry even after cleanup is disabled.

## Final verification

Verified on Node 22.23.3 with pnpm 10.30.3 and isolated Docker databases. Rust network tests used `NO_PROXY=127.0.0.1,localhost`. The full TypeScript run included memory projection atomicity and bounded-retry fixes. Subsequent targeted package coverage and Rust regression runs include the final changed-line tests and integer TTL timestamp fix; unchanged package results were reused. CI fixture changes are verified separately below.

| Check | Result |
| --- | --- |
| `pnpm build` | Passed |
| `pnpm lint` | Passed |
| Package tests through each existing coverage config | 2,435 tests passed across all 12 package configs with TESTCONTAINERS=1; all configured coverage gates passed |
| Core tests | 921 passed; 100% lines/functions, 97.13% branches |
| PostgreSQL tests | 160 passed in the final package coverage run, including cleanup and migration integration |
| Server Redis integration | Included in the final 531-test server coverage run, no skips |
| Rust core `cargo test -p taskcast-core` | 682 passed |
| Rust PostgreSQL `store_tests` and `cleanup_store` | 67 passed against isolated PostgreSQL |
| `cargo build -p taskcast-cli` | Passed |
| Affected product `cargo clippy -p taskcast-core -p taskcast-postgres -p taskcast-server -- -D warnings` | Passed |
| Real Node/Rust CLI parity with Redis/PostgreSQL | 2 passed, no skips |
| Migration generation, CI YAML, coverage-runner good/bad tests | Passed earlier on unchanged source/configuration; results reused |

The cross-process parity suite verifies all live notifications arrive while latest history compacts per entity, normal accumulate/keep-all semantics, policy snapshots, empty overrides, legacy opt-out, failed/timeout differences, events/all expiry, API/SSE/export behavior, late writes, overwrite and restart after partial deletion.

### Coverage and remote verification

The initial full TypeScript run exposed previously unenforced core/CLI/server/SQLite gates. Public-contract tests now cover recovery, archive restore, adapter rejection atomicity, lifecycle timers, HTTP/SSE failure paths and configuration boundaries. The final full run exits 0: core/server/SQLite have 100% lines/functions; CLI meets its configured 100% line requirement. No coverage thresholds or source exclusions changed.

The added tests found a real memory adapter bug: a conflicting worker assignment rejected terminal projection after task/events had already changed. TypeScript and Rust now validate before mutation; two regressions in each runtime failed before and pass after the fix, and the affected Rust suites total 10 passing tests.

At pushed revision `524e281`, Rust coverage and Clippy, release build and all three E2E jobs pass. The TS job reflects the earlier coverage gaps, which are now fixed locally. Rust migration-readiness and parity-fixture startup failures are fixed locally: all four migration tests pass with bounded health polling, and both hot/cold parity tests pass using the prebuilt release fixture. Startup/exit diagnostic probes also report the real failure. Final remote confirmation is pending. Codecov changed-line coverage remains a separate merge gate and has not yet passed. Local package line coverage does not substitute for that combined TS/Rust check.

Further changed-line tests exercise concurrent deletion during REST/SSE replay, rejected imports after identity/epoch changes, adapter compatibility and cleanup lease failures. They exposed fractional Rust TTL completion timestamps that made an explicitly configured timeout cleanup rule ineligible. The TTL clock now uses integer milliseconds; the real timeout-to-cleanup regression and related core suites pass. A focused independent review of the five product-file changes found no blocking issues, including Rust assignment-lock ordering and three-attempt retry behavior. Final affected coverage: core 921, server 531, client 27, PostgreSQL 160 tests; all four commands exit 0. Together with the unchanged passing package results, this is 2,476 tests. Intersecting the final TypeScript LCOV with added product lines finds no zero-hit branches. Rust core affected suites pass 23 tests, server suites 10, PostgreSQL integration 8 plus one pure deadline unit test. Final affected product Clippy passes.

The optional broader Rust `--all-targets` clippy run finds existing warnings in `proptest_filter.rs`, `series_collapse.rs` and `archive.rs`; the corresponding expressions were verified in base `20a09bb`. Earlier workspace clippy also found an unchanged macOS-only CLI warning. These are distinct from the passing affected product clippy command and from the passing Linux CI Clippy result at `524e281`.

## Implementation rulings and practical limits

1. Rust adds a cleanup builder instead of changing required engine options, and optional trait methods preserve third-party source compatibility. Enabled cleanup still requires complete adapter capabilities. Incorrect capability declarations fail startup rather than silently enabling partial cleanup.
2. Playground assets are built before Rust checks. Only relevant Rust formatting is changed; unrelated formatting/lint drift is not folded into this feature.
3. Durable expiry is committed before physical deletion. This prevents partial-history reads during retries; once expiry begins it is authoritative, including while cleanup is disabled.
4. The coordinator checks assignments/outbox/archive readiness before Redis release and rechecks before deletion. Release uses its own lease; cleanup acquires its lease afterward. Rust polls renewal and work concurrently to avoid waiting on a transaction while suspending that transaction.
5. Global SSE remains live-only because neither runtime has global historical replay. Any future global replay must use guarded history reads.
6. Overwrite restore uses PostgreSQL transaction semantics plus the existing renewed storage lease and atomic hot restore. It rotates the creation token and removes enrollment. Cross-store failure recovery is an explicit retry; no distributed rollback system was added. A failed import may require retry before hot state is fully usable.
7. Tokens are generated in adapters to avoid requiring an unavailable PostgreSQL UUID extension. Tokens remain opaque generation identities.
8. The existing Rust integration job already includes the new parity suite. Its dependency path triggers were expanded; the separate E2E workflow was not duplicated.
9. The root coverage entry now runs every package config once, preserving includes/excludes and thresholds. Previously unenforced gaps now block the gate. The final local package run passes; the separate remote changed-line gate must also pass before merge.
10. Terminal publish preserves the existing HTTP 400 contract. Only expired archive export introduces HTTP 409 with `TASKCAST_HISTORY_EXPIRED`.
11. Existing same-state verification was reused rather than repeated at every commit. Detailed local command logs remain in this worktree's ignored `.superpowers/sdd/2026-09-28-taskcast-terminal-retention/` directory while coverage/CI verification is still pending.

## Remaining work before release

- Complete changed-line coverage and CI verification on the final pushed revision.
- Obtain the intended merge/release/deployment instruction. Taskcast PR #67 is pushed; no release or deployment is authorized by pushing.
- Deploy migration 006 and all v3 writers before enabling cleanup in an approved environment.
- Treat old-task enrollment and any production deletion as a separate, reviewed operation. This version contains no bulk legacy enrollment switch.
