# Bad Case Test Audit — Design & Implementation Plan

**Date:** 2026-03-07
**Motivation:** Duplicate task ID bug found despite 100% line coverage. Audit revealed systematic gaps in negative/error/edge case testing.

## Scope

Tier 1 (bug-finding) + Tier 2 (robustness) — 22 items across all packages. Each test either discovers a real bug (requiring code fix) or prevents future regressions.

## Tier 1: Bug-Finding Tests

These may uncover real bugs requiring code changes.

| # | Package | Gap | Risk |
|---|---------|-----|------|
| 1 | core/engine | Duplicate task ID silently overwrites | Data loss |
| 2 | core/engine | Store failure mid-transition (shortTerm ok, longTerm/broadcast fails) | Data inconsistency |
| 3 | core/engine | Negative/zero TTL passed to store | Undefined behavior |
| 4 | core/worker-manager | Negative capacity/cost values | Arithmetic overflow |
| 5 | core/heartbeat | disconnectGraceMs: 0 | Immediate trigger, no grace |
| 6 | core/scheduler | checkIntervalMs: 0 or negative | CPU tight loop |
| 7 | server/tasks | POST resolve on non-blocked task | State machine bypass |
| 8 | server/tasks | Concurrent PATCH to conflicting terminal states | Race condition |
| 9 | server/webhook | fetch timeout / DNS unreachable | Hanging promise |
| 10 | sqlite/short-term | claimTask transaction partial failure | Data inconsistency |

## Tier 2: Robustness Tests

Prevent future regressions on known edge cases.

| # | Package | Gap |
|---|---------|-----|
| 11 | core/series | accumulate with non-object data (null, string, array) |
| 12 | core/config | Malformed JSON/YAML config parsing |
| 13 | server/tasks | Malformed JSON request body |
| 14 | server/auth | Malformed Bearer token format |
| 15 | server/sse | 100 concurrent SSE subscribers + rapid events |
| 16 | server/worker-ws | Messages after disconnect, missing fields |
| 17 | client | Non-200 HTTP responses (500/502/503) |
| 18 | client | AbortController mid-stream cancellation |
| 19 | react | taskId/baseUrl change triggers resubscription |
| 20 | server-sdk | Network timeout, 401/403, malformed JSON response |
| 21 | sentry | Sentry SDK throws inside hook |
| 22 | sqlite | SQL injection attempt (verify parameterized queries) |

## Implementation Strategy

Split into 4 parallel work streams by package group:

### Stream A: core (items 1-6, 11-12)
- engine bad cases + worker-manager + heartbeat + scheduler + series + config
- Bug fixes inline when tests reveal issues

### Stream B: server (items 7-9, 13-16)
- HTTP routes + auth + SSE + webhook + worker-ws
- Bug fixes inline

### Stream C: client + react + server-sdk + sentry (items 17-21)
- All client-side packages

### Stream D: sqlite (items 10, 22)
- Storage layer bad cases

## Rules

- Write the test FIRST, verify it fails or passes
- If it reveals a bug: fix the code, verify test now passes
- If it passes (code already handles it correctly): keep the test as regression guard
- All tests in `tests/unit/` using in-memory adapters (no Docker)
- Follow existing test file conventions per package
