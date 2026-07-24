# HTTP 5xx Logging Design

**Date:** 2026-07-24

## Context

Taskcast 1.5.4 can return an HTTP 500 response containing a storage error
without writing any corresponding server log. In the observed production
incident, the Rust server returned `broken pipe` from `GET /tasks`, but the
Taskcast container emitted only startup and shutdown messages. The caller
logged the error, while the component that owned the failing Redis connection
did not.

The production deployment sets `TASKCAST_LOG_LEVEL=info`, but neither server
implementation currently uses that value for HTTP error logging. Request
logging is instead controlled by the separate `--verbose` flag and is disabled
in production.

The TypeScript and Rust servers must retain equivalent HTTP and operational
behavior, so this change applies to both implementations.

## Goals

- Emit exactly one error log for every completed HTTP response with a status
  code from 500 through 599.
- Include the request method, path, status, error category, and sanitized error
  message when an underlying error is available.
- Produce structured JSON on stderr so log collectors such as SLS can index
  individual fields.
- Preserve all existing response status codes, headers, and bodies.
- Keep verbose request logging independent from always-on server-error logging.
- Make the log sink injectable so tests can assert emitted records without
  intercepting process-global stderr.
- Resolve and validate `TASKCAST_LOG_LEVEL` consistently in the Node.js and
  Rust CLIs.

## Non-goals

- Adding retries or reconnect behavior to Redis, PostgreSQL, or HTTP clients.
- Changing health-check behavior.
- Logging 4xx responses.
- Logging request or response bodies.
- Replacing all existing Taskcast console output with a new logging framework.
- Adding distributed tracing or request IDs in this change.

## Log Contract

Each 5xx response emits one JSON object with this schema:

```json
{
  "timestamp": "2026-07-24T08:21:32.387Z",
  "level": "error",
  "event": "http_request_failed",
  "method": "GET",
  "path": "/tasks",
  "status": 500,
  "errorKind": "store",
  "error": "broken pipe"
}
```

Required fields are `timestamp`, `level`, `event`, `method`, `path`, and
`status`. `errorKind` and `error` are included only when a typed or uncaught
error is available.

The path excludes the query string. Headers, cookies, authorization data, and
request and response bodies are never logged. Error text is limited to 2,048
Unicode scalar values and sanitizes credentials embedded in URL userinfo
(`scheme://user:password@host` becomes `scheme://***@host`). Empty error
messages are omitted.

The logger serializes the record itself. Callers pass structured fields rather
than preformatted JSON, preventing differences between the TypeScript and Rust
implementations.

## TypeScript Design

`@taskcast/server` gains an always-on HTTP failure middleware installed before
all routes. After downstream handling completes, it inspects the final
response. For a 5xx response it emits one `HttpFailureLog` record.

Hono exposes an uncaught route error on the request context. When present, the
middleware derives `errorKind` from the error constructor or a stable fallback
and sanitizes the error message. A route that deliberately constructs a 5xx
response still produces a record with the required request and status fields,
but without invented error details.

`TaskcastServerOptions` gains an optional `errorLogger` callback. Its default
writes one JSON line with `console.error`. Tests inject a collecting callback.
The middleware owns emission so a thrown error that becomes a 500 is not
double-logged by a separate error handler.

The Node.js CLI resolves `TASKCAST_LOG_LEVEL` from its supplied environment,
normalizes it case-insensitively, and defaults to `info`. Supported values are
`debug`, `info`, `warn`, and `error`; an invalid non-empty value fails startup
with a clear message. Every supported threshold permits `error` records, so
5xx logging remains enabled for all documented values.

## Rust Design

`taskcast-server` gains an always-on Axum middleware around all routes. It
captures method and URI path before dispatch, then inspects the final response.
Every 5xx response emits exactly one `HttpFailureLog` through an injected
`HttpFailureLogger`.

`AppError::into_response` attaches private failure metadata to the response
extensions before returning the existing JSON response. The middleware reads
that extension to obtain the stable error category and sanitized message.
Manually constructed 5xx responses have no metadata and therefore log only the
required request and status fields. Error conversion itself does not print,
which prevents duplicate records.

The default logger writes one serialized JSON object per line to stderr. A
collecting implementation is available to server tests. The existing verbose
middleware remains opt-in and unchanged.

The Rust CLI resolves `TASKCAST_LOG_LEVEL` with the same accepted values,
case-normalization, default, and invalid-value failure as the Node.js CLI, then
passes the resolved level into server construction. All supported levels emit
`error` records.

## Error Categories

Categories are deliberately low-cardinality:

- `store` for short-term storage failures such as Redis errors.
- `archive` for long-term storage failures such as PostgreSQL errors.
- `internal` for uncaught or otherwise unclassified failures.

Language-specific type or constructor names are not used as categories because
they would make dashboards and alerts diverge between implementations.

## Testing

Tests follow red-green TDD independently for both implementations.

### Shared behavioral cases

- A short-term store returning `broken pipe` causes the existing 500 response
  and emits exactly one record containing method, path, status, `store`, and
  the sanitized message.
- A manually constructed 500 emits exactly one record without fabricated
  error details.
- A 2xx response emits no failure record.
- A 4xx response emits no failure record.
- Query strings, authorization headers, and request bodies do not appear in
  the serialized record.
- URL credentials are redacted and long messages are truncated.
- The existing HTTP response body and status remain byte-for-byte compatible
  where the test harness permits direct comparison.

### Configuration cases

- Missing `TASKCAST_LOG_LEVEL` resolves to `info`.
- Each documented value is accepted case-insensitively.
- An invalid non-empty value fails startup resolution.
- An `error` record is enabled at every supported threshold.

The focused TypeScript server/CLI and Rust server/CLI suites must pass, followed
by the repository's full typecheck, test suite, formatting checks, and
`git diff --check`.

## Release and Operations

The implementation includes a patch changeset describing the restored 5xx
observability. No deployment manifest change is required: both runtimes emit to
stderr, which the existing Kubernetes log collector already sends to SLS.

After deployment, a controlled test server using a failing in-memory store
should demonstrate one indexed `http_request_failed` event. Production should
not be intentionally forced into a storage failure. Acceptance is complete
when an organic or staging 500 can be correlated by `method`, `path`, `status`,
and `errorKind` without inspecting caller logs.
