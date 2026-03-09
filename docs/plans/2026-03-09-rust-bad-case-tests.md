# Rust Bad Case Tests Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Port ~71 missing bad case / negative tests from TypeScript to Rust, ensuring the Rust server produces identical error behavior.

**Architecture:** All tests are integration tests in `rust/taskcast-server/tests/`. They use `axum_test::TestServer` for HTTP tests and WebSocket testing. New tests are added to existing files where possible, or as new focused test files.

**Tech Stack:** Rust, axum-test (with ws feature), tokio, serde_json, jsonwebtoken

---

### Task 1: Malformed JSON HTTP Tests

**Files:**
- Create: `rust/taskcast-server/tests/malformed_json.rs`

**Why:** TS has 6 tests verifying that malformed JSON bodies on POST /tasks, POST /tasks/:id/events, and PATCH /tasks/:id/status return 400-level errors. Rust has 0 HTTP-layer malformed JSON tests.

**Step 1: Write the tests**

```rust
use std::sync::Arc;

use axum_test::TestServer;
use serde_json::json;
use taskcast_core::{
    MemoryBroadcastProvider, MemoryShortTermStore, TaskEngine, TaskEngineOptions,
};
use taskcast_server::{create_app, AuthMode, CorsConfig};

fn make_server() -> (Arc<TaskEngine>, TestServer) {
    let engine = Arc::new(TaskEngine::new(TaskEngineOptions {
        short_term_store: Arc::new(MemoryShortTermStore::new()),
        broadcast: Arc::new(MemoryBroadcastProvider::new()),
        long_term_store: None,
        hooks: None,
    }));
    let (app, _) = create_app(
        Arc::clone(&engine),
        AuthMode::None,
        None,
        None,
        CorsConfig::default(),
    );
    let server = TestServer::new(app);
    (engine, server)
}

// ─── POST /tasks — malformed JSON ──────────────────────────────────────────

#[tokio::test]
async fn post_tasks_malformed_json_not_json() {
    let (_engine, server) = make_server();
    let response = server
        .post("/tasks")
        .content_type("application/json")
        .bytes("not json".into())
        .await;
    let status = response.status_code().as_u16();
    assert!(status >= 400 && status < 500, "expected 4xx, got {status}");
}

#[tokio::test]
async fn post_tasks_malformed_json_empty_body() {
    let (_engine, server) = make_server();
    let response = server
        .post("/tasks")
        .content_type("application/json")
        .bytes("".into())
        .await;
    let status = response.status_code().as_u16();
    assert!(status >= 400 && status < 500, "expected 4xx, got {status}");
}

#[tokio::test]
async fn post_tasks_malformed_json_truncated() {
    let (_engine, server) = make_server();
    let response = server
        .post("/tasks")
        .content_type("application/json")
        .bytes("{invalid".into())
        .await;
    let status = response.status_code().as_u16();
    assert!(status >= 400 && status < 500, "expected 4xx, got {status}");
}

// ─── POST /tasks/:id/events — malformed JSON ──────────────────────────────

#[tokio::test]
async fn post_events_malformed_json_not_json() {
    let (engine, server) = make_server();
    let task = engine
        .create_task(taskcast_core::CreateTaskInput {
            id: Some("mal-evt-1".into()),
            ..Default::default()
        })
        .await
        .unwrap();
    engine
        .transition_task(&task.id, taskcast_core::TaskStatus::Running, None)
        .await
        .unwrap();

    let response = server
        .post(&format!("/tasks/{}/events", task.id))
        .content_type("application/json")
        .bytes("not json".into())
        .await;
    let status = response.status_code().as_u16();
    assert!(status >= 400 && status < 500, "expected 4xx, got {status}");
}

#[tokio::test]
async fn post_events_malformed_json_truncated() {
    let (engine, server) = make_server();
    let task = engine
        .create_task(taskcast_core::CreateTaskInput {
            id: Some("mal-evt-2".into()),
            ..Default::default()
        })
        .await
        .unwrap();
    engine
        .transition_task(&task.id, taskcast_core::TaskStatus::Running, None)
        .await
        .unwrap();

    let response = server
        .post(&format!("/tasks/{}/events", task.id))
        .content_type("application/json")
        .bytes("{invalid".into())
        .await;
    let status = response.status_code().as_u16();
    assert!(status >= 400 && status < 500, "expected 4xx, got {status}");
}

// ─── PATCH /tasks/:id/status — malformed JSON ─────────────────────────────

#[tokio::test]
async fn patch_status_malformed_json_not_json() {
    let (engine, server) = make_server();
    let task = engine
        .create_task(taskcast_core::CreateTaskInput {
            id: Some("mal-status-1".into()),
            ..Default::default()
        })
        .await
        .unwrap();

    let response = server
        .patch(&format!("/tasks/{}/status", task.id))
        .content_type("application/json")
        .bytes("not json".into())
        .await;
    let status = response.status_code().as_u16();
    assert!(status >= 400 && status < 500, "expected 4xx, got {status}");
}
```

**Step 2: Run tests**

```bash
cd rust && cargo test --package taskcast-server --test malformed_json -- --nocapture
```

Expected: All 6 tests PASS. Axum's `Json` extractor returns 422 (Unprocessable Entity) or 400 for malformed JSON.

**Step 3: Commit**

---

### Task 2: Malformed Bearer Token Tests

**Files:**
- Create: `rust/taskcast-server/tests/malformed_bearer.rs`

**Why:** TS has 7 tests for malformed Authorization headers. Rust only tests "no header" and "invalid token" — missing 5 edge cases for Bearer parsing.

**Step 1: Write the tests**

```rust
use std::sync::Arc;

use axum_test::http::HeaderValue;
use axum_test::TestServer;
use serde_json::json;
use taskcast_core::{
    MemoryBroadcastProvider, MemoryShortTermStore, TaskEngine, TaskEngineOptions,
};
use taskcast_server::{create_app, AuthMode, CorsConfig, JwtConfig};

const JWT_SECRET: &str = "test-secret-key-for-jwt-signing-needs-to-be-long-enough";

fn make_jwt_server() -> TestServer {
    let engine = Arc::new(TaskEngine::new(TaskEngineOptions {
        short_term_store: Arc::new(MemoryShortTermStore::new()),
        broadcast: Arc::new(MemoryBroadcastProvider::new()),
        long_term_store: None,
        hooks: None,
    }));
    let auth_mode = AuthMode::Jwt(JwtConfig {
        algorithm: jsonwebtoken::Algorithm::HS256,
        secret: Some(JWT_SECRET.to_string()),
        public_key: None,
        issuer: None,
        audience: None,
    });
    let (app, _) = create_app(engine, auth_mode, None, None, CorsConfig::default());
    TestServer::new(app)
}

#[tokio::test]
async fn bearer_empty_token_after_space_returns_401() {
    let server = make_jwt_server();
    let response = server
        .post("/tasks")
        .add_header(
            axum_test::http::header::AUTHORIZATION,
            HeaderValue::from_static("Bearer "),
        )
        .json(&json!({}))
        .await;
    response.assert_status(axum_test::http::StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn bearer_no_space_returns_401() {
    let server = make_jwt_server();
    let response = server
        .post("/tasks")
        .add_header(
            axum_test::http::header::AUTHORIZATION,
            HeaderValue::from_static("Bearer"),
        )
        .json(&json!({}))
        .await;
    response.assert_status(axum_test::http::StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn bearer_lowercase_returns_401() {
    let server = make_jwt_server();
    let response = server
        .post("/tasks")
        .add_header(
            axum_test::http::header::AUTHORIZATION,
            HeaderValue::from_static("bearer some-token-value"),
        )
        .json(&json!({}))
        .await;
    response.assert_status(axum_test::http::StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn basic_auth_scheme_returns_401() {
    let server = make_jwt_server();
    let response = server
        .post("/tasks")
        .add_header(
            axum_test::http::header::AUTHORIZATION,
            HeaderValue::from_static("Basic dXNlcjpwYXNz"),
        )
        .json(&json!({}))
        .await;
    response.assert_status(axum_test::http::StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn bearer_extra_whitespace_returns_401() {
    let server = make_jwt_server();
    let response = server
        .post("/tasks")
        .add_header(
            axum_test::http::header::AUTHORIZATION,
            HeaderValue::from_static("Bearer   "),
        )
        .json(&json!({}))
        .await;
    response.assert_status(axum_test::http::StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn no_authorization_header_returns_401_with_message() {
    let server = make_jwt_server();
    let response = server
        .post("/tasks")
        .json(&json!({}))
        .await;
    response.assert_status(axum_test::http::StatusCode::UNAUTHORIZED);
    let body: serde_json::Value = response.json();
    assert_eq!(body["error"], "Missing Bearer token");
}

#[tokio::test]
async fn garbled_token_returns_401_with_message() {
    let server = make_jwt_server();
    let response = server
        .post("/tasks")
        .add_header(
            axum_test::http::header::AUTHORIZATION,
            HeaderValue::from_static("Bearer not.a.valid.jwt.at.all"),
        )
        .json(&json!({}))
        .await;
    response.assert_status(axum_test::http::StatusCode::UNAUTHORIZED);
    let body: serde_json::Value = response.json();
    assert_eq!(body["error"], "Invalid or expired token");
}
```

**Step 2: Run tests**

```bash
cd rust && cargo test --package taskcast-server --test malformed_bearer -- --nocapture
```

Expected: All 7 tests PASS. The Rust auth middleware uses `header.starts_with("Bearer ")` which rejects "Bearer" (no space), "bearer" (lowercase), "Basic" scheme. Empty/whitespace tokens will fail JWT decode.

**Step 3: Commit**

---

### Task 3: WebSocket Bad Message Tests

**Files:**
- Create: `rust/taskcast-server/tests/ws_bad_messages.rs`

**Why:** TS has 10 tests for WebSocket bad messages (wrong types for taskId, unknown message types, empty type, numeric type, etc.). Rust has only 1 (invalid JSON). The Rust serde `#[serde(tag = "type")]` deserialization handles some of these implicitly, but we need to verify the actual behavior matches TS.

**Step 1: Write the tests**

```rust
use std::sync::Arc;

use axum_test::TestServer;
use serde_json::json;
use taskcast_core::worker_manager::{WorkerManager, WorkerManagerOptions};
use taskcast_core::{
    MemoryBroadcastProvider, MemoryShortTermStore, TaskEngine, TaskEngineOptions,
};
use taskcast_server::{create_app, AuthMode, CorsConfig};

fn make_ws_server() -> (Arc<TaskEngine>, Arc<WorkerManager>, TestServer) {
    let store = Arc::new(MemoryShortTermStore::new());
    let broadcast = Arc::new(MemoryBroadcastProvider::new());
    let engine = Arc::new(TaskEngine::new(TaskEngineOptions {
        short_term_store: Arc::clone(&store),
        broadcast: Arc::clone(&broadcast),
        long_term_store: None,
        hooks: None,
    }));
    let manager = Arc::new(WorkerManager::new(WorkerManagerOptions {
        engine: Arc::clone(&engine),
        short_term_store: Arc::clone(&store) as _,
        broadcast: Arc::clone(&broadcast) as _,
        heartbeat_interval_ms: None,
        heartbeat_timeout_ms: None,
        heartbeat_timeout_policy: None,
        defaults: None,
        hooks: None,
    }));
    let (app, _) = create_app(
        Arc::clone(&engine),
        AuthMode::None,
        Some(Arc::clone(&manager)),
        None,
        CorsConfig::default(),
    );
    let server = TestServer::new(app);
    (engine, manager, server)
}

// ─── Wrong types for taskId ────────────────────────────────────────────────

#[tokio::test]
async fn ws_claim_with_numeric_task_id_returns_parse_error() {
    let (_engine, _manager, server) = make_ws_server();
    let mut ws = server.get_websocket("/workers/ws").await.into_websocket().await;

    // Register first
    ws.send_json(&json!({
        "type": "register",
        "matchRule": {},
        "capacity": 5
    })).await;
    let _reg: serde_json::Value = ws.receive_json().await;

    // claim with taskId as number (should fail deserialization)
    ws.send_json(&json!({
        "type": "claim",
        "taskId": 123
    })).await;

    let response: serde_json::Value = ws.receive_json().await;
    assert_eq!(response["type"], "error");
    assert_eq!(response["code"], "PARSE_ERROR");

    ws.close().await;
}

#[tokio::test]
async fn ws_accept_with_boolean_task_id_returns_parse_error() {
    let (_engine, _manager, server) = make_ws_server();
    let mut ws = server.get_websocket("/workers/ws").await.into_websocket().await;

    ws.send_json(&json!({
        "type": "register",
        "matchRule": {},
        "capacity": 5
    })).await;
    let _reg: serde_json::Value = ws.receive_json().await;

    ws.send_json(&json!({
        "type": "accept",
        "taskId": true
    })).await;

    let response: serde_json::Value = ws.receive_json().await;
    assert_eq!(response["type"], "error");
    assert_eq!(response["code"], "PARSE_ERROR");

    ws.close().await;
}

#[tokio::test]
async fn ws_decline_with_null_task_id_returns_parse_error() {
    let (_engine, _manager, server) = make_ws_server();
    let mut ws = server.get_websocket("/workers/ws").await.into_websocket().await;

    ws.send_json(&json!({
        "type": "register",
        "matchRule": {},
        "capacity": 5
    })).await;
    let _reg: serde_json::Value = ws.receive_json().await;

    ws.send_json(&json!({
        "type": "decline",
        "taskId": null
    })).await;

    let response: serde_json::Value = ws.receive_json().await;
    assert_eq!(response["type"], "error");
    assert_eq!(response["code"], "PARSE_ERROR");

    ws.close().await;
}

// ─── Unknown message types ─────────────────────────────────────────────────

#[tokio::test]
async fn ws_unknown_message_type_returns_parse_error() {
    let (_engine, _manager, server) = make_ws_server();
    let mut ws = server.get_websocket("/workers/ws").await.into_websocket().await;

    ws.send_json(&json!({
        "type": "register",
        "matchRule": {},
        "capacity": 5
    })).await;
    let _reg: serde_json::Value = ws.receive_json().await;

    ws.send_json(&json!({ "type": "fly_to_moon" })).await;

    let response: serde_json::Value = ws.receive_json().await;
    assert_eq!(response["type"], "error");
    assert_eq!(response["code"], "PARSE_ERROR");
    // serde will say "unknown variant `fly_to_moon`"
    assert!(response["message"].as_str().unwrap().contains("Invalid message"));

    ws.close().await;
}

#[tokio::test]
async fn ws_empty_type_string_returns_parse_error() {
    let (_engine, _manager, server) = make_ws_server();
    let mut ws = server.get_websocket("/workers/ws").await.into_websocket().await;

    ws.send_json(&json!({ "type": "" })).await;

    let response: serde_json::Value = ws.receive_json().await;
    assert_eq!(response["type"], "error");
    assert_eq!(response["code"], "PARSE_ERROR");

    ws.close().await;
}

#[tokio::test]
async fn ws_numeric_type_returns_parse_error() {
    let (_engine, _manager, server) = make_ws_server();
    let mut ws = server.get_websocket("/workers/ws").await.into_websocket().await;

    ws.send_json(&json!({ "type": 42 })).await;

    let response: serde_json::Value = ws.receive_json().await;
    assert_eq!(response["type"], "error");
    assert_eq!(response["code"], "PARSE_ERROR");

    ws.close().await;
}

// ─── Missing required fields ───────────────────────────────────────────────

#[tokio::test]
async fn ws_claim_missing_task_id_returns_parse_error() {
    let (_engine, _manager, server) = make_ws_server();
    let mut ws = server.get_websocket("/workers/ws").await.into_websocket().await;

    ws.send_json(&json!({
        "type": "register",
        "matchRule": {},
        "capacity": 5
    })).await;
    let _reg: serde_json::Value = ws.receive_json().await;

    // claim without taskId field
    ws.send_json(&json!({ "type": "claim" })).await;

    let response: serde_json::Value = ws.receive_json().await;
    assert_eq!(response["type"], "error");
    assert_eq!(response["code"], "PARSE_ERROR");

    ws.close().await;
}

#[tokio::test]
async fn ws_register_missing_capacity_returns_parse_error() {
    let (_engine, _manager, server) = make_ws_server();
    let mut ws = server.get_websocket("/workers/ws").await.into_websocket().await;

    // register without capacity (required field)
    ws.send_json(&json!({
        "type": "register",
        "matchRule": {}
    })).await;

    let response: serde_json::Value = ws.receive_json().await;
    assert_eq!(response["type"], "error");
    assert_eq!(response["code"], "PARSE_ERROR");

    ws.close().await;
}
```

**Note:** The TS behavior differs from Rust here:
- TS: `register` without capacity defaults to 10, `register` without matchRule defaults to `{}`
- Rust: capacity is a required `u32` field (no default), so missing capacity is a PARSE_ERROR
- This is a **behavior difference** that should be documented but may be acceptable since the Rust serde model is stricter

**Step 2: Run tests**

```bash
cd rust && cargo test --package taskcast-server --test ws_bad_messages -- --nocapture
```

Expected: All 8 tests PASS.

**Step 3: Commit**

---

### Task 4: Webhook Failure Scenario Tests

**Files:**
- Modify: `rust/taskcast-server/src/webhook.rs` (add tests to existing `#[cfg(test)]` module)

**Why:** TS has 9 webhook failure tests. Rust has inline tests for backoff math and signing, but no integration tests for actual HTTP failure scenarios (timeout, DNS failure, retry-then-success, no signature header). Some of these require a real HTTP mock server.

**Step 1: Write the tests**

Add to the existing `#[cfg(test)] mod tests` block in `webhook.rs`:

```rust
#[tokio::test]
async fn send_fails_after_retries_on_server_error() {
    // Start a mock server that always returns 500
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    let call_count = Arc::new(std::sync::atomic::AtomicU32::new(0));
    let call_count_clone = call_count.clone();

    let mock_app = axum::Router::new().route(
        "/hook",
        axum::routing::post(move || async move {
            call_count_clone.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            axum::http::StatusCode::INTERNAL_SERVER_ERROR
        }),
    );
    tokio::spawn(async move {
        axum::serve(listener, mock_app).await.unwrap();
    });

    let delivery = WebhookDelivery::new();
    let event = make_test_event();
    let config = WebhookConfig {
        url: format!("http://{addr}/hook"),
        filter: None,
        secret: None,
        wrap: None,
        retry: Some(RetryConfig {
            retries: 2,
            backoff: BackoffStrategy::Fixed,
            initial_delay_ms: 1,
            max_delay_ms: 1,
            timeout_ms: 5000,
        }),
    };

    let result = delivery.send(&event, &config).await;
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(err.to_string().contains("3 attempts")); // 1 initial + 2 retries
    assert_eq!(call_count.load(std::sync::atomic::Ordering::SeqCst), 3);
}

#[tokio::test]
async fn send_succeeds_after_transient_failure() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    let call_count = Arc::new(std::sync::atomic::AtomicU32::new(0));
    let call_count_clone = call_count.clone();

    let mock_app = axum::Router::new().route(
        "/hook",
        axum::routing::post(move || {
            let count = call_count_clone.clone();
            async move {
                let n = count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                if n < 2 {
                    axum::http::StatusCode::INTERNAL_SERVER_ERROR
                } else {
                    axum::http::StatusCode::OK
                }
            }
        }),
    );
    tokio::spawn(async move {
        axum::serve(listener, mock_app).await.unwrap();
    });

    let delivery = WebhookDelivery::new();
    let event = make_test_event();
    let config = WebhookConfig {
        url: format!("http://{addr}/hook"),
        filter: None,
        secret: None,
        wrap: None,
        retry: Some(RetryConfig {
            retries: 3,
            backoff: BackoffStrategy::Fixed,
            initial_delay_ms: 1,
            max_delay_ms: 1,
            timeout_ms: 5000,
        }),
    };

    let result = delivery.send(&event, &config).await;
    assert!(result.is_ok());
    assert_eq!(call_count.load(std::sync::atomic::Ordering::SeqCst), 3);
}

#[tokio::test]
async fn send_timeout_counts_as_failure() {
    // Server that never responds (sleeps longer than timeout)
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    let call_count = Arc::new(std::sync::atomic::AtomicU32::new(0));
    let call_count_clone = call_count.clone();

    let mock_app = axum::Router::new().route(
        "/hook",
        axum::routing::post(move || {
            let count = call_count_clone.clone();
            async move {
                count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                tokio::time::sleep(std::time::Duration::from_secs(10)).await;
                axum::http::StatusCode::OK
            }
        }),
    );
    tokio::spawn(async move {
        axum::serve(listener, mock_app).await.unwrap();
    });

    let delivery = WebhookDelivery::new();
    let event = make_test_event();
    let config = WebhookConfig {
        url: format!("http://{addr}/hook"),
        filter: None,
        secret: None,
        wrap: None,
        retry: Some(RetryConfig {
            retries: 1,
            backoff: BackoffStrategy::Fixed,
            initial_delay_ms: 1,
            max_delay_ms: 1,
            timeout_ms: 50, // Very short timeout
        }),
    };

    let result = delivery.send(&event, &config).await;
    assert!(result.is_err());
    // Should have attempted 2 times (initial + 1 retry)
    assert_eq!(call_count.load(std::sync::atomic::Ordering::SeqCst), 2);
}

#[tokio::test]
async fn send_dns_failure_retries_and_fails() {
    let delivery = WebhookDelivery::new();
    let event = make_test_event();
    let config = WebhookConfig {
        url: "http://nonexistent.invalid:9999/hook".to_string(),
        filter: None,
        secret: None,
        wrap: None,
        retry: Some(RetryConfig {
            retries: 1,
            backoff: BackoffStrategy::Fixed,
            initial_delay_ms: 1,
            max_delay_ms: 1,
            timeout_ms: 1000,
        }),
    };

    let result = delivery.send(&event, &config).await;
    assert!(result.is_err());
}

#[tokio::test]
async fn send_without_secret_omits_signature_header() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    let had_signature = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let had_signature_clone = had_signature.clone();

    let mock_app = axum::Router::new().route(
        "/hook",
        axum::routing::post(
            move |headers: axum::http::HeaderMap| {
                let sig = had_signature_clone.clone();
                async move {
                    if headers.contains_key("x-taskcast-signature") {
                        sig.store(true, std::sync::atomic::Ordering::SeqCst);
                    }
                    axum::http::StatusCode::OK
                }
            },
        ),
    );
    tokio::spawn(async move {
        axum::serve(listener, mock_app).await.unwrap();
    });

    let delivery = WebhookDelivery::new();
    let event = make_test_event();
    let config = WebhookConfig {
        url: format!("http://{addr}/hook"),
        filter: None,
        secret: None, // No secret
        wrap: None,
        retry: Some(RetryConfig {
            retries: 0,
            backoff: BackoffStrategy::Fixed,
            initial_delay_ms: 0,
            max_delay_ms: 0,
            timeout_ms: 5000,
        }),
    };

    delivery.send(&event, &config).await.unwrap();
    assert!(!had_signature.load(std::sync::atomic::Ordering::SeqCst));
}

#[tokio::test]
async fn send_uses_default_retry_when_none_provided() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    let call_count = Arc::new(std::sync::atomic::AtomicU32::new(0));
    let call_count_clone = call_count.clone();

    let mock_app = axum::Router::new().route(
        "/hook",
        axum::routing::post(move || {
            let count = call_count_clone.clone();
            async move {
                let n = count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                if n == 0 {
                    axum::http::StatusCode::INTERNAL_SERVER_ERROR
                } else {
                    axum::http::StatusCode::OK
                }
            }
        }),
    );
    tokio::spawn(async move {
        axum::serve(listener, mock_app).await.unwrap();
    });

    let delivery = WebhookDelivery::new();
    let event = make_test_event();
    let config = WebhookConfig {
        url: format!("http://{addr}/hook"),
        filter: None,
        secret: None,
        wrap: None,
        retry: None, // Use default retry config
    };

    let result = delivery.send(&event, &config).await;
    assert!(result.is_ok());
    // Default config has 3 retries — first attempt fails, second succeeds
    assert_eq!(call_count.load(std::sync::atomic::Ordering::SeqCst), 2);
}
```

**Step 2: Run tests**

```bash
cd rust && cargo test --package taskcast-server webhook -- --nocapture
```

Expected: All tests PASS (existing + 6 new).

**Step 3: Commit**

---

### Task 5: Task List & Concurrent HTTP Transition Tests

**Files:**
- Modify: `rust/taskcast-server/tests/server_tests.rs` (add to existing file)

**Why:** TS has tests for task list scope enforcement, empty results, and sequential double-complete returning 409. These are missing in Rust.

**Step 1: Write the tests**

Append to `server_tests.rs`:

```rust
// ─── GET /tasks (list) ─────────────────────────────────────────────────────

#[tokio::test]
async fn list_tasks_requires_event_subscribe_scope() {
    let (_engine, server) = make_jwt_server();

    // Token with only task:create scope (no event:subscribe)
    let token = make_token(json!({
        "sub": "user",
        "scope": ["task:create"],
        "taskIds": "*",
        "exp": 9999999999u64
    }));

    let response = server
        .get("/tasks")
        .add_header(
            axum_test::http::header::AUTHORIZATION,
            bearer_header(&token),
        )
        .await;

    response.assert_status(axum_test::http::StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn list_tasks_returns_empty_array_when_no_tasks() {
    let (_engine, server) = make_no_auth_server();

    let response = server.get("/tasks").await;
    response.assert_status(axum_test::http::StatusCode::OK);
    let body: serde_json::Value = response.json();
    assert!(body["tasks"].is_array());
    assert_eq!(body["tasks"].as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn list_tasks_filter_by_status_returns_matching() {
    let (engine, server) = make_no_auth_server();

    // Create two tasks, transition one to running
    engine
        .create_task(taskcast_core::CreateTaskInput {
            id: Some("list-1".into()),
            ..Default::default()
        })
        .await
        .unwrap();
    let t2 = engine
        .create_task(taskcast_core::CreateTaskInput {
            id: Some("list-2".into()),
            ..Default::default()
        })
        .await
        .unwrap();
    engine
        .transition_task(&t2.id, taskcast_core::TaskStatus::Running, None)
        .await
        .unwrap();

    let response = server.get("/tasks?status=running").await;
    response.assert_status(axum_test::http::StatusCode::OK);
    let body: serde_json::Value = response.json();
    let tasks = body["tasks"].as_array().unwrap();
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0]["id"], "list-2");
}

#[tokio::test]
async fn list_tasks_filter_no_match_returns_empty() {
    let (_engine, server) = make_no_auth_server();

    // Create a pending task, filter for completed
    server
        .post("/tasks")
        .json(&json!({ "id": "list-nomatch" }))
        .await;

    let response = server.get("/tasks?status=completed").await;
    response.assert_status(axum_test::http::StatusCode::OK);
    let body: serde_json::Value = response.json();
    assert_eq!(body["tasks"].as_array().unwrap().len(), 0);
}

// ─── Sequential double-complete (HTTP layer) ───────────────────────────────

#[tokio::test]
async fn double_complete_second_attempt_returns_conflict() {
    let (_engine, server) = make_no_auth_server();

    server
        .post("/tasks")
        .json(&json!({ "id": "double-1" }))
        .await;
    server
        .patch("/tasks/double-1/status")
        .json(&json!({ "status": "running" }))
        .await;

    // First complete — should succeed
    let r1 = server
        .patch("/tasks/double-1/status")
        .json(&json!({ "status": "completed" }))
        .await;
    r1.assert_status(axum_test::http::StatusCode::OK);

    // Second complete — should fail (terminal state, no backward transitions)
    let r2 = server
        .patch("/tasks/double-1/status")
        .json(&json!({ "status": "completed" }))
        .await;
    // InvalidTransition maps to CONFLICT in the error handler
    let status = r2.status_code().as_u16();
    assert!(status == 409 || status == 400, "expected 409 or 400, got {status}");
}

// ─── Publish event to terminal task (HTTP layer) ───────────────────────────

#[tokio::test]
async fn publish_event_to_completed_task_returns_error() {
    let (_engine, server) = make_no_auth_server();

    server
        .post("/tasks")
        .json(&json!({ "id": "term-1" }))
        .await;
    server
        .patch("/tasks/term-1/status")
        .json(&json!({ "status": "running" }))
        .await;
    server
        .patch("/tasks/term-1/status")
        .json(&json!({ "status": "completed" }))
        .await;

    let response = server
        .post("/tasks/term-1/events")
        .json(&json!({
            "type": "progress",
            "level": "info",
            "data": null
        }))
        .await;

    let status = response.status_code().as_u16();
    assert!(status >= 400, "expected 4xx, got {status}");
}

// ─── Health endpoint accessible without auth ───────────────────────────────

#[tokio::test]
async fn health_endpoint_accessible_without_auth() {
    let server = make_jwt_server().1;

    // Health should not require auth
    let response = server.get("/health").await;
    response.assert_status(axum_test::http::StatusCode::OK);
}

// ─── Invalid status value ──────────────────────────────────────────────────

#[tokio::test]
async fn patch_status_invalid_status_value_returns_4xx() {
    let (_engine, server) = make_no_auth_server();

    server
        .post("/tasks")
        .json(&json!({ "id": "bad-status" }))
        .await;

    let response = server
        .patch("/tasks/bad-status/status")
        .json(&json!({ "status": "invalid-state" }))
        .await;

    let status = response.status_code().as_u16();
    assert!(status >= 400 && status < 500, "expected 4xx, got {status}");
}
```

**Step 2: Run tests**

```bash
cd rust && cargo test --package taskcast-server --test server_tests -- --nocapture
```

Expected: All tests PASS (existing + 8 new).

**Step 3: Commit**

---

## Execution Summary

| Task | New Tests | File |
|------|----------|------|
| 1. Malformed JSON | 6 | `tests/malformed_json.rs` (new) |
| 2. Malformed Bearer | 7 | `tests/malformed_bearer.rs` (new) |
| 3. WS Bad Messages | 8 | `tests/ws_bad_messages.rs` (new) |
| 4. Webhook Failures | 6 | `src/webhook.rs` (existing `#[cfg(test)]`) |
| 5. Task List + Misc | 8 | `tests/server_tests.rs` (existing) |
| **Total** | **35** | |

Remaining 36 tests (admin token endpoint, SSE concurrent, worker drain, etc.) can be addressed in a follow-up plan after the admin token feature is verified to exist in the Rust codebase.
