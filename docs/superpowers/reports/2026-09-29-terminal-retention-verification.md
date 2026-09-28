# Terminal retention implementation and verification

Implementation and the independent review fix pass are complete locally. **This branch is not ready to merge:** the full TypeScript coverage run exposes failing package thresholds. No thresholds were reduced, no files were excluded, and no remote CI, release, deployment or production cleanup was performed.

## Scope and revisions

- Taskcast branch: `codex/terminal-retention`, base `20a09bb1933a5422673b9fe66622de534bfe591c`.
- Implementation commits: `7e0c539`, `e9cd808`, `ecbd68b`, `d17aaa7`, `2b17a30`, plus the final fixes committed with this report.
- Companion WonderAgent branch: `codex/taskcast-retention-design`; notification compaction commits `e76ba329` and `98fb7820`. Its independent review and affected tests were already complete; those unchanged results were reused.
- Taskcast cleanup remains disabled by default. New tasks snapshot type/status policies; task overrides replace defaults, and empty rules opt out. Old tasks and imported archives remain unenrolled.
- TypeScript and Rust implement the same bounded events/all cleanup, durable expiry marker, REST/SSE semantics and protected archive overwrite. Search 24-hour/7-day policies are documented examples, not deployment changes.

## Independent review and final fixes

The whole-branch reviewer assessed core requirements, actual usage and existing constraints, explicitly avoiding speculative mechanisms. Two Important findings were accepted and fixed; no deferred Minor findings were reported.

1. **Busy tasks could starve later due tasks.** Ordering only by the original deadline repeatedly selected the same busy tasks after their retry delay elapsed. Both PostgreSQL and memory adapters now order by the later of the policy deadline and retry deadline. Regression tests failed before the change and pass in both runtimes. The Rust memory regression uses a strictly later millisecond retry deadline to avoid an unrelated timestamp tie.
2. **Legacy writes could contaminate overwrite imports.** An imported task has a new creation token but no retention enrollment. Writes without captured context were still accepted. Both runtimes now require matching context whenever the stored task has a token, preserving compatibility for genuinely tokenless legacy rows. PostgreSQL overwrite regression tests verify rejection of both old-token and unscoped writes.
3. **Author-found WorkerManager regression.** Claim and decline persisted tasks without context. Both implementations now capture the creation token before mutation and pass it to persistence. The enrolled claim/decline regression failed before this fix and passes afterward. Existing adapter tests that intentionally mutate a claimed task now provide the token they claimed.
4. **Full-stack test migrations.** The core test fixture hard-coded migrations 001–005, missing 006. It now invokes the real migration runner. Additional real Redis/PostgreSQL core tests cover hot and expired archive overwrite, series/results preservation, fresh identity, stripped enrollment, and lease-loss retry. A memory regression verifies that stale cached terminal state cannot override durable expiry even after cleanup is disabled.

## Final verification

Verified on Node 22.23.3 with pnpm 10.30.3 and isolated Docker databases. Rust network tests used `NO_PROXY=127.0.0.1,localhost`. All final feature source changes were present. Later changes were tests, this report and formatting only.

| Check | Result |
| --- | --- |
| `pnpm build` | Passed |
| `pnpm lint` | Passed |
| Package tests through each existing coverage config | 2,249 tests passed across 12 packages; coverage gate failures listed below |
| Core tests | 724 passed |
| PostgreSQL tests | 157 passed, including cleanup and migration integration |
| Server Redis integration | 2 passed with `TESTCONTAINERS=1`; these were skipped by the environment guard in the initial package run and then explicitly executed |
| Rust core `cargo test -p taskcast-core` | 682 passed |
| Rust PostgreSQL `store_tests` and `cleanup_store` | 67 passed against isolated PostgreSQL |
| `cargo build -p taskcast-cli` | Passed |
| Affected product `cargo clippy -p taskcast-core -p taskcast-postgres -p taskcast-server -- -D warnings` | Passed |
| Real Node/Rust CLI parity with Redis/PostgreSQL | 2 passed, no skips |
| Migration generation, CI YAML, coverage-runner good/bad tests | Passed earlier on unchanged source/configuration; results reused |

The cross-process parity suite verifies all live notifications arrive while latest history compacts per entity, normal accumulate/keep-all semantics, policy snapshots, empty overrides, legacy opt-out, failed/timeout differences, events/all expiry, API/SSE/export behavior, late writes, overwrite and restart after partial deletion.

### Coverage gates still blocking merge

Every existing package configuration was executed independently so its actual threshold applies. Tests pass; these four coverage commands exit 1:

| Package | Lines | Functions | Branches | Failing requirements |
| --- | ---: | ---: | ---: | --- |
| core | 89.29% | 96.20% | 86.41% | 100% lines/functions, 90% branches |
| cli | 99.34% | 96.11% | 94.50% | 100% lines |
| server | 97.57% | 97.97% | 93.04% | 100% lines/functions |
| sqlite | 96.06% | 98.21% | 97.44% | 100% lines/functions |

The new cleanup policy, coordinator and PostgreSQL cleanup module each have 100% line/function coverage. This does **not** establish 100% changed-line coverage across all modified existing files. Current reports also show uncovered unchanged storage/engine paths; SQLite source is unchanged by this branch. A baseline coverage run was not performed, so these numbers are not a measured before/after comparison.

Other package coverage commands exited 0. That does not imply every package is above 90%: existing configurations without package thresholds include Redis and dashboard-web. The configured Codecov project/patch gates and Rust coverage gate have not been verified remotely. Do not bypass them to merge.

The optional broader Rust `--all-targets` clippy run finds existing warnings in `proptest_filter.rs`, `series_collapse.rs` and `archive.rs`; the corresponding expressions were verified in base `20a09bb`. Earlier workspace clippy also found an unchanged macOS-only CLI warning. These are distinct from the passing affected product clippy command and from the Linux CI result, which remains unverified.

## Implementation rulings and practical limits

1. Rust adds a cleanup builder instead of changing required engine options, and optional trait methods preserve third-party source compatibility. Enabled cleanup still requires complete adapter capabilities. Incorrect capability declarations fail startup rather than silently enabling partial cleanup.
2. Playground assets are built before Rust checks. Only relevant Rust formatting is changed; unrelated formatting/lint drift is not folded into this feature.
3. Durable expiry is committed before physical deletion. This prevents partial-history reads during retries; once expiry begins it is authoritative, including while cleanup is disabled.
4. The coordinator checks assignments/outbox/archive readiness before Redis release and rechecks before deletion. Release uses its own lease; cleanup acquires its lease afterward. Rust polls renewal and work concurrently to avoid waiting on a transaction while suspending that transaction.
5. Global SSE remains live-only because neither runtime has global historical replay. Any future global replay must use guarded history reads.
6. Overwrite restore uses PostgreSQL transaction semantics plus the existing renewed storage lease and atomic hot restore. It rotates the creation token and removes enrollment. Cross-store failure recovery is an explicit retry; no distributed rollback system was added. A failed import may require retry before hot state is fully usable.
7. Tokens are generated in adapters to avoid requiring an unavailable PostgreSQL UUID extension. Tokens remain opaque generation identities.
8. The existing Rust integration job already includes the new parity suite. Its dependency path triggers were expanded; the separate E2E workflow was not duplicated.
9. The root coverage entry now runs every package config once, preserving includes/excludes and thresholds. Previously unenforced gaps now block the gate. Remaining coverage work must be completed before merge; it is not waived by functional test success.
10. Terminal publish preserves the existing HTTP 400 contract. Only expired archive export introduces HTTP 409 with `TASKCAST_HISTORY_EXPIRED`.
11. Existing same-state verification was reused rather than repeated at every commit. Detailed local command logs remain in this worktree's ignored `.superpowers/sdd/2026-09-28-taskcast-terminal-retention/` directory while coverage/CI verification is still pending.

## Remaining work before release

- Bring the required coverage gates to green and run CI on the final pushed revision.
- Obtain the intended merge/release/deployment instruction; neither repository was pushed or merged for this feature.
- Deploy migration 006 and all v3 writers before enabling cleanup in an approved environment.
- Treat old-task enrollment and any production deletion as a separate, reviewed operation. This version contains no bulk legacy enrollment switch.
