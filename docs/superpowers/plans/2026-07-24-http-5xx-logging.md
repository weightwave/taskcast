# HTTP 5xx Logging Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the TypeScript and Rust Taskcast servers emit one safe, structured stderr record for every HTTP 5xx response while preserving existing HTTP behavior.

**Architecture:** Each server installs an always-on outer HTTP middleware that observes the final response and owns the single log emission. The Rust `AppError` response carries private typed failure metadata through response extensions; TypeScript uses the thrown error when available and stable route-based categorization otherwise. Both implementations share the same JSON field contract, sanitization limits, log-level values, and injectable logger pattern.

**Tech Stack:** TypeScript, Hono, Vitest, Rust, Axum, Serde, Regex, Tokio, Cargo test, pnpm, Changesets.

## Global Constraints

- Update the Node.js/TypeScript and Rust implementations in the same change.
- Emit exactly one record for every final HTTP status from 500 through 599.
- Do not log 2xx, 3xx, or 4xx responses.
- Write JSON lines to stderr by default.
- Keep HTTP status codes, headers, and bodies unchanged.
- Never log query strings, headers, cookies, authorization data, or request/response bodies.
- Limit error text to 2,048 Unicode scalar values and redact URL userinfo as `scheme://***@host`.
- Keep `--verbose` request logging independent and opt-in.
- Accept `TASKCAST_LOG_LEVEL` values `debug`, `info`, `warn`, and `error` case-insensitively; default to `info`; reject invalid non-empty values.
- Add tests before production code and observe each new test fail for the expected missing behavior.
- Do not add Redis/PostgreSQL retry or reconnect behavior in this change.

---

## File Map

### New files

- `packages/server/src/middleware/http-failure-logger.ts` — TypeScript log contract, sanitization, level parsing, default sink, and Hono middleware.
- `packages/server/tests/http-failure-logger.test.ts` — TypeScript middleware and server-wiring regressions.
- `rust/taskcast-server/src/http_failure.rs` — Rust log contract, sanitization, level parsing, logger trait/implementations, response metadata, and Axum middleware.
- `rust/taskcast-server/tests/http_failure_logger.rs` — Rust middleware and `AppError` integration regressions.
- `.changeset/quiet-servers-report.md` — Patch release note for `@taskcast/server` and `@taskcast/cli`.

### Modified files

- `packages/server/src/index.ts` — Export the TypeScript logging API, extend `TaskcastServerOptions`, and install the middleware.
- `packages/cli/src/commands/start.ts` — Resolve `TASKCAST_LOG_LEVEL` from the supplied environment and pass it to the server.
- `packages/cli/tests/unit/start-command.test.ts` — Verify default, explicit, mixed-case, and invalid log levels.
- `rust/taskcast-server/Cargo.toml` — Move/add `regex` as a runtime workspace dependency.
- `rust/taskcast-server/src/lib.rs` — Export the Rust logging API.
- `rust/taskcast-server/src/error.rs` — Attach typed private metadata to 5xx `AppError` responses.
- `rust/taskcast-server/src/app.rs` — Add an injectable app constructor and install the middleware.
- `rust/taskcast-cli/src/commands/start.rs` — Resolve `TASKCAST_LOG_LEVEL` and construct the filtered stderr logger.
- `README.md`, `README.zh.md`, `packages/cli/README.md`, `docs/guide/deployment.md`, `docs/guide/deployment.zh.md` — Document effective log-level validation and always-on 5xx JSON records.

---

### Task 1: TypeScript HTTP Failure Logging

**Files:**

- Create: `packages/server/src/middleware/http-failure-logger.ts`
- Create: `packages/server/tests/http-failure-logger.test.ts`
- Modify: `packages/server/src/index.ts`

**Interfaces:**

- Produces:
  - `type LogLevel = 'debug' | 'info' | 'warn' | 'error'`
  - `type HttpFailureKind = 'store' | 'archive' | 'internal'`
  - `interface HttpFailureLog`
  - `type HttpFailureLogger = (record: HttpFailureLog) => void`
  - `parseLogLevel(value?: string): LogLevel`
  - `sanitizeErrorMessage(value: string): string | undefined`
  - `createHttpFailureLogger(options?: HttpFailureLoggerOptions): MiddlewareHandler`
- Consumed later by `packages/cli/src/commands/start.ts`.

- [ ] **Step 1: Write the TypeScript failing middleware tests**

Create `packages/server/tests/http-failure-logger.test.ts` with focused cases:

```ts
import { describe, expect, it, vi } from 'vitest'
import { Hono } from 'hono'
import {
  createHttpFailureLogger,
  createTaskcastApp,
  parseLogLevel,
  sanitizeErrorMessage,
} from '../src/index.js'
import {
  MemoryBroadcastProvider,
  MemoryShortTermStore,
  TaskEngine,
} from '@taskcast/core'
import type { HttpFailureLog } from '../src/index.js'

function collectingApp(
  route: (app: Hono) => void,
): { app: Hono; records: HttpFailureLog[] } {
  const records: HttpFailureLog[] = []
  const app = new Hono()
  app.use('*', createHttpFailureLogger({
    logLevel: 'info',
    logger: (record) => records.push(record),
  }))
  route(app)
  return { app, records }
}

describe('HTTP failure logging', () => {
  it('logs a thrown /tasks failure exactly once without changing the response', async () => {
    const { app, records } = collectingApp((router) => {
      router.get('/tasks', () => {
        throw new Error(
          'redis://admin:secret@redis.example.com:6379 broken pipe',
        )
      })
    })

    const response = await app.request('/tasks?access_token=do-not-log', {
      headers: { authorization: 'Bearer do-not-log' },
    })

    expect(response.status).toBe(500)
    expect(await response.text()).toBe('Internal Server Error')
    expect(records).toHaveLength(1)
    expect(records[0]).toMatchObject({
      level: 'error',
      event: 'http_request_failed',
      method: 'GET',
      path: '/tasks',
      status: 500,
      errorKind: 'store',
      error: 'redis://***@redis.example.com:6379 broken pipe',
    })
    expect(JSON.stringify(records[0])).not.toContain('access_token')
    expect(JSON.stringify(records[0])).not.toContain('Bearer')
    expect(JSON.stringify(records[0])).not.toContain('secret')
  })

  it('logs a manually returned 500 exactly once without invented details', async () => {
    const { app, records } = collectingApp((router) => {
      router.post('/manual', (c) => c.text('existing response', 500))
    })

    const response = await app.request('/manual?secret=query-secret', {
      method: 'POST',
      headers: {
        authorization: 'Bearer header-secret',
        'content-type': 'text/plain',
      },
      body: 'body-secret',
    })

    expect(response.status).toBe(500)
    expect(await response.text()).toBe('existing response')
    expect(records).toHaveLength(1)
    expect(records[0]).toMatchObject({
      method: 'POST',
      path: '/manual',
      status: 500,
    })
    expect(records[0]?.error).toBeUndefined()
    expect(records[0]?.errorKind).toBeUndefined()
    const serialized = JSON.stringify(records[0])
    expect(serialized).not.toContain('query-secret')
    expect(serialized).not.toContain('header-secret')
    expect(serialized).not.toContain('body-secret')
  })

  it('logs the upper 5xx boundary', async () => {
    const { app, records } = collectingApp((router) => {
      router.get('/upper-bound', () => new Response(null, { status: 599 }))
    })

    await app.request('/upper-bound')

    expect(records).toHaveLength(1)
    expect(records[0]?.status).toBe(599)
  })

  it('does not log 2xx, 3xx, or 4xx responses', async () => {
    const { app, records } = collectingApp((router) => {
      router.get('/ok', (c) => c.body(null, 200))
      router.get('/redirect', (c) => c.redirect('/ok'))
      router.get('/bad', (c) => c.body(null, 400))
      router.get('/missing', (c) => c.notFound())
    })

    await Promise.all([
      app.request('/ok'),
      app.request('/redirect'),
      app.request('/bad'),
      app.request('/missing'),
    ])
    expect(records).toEqual([])
  })

  it.each(['debug', 'info', 'warn', 'error'] as const)(
    'emits error records at the %s threshold',
    async (logLevel) => {
      const records: HttpFailureLog[] = []
      const app = new Hono()
      app.use('*', createHttpFailureLogger({
        logLevel,
        logger: (record) => records.push(record),
      }))
      app.get('/failure', (c) => c.body(null, 500))

      await app.request('/failure')

      expect(records).toHaveLength(1)
    },
  )

  it.each([
    ['/tasks/import', 'archive'],
    ['/tasks/task-1/archive', 'archive'],
    ['/tasks', 'store'],
    ['/tasks/task-1', 'store'],
    ['/events', 'store'],
    ['/workers/ws', 'store'],
    ['/other', 'internal'],
  ] as const)('classifies an error at %s as %s', async (path, errorKind) => {
    const { app, records } = collectingApp((router) => {
      router.get(path, () => {
        throw new Error('failure')
      })
    })

    await app.request(path)

    expect(records[0]?.errorKind).toBe(errorKind)
  })

  it('uses stderr JSON logging by default', async () => {
    const stderr = vi.spyOn(console, 'error').mockImplementation(() => {})
    const app = new Hono()
    app.use('*', createHttpFailureLogger())
    app.get('/failure', (c) => c.body(null, 500))

    await app.request('/failure')

    expect(stderr).toHaveBeenCalledTimes(1)
    expect(JSON.parse(String(stderr.mock.calls[0]?.[0]))).toMatchObject({
      event: 'http_request_failed',
      status: 500,
    })
    stderr.mockRestore()
  })

  it('truncates error text by Unicode scalar value', () => {
    const message = `${'😀'.repeat(2048)}tail`
    expect(Array.from(sanitizeErrorMessage(message) ?? '')).toHaveLength(2048)
    expect(sanitizeErrorMessage(message)).not.toContain('tail')
    expect(sanitizeErrorMessage('')).toBeUndefined()
  })

  it('parses documented log levels case-insensitively', () => {
    expect(parseLogLevel(undefined)).toBe('info')
    expect(parseLogLevel('DEBUG')).toBe('debug')
    expect(parseLogLevel('Info')).toBe('info')
    expect(parseLogLevel('warn')).toBe('warn')
    expect(parseLogLevel('error')).toBe('error')
    expect(() => parseLogLevel('trace')).toThrow(
      'invalid TASKCAST_LOG_LEVEL "trace"',
    )
  })

  it('is installed by createTaskcastApp', async () => {
    class BrokenStore extends MemoryShortTermStore {
      override async listTasks(): Promise<never> {
        throw new Error('broken pipe')
      }
    }

    const records: HttpFailureLog[] = []
    const shortTermStore = new BrokenStore()
    const engine = new TaskEngine({
      shortTermStore,
      broadcast: new MemoryBroadcastProvider(),
    })
    const taskcast = createTaskcastApp({
      engine,
      shortTermStore,
      auth: { mode: 'none' },
      errorLogger: (record) => records.push(record),
    })

    const response = await taskcast.app.request('/tasks')

    expect(response.status).toBe(500)
    expect(records).toHaveLength(1)
    expect(records[0]).toMatchObject({
      method: 'GET',
      path: '/tasks',
      status: 500,
      errorKind: 'store',
      error: 'broken pipe',
    })
    taskcast.stop()
  })
})
```

- [ ] **Step 2: Run the TypeScript test and verify RED**

Run:

```bash
pnpm --filter @taskcast/server test -- tests/http-failure-logger.test.ts
```

Expected: FAIL because `createHttpFailureLogger`, `parseLogLevel`,
`sanitizeErrorMessage`, `HttpFailureLog`, and `TaskcastServerOptions.errorLogger`
do not exist.

- [ ] **Step 3: Implement the TypeScript log contract and middleware**

Create `packages/server/src/middleware/http-failure-logger.ts`:

```ts
import type { MiddlewareHandler } from 'hono'

export type LogLevel = 'debug' | 'info' | 'warn' | 'error'
export type HttpFailureKind = 'store' | 'archive' | 'internal'

export interface HttpFailureLog {
  timestamp: string
  level: 'error'
  event: 'http_request_failed'
  method: string
  path: string
  status: number
  errorKind?: HttpFailureKind
  error?: string
}

export type HttpFailureLogger = (record: HttpFailureLog) => void

export interface HttpFailureLoggerOptions {
  logLevel?: LogLevel
  logger?: HttpFailureLogger
}

const MAX_ERROR_SCALARS = 2048
const URL_USERINFO = /([a-z][a-z0-9+.-]*:\/\/)[^@\s/]+@/giu

export function parseLogLevel(value?: string): LogLevel {
  const normalized = value?.trim().toLowerCase() || 'info'
  if (
    normalized === 'debug' ||
    normalized === 'info' ||
    normalized === 'warn' ||
    normalized === 'error'
  ) {
    return normalized
  }
  throw new Error(
    `invalid TASKCAST_LOG_LEVEL "${value}"; expected debug, info, warn, or error`,
  )
}

export function sanitizeErrorMessage(value: string): string | undefined {
  const redacted = value.replace(URL_USERINFO, '$1***@')
  const truncated = Array.from(redacted).slice(0, MAX_ERROR_SCALARS).join('')
  return truncated.length > 0 ? truncated : undefined
}

function inferErrorKind(path: string): HttpFailureKind {
  if (path === '/tasks/import' || /\/tasks\/[^/]+\/archive$/.test(path)) {
    return 'archive'
  }
  if (
    path === '/tasks' ||
    path.startsWith('/tasks/') ||
    path === '/events' ||
    path.startsWith('/workers')
  ) {
    return 'store'
  }
  return 'internal'
}

function errorMessage(error?: Error): string | undefined {
  return error ? sanitizeErrorMessage(error.message) : undefined
}

function defaultLogger(record: HttpFailureLog): void {
  console.error(JSON.stringify(record))
}

export function createHttpFailureLogger(
  options: HttpFailureLoggerOptions = {},
): MiddlewareHandler {
  const logger = options.logger ?? defaultLogger

  function emit(
    method: string,
    path: string,
    status: number,
    error?: unknown,
  ): void {
    const typedError = error instanceof Error ? error : undefined
    const message = errorMessage(typedError)
    const record: HttpFailureLog = {
      timestamp: new Date().toISOString(),
      level: 'error',
      event: 'http_request_failed',
      method,
      path,
      status,
    }
    if (typedError !== undefined) record.errorKind = inferErrorKind(path)
    if (message !== undefined) record.error = message
    logger(record)
  }

  return async (c, next) => {
    const method = c.req.method
    const path = c.req.path
    await next()

    if (c.res.status >= 500 && c.res.status <= 599) {
      emit(method, path, c.res.status, c.error)
    }
  }
}
```

Modify `packages/server/src/index.ts`:

```ts
import {
  createHttpFailureLogger,
  type HttpFailureLogger,
  type LogLevel,
} from './middleware/http-failure-logger.js'

export {
  createHttpFailureLogger,
  parseLogLevel,
  sanitizeErrorMessage,
} from './middleware/http-failure-logger.js'
export type {
  HttpFailureKind,
  HttpFailureLog,
  HttpFailureLogger,
  HttpFailureLoggerOptions,
  LogLevel,
} from './middleware/http-failure-logger.js'
```

Extend `TaskcastServerOptions`:

```ts
  /** Minimum server log level. Defaults to info. */
  logLevel?: LogLevel
  /** Structured 5xx log sink. Defaults to one JSON line on stderr. */
  errorLogger?: HttpFailureLogger
```

Install the failure middleware immediately after creating the Hono app, before
verbose logging and all routes:

```ts
  app.use('*', createHttpFailureLogger({
    logLevel: opts.logLevel ?? 'info',
    ...(opts.errorLogger ? { logger: opts.errorLogger } : {}),
  }))
```

- [ ] **Step 4: Run focused TypeScript tests and verify GREEN**

Run:

```bash
pnpm --filter @taskcast/server test -- tests/http-failure-logger.test.ts tests/verbose-logger.test.ts
pnpm --filter @taskcast/server build
```

Expected: all selected tests PASS and the package build exits 0.

- [ ] **Step 5: Commit Task 1**

```bash
git add packages/server/src/index.ts \
  packages/server/src/middleware/http-failure-logger.ts \
  packages/server/tests/http-failure-logger.test.ts
git commit -m "fix(server): log TypeScript HTTP 5xx responses"
```

---

### Task 2: Rust HTTP Failure Logging

**Files:**

- Create: `rust/taskcast-server/src/http_failure.rs`
- Create: `rust/taskcast-server/tests/http_failure_logger.rs`
- Modify: `rust/taskcast-server/Cargo.toml`
- Modify: `rust/taskcast-server/src/lib.rs`
- Modify: `rust/taskcast-server/src/error.rs`
- Modify: `rust/taskcast-server/src/app.rs`

**Interfaces:**

- Produces:
  - `enum LogLevel`
  - `enum HttpFailureKind`
  - `struct HttpFailureLog`
  - `trait HttpFailureLogger`
  - `struct StderrHttpFailureLogger`
  - `struct CollectingHttpFailureLogger`
  - `struct HttpFailureDetail` with crate-private fields
  - `http_failure_logger_middleware`
  - `create_app_with_failure_logger(...)`
- Consumed later by `rust/taskcast-cli/src/commands/start.rs`.

- [ ] **Step 1: Write the Rust failing integration tests**

Create `rust/taskcast-server/tests/http_failure_logger.rs`:

```rust
use std::io;
use std::sync::Arc;

use axum::http::StatusCode;
use axum::middleware;
use axum::routing::{get, post};
use axum::{Json, Router};
use axum_test::TestServer;
use serde_json::{json, Value};
use taskcast_core::{
    BroadcastProvider, EngineError, MemoryShortTermStore, TaskEngine,
    TaskEngineOptions, TaskEvent,
};
use taskcast_server::{
    create_app_with_failure_logger, http_failure_logger_middleware, AppError,
    AuthMode, CollectingHttpFailureLogger, CorsConfig, HttpFailureKind,
    HttpFailureLogger, LogLevel,
};

struct UnsupportedBroadcast;

#[async_trait::async_trait]
impl BroadcastProvider for UnsupportedBroadcast {
    async fn publish(
        &self,
        _channel: &str,
        _event: TaskEvent,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        Ok(())
    }

    async fn subscribe(
        &self,
        _channel: &str,
        _handler: Box<dyn Fn(TaskEvent) + Send + Sync>,
    ) -> Box<dyn Fn() + Send + Sync> {
        Box::new(|| {})
    }
}

async fn broken_pipe() -> Result<Json<Value>, AppError> {
    Err(AppError::Engine(EngineError::Store(Box::new(
        io::Error::new(
            io::ErrorKind::BrokenPipe,
            "redis://admin:secret@redis.example.com:6379 broken pipe",
        ),
    ))))
}

async fn manual_500() -> (StatusCode, &'static str) {
    (StatusCode::INTERNAL_SERVER_ERROR, "existing response")
}

#[tokio::test]
async fn logs_typed_store_500_once_and_preserves_response() {
    let logger = CollectingHttpFailureLogger::default();
    let logger_arc: Arc<dyn HttpFailureLogger> = Arc::new(logger.clone());
    let app = Router::new()
        .route("/tasks", get(broken_pipe))
        .layer(middleware::from_fn_with_state(
            logger_arc,
            http_failure_logger_middleware,
        ));
    let server = TestServer::new(app);

    let response = server
        .get("/tasks?access_token=do-not-log")
        .add_header(
            axum::http::header::AUTHORIZATION,
            axum::http::HeaderValue::from_static("Bearer do-not-log"),
        )
        .await;

    response.assert_status(StatusCode::INTERNAL_SERVER_ERROR);
    response.assert_json(&json!({
        "error": "redis://admin:secret@redis.example.com:6379 broken pipe"
    }));

    let records = logger.records();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].method, "GET");
    assert_eq!(records[0].path, "/tasks");
    assert_eq!(records[0].status, 500);
    assert_eq!(records[0].error_kind, Some(HttpFailureKind::Store));
    assert_eq!(
        records[0].error.as_deref(),
        Some("redis://***@redis.example.com:6379 broken pipe")
    );
    let serialized = serde_json::to_string(&records[0]).unwrap();
    assert!(!serialized.contains("access_token"));
    assert!(!serialized.contains("Bearer"));
    assert!(!serialized.contains("secret"));
}

#[tokio::test]
async fn logs_manual_500_once_without_invented_details() {
    let logger = CollectingHttpFailureLogger::default();
    let logger_arc: Arc<dyn HttpFailureLogger> = Arc::new(logger.clone());
    let app = Router::new()
        .route("/manual", post(manual_500))
        .layer(middleware::from_fn_with_state(
            logger_arc,
            http_failure_logger_middleware,
        ));
    let server = TestServer::new(app);

    let response = server
        .post("/manual?secret=query-secret")
        .add_header(
            axum::http::header::AUTHORIZATION,
            axum::http::HeaderValue::from_static("Bearer header-secret"),
        )
        .text("body-secret")
        .await;

    response.assert_status(StatusCode::INTERNAL_SERVER_ERROR);
    response.assert_text("existing response");
    let records = logger.records();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].method, "POST");
    assert_eq!(records[0].path, "/manual");
    assert_eq!(records[0].status, 500);
    assert!(records[0].error_kind.is_none());
    assert!(records[0].error.is_none());
    let serialized = serde_json::to_string(&records[0]).unwrap();
    assert!(!serialized.contains("query-secret"));
    assert!(!serialized.contains("header-secret"));
    assert!(!serialized.contains("body-secret"));
}

#[tokio::test]
async fn logs_the_upper_5xx_boundary() {
    let logger = CollectingHttpFailureLogger::default();
    let logger_arc: Arc<dyn HttpFailureLogger> = Arc::new(logger.clone());
    let app = Router::new()
        .route(
            "/upper-bound",
            get(|| async { StatusCode::from_u16(599).unwrap() }),
        )
        .layer(middleware::from_fn_with_state(
            logger_arc,
            http_failure_logger_middleware,
        ));
    let server = TestServer::new(app);

    server
        .get("/upper-bound")
        .await
        .assert_status(StatusCode::from_u16(599).unwrap());

    let records = logger.records();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].status, 599);
}

#[tokio::test]
async fn injectable_app_constructor_installs_logger_and_marks_internal_errors() {
    let logger = CollectingHttpFailureLogger::default();
    let logger_arc: Arc<dyn HttpFailureLogger> = Arc::new(logger.clone());
    let engine = Arc::new(TaskEngine::new(TaskEngineOptions {
        short_term_store: Arc::new(MemoryShortTermStore::new()),
        broadcast: Arc::new(UnsupportedBroadcast),
        long_term_store: None,
        hooks: None,
    }));
    let (app, _) = create_app_with_failure_logger(
        engine,
        AuthMode::None,
        None,
        None,
        CorsConfig::default(),
        logger_arc,
    );
    let server = TestServer::new(app);

    let response = server.get("/events").await;

    response.assert_status(StatusCode::NOT_IMPLEMENTED);
    let records = logger.records();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].error_kind, Some(HttpFailureKind::Internal));
    assert_eq!(
        records[0].error.as_deref(),
        Some("Global SSE not supported with this broadcast provider")
    );
}

#[tokio::test]
async fn does_not_log_success_redirect_or_client_error() {
    let logger = CollectingHttpFailureLogger::default();
    let logger_arc: Arc<dyn HttpFailureLogger> = Arc::new(logger.clone());
    let app = Router::new()
        .route("/ok", get(|| async { StatusCode::OK }))
        .route("/redirect", get(|| async { StatusCode::FOUND }))
        .route("/bad", get(|| async { StatusCode::BAD_REQUEST }))
        .layer(middleware::from_fn_with_state(
            logger_arc,
            http_failure_logger_middleware,
        ));
    let server = TestServer::new(app);

    server.get("/ok").await.assert_status(StatusCode::OK);
    server
        .get("/redirect")
        .await
        .assert_status(StatusCode::FOUND);
    server
        .get("/bad")
        .await
        .assert_status(StatusCode::BAD_REQUEST);

    assert!(logger.records().is_empty());
}

#[test]
fn parses_levels_and_truncates_unicode() {
    assert_eq!(LogLevel::parse(None).unwrap(), LogLevel::Info);
    assert_eq!(LogLevel::parse(Some("DEBUG")).unwrap(), LogLevel::Debug);
    assert_eq!(LogLevel::parse(Some("Info")).unwrap(), LogLevel::Info);
    assert_eq!(LogLevel::parse(Some("Warn")).unwrap(), LogLevel::Warn);
    assert_eq!(LogLevel::parse(Some("error")).unwrap(), LogLevel::Error);
    assert!(LogLevel::parse(Some("trace")).is_err());
    for level in [
        LogLevel::Debug,
        LogLevel::Info,
        LogLevel::Warn,
        LogLevel::Error,
    ] {
        assert!(level.allows_error());
    }

    let message = format!("{}tail", "😀".repeat(2048));
    let sanitized = taskcast_server::sanitize_error_message(&message).unwrap();
    assert_eq!(sanitized.chars().count(), 2048);
    assert!(!sanitized.contains("tail"));
    assert!(taskcast_server::sanitize_error_message("").is_none());
}
```

- [ ] **Step 2: Run the Rust test and verify RED**

Run:

```bash
cd rust
cargo test -p taskcast-server --test http_failure_logger
```

Expected: compilation FAIL because the HTTP failure logging module and exports
do not exist.

- [ ] **Step 3: Implement the Rust log contract and middleware**

Move `regex` from `[dev-dependencies]` to `[dependencies]` in
`rust/taskcast-server/Cargo.toml`, using the existing workspace version:

```toml
regex = { workspace = true }
```

Create `rust/taskcast-server/src/http_failure.rs` with these definitions:

```rust
use std::sync::{Arc, Mutex, OnceLock};

use axum::extract::{Request, State};
use axum::middleware::Next;
use axum::response::Response;
use chrono::Utc;
use regex::Regex;
use serde::Serialize;

const MAX_ERROR_SCALARS: usize = 2048;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LogLevel {
    Debug,
    Info,
    Warn,
    Error,
}

impl LogLevel {
    pub fn parse(value: Option<&str>) -> Result<Self, String> {
        let normalized = value.unwrap_or("info").trim().to_ascii_lowercase();
        match normalized.as_str() {
            "" | "info" => Ok(Self::Info),
            "debug" => Ok(Self::Debug),
            "warn" => Ok(Self::Warn),
            "error" => Ok(Self::Error),
            _ => Err(format!(
                "invalid TASKCAST_LOG_LEVEL \"{}\"; expected debug, info, warn, or error",
                value.unwrap_or_default()
            )),
        }
    }

    fn priority(self) -> u8 {
        match self {
            Self::Debug => 10,
            Self::Info => 20,
            Self::Warn => 30,
            Self::Error => 40,
        }
    }

    pub fn allows_error(self) -> bool {
        self.priority() <= Self::Error.priority()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum HttpFailureKind {
    Store,
    Archive,
    Internal,
}

#[derive(Clone, Debug)]
pub(crate) struct HttpFailureDetail {
    pub(crate) error_kind: HttpFailureKind,
    pub(crate) error: String,
}

impl HttpFailureDetail {
    pub(crate) fn new(error_kind: HttpFailureKind, error: impl Into<String>) -> Self {
        Self {
            error_kind,
            error: error.into(),
        }
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HttpFailureLog {
    pub timestamp: String,
    pub level: &'static str,
    pub event: &'static str,
    pub method: String,
    pub path: String,
    pub status: u16,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_kind: Option<HttpFailureKind>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

pub trait HttpFailureLogger: Send + Sync + 'static {
    fn log(&self, record: &HttpFailureLog);
}

pub struct StderrHttpFailureLogger {
    level: LogLevel,
}

impl StderrHttpFailureLogger {
    pub fn new(level: LogLevel) -> Self {
        Self { level }
    }
}

impl HttpFailureLogger for StderrHttpFailureLogger {
    fn log(&self, record: &HttpFailureLog) {
        if self.level.allows_error() {
            eprintln!(
                "{}",
                serde_json::to_string(record)
                    .expect("HttpFailureLog contains only serializable fields")
            );
        }
    }
}

#[derive(Clone, Default)]
pub struct CollectingHttpFailureLogger {
    records: Arc<Mutex<Vec<HttpFailureLog>>>,
}

impl CollectingHttpFailureLogger {
    pub fn records(&self) -> Vec<HttpFailureLog> {
        self.records.lock().unwrap().clone()
    }
}

impl HttpFailureLogger for CollectingHttpFailureLogger {
    fn log(&self, record: &HttpFailureLog) {
        self.records.lock().unwrap().push(record.clone());
    }
}

pub fn sanitize_error_message(value: &str) -> Option<String> {
    static URL_USERINFO: OnceLock<Regex> = OnceLock::new();
    let regex = URL_USERINFO.get_or_init(|| {
        Regex::new(r"(?i)([a-z][a-z0-9+.-]*://)[^@\s/]+@")
            .expect("URL userinfo regex must compile")
    });
    let redacted = regex.replace_all(value, "${1}***@");
    let truncated: String = redacted.chars().take(MAX_ERROR_SCALARS).collect();
    (!truncated.is_empty()).then_some(truncated)
}

pub async fn http_failure_logger_middleware(
    State(logger): State<Arc<dyn HttpFailureLogger>>,
    request: Request,
    next: Next,
) -> Response {
    let method = request.method().to_string();
    let path = request.uri().path().to_string();
    let response = next.run(request).await;

    if response.status().is_server_error() {
        let detail = response.extensions().get::<HttpFailureDetail>();
        let record = HttpFailureLog {
            timestamp: Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
            level: "error",
            event: "http_request_failed",
            method,
            path,
            status: response.status().as_u16(),
            error_kind: detail.map(|value| value.error_kind),
            error: detail.and_then(|value| sanitize_error_message(&value.error)),
        };
        logger.log(&record);
    }

    response
}
```

Modify `rust/taskcast-server/src/error.rs` so its match returns a third
`Option<HttpFailureDetail>` value. Attach it only to 5xx responses:

```rust
use crate::http_failure::{HttpFailureDetail, HttpFailureKind};

let (status, message, detail) = match &self {
    AppError::Engine(e) => match e {
        EngineError::TaskNotFound(msg) => (StatusCode::NOT_FOUND, msg.clone(), None),
        EngineError::TaskConflict(msg) => (
            StatusCode::CONFLICT,
            format!("Task already exists: {msg}"),
            None,
        ),
        EngineError::InvalidTransition { from, to } => (
            StatusCode::CONFLICT,
            format!("Invalid transition: {from:?} \u{2192} {to:?}"),
            None,
        ),
        EngineError::TaskTerminal(status) => (
            StatusCode::CONFLICT,
            format!("Cannot publish to task in terminal status: {status:?}"),
            None,
        ),
        EngineError::InvalidInput(msg) => (StatusCode::BAD_REQUEST, msg.clone(), None),
        EngineError::Archive(error) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            error.to_string(),
            Some(HttpFailureDetail::new(
                HttpFailureKind::Archive,
                error.to_string(),
            )),
        ),
        EngineError::Store(error) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            error.to_string(),
            Some(HttpFailureDetail::new(
                HttpFailureKind::Store,
                error.to_string(),
            )),
        ),
    },
    AppError::BadRequest(msg) => (StatusCode::BAD_REQUEST, msg.clone(), None),
    AppError::NotFound(msg) => (StatusCode::NOT_FOUND, msg.clone(), None),
    AppError::Forbidden => (StatusCode::FORBIDDEN, "Forbidden".to_string(), None),
    AppError::MissingToken => (
        StatusCode::UNAUTHORIZED,
        "Missing Bearer token".to_string(),
        None,
    ),
    AppError::InvalidToken => (
        StatusCode::UNAUTHORIZED,
        "Invalid or expired token".to_string(),
        None,
    ),
    AppError::NotImplemented(msg) => (
        StatusCode::NOT_IMPLEMENTED,
        msg.clone(),
        Some(HttpFailureDetail::new(
            HttpFailureKind::Internal,
            msg.clone(),
        )),
    ),
};

let mut response = (status, axum::Json(json!({ "error": message }))).into_response();
if let Some(detail) = detail {
    response.extensions_mut().insert(detail);
}
response
```

Modify `rust/taskcast-server/src/app.rs` without changing existing call sites.
First insert this compatibility wrapper immediately before the current
constructor:

```rust
pub fn create_app(
    engine: Arc<TaskEngine>,
    auth_mode: AuthMode,
    worker_manager: Option<Arc<WorkerManager>>,
    config: Option<TaskcastConfig>,
    cors_config: CorsConfig,
) -> (Router, Option<WsRegistry>) {
    create_app_with_failure_logger(
        engine,
        auth_mode,
        worker_manager,
        config,
        cors_config,
        Arc::new(crate::http_failure::StderrHttpFailureLogger::new(
            crate::http_failure::LogLevel::Info,
        )),
    )
}
```

Then rename the existing `create_app` function to
`create_app_with_failure_logger` and add only the final parameter shown here;
the existing statements from `let auth_mode = Arc::new(auth_mode);` through
the CORS `match` remain in that function in their current order:

```rust
pub fn create_app_with_failure_logger(
    engine: Arc<TaskEngine>,
    auth_mode: AuthMode,
    worker_manager: Option<Arc<WorkerManager>>,
    config: Option<TaskcastConfig>,
    cors_config: CorsConfig,
    failure_logger: Arc<dyn crate::http_failure::HttpFailureLogger>,
) -> (Router, Option<WsRegistry>) {
    let auth_mode = Arc::new(auth_mode);
```

Replace the existing final return:

```rust
    (app, ws_registry_out)
```

with this exact outermost middleware installation and return:

```rust
    let app = app.layer(middleware::from_fn_with_state(
        failure_logger,
        crate::http_failure::http_failure_logger_middleware,
    ));

    (app, ws_registry_out)
}
```

Modify `rust/taskcast-server/src/lib.rs`:

```rust
pub mod http_failure;

pub use app::{
    auto_release_worker, create_app, create_app_with_failure_logger,
    dispatch_ws_offer, dispatch_ws_race, start_background_services, AppState,
    BackgroundServices, CorsConfig,
};
pub use http_failure::{
    http_failure_logger_middleware, sanitize_error_message,
    CollectingHttpFailureLogger, HttpFailureKind, HttpFailureLog,
    HttpFailureLogger, LogLevel, StderrHttpFailureLogger,
};
```

- [ ] **Step 4: Run focused Rust tests and verify GREEN**

Run:

```bash
cd rust
cargo fmt --all
cargo test -p taskcast-server --test http_failure_logger
cargo test -p taskcast-server --test server_tests
cargo test -p taskcast-server --test verbose_logger
```

Expected: all selected tests PASS.

- [ ] **Step 5: Commit Task 2**

```bash
git add rust/taskcast-server/Cargo.toml \
  rust/taskcast-server/src/app.rs \
  rust/taskcast-server/src/error.rs \
  rust/taskcast-server/src/http_failure.rs \
  rust/taskcast-server/src/lib.rs \
  rust/taskcast-server/tests/http_failure_logger.rs
git commit -m "fix(server): log Rust HTTP 5xx responses"
```

---

### Task 3: Wire `TASKCAST_LOG_LEVEL` Through Both CLIs

**Files:**

- Modify: `packages/cli/src/commands/start.ts`
- Modify: `packages/cli/tests/unit/start-command.test.ts`
- Modify: `rust/taskcast-cli/src/commands/start.rs`
- Test: `rust/taskcast-server/tests/http_failure_logger.rs`

**Interfaces:**

- Consumes TypeScript `parseLogLevel` and `LogLevel` from Task 1.
- Consumes Rust `LogLevel`, `StderrHttpFailureLogger`, and
  `create_app_with_failure_logger` from Task 2.
- Produces no new public API.

- [ ] **Step 1: Add failing Node.js CLI tests**

In `packages/cli/tests/unit/start-command.test.ts`, add:

```ts
it('passes TASKCAST_LOG_LEVEL to createTaskcastApp', async () => {
  const options: RunStartOptions = {
    broadcast: {},
    shortTermStore: {},
    port: 3721,
    config: {},
    verbose: false,
    playground: false,
    env: { TASKCAST_LOG_LEVEL: 'ERROR' },
  }

  await runStart(options)

  const { createTaskcastApp } = await import('@taskcast/server')
  expect(createTaskcastApp).toHaveBeenCalledWith(
    expect.objectContaining({ logLevel: 'error' }),
  )
})

it('defaults TASKCAST_LOG_LEVEL to info', async () => {
  await runStart({
    broadcast: {},
    shortTermStore: {},
    port: 3721,
    config: {},
    verbose: false,
    playground: false,
    env: {},
  })

  const { createTaskcastApp } = await import('@taskcast/server')
  expect(createTaskcastApp).toHaveBeenCalledWith(
    expect.objectContaining({ logLevel: 'info' }),
  )
})

it('rejects an invalid TASKCAST_LOG_LEVEL before serving', async () => {
  await expect(runStart({
    broadcast: {},
    shortTermStore: {},
    port: 3721,
    config: {},
    verbose: false,
    playground: false,
    env: { TASKCAST_LOG_LEVEL: 'trace' },
  })).rejects.toThrow('invalid TASKCAST_LOG_LEVEL "trace"')

  const { serve } = await import('@hono/node-server')
  expect(serve).not.toHaveBeenCalled()
})
```

Update the existing `@taskcast/server` mock to export a behaviorally accurate
`parseLogLevel` test double:

```ts
parseLogLevel: vi.fn((value?: string) => {
  const normalized = value?.trim().toLowerCase() || 'info'
  if (['debug', 'info', 'warn', 'error'].includes(normalized)) return normalized
  throw new Error(
    `invalid TASKCAST_LOG_LEVEL "${value}"; expected debug, info, warn, or error`,
  )
}),
```

- [ ] **Step 2: Add the failing Rust CLI resolution tests**

Add a pure helper and tests in `rust/taskcast-cli/src/commands/start.rs`.
Write the tests before the helper:

```rust
#[cfg(test)]
mod log_level_tests {
    use super::resolve_log_level;
    use taskcast_server::LogLevel;

    #[test]
    fn defaults_to_info() {
        assert_eq!(resolve_log_level(None).unwrap(), LogLevel::Info);
    }

    #[test]
    fn accepts_case_insensitive_levels() {
        assert_eq!(resolve_log_level(Some("DEBUG")).unwrap(), LogLevel::Debug);
        assert_eq!(resolve_log_level(Some("Warn")).unwrap(), LogLevel::Warn);
        assert_eq!(resolve_log_level(Some("error")).unwrap(), LogLevel::Error);
    }

    #[test]
    fn rejects_invalid_level() {
        assert!(resolve_log_level(Some("trace"))
            .unwrap_err()
            .contains("invalid TASKCAST_LOG_LEVEL"));
    }
}
```

- [ ] **Step 3: Run both CLI tests and verify RED**

Run:

```bash
pnpm --filter @taskcast/cli test -- tests/unit/start-command.test.ts
cd rust
cargo test -p taskcast-cli log_level_tests
```

Expected: TypeScript assertions FAIL because `logLevel` is not passed; Rust
compilation FAIL because `resolve_log_level` does not exist.

- [ ] **Step 4: Implement TypeScript CLI wiring**

Modify the imports in `packages/cli/src/commands/start.ts`:

```ts
import { createTaskcastApp, parseLogLevel } from '@taskcast/server'
```

At the beginning of `runStart`, before auto-migration or server construction:

```ts
const logLevel = parseLogLevel(options.env?.['TASKCAST_LOG_LEVEL'])
```

Add it to `serverOpts`:

```ts
const serverOpts: Parameters<typeof createTaskcastApp>[0] = {
  engine,
  shortTermStore: options.shortTermStore,
  auth,
  config: options.config,
  verbose: options.verbose,
  logLevel,
}
```

- [ ] **Step 5: Implement Rust CLI wiring**

Add this helper near `env_non_empty` in
`rust/taskcast-cli/src/commands/start.rs`:

```rust
fn resolve_log_level(value: Option<&str>) -> Result<taskcast_server::LogLevel, String> {
    taskcast_server::LogLevel::parse(value)
}
```

Resolve it immediately after destructuring `StartArgs`, before opening any
storage connection:

```rust
let log_level = resolve_log_level(
    env_non_empty("TASKCAST_LOG_LEVEL").as_deref(),
)?;
```

Replace the `taskcast_server::create_app(...)` call with:

```rust
let (app, _ws_registry) = taskcast_server::create_app_with_failure_logger(
    engine,
    auth_mode,
    worker_manager,
    file_config_for_server,
    taskcast_server::CorsConfig::default(),
    Arc::new(taskcast_server::StderrHttpFailureLogger::new(log_level)),
);
```

Use the existing local variables at the current call site; do not reload
configuration or clone secrets solely for logging.

- [ ] **Step 6: Run both CLI suites and verify GREEN**

Run:

```bash
pnpm --filter @taskcast/cli test -- tests/unit/start-command.test.ts
pnpm --filter @taskcast/cli build
cd rust
cargo fmt --all
cargo test -p taskcast-cli log_level_tests
cargo test -p taskcast-cli
```

Expected: all tests PASS and both builds exit 0.

- [ ] **Step 7: Commit Task 3**

```bash
git add packages/cli/src/commands/start.ts \
  packages/cli/tests/unit/start-command.test.ts \
  rust/taskcast-cli/src/commands/start.rs
git commit -m "fix(cli): honor Taskcast log level"
```

---

### Task 4: Documentation, Release Note, and Full Verification

**Files:**

- Create: `.changeset/quiet-servers-report.md`
- Modify: `README.md`
- Modify: `README.zh.md`
- Modify: `packages/cli/README.md`
- Modify: `docs/guide/deployment.md`
- Modify: `docs/guide/deployment.zh.md`

**Interfaces:**

- Consumes the completed TypeScript and Rust behavior.
- Produces the release note and operator-facing documentation.

- [ ] **Step 1: Update operator documentation**

In all five environment-variable tables, keep the existing
`TASKCAST_LOG_LEVEL` row and describe it as:

```md
| `TASKCAST_LOG_LEVEL` | Minimum server log level (`debug`, `info`, `warn`, or `error`); invalid values fail startup. HTTP 5xx failures are emitted as structured JSON on stderr. | `info` |
```

Use the equivalent Chinese wording in `README.zh.md`:

```md
| `TASKCAST_LOG_LEVEL` | 服务端最低日志级别（`debug`、`info`、`warn` 或 `error`）；非法值会阻止启动。HTTP 5xx 故障会以结构化 JSON 写入 stderr。 | `info` |
```

Do not document `--verbose` as required for 5xx logging; it remains the
separate all-request diagnostic mode.

- [ ] **Step 2: Add a patch changeset**

Create `.changeset/quiet-servers-report.md`:

```md
---
"@taskcast/server": patch
"@taskcast/cli": patch
---

Log every HTTP 5xx response once as sanitized structured JSON in both the
TypeScript and Rust servers, and validate `TASKCAST_LOG_LEVEL` at startup.
```

- [ ] **Step 3: Run focused parity verification**

Run:

```bash
pnpm --filter @taskcast/server test -- tests/http-failure-logger.test.ts tests/verbose-logger.test.ts
pnpm --filter @taskcast/cli test -- tests/unit/start-command.test.ts
cd rust
cargo test -p taskcast-server --test http_failure_logger
cargo test -p taskcast-server --test verbose_logger
cargo test -p taskcast-cli log_level_tests
```

Expected: all selected tests PASS.

- [ ] **Step 4: Run full TypeScript verification**

From the repository root:

```bash
pnpm lint
pnpm build
pnpm test
pnpm test:coverage
```

Expected: all commands exit 0; coverage meets the repository thresholds with
no uncovered new logging branches.

- [ ] **Step 5: Run full Rust verification**

From `rust/`:

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

Expected: all commands exit 0 with no formatting, lint, or test failures.

- [ ] **Step 6: Run repository hygiene checks**

From the repository root:

```bash
git diff --check
git status --short
git log --oneline --decorate -5
```

Expected: `git diff --check` exits 0; status contains only the intended
documentation/changeset files before the final commit.

- [ ] **Step 7: Commit Task 4**

```bash
git add README.md README.zh.md packages/cli/README.md \
  docs/guide/deployment.md docs/guide/deployment.zh.md \
  .changeset/quiet-servers-report.md
git commit -m "docs: document structured server error logs"
```

- [ ] **Step 8: Perform final verification after the commit**

Run:

```bash
git status --short --branch
git diff --check HEAD^ HEAD
git show --stat --oneline HEAD
```

Expected: the worktree is clean, the branch contains only intended commits,
and the final commit contains the documentation and changeset.
