# PostgreSQL Release Recovery Fix

## Context

Taskcast 1.6.0 was deployed to the development environment with automatic hot
retention disabled. A synthetic terminal canary successfully archived through
event index 4, finalized its archive receipt, and deleted its task-local Redis
keys. The final PostgreSQL compare-and-set from `releasing` to `cold` failed
with:

```text
storage_integrity_error: Storage metadata timestamps must be PostgreSQL BIGINT values
```

The Rust storage coordinator computes wall-clock milliseconds with
`SystemTime::as_secs_f64() * 1_000.0`. That value commonly has a fractional
component. The PostgreSQL adapter correctly rejects fractional timestamps
before writing a `BIGINT`, so the task remains safely recoverable in
`releasing` after its verified hot data has been deleted.

## Goals

- Generate integer Unix-millisecond lifecycle timestamps in Rust.
- Make an explicit release retry recover an interrupted `releasing` task before
  attempting a new release.
- Keep TypeScript and Rust release behavior identical.
- Preserve the durable release request until release or recovery completes.
- Recover the existing development canary through the normal release API.

## Non-goals

- Do not enable automatic hot retention or durable TTL processing.
- Do not change event limits, series compaction, archive formats, or migrations.
- Do not release the incident task or alter Team9 ownership policy.
- Do not promote this patch to staging or production as part of the development
  canary.

## Design

### Integer lifecycle timestamps

The Rust storage coordinator will derive wall-clock milliseconds with
`SystemTime::duration_since(UNIX_EPOCH).as_millis()` and convert the integer
result to the existing `f64` timestamp type. A small pure helper will make the
fractional-time regression deterministic in unit tests.

TypeScript already uses integer-valued `Date.now()` and needs no clock change.

### Recovery-first explicit release

`TaskEngine.releaseTaskStorage` will retain its existing validation and first
persist the guarded durable release request. It will then call the storage
coordinator's existing recovery operation:

1. If recovery reports `cold`, clear the matching durable request and return
   the recovery result.
2. If recovery reports `hot`, continue with the existing release operation
   using the caller's exact `expectedLastEventIndex` and `inactiveSince`
   preconditions.
3. If recovery or release fails with a transient, busy, fence, or integrity
   error, leave the durable request in place for the lifecycle sweeper.
4. Preserve the existing behavior that clears a stale request after a
   precondition failure.

The same ordering and result semantics will be implemented in TypeScript and
Rust. No new HTTP endpoint or permission scope is introduced.

### Safety properties

- Recovery never fabricates archive coverage. It uses the existing finalized
  watermark and source-presence checks.
- A recovered-hot task receives a new storage epoch before release is retried.
- The original index and inactivity preconditions remain unchanged.
- Reads from durable history remain read-only and do not rehydrate Redis.
- The operator release route remains restricted to `task:manage`.

## Tests

- Rust unit test: a `SystemTime` containing sub-millisecond nanoseconds produces
  an integer millisecond value.
- TypeScript and Rust engine regression tests: a persisted interrupted release
  is recovered by a repeated explicit release call, returns `cold`, and clears
  the durable request.
- Rust PostgreSQL integration test: a real PostgreSQL-backed release reaches
  `cold` and persists an integer `cold_at`, covering the production failure
  boundary.
- Existing storage coordinator, server route, and full workspace suites remain
  green.

## Release and development verification

Add a patch changeset, publish the next fixed Taskcast version, and deploy only
the development environment with automatic retention still disabled. Reissue
the exact canary release request and verify:

- PostgreSQL state is `cold` with archive watermark 4 and no pending request.
- The finalized archive receipt still covers the source.
- Task, history, archive, and terminal SSE reads match the pre-release canonical
  data.
- Repeated reads create no task-local Redis keys.
- Storage readiness remains healthy with protocol version 2.
