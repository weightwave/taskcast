use std::sync::Arc;

use axum::response::IntoResponse;
use axum_test::http::HeaderValue;
use axum_test::TestServer;
use jsonwebtoken::{encode, EncodingKey, Header};
use serde_json::json;
use taskcast_core::worker_manager::{WorkerManager, WorkerManagerOptions, WorkerRegistration};
use taskcast_core::{
    BroadcastProvider, ConnectionMode, EngineError, Level, MemoryBroadcastProvider,
    MemoryShortTermStore, ShortTermStore, TaskEngine, TaskEngineOptions, TaskStatus,
    WorkerMatchRule,
};
use taskcast_server::{create_app, AppError, AuthMode, JwtConfig, WebhookDelivery};

fn make_engine() -> Arc<TaskEngine> {
    Arc::new(TaskEngine::new(TaskEngineOptions {
        short_term_store: Arc::new(MemoryShortTermStore::new()),
        broadcast: Arc::new(MemoryBroadcastProvider::new()),
        long_term_store: None,
        hooks: None,
    }))
}

fn make_server(engine: Arc<TaskEngine>, auth_mode: AuthMode) -> TestServer {
    let app = create_app(engine, auth_mode, None);
    TestServer::new(app)
}

fn make_no_auth_server() -> (Arc<TaskEngine>, TestServer) {
    let engine = make_engine();
    let server = make_server(Arc::clone(&engine), AuthMode::None);
    (engine, server)
}

const JWT_SECRET: &str = "test-secret-key-for-jwt-signing-needs-to-be-long-enough";

fn make_jwt_server() -> (Arc<TaskEngine>, TestServer) {
    let engine = make_engine();
    let auth_mode = AuthMode::Jwt(JwtConfig {
        algorithm: jsonwebtoken::Algorithm::HS256,
        secret: Some(JWT_SECRET.to_string()),
        public_key: None,
        issuer: None,
        audience: None,
    });
    let server = make_server(Arc::clone(&engine), auth_mode);
    (engine, server)
}

fn make_token(claims: serde_json::Value) -> String {
    encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(JWT_SECRET.as_bytes()),
    )
    .unwrap()
}

fn make_full_access_token() -> String {
    make_token(json!({
        "sub": "test-user",
        "scope": ["*"],
        "taskIds": "*",
        "exp": 9999999999u64
    }))
}

fn bearer_header(token: &str) -> HeaderValue {
    HeaderValue::from_str(&format!("Bearer {token}")).unwrap()
}

// ─── POST /tasks ─────────────────────────────────────────────────────────────

#[tokio::test]
async fn post_tasks_creates_task_returns_201() {
    let (_engine, server) = make_no_auth_server();

    let response = server
        .post("/tasks")
        .json(&json!({
            "type": "crawl",
            "params": { "url": "https://example.com" }
        }))
        .await;

    response.assert_status(axum_test::http::StatusCode::CREATED);
    let body: serde_json::Value = response.json();
    assert!(body["id"].is_string());
    assert_eq!(body["status"], "pending");
    assert_eq!(body["type"], "crawl");
    assert_eq!(body["params"]["url"], "https://example.com");
}

#[tokio::test]
async fn post_tasks_with_custom_id() {
    let (_engine, server) = make_no_auth_server();

    let response = server
        .post("/tasks")
        .json(&json!({ "id": "my-task-123" }))
        .await;

    response.assert_status(axum_test::http::StatusCode::CREATED);
    let body: serde_json::Value = response.json();
    assert_eq!(body["id"], "my-task-123");
}

#[tokio::test]
async fn post_tasks_empty_body() {
    let (_engine, server) = make_no_auth_server();

    let response = server
        .post("/tasks")
        .json(&json!({}))
        .await;

    response.assert_status(axum_test::http::StatusCode::CREATED);
    let body: serde_json::Value = response.json();
    assert!(body["id"].is_string());
    assert_eq!(body["status"], "pending");
}

// ─── GET /tasks/:taskId ──────────────────────────────────────────────────────

#[tokio::test]
async fn get_task_returns_task() {
    let (_engine, server) = make_no_auth_server();

    // Create a task first
    let create_response = server
        .post("/tasks")
        .json(&json!({ "id": "task-get-1", "type": "test" }))
        .await;
    create_response.assert_status(axum_test::http::StatusCode::CREATED);

    // Get it
    let response = server.get("/tasks/task-get-1").await;
    response.assert_status(axum_test::http::StatusCode::OK);
    let body: serde_json::Value = response.json();
    assert_eq!(body["id"], "task-get-1");
    assert_eq!(body["type"], "test");
    assert_eq!(body["status"], "pending");
}

#[tokio::test]
async fn get_task_returns_404_for_missing() {
    let (_engine, server) = make_no_auth_server();

    let response = server.get("/tasks/nonexistent").await;
    response.assert_status(axum_test::http::StatusCode::NOT_FOUND);
    let body: serde_json::Value = response.json();
    assert_eq!(body["error"], "Task not found");
}

// ─── PATCH /tasks/:taskId/status ─────────────────────────────────────────────

#[tokio::test]
async fn patch_task_status_transitions_successfully() {
    let (_engine, server) = make_no_auth_server();

    // Create a task
    server
        .post("/tasks")
        .json(&json!({ "id": "task-trans-1" }))
        .await;

    // Transition to running
    let response = server
        .patch("/tasks/task-trans-1/status")
        .json(&json!({ "status": "running" }))
        .await;

    response.assert_status(axum_test::http::StatusCode::OK);
    let body: serde_json::Value = response.json();
    assert_eq!(body["status"], "running");
}

#[tokio::test]
async fn patch_task_status_returns_400_for_invalid_transition() {
    let (_engine, server) = make_no_auth_server();

    // Create a task (pending)
    server
        .post("/tasks")
        .json(&json!({ "id": "task-invalid" }))
        .await;

    // Try to transition pending -> completed (invalid)
    let response = server
        .patch("/tasks/task-invalid/status")
        .json(&json!({ "status": "completed" }))
        .await;

    response.assert_status(axum_test::http::StatusCode::BAD_REQUEST);
    let body: serde_json::Value = response.json();
    assert!(body["error"].as_str().unwrap().contains("Invalid transition"));
}

#[tokio::test]
async fn patch_task_status_returns_404_for_missing_task() {
    let (_engine, server) = make_no_auth_server();

    let response = server
        .patch("/tasks/nonexistent/status")
        .json(&json!({ "status": "running" }))
        .await;

    response.assert_status(axum_test::http::StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn patch_task_status_with_result_payload() {
    let (_engine, server) = make_no_auth_server();

    server
        .post("/tasks")
        .json(&json!({ "id": "task-result" }))
        .await;
    server
        .patch("/tasks/task-result/status")
        .json(&json!({ "status": "running" }))
        .await;

    let response = server
        .patch("/tasks/task-result/status")
        .json(&json!({
            "status": "completed",
            "result": { "output": "done" }
        }))
        .await;

    response.assert_status(axum_test::http::StatusCode::OK);
    let body: serde_json::Value = response.json();
    assert_eq!(body["status"], "completed");
    assert_eq!(body["result"]["output"], "done");
}

#[tokio::test]
async fn patch_task_status_with_error_payload() {
    let (_engine, server) = make_no_auth_server();

    server
        .post("/tasks")
        .json(&json!({ "id": "task-err" }))
        .await;
    server
        .patch("/tasks/task-err/status")
        .json(&json!({ "status": "running" }))
        .await;

    let response = server
        .patch("/tasks/task-err/status")
        .json(&json!({
            "status": "failed",
            "error": { "code": "ERR_001", "message": "something broke" }
        }))
        .await;

    response.assert_status(axum_test::http::StatusCode::OK);
    let body: serde_json::Value = response.json();
    assert_eq!(body["status"], "failed");
    assert_eq!(body["error"]["code"], "ERR_001");
    assert_eq!(body["error"]["message"], "something broke");
}

// ─── POST /tasks/:taskId/events ──────────────────────────────────────────────

#[tokio::test]
async fn post_events_single_publish() {
    let (_engine, server) = make_no_auth_server();

    // Create and transition to running
    server
        .post("/tasks")
        .json(&json!({ "id": "task-evt-1" }))
        .await;
    server
        .patch("/tasks/task-evt-1/status")
        .json(&json!({ "status": "running" }))
        .await;

    let response = server
        .post("/tasks/task-evt-1/events")
        .json(&json!({
            "type": "progress",
            "level": "info",
            "data": { "percent": 50 }
        }))
        .await;

    response.assert_status(axum_test::http::StatusCode::CREATED);
    let body: serde_json::Value = response.json();
    assert_eq!(body["type"], "progress");
    assert_eq!(body["taskId"], "task-evt-1");
    assert_eq!(body["data"]["percent"], 50);
}

#[tokio::test]
async fn post_events_batch_publish() {
    let (_engine, server) = make_no_auth_server();

    server
        .post("/tasks")
        .json(&json!({ "id": "task-batch" }))
        .await;
    server
        .patch("/tasks/task-batch/status")
        .json(&json!({ "status": "running" }))
        .await;

    let response = server
        .post("/tasks/task-batch/events")
        .json(&json!([
            { "type": "log", "level": "info", "data": "hello" },
            { "type": "log", "level": "debug", "data": "world" }
        ]))
        .await;

    response.assert_status(axum_test::http::StatusCode::CREATED);
    let body: serde_json::Value = response.json();
    assert!(body.is_array());
    let events = body.as_array().unwrap();
    assert_eq!(events.len(), 2);
    assert_eq!(events[0]["type"], "log");
    assert_eq!(events[1]["type"], "log");
}

#[tokio::test]
async fn post_events_returns_404_for_missing_task() {
    let (_engine, server) = make_no_auth_server();

    let response = server
        .post("/tasks/nonexistent/events")
        .json(&json!({
            "type": "progress",
            "level": "info",
            "data": null
        }))
        .await;

    response.assert_status(axum_test::http::StatusCode::NOT_FOUND);
}

// ─── GET /tasks/:taskId/events/history ───────────────────────────────────────

#[tokio::test]
async fn get_events_history_returns_events() {
    let (_engine, server) = make_no_auth_server();

    server
        .post("/tasks")
        .json(&json!({ "id": "task-hist" }))
        .await;
    server
        .patch("/tasks/task-hist/status")
        .json(&json!({ "status": "running" }))
        .await;

    // Publish some events
    server
        .post("/tasks/task-hist/events")
        .json(&json!({
            "type": "progress",
            "level": "info",
            "data": { "percent": 25 }
        }))
        .await;
    server
        .post("/tasks/task-hist/events")
        .json(&json!({
            "type": "progress",
            "level": "info",
            "data": { "percent": 75 }
        }))
        .await;

    let response = server.get("/tasks/task-hist/events/history").await;
    response.assert_status(axum_test::http::StatusCode::OK);
    let body: serde_json::Value = response.json();
    let events = body.as_array().unwrap();
    // 1 status event from transition + 2 progress events
    assert_eq!(events.len(), 3);
}

#[tokio::test]
async fn get_events_history_with_since_index() {
    let (_engine, server) = make_no_auth_server();

    server
        .post("/tasks")
        .json(&json!({ "id": "task-since" }))
        .await;
    server
        .patch("/tasks/task-since/status")
        .json(&json!({ "status": "running" }))
        .await;

    server
        .post("/tasks/task-since/events")
        .json(&json!({
            "type": "progress",
            "level": "info",
            "data": { "step": 1 }
        }))
        .await;
    server
        .post("/tasks/task-since/events")
        .json(&json!({
            "type": "progress",
            "level": "info",
            "data": { "step": 2 }
        }))
        .await;

    // Get events since index 1 (should skip index 0 and 1)
    let response = server
        .get("/tasks/task-since/events/history")
        .add_query_param("since.index", "1")
        .await;

    response.assert_status(axum_test::http::StatusCode::OK);
    let body: serde_json::Value = response.json();
    let events = body.as_array().unwrap();
    // Index 0 = status event, index 1 = first progress
    // since.index=1 means events with index > 1, so index 2 only
    assert_eq!(events.len(), 1);
}

#[tokio::test]
async fn get_events_history_returns_404_for_missing_task() {
    let (_engine, server) = make_no_auth_server();

    let response = server.get("/tasks/nonexistent/events/history").await;
    response.assert_status(axum_test::http::StatusCode::NOT_FOUND);
}

// ─── Auth: JWT mode ──────────────────────────────────────────────────────────

#[tokio::test]
async fn jwt_mode_returns_401_without_token() {
    let (_engine, server) = make_jwt_server();

    let response = server
        .post("/tasks")
        .json(&json!({}))
        .await;

    response.assert_status(axum_test::http::StatusCode::UNAUTHORIZED);
    let body: serde_json::Value = response.json();
    assert_eq!(body["error"], "Missing Bearer token");
}

#[tokio::test]
async fn jwt_mode_returns_401_with_invalid_token() {
    let (_engine, server) = make_jwt_server();

    let response = server
        .post("/tasks")
        .add_header(
            axum_test::http::header::AUTHORIZATION,
            HeaderValue::from_static("Bearer invalid-token-here"),
        )
        .json(&json!({}))
        .await;

    response.assert_status(axum_test::http::StatusCode::UNAUTHORIZED);
    let body: serde_json::Value = response.json();
    assert_eq!(body["error"], "Invalid or expired token");
}

#[tokio::test]
async fn jwt_mode_succeeds_with_valid_token() {
    let (_engine, server) = make_jwt_server();
    let token = make_full_access_token();

    let response = server
        .post("/tasks")
        .add_header(
            axum_test::http::header::AUTHORIZATION,
            bearer_header(&token),
        )
        .json(&json!({ "id": "jwt-task" }))
        .await;

    response.assert_status(axum_test::http::StatusCode::CREATED);
    let body: serde_json::Value = response.json();
    assert_eq!(body["id"], "jwt-task");
}

#[tokio::test]
async fn jwt_mode_returns_403_for_insufficient_scope() {
    let (_engine, server) = make_jwt_server();

    // Token with only event:subscribe scope
    let token = make_token(json!({
        "sub": "limited-user",
        "scope": ["event:subscribe"],
        "taskIds": "*",
        "exp": 9999999999u64
    }));

    // Try to create a task (requires task:create)
    let response = server
        .post("/tasks")
        .add_header(
            axum_test::http::header::AUTHORIZATION,
            bearer_header(&token),
        )
        .json(&json!({}))
        .await;

    response.assert_status(axum_test::http::StatusCode::FORBIDDEN);
    let body: serde_json::Value = response.json();
    assert_eq!(body["error"], "Forbidden");
}

#[tokio::test]
async fn jwt_mode_returns_403_for_restricted_task_ids() {
    let (_engine, server) = make_jwt_server();

    // Token scoped to specific task IDs
    let token = make_token(json!({
        "sub": "scoped-user",
        "scope": ["*"],
        "taskIds": ["task-allowed"],
        "exp": 9999999999u64
    }));

    // Create a task first (task:create doesn't check taskId)
    let create_response = server
        .post("/tasks")
        .add_header(
            axum_test::http::header::AUTHORIZATION,
            bearer_header(&token),
        )
        .json(&json!({ "id": "task-forbidden" }))
        .await;
    create_response.assert_status(axum_test::http::StatusCode::CREATED);

    // Try to get a task we don't have access to
    let response = server
        .get("/tasks/task-forbidden")
        .add_header(
            axum_test::http::header::AUTHORIZATION,
            bearer_header(&token),
        )
        .await;

    response.assert_status(axum_test::http::StatusCode::FORBIDDEN);
}

// ─── Auth: None mode ─────────────────────────────────────────────────────────

#[tokio::test]
async fn none_auth_mode_all_requests_succeed() {
    let (_engine, server) = make_no_auth_server();

    // Create task
    let response = server
        .post("/tasks")
        .json(&json!({ "id": "open-task" }))
        .await;
    response.assert_status(axum_test::http::StatusCode::CREATED);

    // Get task
    let response = server.get("/tasks/open-task").await;
    response.assert_status(axum_test::http::StatusCode::OK);

    // Transition
    let response = server
        .patch("/tasks/open-task/status")
        .json(&json!({ "status": "running" }))
        .await;
    response.assert_status(axum_test::http::StatusCode::OK);

    // Publish events
    let response = server
        .post("/tasks/open-task/events")
        .json(&json!({
            "type": "log",
            "level": "info",
            "data": "test"
        }))
        .await;
    response.assert_status(axum_test::http::StatusCode::CREATED);

    // Get history
    let response = server.get("/tasks/open-task/events/history").await;
    response.assert_status(axum_test::http::StatusCode::OK);
}

// ─── Full workflow test ──────────────────────────────────────────────────────

#[tokio::test]
async fn full_task_lifecycle() {
    let (_engine, server) = make_no_auth_server();

    // 1. Create task
    let response = server
        .post("/tasks")
        .json(&json!({
            "id": "lifecycle-task",
            "type": "process",
            "params": { "input": "data" },
            "metadata": { "source": "test" }
        }))
        .await;
    response.assert_status(axum_test::http::StatusCode::CREATED);

    // 2. Transition to running
    let response = server
        .patch("/tasks/lifecycle-task/status")
        .json(&json!({ "status": "running" }))
        .await;
    response.assert_status(axum_test::http::StatusCode::OK);

    // 3. Publish progress events
    server
        .post("/tasks/lifecycle-task/events")
        .json(&json!({
            "type": "progress",
            "level": "info",
            "data": { "percent": 50 }
        }))
        .await;

    server
        .post("/tasks/lifecycle-task/events")
        .json(&json!({
            "type": "progress",
            "level": "info",
            "data": { "percent": 100 }
        }))
        .await;

    // 4. Complete the task
    let response = server
        .patch("/tasks/lifecycle-task/status")
        .json(&json!({
            "status": "completed",
            "result": { "output": "processed" }
        }))
        .await;
    response.assert_status(axum_test::http::StatusCode::OK);
    let body: serde_json::Value = response.json();
    assert_eq!(body["status"], "completed");
    assert!(body["completedAt"].is_number());

    // 5. Verify final state
    let response = server.get("/tasks/lifecycle-task").await;
    response.assert_status(axum_test::http::StatusCode::OK);
    let body: serde_json::Value = response.json();
    assert_eq!(body["status"], "completed");
    assert_eq!(body["result"]["output"], "processed");

    // 6. Verify event history
    let response = server
        .get("/tasks/lifecycle-task/events/history")
        .await;
    response.assert_status(axum_test::http::StatusCode::OK);
    let body: serde_json::Value = response.json();
    let events = body.as_array().unwrap();
    // 2 status events (running, completed) + 2 progress events
    assert_eq!(events.len(), 4);
}

// ─── SSE: GET /tasks/:taskId/events ──────────────────────────────────────────

#[tokio::test]
async fn sse_returns_404_for_missing_task() {
    let (_engine, server) = make_no_auth_server();

    let response = server.get("/tasks/nonexistent/events").await;
    response.assert_status(axum_test::http::StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn sse_returns_403_when_jwt_scope_insufficient() {
    let (_engine, server) = make_jwt_server();

    // Token with only task:create scope (no event:subscribe)
    let token = make_token(json!({
        "sub": "user",
        "scope": ["task:create"],
        "taskIds": "*",
        "exp": 9999999999u64
    }));

    // Create task first
    server
        .post("/tasks")
        .add_header(
            axum_test::http::header::AUTHORIZATION,
            bearer_header(&token),
        )
        .json(&json!({ "id": "sse-forbidden" }))
        .await;

    // Try to subscribe (requires event:subscribe)
    let response = server
        .get("/tasks/sse-forbidden/events")
        .add_header(
            axum_test::http::header::AUTHORIZATION,
            bearer_header(&token),
        )
        .await;

    response.assert_status(axum_test::http::StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn sse_replays_history_for_terminal_task() {
    let (_engine, server) = make_no_auth_server();

    // Create task, transition to running, publish events, complete
    server
        .post("/tasks")
        .json(&json!({ "id": "sse-terminal" }))
        .await;
    server
        .patch("/tasks/sse-terminal/status")
        .json(&json!({ "status": "running" }))
        .await;
    server
        .post("/tasks/sse-terminal/events")
        .json(&json!({ "type": "progress", "level": "info", "data": { "p": 50 } }))
        .await;
    server
        .patch("/tasks/sse-terminal/status")
        .json(&json!({ "status": "completed", "result": { "ok": true } }))
        .await;

    // Connect to SSE — for terminal tasks, it should replay all history
    // and send a done event, then close the stream.
    let response = server.get("/tasks/sse-terminal/events").await;
    response.assert_status(axum_test::http::StatusCode::OK);

    let text = response.text();
    // Should contain taskcast.event lines from history replay
    assert!(text.contains("event: taskcast.event"), "should have event lines");
    // Should contain a done event because task is already terminal
    assert!(text.contains("event: taskcast.done"), "should have done event");
    assert!(text.contains("completed"), "done reason should be completed");
}

#[tokio::test]
async fn sse_wraps_events_in_envelope_by_default() {
    let (_engine, server) = make_no_auth_server();

    server
        .post("/tasks")
        .json(&json!({ "id": "sse-wrap" }))
        .await;
    server
        .patch("/tasks/sse-wrap/status")
        .json(&json!({ "status": "running" }))
        .await;
    server
        .post("/tasks/sse-wrap/events")
        .json(&json!({ "type": "log", "level": "info", "data": "hello" }))
        .await;
    server
        .patch("/tasks/sse-wrap/status")
        .json(&json!({ "status": "completed" }))
        .await;

    let response = server.get("/tasks/sse-wrap/events").await;
    let text = response.text();

    // Envelope should contain filteredIndex and rawIndex fields
    assert!(text.contains("filteredIndex"), "envelope should have filteredIndex");
    assert!(text.contains("rawIndex"), "envelope should have rawIndex");
    assert!(text.contains("eventId"), "envelope should have eventId");
}

#[tokio::test]
async fn sse_unwrap_mode_sends_raw_events() {
    let (_engine, server) = make_no_auth_server();

    server
        .post("/tasks")
        .json(&json!({ "id": "sse-nowrap" }))
        .await;
    server
        .patch("/tasks/sse-nowrap/status")
        .json(&json!({ "status": "running" }))
        .await;
    server
        .post("/tasks/sse-nowrap/events")
        .json(&json!({ "type": "log", "level": "info", "data": "test" }))
        .await;
    server
        .patch("/tasks/sse-nowrap/status")
        .json(&json!({ "status": "completed" }))
        .await;

    let response = server
        .get("/tasks/sse-nowrap/events")
        .add_query_param("wrap", "false")
        .await;
    let text = response.text();

    // Raw events have taskId but NOT filteredIndex
    assert!(text.contains("taskId"), "raw event should have taskId");
    assert!(!text.contains("filteredIndex"), "raw event should NOT have filteredIndex");
}

#[tokio::test]
async fn sse_type_filter_only_returns_matching_events() {
    let (_engine, server) = make_no_auth_server();

    server
        .post("/tasks")
        .json(&json!({ "id": "sse-filter" }))
        .await;
    server
        .patch("/tasks/sse-filter/status")
        .json(&json!({ "status": "running" }))
        .await;
    server
        .post("/tasks/sse-filter/events")
        .json(&json!([
            { "type": "progress", "level": "info", "data": { "p": 25 } },
            { "type": "log", "level": "debug", "data": "debug msg" },
            { "type": "progress", "level": "info", "data": { "p": 75 } }
        ]))
        .await;
    server
        .patch("/tasks/sse-filter/status")
        .json(&json!({ "status": "completed" }))
        .await;

    // Filter only "progress" type events
    let response = server
        .get("/tasks/sse-filter/events")
        .add_query_param("types", "progress")
        .add_query_param("wrap", "false")
        .await;
    let text = response.text();

    // Count occurrences of "taskcast.event"
    let event_count = text.matches("event: taskcast.event").count();
    // Should have 2 progress events (not the log or status events)
    assert_eq!(event_count, 2, "should only see 2 progress events, got text:\n{text}");
    assert!(!text.contains("debug msg"), "log event should be filtered out");
}

#[tokio::test]
async fn sse_since_index_skips_replayed_events() {
    let (_engine, server) = make_no_auth_server();

    server
        .post("/tasks")
        .json(&json!({ "id": "sse-since" }))
        .await;
    server
        .patch("/tasks/sse-since/status")
        .json(&json!({ "status": "running" }))
        .await;
    // index 0 = status event from transition
    // index 1, 2, 3 = three progress events
    server
        .post("/tasks/sse-since/events")
        .json(&json!([
            { "type": "progress", "level": "info", "data": { "step": 1 } },
            { "type": "progress", "level": "info", "data": { "step": 2 } },
            { "type": "progress", "level": "info", "data": { "step": 3 } }
        ]))
        .await;
    server
        .patch("/tasks/sse-since/status")
        .json(&json!({ "status": "completed" }))
        .await;

    // Request SSE with since.index=2 (should skip events at index 0,1,2)
    let response = server
        .get("/tasks/sse-since/events")
        .add_query_param("since.index", "2")
        .add_query_param("wrap", "false")
        .await;
    let text = response.text();

    // Should only replay events with index > 2 (index 3 = step 3, index 4 = completed status)
    let event_count = text.matches("event: taskcast.event").count();
    assert_eq!(event_count, 2, "should have 2 events after since.index=2, got:\n{text}");
}

// ─── Error Response Format Tests ─────────────────────────────────────────────

#[test]
fn app_error_bad_request_returns_400_json() {
    let error = AppError::BadRequest("invalid input".to_string());
    let response = error.into_response();
    assert_eq!(response.status(), axum_test::http::StatusCode::BAD_REQUEST);
}

#[test]
fn app_error_not_found_returns_404_json() {
    let error = AppError::NotFound("task missing".to_string());
    let response = error.into_response();
    assert_eq!(response.status(), axum_test::http::StatusCode::NOT_FOUND);
}

#[test]
fn app_error_forbidden_returns_403_json() {
    let error = AppError::Forbidden;
    let response = error.into_response();
    assert_eq!(response.status(), axum_test::http::StatusCode::FORBIDDEN);
}

#[test]
fn app_error_missing_token_returns_401_json() {
    let error = AppError::MissingToken;
    let response = error.into_response();
    assert_eq!(response.status(), axum_test::http::StatusCode::UNAUTHORIZED);
}

#[test]
fn app_error_invalid_token_returns_401_json() {
    let error = AppError::InvalidToken;
    let response = error.into_response();
    assert_eq!(response.status(), axum_test::http::StatusCode::UNAUTHORIZED);
}

#[test]
fn app_error_engine_task_not_found_returns_404() {
    let error = AppError::Engine(EngineError::TaskNotFound("t1".to_string()));
    let response = error.into_response();
    assert_eq!(response.status(), axum_test::http::StatusCode::NOT_FOUND);
}

#[test]
fn app_error_engine_invalid_transition_returns_400() {
    let error = AppError::Engine(EngineError::InvalidTransition {
        from: TaskStatus::Pending,
        to: TaskStatus::Completed,
    });
    let response = error.into_response();
    assert_eq!(response.status(), axum_test::http::StatusCode::BAD_REQUEST);
}

#[test]
fn app_error_engine_task_terminal_returns_400() {
    let error = AppError::Engine(EngineError::TaskTerminal(TaskStatus::Completed));
    let response = error.into_response();
    assert_eq!(response.status(), axum_test::http::StatusCode::BAD_REQUEST);
}

#[test]
fn app_error_engine_store_error_returns_500() {
    let store_err: Box<dyn std::error::Error + Send + Sync> =
        Box::new(std::io::Error::new(std::io::ErrorKind::Other, "db error"));
    let error = AppError::Engine(EngineError::Store(store_err));
    let response = error.into_response();
    assert_eq!(
        response.status(),
        axum_test::http::StatusCode::INTERNAL_SERVER_ERROR
    );
}

// ─── Webhook Delivery Tests ──────────────────────────────────────────────────

#[tokio::test]
async fn webhook_delivery_sends_to_mock_server() {
    use axum::{routing::post as axum_post, Router};
    use std::sync::atomic::{AtomicU32, Ordering};

    let call_count = Arc::new(AtomicU32::new(0));
    let count_clone = Arc::clone(&call_count);

    let mock_app = Router::new().route(
        "/hook",
        axum_post(move || async move {
            count_clone.fetch_add(1, Ordering::SeqCst);
            axum_test::http::StatusCode::OK
        }),
    );

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, mock_app).await.unwrap();
    });

    let delivery = WebhookDelivery::new();
    let event = taskcast_core::TaskEvent {
        id: "evt_01".to_string(),
        task_id: "task_01".to_string(),
        index: 0,
        timestamp: 1700000000000.0,
        r#type: "progress".to_string(),
        level: Level::Info,
        data: json!({ "percent": 50 }),
        series_id: None,
        series_mode: None,

        series_acc_field: None,
    };
    let config = taskcast_core::WebhookConfig {
        url: format!("http://{addr}/hook"),
        filter: None,
        secret: Some("test-secret".to_string()),
        wrap: None,
        retry: Some(taskcast_core::RetryConfig {
            retries: 0,
            backoff: taskcast_core::BackoffStrategy::Fixed,
            initial_delay_ms: 100,
            max_delay_ms: 100,
            timeout_ms: 5000,
        }),
    };

    let result = delivery.send(&event, &config).await;
    assert!(result.is_ok());
    assert_eq!(call_count.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn webhook_delivery_retries_on_failure() {
    use axum::{routing::post as axum_post, Router};
    use std::sync::atomic::{AtomicU32, Ordering};

    let call_count = Arc::new(AtomicU32::new(0));
    let count_clone = Arc::clone(&call_count);

    // Mock server that always returns 500
    let mock_app = Router::new().route(
        "/hook",
        axum_post(move || async move {
            count_clone.fetch_add(1, Ordering::SeqCst);
            axum_test::http::StatusCode::INTERNAL_SERVER_ERROR
        }),
    );

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, mock_app).await.unwrap();
    });

    let delivery = WebhookDelivery::new();
    let event = taskcast_core::TaskEvent {
        id: "evt_02".to_string(),
        task_id: "task_02".to_string(),
        index: 0,
        timestamp: 1700000000000.0,
        r#type: "log".to_string(),
        level: Level::Info,
        data: json!(null),
        series_id: None,
        series_mode: None,

        series_acc_field: None,
    };
    let config = taskcast_core::WebhookConfig {
        url: format!("http://{addr}/hook"),
        filter: None,
        secret: None,
        wrap: None,
        retry: Some(taskcast_core::RetryConfig {
            retries: 2,
            backoff: taskcast_core::BackoffStrategy::Fixed,
            initial_delay_ms: 10, // fast retries for test
            max_delay_ms: 10,
            timeout_ms: 5000,
        }),
    };

    let result = delivery.send(&event, &config).await;
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(err.to_string().contains("3 attempts")); // 1 initial + 2 retries
    assert_eq!(call_count.load(Ordering::SeqCst), 3);
}

#[tokio::test]
async fn webhook_delivery_succeeds_on_retry() {
    use axum::{routing::post as axum_post, Router};
    use std::sync::atomic::{AtomicU32, Ordering};

    let call_count = Arc::new(AtomicU32::new(0));
    let count_clone = Arc::clone(&call_count);

    // Mock server that fails first 2 times, then succeeds
    let mock_app = Router::new().route(
        "/hook",
        axum_post(move || async move {
            let count = count_clone.fetch_add(1, Ordering::SeqCst);
            if count < 2 {
                axum_test::http::StatusCode::INTERNAL_SERVER_ERROR
            } else {
                axum_test::http::StatusCode::OK
            }
        }),
    );

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, mock_app).await.unwrap();
    });

    let delivery = WebhookDelivery::new();
    let event = taskcast_core::TaskEvent {
        id: "evt_03".to_string(),
        task_id: "task_03".to_string(),
        index: 0,
        timestamp: 1700000000000.0,
        r#type: "progress".to_string(),
        level: Level::Info,
        data: json!({ "step": 1 }),
        series_id: None,
        series_mode: None,

        series_acc_field: None,
    };
    let config = taskcast_core::WebhookConfig {
        url: format!("http://{addr}/hook"),
        filter: None,
        secret: None,
        wrap: None,
        retry: Some(taskcast_core::RetryConfig {
            retries: 3,
            backoff: taskcast_core::BackoffStrategy::Fixed,
            initial_delay_ms: 10,
            max_delay_ms: 10,
            timeout_ms: 5000,
        }),
    };

    let result = delivery.send(&event, &config).await;
    assert!(result.is_ok());
    assert_eq!(call_count.load(Ordering::SeqCst), 3); // 2 failures + 1 success
}

// ─── JWT Issuer Validation ──────────────────────────────────────────────────

fn make_jwt_server_with_issuer(issuer: &str) -> (Arc<TaskEngine>, TestServer) {
    let engine = make_engine();
    let auth_mode = AuthMode::Jwt(JwtConfig {
        algorithm: jsonwebtoken::Algorithm::HS256,
        secret: Some(JWT_SECRET.to_string()),
        public_key: None,
        issuer: Some(issuer.to_string()),
        audience: None,
    });
    let server = make_server(Arc::clone(&engine), auth_mode);
    (engine, server)
}

#[tokio::test]
async fn jwt_with_issuer_accepts_matching_issuer() {
    let (_engine, server) = make_jwt_server_with_issuer("https://auth.example.com");

    let token = make_token(json!({
        "sub": "user",
        "scope": ["*"],
        "taskIds": "*",
        "iss": "https://auth.example.com",
        "exp": 9999999999u64
    }));

    let response = server
        .post("/tasks")
        .add_header(
            axum_test::http::header::AUTHORIZATION,
            bearer_header(&token),
        )
        .json(&json!({}))
        .await;

    response.assert_status(axum_test::http::StatusCode::CREATED);
}

#[tokio::test]
async fn jwt_with_issuer_rejects_wrong_issuer() {
    let (_engine, server) = make_jwt_server_with_issuer("https://auth.example.com");

    let token = make_token(json!({
        "sub": "user",
        "scope": ["*"],
        "taskIds": "*",
        "iss": "https://wrong-issuer.com",
        "exp": 9999999999u64
    }));

    let response = server
        .post("/tasks")
        .add_header(
            axum_test::http::header::AUTHORIZATION,
            bearer_header(&token),
        )
        .json(&json!({}))
        .await;

    response.assert_status(axum_test::http::StatusCode::UNAUTHORIZED);
}

// ─── JWT Audience Validation ────────────────────────────────────────────────

fn make_jwt_server_with_audience(audience: &str) -> (Arc<TaskEngine>, TestServer) {
    let engine = make_engine();
    let auth_mode = AuthMode::Jwt(JwtConfig {
        algorithm: jsonwebtoken::Algorithm::HS256,
        secret: Some(JWT_SECRET.to_string()),
        public_key: None,
        issuer: None,
        audience: Some(audience.to_string()),
    });
    let server = make_server(Arc::clone(&engine), auth_mode);
    (engine, server)
}

#[tokio::test]
async fn jwt_with_audience_accepts_matching_audience() {
    let (_engine, server) = make_jwt_server_with_audience("taskcast-api");

    let token = make_token(json!({
        "sub": "user",
        "scope": ["*"],
        "taskIds": "*",
        "aud": "taskcast-api",
        "exp": 9999999999u64
    }));

    let response = server
        .post("/tasks")
        .add_header(
            axum_test::http::header::AUTHORIZATION,
            bearer_header(&token),
        )
        .json(&json!({}))
        .await;

    response.assert_status(axum_test::http::StatusCode::CREATED);
}

#[tokio::test]
async fn jwt_with_audience_rejects_wrong_audience() {
    let (_engine, server) = make_jwt_server_with_audience("taskcast-api");

    let token = make_token(json!({
        "sub": "user",
        "scope": ["*"],
        "taskIds": "*",
        "aud": "wrong-audience",
        "exp": 9999999999u64
    }));

    let response = server
        .post("/tasks")
        .add_header(
            axum_test::http::header::AUTHORIZATION,
            bearer_header(&token),
        )
        .json(&json!({}))
        .await;

    response.assert_status(axum_test::http::StatusCode::UNAUTHORIZED);
}

// ─── JWT No-Key Error ───────────────────────────────────────────────────────

#[tokio::test]
async fn jwt_no_key_returns_401() {
    let engine = make_engine();
    let auth_mode = AuthMode::Jwt(JwtConfig {
        algorithm: jsonwebtoken::Algorithm::HS256,
        secret: None,
        public_key: None,
        issuer: None,
        audience: None,
    });
    let server = make_server(Arc::clone(&engine), auth_mode);

    // Use a valid token, but server has no key to validate it
    let token = make_full_access_token();

    let response = server
        .post("/tasks")
        .add_header(
            axum_test::http::header::AUTHORIZATION,
            bearer_header(&token),
        )
        .json(&json!({}))
        .await;

    response.assert_status(axum_test::http::StatusCode::UNAUTHORIZED);
}

// ─── JWT TaskIds Claim Variants ─────────────────────────────────────────────

#[tokio::test]
async fn jwt_non_star_wildcard_task_ids_maps_to_all() {
    let (_engine, server) = make_jwt_server();

    // Token with taskIds as a non-"*" string (e.g. "all")
    let token = make_token(json!({
        "sub": "user",
        "scope": ["*"],
        "taskIds": "all",
        "exp": 9999999999u64
    }));

    // This should still grant access to all tasks (TaskIdAccess::All)
    let response = server
        .post("/tasks")
        .add_header(
            axum_test::http::header::AUTHORIZATION,
            bearer_header(&token),
        )
        .json(&json!({ "id": "wildcard-task" }))
        .await;
    response.assert_status(axum_test::http::StatusCode::CREATED);

    // Should be able to get the task (verifies TaskIdAccess::All)
    let response = server
        .get("/tasks/wildcard-task")
        .add_header(
            axum_test::http::header::AUTHORIZATION,
            bearer_header(&token),
        )
        .await;
    response.assert_status(axum_test::http::StatusCode::OK);
}

#[tokio::test]
async fn jwt_no_task_ids_field_defaults_to_all() {
    let (_engine, server) = make_jwt_server();

    // Token without taskIds field at all
    let token = make_token(json!({
        "sub": "user",
        "scope": ["*"],
        "exp": 9999999999u64
    }));

    let response = server
        .post("/tasks")
        .add_header(
            axum_test::http::header::AUTHORIZATION,
            bearer_header(&token),
        )
        .json(&json!({ "id": "no-taskids-task" }))
        .await;
    response.assert_status(axum_test::http::StatusCode::CREATED);

    // Should be able to get any task (TaskIdAccess::All by default)
    let response = server
        .get("/tasks/no-taskids-task")
        .add_header(
            axum_test::http::header::AUTHORIZATION,
            bearer_header(&token),
        )
        .await;
    response.assert_status(axum_test::http::StatusCode::OK);
}

// ─── Forbidden for transition_task ──────────────────────────────────────────

#[tokio::test]
async fn transition_task_returns_403_without_task_manage_scope() {
    let (_engine, server) = make_jwt_server();

    // Token with only event:subscribe scope (no task:manage)
    let limited_token = make_token(json!({
        "sub": "limited-user",
        "scope": ["event:subscribe"],
        "taskIds": "*",
        "exp": 9999999999u64
    }));

    // First create a task with full access
    let full_token = make_full_access_token();
    server
        .post("/tasks")
        .add_header(
            axum_test::http::header::AUTHORIZATION,
            bearer_header(&full_token),
        )
        .json(&json!({ "id": "task-trans-forbidden" }))
        .await;

    // Try to transition with limited token
    let response = server
        .patch("/tasks/task-trans-forbidden/status")
        .add_header(
            axum_test::http::header::AUTHORIZATION,
            bearer_header(&limited_token),
        )
        .json(&json!({ "status": "running" }))
        .await;

    response.assert_status(axum_test::http::StatusCode::FORBIDDEN);
}

// ─── TaskTerminal error in transition_task ──────────────────────────────────

#[tokio::test]
async fn transition_task_returns_400_for_terminal_task() {
    let (_engine, server) = make_no_auth_server();

    // Create -> run -> complete the task
    server
        .post("/tasks")
        .json(&json!({ "id": "task-terminal-trans" }))
        .await;
    server
        .patch("/tasks/task-terminal-trans/status")
        .json(&json!({ "status": "running" }))
        .await;
    server
        .patch("/tasks/task-terminal-trans/status")
        .json(&json!({ "status": "completed" }))
        .await;

    // Try to transition again — task is already terminal
    let response = server
        .patch("/tasks/task-terminal-trans/status")
        .json(&json!({ "status": "running" }))
        .await;

    response.assert_status(axum_test::http::StatusCode::BAD_REQUEST);
    let body: serde_json::Value = response.json();
    let error_msg = body["error"].as_str().unwrap();
    assert!(
        error_msg.contains("terminal") || error_msg.contains("Terminal") || error_msg.contains("Invalid"),
        "Error message should indicate terminal state, got: {error_msg}"
    );
}

// ─── Forbidden for publish_events ───────────────────────────────────────────

#[tokio::test]
async fn publish_events_returns_403_without_event_publish_scope() {
    let (_engine, server) = make_jwt_server();

    // Full access token to set up the task
    let full_token = make_full_access_token();
    server
        .post("/tasks")
        .add_header(
            axum_test::http::header::AUTHORIZATION,
            bearer_header(&full_token),
        )
        .json(&json!({ "id": "task-pub-forbidden" }))
        .await;
    server
        .patch("/tasks/task-pub-forbidden/status")
        .add_header(
            axum_test::http::header::AUTHORIZATION,
            bearer_header(&full_token),
        )
        .json(&json!({ "status": "running" }))
        .await;

    // Token without event:publish scope
    let limited_token = make_token(json!({
        "sub": "limited-user",
        "scope": ["event:subscribe"],
        "taskIds": "*",
        "exp": 9999999999u64
    }));

    let response = server
        .post("/tasks/task-pub-forbidden/events")
        .add_header(
            axum_test::http::header::AUTHORIZATION,
            bearer_header(&limited_token),
        )
        .json(&json!({
            "type": "progress",
            "level": "info",
            "data": null
        }))
        .await;

    response.assert_status(axum_test::http::StatusCode::FORBIDDEN);
}

// ─── TaskTerminal error in publish_events ───────────────────────────────────

#[tokio::test]
async fn publish_events_returns_400_for_terminal_task() {
    let (_engine, server) = make_no_auth_server();

    // Create -> run -> complete the task
    server
        .post("/tasks")
        .json(&json!({ "id": "task-terminal-pub" }))
        .await;
    server
        .patch("/tasks/task-terminal-pub/status")
        .json(&json!({ "status": "running" }))
        .await;
    server
        .patch("/tasks/task-terminal-pub/status")
        .json(&json!({ "status": "completed" }))
        .await;

    // Try to publish events to a completed task
    let response = server
        .post("/tasks/task-terminal-pub/events")
        .json(&json!({
            "type": "progress",
            "level": "info",
            "data": null
        }))
        .await;

    response.assert_status(axum_test::http::StatusCode::BAD_REQUEST);
    let body: serde_json::Value = response.json();
    let error_msg = body["error"].as_str().unwrap();
    assert!(
        error_msg.contains("terminal") || error_msg.contains("Terminal"),
        "Error message should indicate terminal state, got: {error_msg}"
    );
}

// ─── Forbidden for get_event_history ────────────────────────────────────────

#[tokio::test]
async fn get_event_history_returns_403_without_event_history_scope() {
    let (_engine, server) = make_jwt_server();

    // Full access token to set up the task
    let full_token = make_full_access_token();
    server
        .post("/tasks")
        .add_header(
            axum_test::http::header::AUTHORIZATION,
            bearer_header(&full_token),
        )
        .json(&json!({ "id": "task-hist-forbidden" }))
        .await;

    // Token without event:history scope
    let limited_token = make_token(json!({
        "sub": "limited-user",
        "scope": ["task:create"],
        "taskIds": "*",
        "exp": 9999999999u64
    }));

    let response = server
        .get("/tasks/task-hist-forbidden/events/history")
        .add_header(
            axum_test::http::header::AUTHORIZATION,
            bearer_header(&limited_token),
        )
        .await;

    response.assert_status(axum_test::http::StatusCode::FORBIDDEN);
}

// ─── WebhookDelivery Default Impl ───────────────────────────────────────────

#[test]
fn webhook_delivery_default_works() {
    // WebhookDelivery::default() should work the same as WebhookDelivery::new()
    let _delivery: WebhookDelivery = WebhookDelivery::default();
    // If this compiles and runs without panic, the Default impl is valid
}

// ─── Webhook with No Custom Retry (uses default_retry) ─────────────────────

#[tokio::test]
async fn webhook_uses_default_retry_when_none_provided() {
    use axum::{routing::post as axum_post, Router};
    use std::sync::atomic::{AtomicU32, Ordering};

    let call_count = Arc::new(AtomicU32::new(0));
    let count_clone = Arc::clone(&call_count);

    let mock_app = Router::new().route(
        "/hook",
        axum_post(move || async move {
            count_clone.fetch_add(1, Ordering::SeqCst);
            axum_test::http::StatusCode::OK
        }),
    );

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, mock_app).await.unwrap();
    });

    let delivery = WebhookDelivery::new();
    let event = taskcast_core::TaskEvent {
        id: "evt_default_retry".to_string(),
        task_id: "task_default_retry".to_string(),
        index: 0,
        timestamp: 1700000000000.0,
        r#type: "progress".to_string(),
        level: Level::Info,
        data: json!({ "percent": 50 }),
        series_id: None,
        series_mode: None,

        series_acc_field: None,
    };
    let config = taskcast_core::WebhookConfig {
        url: format!("http://{addr}/hook"),
        filter: None,
        secret: None,
        wrap: None,
        retry: None, // No custom retry — should use default_retry()
    };

    let result = delivery.send(&event, &config).await;
    assert!(result.is_ok());
    assert_eq!(call_count.load(Ordering::SeqCst), 1);
}

// ─── Webhook Network Error ──────────────────────────────────────────────────

#[tokio::test]
async fn webhook_network_error_captures_error_string() {
    let delivery = WebhookDelivery::new();
    let event = taskcast_core::TaskEvent {
        id: "evt_net_err".to_string(),
        task_id: "task_net_err".to_string(),
        index: 0,
        timestamp: 1700000000000.0,
        r#type: "progress".to_string(),
        level: Level::Info,
        data: json!({ "step": 1 }),
        series_id: None,
        series_mode: None,

        series_acc_field: None,
    };
    // Unreachable address — should trigger a network error (not an HTTP status error)
    let config = taskcast_core::WebhookConfig {
        url: "http://127.0.0.1:1/hook".to_string(),
        filter: None,
        secret: None,
        wrap: None,
        retry: Some(taskcast_core::RetryConfig {
            retries: 0, // No retries — just fail immediately
            backoff: taskcast_core::BackoffStrategy::Fixed,
            initial_delay_ms: 10,
            max_delay_ms: 10,
            timeout_ms: 2000,
        }),
    };

    let result = delivery.send(&event, &config).await;
    assert!(result.is_err());
    let err_msg = result.unwrap_err().to_string();
    assert!(
        err_msg.contains("1 attempts"),
        "Error should mention attempt count, got: {err_msg}"
    );
    // The message should contain the network error (connection refused or similar)
    assert!(
        err_msg.contains("error") || err_msg.contains("connect") || err_msg.contains("Connection"),
        "Error should contain network error detail, got: {err_msg}"
    );
}

// ─── SSE with Level Filter ──────────────────────────────────────────────────

#[tokio::test]
async fn sse_level_filter_only_returns_matching_levels() {
    let (_engine, server) = make_no_auth_server();

    server
        .post("/tasks")
        .json(&json!({ "id": "sse-level-filter" }))
        .await;
    server
        .patch("/tasks/sse-level-filter/status")
        .json(&json!({ "status": "running" }))
        .await;
    server
        .post("/tasks/sse-level-filter/events")
        .json(&json!([
            { "type": "log", "level": "info", "data": "info msg" },
            { "type": "log", "level": "warn", "data": "warn msg" },
            { "type": "log", "level": "error", "data": "error msg" },
            { "type": "log", "level": "debug", "data": "debug msg" }
        ]))
        .await;
    server
        .patch("/tasks/sse-level-filter/status")
        .json(&json!({ "status": "completed" }))
        .await;

    // Filter for warn,error levels only
    let response = server
        .get("/tasks/sse-level-filter/events")
        .add_query_param("levels", "warn,error")
        .add_query_param("wrap", "false")
        .await;
    let text = response.text();

    // Should contain warn and error messages
    assert!(text.contains("warn msg"), "should contain warn message");
    assert!(text.contains("error msg"), "should contain error message");
    // Should NOT contain info or debug messages
    assert!(!text.contains("info msg"), "info msg should be filtered out");
    assert!(!text.contains("debug msg"), "debug msg should be filtered out");
}

// ─── SSE Live Streaming with Running Task ───────────────────────────────────

#[tokio::test]
async fn sse_live_streaming_receives_events_and_done() {
    let engine = make_engine();
    let app = create_app(Arc::clone(&engine), AuthMode::None, None);

    // Bind to a random port
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let client = reqwest::Client::new();

    // Create task and transition to running via HTTP
    client
        .post(format!("http://{addr}/tasks"))
        .json(&json!({ "id": "sse-live" }))
        .send()
        .await
        .unwrap();
    client
        .patch(format!("http://{addr}/tasks/sse-live/status"))
        .json(&json!({ "status": "running" }))
        .send()
        .await
        .unwrap();

    // Spawn a background task to publish events after SSE connects
    let engine_clone = Arc::clone(&engine);
    tokio::spawn(async move {
        // Wait for SSE subscription to establish
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;

        // Publish events directly via the engine
        engine_clone
            .publish_event(
                "sse-live",
                taskcast_core::PublishEventInput {
                    r#type: "progress".to_string(),
                    level: Level::Info,
                    data: json!({ "step": 1 }),
                    series_id: None,
                    series_mode: None,

                    series_acc_field: None,
                },
            )
            .await
            .unwrap();

        engine_clone
            .publish_event(
                "sse-live",
                taskcast_core::PublishEventInput {
                    r#type: "progress".to_string(),
                    level: Level::Info,
                    data: json!({ "step": 2 }),
                    series_id: None,
                    series_mode: None,

                    series_acc_field: None,
                },
            )
            .await
            .unwrap();

        // Transition to completed — this will trigger the done event and close the SSE stream
        engine_clone
            .transition_task("sse-live", TaskStatus::Completed, None)
            .await
            .unwrap();
    });

    // Connect to SSE stream — reqwest::text() will block until stream closes (when task completes)
    let response = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        client
            .get(format!("http://{addr}/tasks/sse-live/events"))
            .send(),
    )
    .await
    .expect("SSE connect timed out")
    .unwrap();
    assert_eq!(response.status(), 200);

    let all_text = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        response.text(),
    )
    .await
    .expect("SSE stream timed out")
    .unwrap();

    // Verify we got live events (1 status from running + 2 progress + 1 status from completed)
    let event_count = all_text.matches("event: taskcast.event").count();
    assert!(
        event_count >= 3,
        "should have at least 3 events (history + live), got {event_count}. Full text:\n{all_text}"
    );

    // Verify done event
    assert!(
        all_text.contains("event: taskcast.done"),
        "should have done event. Full text:\n{all_text}"
    );
    assert!(
        all_text.contains("completed"),
        "done reason should contain completed. Full text:\n{all_text}"
    );
}

// ─── Worker Test Helpers ─────────────────────────────────────────────────────

fn make_worker_server() -> (Arc<TaskEngine>, Arc<WorkerManager>, TestServer) {
    let short_term_store = Arc::new(MemoryShortTermStore::new());
    let broadcast = Arc::new(MemoryBroadcastProvider::new());
    let engine = Arc::new(TaskEngine::new(TaskEngineOptions {
        short_term_store: Arc::clone(&short_term_store) as Arc<dyn ShortTermStore>,
        broadcast: Arc::clone(&broadcast) as Arc<dyn BroadcastProvider>,
        long_term_store: None,
        hooks: None,
    }));
    let manager = Arc::new(WorkerManager::new(WorkerManagerOptions {
        engine: Arc::clone(&engine),
        short_term_store: short_term_store as Arc<dyn ShortTermStore>,
        broadcast: broadcast as Arc<dyn BroadcastProvider>,
        long_term_store: None,
        hooks: None,
        defaults: None,
    }));
    let app = create_app(
        Arc::clone(&engine),
        AuthMode::None,
        Some(Arc::clone(&manager)),
    );
    let server = TestServer::new(app);
    (engine, manager, server)
}

fn make_worker_ws_server() -> (Arc<TaskEngine>, Arc<WorkerManager>, TestServer) {
    let short_term_store = Arc::new(MemoryShortTermStore::new());
    let broadcast = Arc::new(MemoryBroadcastProvider::new());
    let engine = Arc::new(TaskEngine::new(TaskEngineOptions {
        short_term_store: Arc::clone(&short_term_store) as Arc<dyn ShortTermStore>,
        broadcast: Arc::clone(&broadcast) as Arc<dyn BroadcastProvider>,
        long_term_store: None,
        hooks: None,
    }));
    let manager = Arc::new(WorkerManager::new(WorkerManagerOptions {
        engine: Arc::clone(&engine),
        short_term_store: short_term_store as Arc<dyn ShortTermStore>,
        broadcast: broadcast as Arc<dyn BroadcastProvider>,
        long_term_store: None,
        hooks: None,
        defaults: None,
    }));
    let app = create_app(
        Arc::clone(&engine),
        AuthMode::None,
        Some(Arc::clone(&manager)),
    );
    let server = TestServer::builder().http_transport().build(app);
    (engine, manager, server)
}

/// Register a worker directly via the manager for REST endpoint tests.
async fn register_test_worker(
    manager: &WorkerManager,
    worker_id: &str,
) -> taskcast_core::Worker {
    manager
        .register_worker(WorkerRegistration {
            worker_id: Some(worker_id.to_string()),
            match_rule: WorkerMatchRule::default(),
            capacity: 5,
            weight: None,
            connection_mode: ConnectionMode::Pull,
            metadata: None,
        })
        .await
        .expect("register_worker failed")
}

// ─── Workers REST: GET /workers ──────────────────────────────────────────────

#[tokio::test]
async fn get_workers_returns_empty_array_when_no_workers() {
    let (_engine, _manager, server) = make_worker_server();

    let response = server.get("/workers").await;
    response.assert_status(axum_test::http::StatusCode::OK);

    let body: serde_json::Value = response.json();
    assert_eq!(body, json!([]));
}

#[tokio::test]
async fn get_workers_returns_registered_workers() {
    let (_engine, manager, server) = make_worker_server();

    register_test_worker(&manager, "w1").await;
    register_test_worker(&manager, "w2").await;

    let response = server.get("/workers").await;
    response.assert_status(axum_test::http::StatusCode::OK);

    let body: Vec<serde_json::Value> = response.json();
    assert_eq!(body.len(), 2);

    let ids: Vec<&str> = body.iter().map(|w| w["id"].as_str().unwrap()).collect();
    assert!(ids.contains(&"w1"));
    assert!(ids.contains(&"w2"));
}

// ─── Workers REST: GET /workers/:workerId ────────────────────────────────────

#[tokio::test]
async fn get_worker_returns_worker_by_id() {
    let (_engine, manager, server) = make_worker_server();

    register_test_worker(&manager, "w1").await;

    let response = server.get("/workers/w1").await;
    response.assert_status(axum_test::http::StatusCode::OK);

    let body: serde_json::Value = response.json();
    assert_eq!(body["id"], "w1");
    assert_eq!(body["status"], "idle");
    assert_eq!(body["capacity"], 5);
}

#[tokio::test]
async fn get_worker_returns_404_for_missing_worker() {
    let (_engine, _manager, server) = make_worker_server();

    let response = server.get("/workers/nonexistent").await;
    response.assert_status(axum_test::http::StatusCode::NOT_FOUND);
}

// ─── Workers REST: DELETE /workers/:workerId ─────────────────────────────────

#[tokio::test]
async fn delete_worker_returns_204() {
    let (_engine, manager, server) = make_worker_server();

    register_test_worker(&manager, "w1").await;

    let response = server.delete("/workers/w1").await;
    response.assert_status(axum_test::http::StatusCode::NO_CONTENT);

    // Verify worker no longer exists
    let response = server.get("/workers/w1").await;
    response.assert_status(axum_test::http::StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn delete_worker_returns_404_for_missing_worker() {
    let (_engine, _manager, server) = make_worker_server();

    let response = server.delete("/workers/nonexistent").await;
    response.assert_status(axum_test::http::StatusCode::NOT_FOUND);
}

// ─── Workers REST: POST /workers/tasks/:taskId/decline ───────────────────────

#[tokio::test]
async fn decline_task_succeeds_for_claimed_task() {
    let (_engine, manager, server) = make_worker_server();

    // Register a worker
    register_test_worker(&manager, "w1").await;

    // Create a task via the REST API
    let create_resp = server
        .post("/tasks")
        .json(&json!({
            "id": "task-decline-1",
            "type": "test",
            "assignMode": "pull"
        }))
        .await;
    create_resp.assert_status(axum_test::http::StatusCode::CREATED);

    // Claim the task directly via manager
    let claim_result = manager.claim_task("task-decline-1", "w1").await.unwrap();
    assert_eq!(claim_result, taskcast_core::worker_manager::ClaimResult::Claimed);

    // Decline via the REST endpoint
    let response = server
        .post("/workers/tasks/task-decline-1/decline")
        .json(&json!({ "workerId": "w1" }))
        .await;
    response.assert_status(axum_test::http::StatusCode::OK);

    let body: serde_json::Value = response.json();
    assert_eq!(body["ok"], true);
}

#[tokio::test]
async fn decline_task_with_blacklist() {
    let (_engine, manager, server) = make_worker_server();

    register_test_worker(&manager, "w1").await;

    let create_resp = server
        .post("/tasks")
        .json(&json!({
            "id": "task-bl-1",
            "type": "test",
            "assignMode": "pull"
        }))
        .await;
    create_resp.assert_status(axum_test::http::StatusCode::CREATED);

    manager.claim_task("task-bl-1", "w1").await.unwrap();

    let response = server
        .post("/workers/tasks/task-bl-1/decline")
        .json(&json!({ "workerId": "w1", "blacklist": true }))
        .await;
    response.assert_status(axum_test::http::StatusCode::OK);

    let body: serde_json::Value = response.json();
    assert_eq!(body["ok"], true);
}

// ─── Workers REST: GET /workers/pull ─────────────────────────────────────────

#[tokio::test]
async fn pull_task_returns_task_when_available() {
    let (_engine, manager, server) = make_worker_server();

    // Register worker
    register_test_worker(&manager, "w-pull-1").await;

    // Create a pull-mode task
    server
        .post("/tasks")
        .json(&json!({
            "id": "pull-task-1",
            "type": "test",
            "assignMode": "pull"
        }))
        .await
        .assert_status(axum_test::http::StatusCode::CREATED);

    // Pull — should find and claim the task immediately
    let response = server.get("/workers/pull?workerId=w-pull-1").await;
    response.assert_status(axum_test::http::StatusCode::OK);

    let body: serde_json::Value = response.json();
    assert_eq!(body["id"], "pull-task-1");
}

// ─── App.rs: Worker routes mount / not-mount ─────────────────────────────────

#[tokio::test]
async fn worker_routes_accessible_when_manager_provided() {
    let (_engine, _manager, server) = make_worker_server();

    // /workers should be reachable and return 200 with empty array
    let response = server.get("/workers").await;
    response.assert_status(axum_test::http::StatusCode::OK);
}

#[tokio::test]
async fn worker_routes_not_found_when_no_manager() {
    let (_engine, server) = make_no_auth_server();

    // /workers should be 404 since no WorkerManager was provided
    let response = server.get("/workers").await;
    response.assert_status(axum_test::http::StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn worker_ws_not_found_when_no_manager() {
    let (_engine, server) = make_no_auth_server();

    // /workers/ws should be 404 since no WorkerManager was provided
    let response = server.get("/workers/ws").await;
    response.assert_status(axum_test::http::StatusCode::NOT_FOUND);
}

// ─── WebSocket: worker_ws.rs ─────────────────────────────────────────────────

#[tokio::test]
async fn ws_register_returns_registered_message() {
    let (_engine, _manager, server) = make_worker_ws_server();

    let mut ws = server
        .get_websocket("/workers/ws")
        .await
        .into_websocket()
        .await;

    ws.send_json(&json!({
        "type": "register",
        "matchRule": {},
        "capacity": 5
    }))
    .await;

    let response: serde_json::Value = ws.receive_json().await;
    assert_eq!(response["type"], "registered");
    assert!(response["workerId"].is_string());

    ws.close().await;
}

#[tokio::test]
async fn ws_register_with_custom_worker_id() {
    let (_engine, _manager, server) = make_worker_ws_server();

    let mut ws = server
        .get_websocket("/workers/ws")
        .await
        .into_websocket()
        .await;

    ws.send_json(&json!({
        "type": "register",
        "matchRule": {},
        "capacity": 3,
        "workerId": "my-worker-42"
    }))
    .await;

    let response: serde_json::Value = ws.receive_json().await;
    assert_eq!(response["type"], "registered");
    assert_eq!(response["workerId"], "my-worker-42");

    ws.close().await;
}

#[tokio::test]
async fn ws_invalid_json_returns_parse_error() {
    let (_engine, _manager, server) = make_worker_ws_server();

    let mut ws = server
        .get_websocket("/workers/ws")
        .await
        .into_websocket()
        .await;

    ws.send_text("this is not json").await;

    let response: serde_json::Value = ws.receive_json().await;
    assert_eq!(response["type"], "error");
    assert_eq!(response["code"], "PARSE_ERROR");
    assert!(response["message"].as_str().unwrap().contains("Invalid message"));

    ws.close().await;
}

#[tokio::test]
async fn ws_update_before_register_returns_not_registered() {
    let (_engine, _manager, server) = make_worker_ws_server();

    let mut ws = server
        .get_websocket("/workers/ws")
        .await
        .into_websocket()
        .await;

    ws.send_json(&json!({
        "type": "update",
        "weight": 80
    }))
    .await;

    let response: serde_json::Value = ws.receive_json().await;
    assert_eq!(response["type"], "error");
    assert_eq!(response["code"], "NOT_REGISTERED");

    ws.close().await;
}

#[tokio::test]
async fn ws_accept_before_register_returns_not_registered() {
    let (_engine, _manager, server) = make_worker_ws_server();

    let mut ws = server
        .get_websocket("/workers/ws")
        .await
        .into_websocket()
        .await;

    ws.send_json(&json!({
        "type": "accept",
        "taskId": "t1"
    }))
    .await;

    let response: serde_json::Value = ws.receive_json().await;
    assert_eq!(response["type"], "error");
    assert_eq!(response["code"], "NOT_REGISTERED");

    ws.close().await;
}

#[tokio::test]
async fn ws_claim_before_register_returns_not_registered() {
    let (_engine, _manager, server) = make_worker_ws_server();

    let mut ws = server
        .get_websocket("/workers/ws")
        .await
        .into_websocket()
        .await;

    ws.send_json(&json!({
        "type": "claim",
        "taskId": "t1"
    }))
    .await;

    let response: serde_json::Value = ws.receive_json().await;
    assert_eq!(response["type"], "error");
    assert_eq!(response["code"], "NOT_REGISTERED");

    ws.close().await;
}

#[tokio::test]
async fn ws_decline_before_register_returns_not_registered() {
    let (_engine, _manager, server) = make_worker_ws_server();

    let mut ws = server
        .get_websocket("/workers/ws")
        .await
        .into_websocket()
        .await;

    ws.send_json(&json!({
        "type": "decline",
        "taskId": "t1"
    }))
    .await;

    let response: serde_json::Value = ws.receive_json().await;
    assert_eq!(response["type"], "error");
    assert_eq!(response["code"], "NOT_REGISTERED");

    ws.close().await;
}

#[tokio::test]
async fn ws_drain_before_register_returns_not_registered() {
    let (_engine, _manager, server) = make_worker_ws_server();

    let mut ws = server
        .get_websocket("/workers/ws")
        .await
        .into_websocket()
        .await;

    ws.send_json(&json!({
        "type": "drain"
    }))
    .await;

    let response: serde_json::Value = ws.receive_json().await;
    assert_eq!(response["type"], "error");
    assert_eq!(response["code"], "NOT_REGISTERED");

    ws.close().await;
}

#[tokio::test]
async fn ws_register_then_update_weight() {
    let (_engine, manager, server) = make_worker_ws_server();

    let mut ws = server
        .get_websocket("/workers/ws")
        .await
        .into_websocket()
        .await;

    // Register
    ws.send_json(&json!({
        "type": "register",
        "matchRule": {},
        "capacity": 5,
        "workerId": "ws-update-w1"
    }))
    .await;

    let response: serde_json::Value = ws.receive_json().await;
    assert_eq!(response["type"], "registered");

    // Update weight
    ws.send_json(&json!({
        "type": "update",
        "weight": 80
    }))
    .await;

    // No response message for successful update, so we verify via the manager
    // Allow a brief moment for processing
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let worker = manager.get_worker("ws-update-w1").await.unwrap().unwrap();
    assert_eq!(worker.weight, 80);

    ws.close().await;
}

#[tokio::test]
async fn ws_register_then_claim_task() {
    let (engine, manager, server) = make_worker_ws_server();

    let mut ws = server
        .get_websocket("/workers/ws")
        .await
        .into_websocket()
        .await;

    // Register
    ws.send_json(&json!({
        "type": "register",
        "matchRule": {},
        "capacity": 5,
        "workerId": "ws-claim-w1"
    }))
    .await;

    let reg_resp: serde_json::Value = ws.receive_json().await;
    assert_eq!(reg_resp["type"], "registered");

    // Create a task via the engine directly
    engine
        .create_task(taskcast_core::CreateTaskInput {
            id: Some("ws-claim-task-1".to_string()),
            r#type: Some("test".to_string()),
            params: None,
            metadata: None,
            ttl: None,
            webhooks: None,
            cleanup: None,
            auth_config: None,
            tags: None,
            assign_mode: Some(taskcast_core::AssignMode::WsOffer),
            cost: None,
            disconnect_policy: None,
        })
        .await
        .unwrap();

    // Claim via the manager so the worker has an assignment
    let claim_result = manager
        .claim_task("ws-claim-task-1", "ws-claim-w1")
        .await
        .unwrap();
    assert_eq!(
        claim_result,
        taskcast_core::worker_manager::ClaimResult::Claimed
    );

    // Now send accept via WS — the task is already claimed by us, so accept
    // should try to claim again which will fail since status is no longer pending
    ws.send_json(&json!({
        "type": "accept",
        "taskId": "ws-claim-task-1"
    }))
    .await;

    let accept_resp: serde_json::Value = ws.receive_json().await;
    // Since the task is already claimed (not pending), accept will get a CLAIM_FAILED error
    assert_eq!(accept_resp["type"], "error");
    assert_eq!(accept_resp["code"], "CLAIM_FAILED");

    ws.close().await;
}

#[tokio::test]
async fn ws_claim_message_returns_claimed_response() {
    let (engine, _manager, server) = make_worker_ws_server();

    let mut ws = server
        .get_websocket("/workers/ws")
        .await
        .into_websocket()
        .await;

    // Register
    ws.send_json(&json!({
        "type": "register",
        "matchRule": {},
        "capacity": 5,
        "workerId": "ws-claim-msg-w1"
    }))
    .await;

    let reg_resp: serde_json::Value = ws.receive_json().await;
    assert_eq!(reg_resp["type"], "registered");

    // Create a pending task
    engine
        .create_task(taskcast_core::CreateTaskInput {
            id: Some("ws-claim-msg-task-1".to_string()),
            r#type: Some("test".to_string()),
            params: None,
            metadata: None,
            ttl: None,
            webhooks: None,
            cleanup: None,
            auth_config: None,
            tags: None,
            assign_mode: None,
            cost: None,
            disconnect_policy: None,
        })
        .await
        .unwrap();

    // Send claim via WS
    ws.send_json(&json!({
        "type": "claim",
        "taskId": "ws-claim-msg-task-1"
    }))
    .await;

    let claim_resp: serde_json::Value = ws.receive_json().await;
    assert_eq!(claim_resp["type"], "claimed");
    assert_eq!(claim_resp["taskId"], "ws-claim-msg-task-1");
    assert_eq!(claim_resp["success"], true);

    ws.close().await;
}

#[tokio::test]
async fn ws_claim_already_claimed_task_returns_success_false() {
    let (engine, manager, server) = make_worker_ws_server();

    // Register a second worker to claim the task first
    register_test_worker(&manager, "other-worker").await;

    let mut ws = server
        .get_websocket("/workers/ws")
        .await
        .into_websocket()
        .await;

    // Register via WS
    ws.send_json(&json!({
        "type": "register",
        "matchRule": {},
        "capacity": 5,
        "workerId": "ws-claim-fail-w1"
    }))
    .await;

    let reg_resp: serde_json::Value = ws.receive_json().await;
    assert_eq!(reg_resp["type"], "registered");

    // Create a task
    engine
        .create_task(taskcast_core::CreateTaskInput {
            id: Some("ws-claim-fail-task-1".to_string()),
            r#type: Some("test".to_string()),
            params: None,
            metadata: None,
            ttl: None,
            webhooks: None,
            cleanup: None,
            auth_config: None,
            tags: None,
            assign_mode: None,
            cost: None,
            disconnect_policy: None,
        })
        .await
        .unwrap();

    // Have another worker claim it first
    let result = manager
        .claim_task("ws-claim-fail-task-1", "other-worker")
        .await
        .unwrap();
    assert_eq!(result, taskcast_core::worker_manager::ClaimResult::Claimed);

    // Try to claim via WS — should fail
    ws.send_json(&json!({
        "type": "claim",
        "taskId": "ws-claim-fail-task-1"
    }))
    .await;

    let claim_resp: serde_json::Value = ws.receive_json().await;
    assert_eq!(claim_resp["type"], "claimed");
    assert_eq!(claim_resp["taskId"], "ws-claim-fail-task-1");
    assert_eq!(claim_resp["success"], false);

    ws.close().await;
}

#[tokio::test]
async fn ws_claim_nonexistent_task_returns_success_false() {
    let (_engine, _manager, server) = make_worker_ws_server();

    let mut ws = server
        .get_websocket("/workers/ws")
        .await
        .into_websocket()
        .await;

    ws.send_json(&json!({
        "type": "register",
        "matchRule": {},
        "capacity": 5,
        "workerId": "ws-claim-noexist-w1"
    }))
    .await;

    let reg_resp: serde_json::Value = ws.receive_json().await;
    assert_eq!(reg_resp["type"], "registered");

    ws.send_json(&json!({
        "type": "claim",
        "taskId": "nonexistent-task"
    }))
    .await;

    let claim_resp: serde_json::Value = ws.receive_json().await;
    assert_eq!(claim_resp["type"], "claimed");
    assert_eq!(claim_resp["success"], false);

    ws.close().await;
}

#[tokio::test]
async fn ws_decline_after_register() {
    let (engine, manager, server) = make_worker_ws_server();

    let mut ws = server
        .get_websocket("/workers/ws")
        .await
        .into_websocket()
        .await;

    // Register
    ws.send_json(&json!({
        "type": "register",
        "matchRule": {},
        "capacity": 5,
        "workerId": "ws-decline-w1"
    }))
    .await;

    let reg_resp: serde_json::Value = ws.receive_json().await;
    assert_eq!(reg_resp["type"], "registered");

    // Create a task and claim it
    engine
        .create_task(taskcast_core::CreateTaskInput {
            id: Some("ws-decline-task-1".to_string()),
            r#type: Some("test".to_string()),
            params: None,
            metadata: None,
            ttl: None,
            webhooks: None,
            cleanup: None,
            auth_config: None,
            tags: None,
            assign_mode: None,
            cost: None,
            disconnect_policy: None,
        })
        .await
        .unwrap();

    manager
        .claim_task("ws-decline-task-1", "ws-decline-w1")
        .await
        .unwrap();

    // Decline via WS
    ws.send_json(&json!({
        "type": "decline",
        "taskId": "ws-decline-task-1"
    }))
    .await;

    let decline_resp: serde_json::Value = ws.receive_json().await;
    assert_eq!(decline_resp["type"], "declined");
    assert_eq!(decline_resp["taskId"], "ws-decline-task-1");

    ws.close().await;
}

#[tokio::test]
async fn ws_decline_with_blacklist_flag() {
    let (engine, manager, server) = make_worker_ws_server();

    let mut ws = server
        .get_websocket("/workers/ws")
        .await
        .into_websocket()
        .await;

    ws.send_json(&json!({
        "type": "register",
        "matchRule": {},
        "capacity": 5,
        "workerId": "ws-decline-bl-w1"
    }))
    .await;

    let reg_resp: serde_json::Value = ws.receive_json().await;
    assert_eq!(reg_resp["type"], "registered");

    engine
        .create_task(taskcast_core::CreateTaskInput {
            id: Some("ws-decline-bl-task-1".to_string()),
            r#type: Some("test".to_string()),
            params: None,
            metadata: None,
            ttl: None,
            webhooks: None,
            cleanup: None,
            auth_config: None,
            tags: None,
            assign_mode: None,
            cost: None,
            disconnect_policy: None,
        })
        .await
        .unwrap();

    manager
        .claim_task("ws-decline-bl-task-1", "ws-decline-bl-w1")
        .await
        .unwrap();

    ws.send_json(&json!({
        "type": "decline",
        "taskId": "ws-decline-bl-task-1",
        "blacklist": true
    }))
    .await;

    let decline_resp: serde_json::Value = ws.receive_json().await;
    assert_eq!(decline_resp["type"], "declined");
    assert_eq!(decline_resp["taskId"], "ws-decline-bl-task-1");

    ws.close().await;
}

#[tokio::test]
async fn ws_drain_sets_worker_to_draining() {
    let (_engine, manager, server) = make_worker_ws_server();

    let mut ws = server
        .get_websocket("/workers/ws")
        .await
        .into_websocket()
        .await;

    ws.send_json(&json!({
        "type": "register",
        "matchRule": {},
        "capacity": 5,
        "workerId": "ws-drain-w1"
    }))
    .await;

    let reg_resp: serde_json::Value = ws.receive_json().await;
    assert_eq!(reg_resp["type"], "registered");

    // Send drain
    ws.send_json(&json!({
        "type": "drain"
    }))
    .await;

    // No response for drain, verify via manager
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let worker = manager.get_worker("ws-drain-w1").await.unwrap().unwrap();
    assert_eq!(worker.status, taskcast_core::WorkerStatus::Draining);

    ws.close().await;
}

#[tokio::test]
async fn ws_pong_heartbeat() {
    let (_engine, _manager, server) = make_worker_ws_server();

    let mut ws = server
        .get_websocket("/workers/ws")
        .await
        .into_websocket()
        .await;

    // Register first
    ws.send_json(&json!({
        "type": "register",
        "matchRule": {},
        "capacity": 5,
        "workerId": "ws-pong-w1"
    }))
    .await;

    let reg_resp: serde_json::Value = ws.receive_json().await;
    assert_eq!(reg_resp["type"], "registered");

    // Send pong — this should not generate a response, just update heartbeat
    ws.send_json(&json!({
        "type": "pong"
    }))
    .await;

    // If we can still send another message and get a response, the connection is alive
    ws.send_json(&json!({
        "type": "update",
        "weight": 50
    }))
    .await;

    // Allow processing time
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    ws.close().await;
}

#[tokio::test]
async fn ws_disconnect_unregisters_worker() {
    let (_engine, manager, server) = make_worker_ws_server();

    let mut ws = server
        .get_websocket("/workers/ws")
        .await
        .into_websocket()
        .await;

    ws.send_json(&json!({
        "type": "register",
        "matchRule": {},
        "capacity": 5,
        "workerId": "ws-disconnect-w1"
    }))
    .await;

    let reg_resp: serde_json::Value = ws.receive_json().await;
    assert_eq!(reg_resp["type"], "registered");

    // Verify worker exists
    let worker = manager.get_worker("ws-disconnect-w1").await.unwrap();
    assert!(worker.is_some());

    // Close connection
    ws.close().await;

    // Allow some time for the disconnect handler to run
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    // Worker should be unregistered
    let worker = manager.get_worker("ws-disconnect-w1").await.unwrap();
    assert!(worker.is_none(), "Worker should be unregistered after disconnect");
}

#[tokio::test]
async fn ws_accept_pending_task_returns_assigned() {
    let (engine, _manager, server) = make_worker_ws_server();

    let mut ws = server
        .get_websocket("/workers/ws")
        .await
        .into_websocket()
        .await;

    // Register
    ws.send_json(&json!({
        "type": "register",
        "matchRule": {},
        "capacity": 5,
        "workerId": "ws-accept-w1"
    }))
    .await;

    let reg_resp: serde_json::Value = ws.receive_json().await;
    assert_eq!(reg_resp["type"], "registered");

    // Create a pending task
    engine
        .create_task(taskcast_core::CreateTaskInput {
            id: Some("ws-accept-task-1".to_string()),
            r#type: Some("test".to_string()),
            params: None,
            metadata: None,
            ttl: None,
            webhooks: None,
            cleanup: None,
            auth_config: None,
            tags: None,
            assign_mode: None,
            cost: None,
            disconnect_policy: None,
        })
        .await
        .unwrap();

    // Accept (which internally calls claim_task) — task is pending, so should succeed
    ws.send_json(&json!({
        "type": "accept",
        "taskId": "ws-accept-task-1"
    }))
    .await;

    let accept_resp: serde_json::Value = ws.receive_json().await;
    assert_eq!(accept_resp["type"], "assigned");
    assert_eq!(accept_resp["taskId"], "ws-accept-task-1");

    ws.close().await;
}

#[tokio::test]
async fn ws_accept_nonexistent_task_returns_claim_failed() {
    let (_engine, _manager, server) = make_worker_ws_server();

    let mut ws = server
        .get_websocket("/workers/ws")
        .await
        .into_websocket()
        .await;

    ws.send_json(&json!({
        "type": "register",
        "matchRule": {},
        "capacity": 5,
        "workerId": "ws-accept-noexist-w1"
    }))
    .await;

    let reg_resp: serde_json::Value = ws.receive_json().await;
    assert_eq!(reg_resp["type"], "registered");

    ws.send_json(&json!({
        "type": "accept",
        "taskId": "nonexistent-task"
    }))
    .await;

    let accept_resp: serde_json::Value = ws.receive_json().await;
    assert_eq!(accept_resp["type"], "error");
    assert_eq!(accept_resp["code"], "CLAIM_FAILED");

    ws.close().await;
}

#[tokio::test]
async fn ws_register_with_weight() {
    let (_engine, manager, server) = make_worker_ws_server();

    let mut ws = server
        .get_websocket("/workers/ws")
        .await
        .into_websocket()
        .await;

    ws.send_json(&json!({
        "type": "register",
        "matchRule": {},
        "capacity": 5,
        "workerId": "ws-weight-w1",
        "weight": 75
    }))
    .await;

    let reg_resp: serde_json::Value = ws.receive_json().await;
    assert_eq!(reg_resp["type"], "registered");
    assert_eq!(reg_resp["workerId"], "ws-weight-w1");

    let worker = manager.get_worker("ws-weight-w1").await.unwrap().unwrap();
    assert_eq!(worker.weight, 75);

    ws.close().await;
}

#[tokio::test]
async fn ws_update_capacity_and_match_rule() {
    let (_engine, manager, server) = make_worker_ws_server();

    let mut ws = server
        .get_websocket("/workers/ws")
        .await
        .into_websocket()
        .await;

    ws.send_json(&json!({
        "type": "register",
        "matchRule": {},
        "capacity": 5,
        "workerId": "ws-update-cap-w1"
    }))
    .await;

    let reg_resp: serde_json::Value = ws.receive_json().await;
    assert_eq!(reg_resp["type"], "registered");

    // Update capacity and matchRule
    ws.send_json(&json!({
        "type": "update",
        "capacity": 10,
        "matchRule": { "taskTypes": ["test"] }
    }))
    .await;

    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let worker = manager
        .get_worker("ws-update-cap-w1")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(worker.capacity, 10);
    assert_eq!(
        worker.match_rule.task_types,
        Some(vec!["test".to_string()])
    );

    ws.close().await;
}

#[tokio::test]
async fn ws_decline_unassigned_task_returns_declined() {
    let (_engine, _manager, server) = make_worker_ws_server();

    let mut ws = server
        .get_websocket("/workers/ws")
        .await
        .into_websocket()
        .await;

    ws.send_json(&json!({
        "type": "register",
        "matchRule": {},
        "capacity": 5,
        "workerId": "ws-decline-unassigned-w1"
    }))
    .await;

    let reg_resp: serde_json::Value = ws.receive_json().await;
    assert_eq!(reg_resp["type"], "registered");

    // Decline a task that isn't assigned to this worker — should succeed quietly
    ws.send_json(&json!({
        "type": "decline",
        "taskId": "nonexistent-task"
    }))
    .await;

    let decline_resp: serde_json::Value = ws.receive_json().await;
    assert_eq!(decline_resp["type"], "declined");
    assert_eq!(decline_resp["taskId"], "nonexistent-task");

    ws.close().await;
}

#[tokio::test]
async fn ws_pong_before_register_is_ignored() {
    let (_engine, _manager, server) = make_worker_ws_server();

    let mut ws = server
        .get_websocket("/workers/ws")
        .await
        .into_websocket()
        .await;

    // Send pong before registering — should be silently ignored (no worker_id to heartbeat)
    ws.send_json(&json!({
        "type": "pong"
    }))
    .await;

    // Connection should still be alive; send register to verify
    ws.send_json(&json!({
        "type": "register",
        "matchRule": {},
        "capacity": 5
    }))
    .await;

    let reg_resp: serde_json::Value = ws.receive_json().await;
    assert_eq!(reg_resp["type"], "registered");

    ws.close().await;
}

// ─── JWT Worker Server Helper ─────────────────────────────────────────────

fn make_jwt_worker_server() -> (Arc<TaskEngine>, Arc<WorkerManager>, TestServer) {
    let short_term_store = Arc::new(MemoryShortTermStore::new());
    let broadcast = Arc::new(MemoryBroadcastProvider::new());
    let engine = Arc::new(TaskEngine::new(TaskEngineOptions {
        short_term_store: Arc::clone(&short_term_store) as Arc<dyn ShortTermStore>,
        broadcast: Arc::clone(&broadcast) as Arc<dyn BroadcastProvider>,
        long_term_store: None,
        hooks: None,
    }));
    let manager = Arc::new(WorkerManager::new(WorkerManagerOptions {
        engine: Arc::clone(&engine),
        short_term_store: short_term_store as Arc<dyn ShortTermStore>,
        broadcast: broadcast as Arc<dyn BroadcastProvider>,
        long_term_store: None,
        hooks: None,
        defaults: None,
    }));
    let auth_mode = AuthMode::Jwt(JwtConfig {
        algorithm: jsonwebtoken::Algorithm::HS256,
        secret: Some(JWT_SECRET.to_string()),
        public_key: None,
        issuer: None,
        audience: None,
    });
    let app = create_app(
        Arc::clone(&engine),
        auth_mode,
        Some(Arc::clone(&manager)),
    );
    let server = TestServer::new(app);
    (engine, manager, server)
}

// ─── Workers JWT Auth Rejection Tests ─────────────────────────────────────

#[tokio::test]
async fn list_workers_returns_403_without_worker_manage_scope() {
    let (_engine, _manager, server) = make_jwt_worker_server();
    let token = make_token(json!({
        "sub": "user",
        "scope": ["task:create"],
        "taskIds": "*",
        "exp": 9999999999u64
    }));
    let response = server
        .get("/workers")
        .add_header(axum_test::http::header::AUTHORIZATION, bearer_header(&token))
        .await;
    response.assert_status(axum_test::http::StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn get_worker_returns_403_without_worker_manage_scope() {
    let (_engine, _manager, server) = make_jwt_worker_server();
    let token = make_token(json!({
        "sub": "user",
        "scope": ["task:create"],
        "taskIds": "*",
        "exp": 9999999999u64
    }));
    let response = server
        .get("/workers/w1")
        .add_header(axum_test::http::header::AUTHORIZATION, bearer_header(&token))
        .await;
    response.assert_status(axum_test::http::StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn delete_worker_returns_403_without_worker_manage_scope() {
    let (_engine, _manager, server) = make_jwt_worker_server();
    let token = make_token(json!({
        "sub": "user",
        "scope": ["task:create"],
        "taskIds": "*",
        "exp": 9999999999u64
    }));
    let response = server
        .delete("/workers/w1")
        .add_header(axum_test::http::header::AUTHORIZATION, bearer_header(&token))
        .await;
    response.assert_status(axum_test::http::StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn pull_task_returns_403_without_worker_connect_scope() {
    let (_engine, _manager, server) = make_jwt_worker_server();
    let token = make_token(json!({
        "sub": "user",
        "scope": ["task:create"],
        "taskIds": "*",
        "exp": 9999999999u64
    }));
    let response = server
        .get("/workers/pull?workerId=w1")
        .add_header(axum_test::http::header::AUTHORIZATION, bearer_header(&token))
        .await;
    response.assert_status(axum_test::http::StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn decline_task_returns_403_without_worker_connect_scope() {
    let (_engine, _manager, server) = make_jwt_worker_server();
    let token = make_token(json!({
        "sub": "user",
        "scope": ["task:create"],
        "taskIds": "*",
        "exp": 9999999999u64
    }));
    let response = server
        .post("/workers/tasks/t1/decline")
        .add_header(axum_test::http::header::AUTHORIZATION, bearer_header(&token))
        .json(&json!({"workerId": "w1"}))
        .await;
    response.assert_status(axum_test::http::StatusCode::FORBIDDEN);
}

// ─── Workers with valid JWT scope ─────────────────────────────────────────

#[tokio::test]
async fn list_workers_succeeds_with_worker_manage_scope() {
    let (_engine, manager, server) = make_jwt_worker_server();
    let token = make_token(json!({
        "sub": "user",
        "scope": ["worker:manage"],
        "taskIds": "*",
        "exp": 9999999999u64
    }));
    register_test_worker(&manager, "w1").await;
    let response = server
        .get("/workers")
        .add_header(axum_test::http::header::AUTHORIZATION, bearer_header(&token))
        .await;
    response.assert_status(axum_test::http::StatusCode::OK);
    let body: Vec<serde_json::Value> = response.json();
    assert_eq!(body.len(), 1);
}

#[tokio::test]
async fn pull_task_with_weight_update_succeeds() {
    let (_engine, manager, server) = make_worker_server();
    register_test_worker(&manager, "w1").await;

    // Create a pull-mode task
    _engine
        .create_task(taskcast_core::engine::CreateTaskInput {
            id: Some("t1".to_string()),
            assign_mode: Some(taskcast_core::AssignMode::Pull),
            ..Default::default()
        })
        .await
        .unwrap();

    // Pull with weight update
    let response = server
        .get("/workers/pull?workerId=w1&weight=80")
        .await;
    response.assert_status(axum_test::http::StatusCode::OK);

    // Verify weight was updated
    let worker = manager.get_worker("w1").await.unwrap().unwrap();
    assert_eq!(worker.weight, 80);
}

// ─── Additional WebSocket Error Path Tests ────────────────────────────────

#[tokio::test]
async fn ws_disconnect_after_register_unregisters_worker_v2() {
    let (_engine, manager, server) = make_worker_ws_server();

    {
        let mut ws = server.get_websocket("/workers/ws").await.into_websocket().await;
        ws.send_json(&json!({
            "type": "register",
            "matchRule": {},
            "capacity": 5,
            "workerId": "disconnect-test"
        }))
        .await;

        let resp: serde_json::Value = ws.receive_json().await;
        assert_eq!(resp["type"], "registered");

        // Verify worker exists
        let worker = manager.get_worker("disconnect-test").await.unwrap();
        assert!(worker.is_some());

        // Drop ws to trigger close
    }

    // Give time for disconnect handler
    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

    // Worker should be unregistered
    let worker = manager.get_worker("disconnect-test").await.unwrap();
    assert!(worker.is_none());
}

#[tokio::test]
async fn ws_binary_message_is_ignored() {
    let (_engine, _manager, server) = make_worker_ws_server();

    let mut ws = server.get_websocket("/workers/ws").await.into_websocket().await;

    // Send binary message (should be silently ignored: Some(Ok(_)) => continue)
    ws.send_message(axum_test::WsMessage::Binary(vec![0x01, 0x02, 0x03].into())).await;

    // Send a valid register to prove the connection is still alive
    ws.send_json(&json!({
        "type": "register",
        "matchRule": {},
        "capacity": 5
    }))
    .await;

    let resp: serde_json::Value = ws.receive_json().await;
    assert_eq!(resp["type"], "registered");

    ws.close().await;
}

#[tokio::test]
async fn ws_accept_nonexistent_task_returns_claim_failed_v2() {
    let (_engine, _manager, server) = make_worker_ws_server();

    let mut ws = server.get_websocket("/workers/ws").await.into_websocket().await;
    ws.send_json(&json!({
        "type": "register",
        "matchRule": {},
        "capacity": 5,
        "workerId": "w1"
    }))
    .await;
    let _: serde_json::Value = ws.receive_json().await;

    // Accept a task that doesn't exist — should get CLAIM_FAILED error
    ws.send_json(&json!({
        "type": "accept",
        "taskId": "nonexistent-task"
    }))
    .await;

    let resp: serde_json::Value = ws.receive_json().await;
    assert_eq!(resp["type"], "error");
    assert_eq!(resp["code"], "CLAIM_FAILED");

    ws.close().await;
}

#[tokio::test]
async fn ws_decline_after_register_succeeds() {
    let (_engine, _manager, server) = make_worker_ws_server();

    let mut ws = server.get_websocket("/workers/ws").await.into_websocket().await;
    ws.send_json(&json!({
        "type": "register",
        "matchRule": {},
        "capacity": 5,
        "workerId": "w1"
    }))
    .await;
    let _: serde_json::Value = ws.receive_json().await;

    // Decline a task (even if no assignment exists, should return declined)
    ws.send_json(&json!({
        "type": "decline",
        "taskId": "t1"
    }))
    .await;

    let resp: serde_json::Value = ws.receive_json().await;
    assert_eq!(resp["type"], "declined");
    assert_eq!(resp["taskId"], "t1");

    ws.close().await;
}

#[tokio::test]
async fn ws_decline_with_blacklist_after_register() {
    let (engine, _manager, server) = make_worker_ws_server();

    // Create a task and claim it
    engine
        .create_task(taskcast_core::engine::CreateTaskInput {
            id: Some("t1".to_string()),
            ..Default::default()
        })
        .await
        .unwrap();

    let mut ws = server.get_websocket("/workers/ws").await.into_websocket().await;
    ws.send_json(&json!({
        "type": "register",
        "matchRule": {},
        "capacity": 5,
        "workerId": "w1"
    }))
    .await;
    let _: serde_json::Value = ws.receive_json().await;

    // Claim the task first
    ws.send_json(&json!({
        "type": "claim",
        "taskId": "t1"
    }))
    .await;
    let _: serde_json::Value = ws.receive_json().await;

    // Decline with blacklist
    ws.send_json(&json!({
        "type": "decline",
        "taskId": "t1",
        "blacklist": true
    }))
    .await;

    let resp: serde_json::Value = ws.receive_json().await;
    assert_eq!(resp["type"], "declined");

    ws.close().await;
}

#[tokio::test]
async fn ws_drain_after_register_sets_status() {
    let (_engine, manager, server) = make_worker_ws_server();

    let mut ws = server.get_websocket("/workers/ws").await.into_websocket().await;
    ws.send_json(&json!({
        "type": "register",
        "matchRule": {},
        "capacity": 5,
        "workerId": "w1"
    }))
    .await;
    let _: serde_json::Value = ws.receive_json().await;

    // Drain
    ws.send_json(&json!({"type": "drain"})).await;

    // Give time for processing
    tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

    // Verify worker status changed to draining
    let worker = manager.get_worker("w1").await.unwrap().unwrap();
    assert_eq!(worker.status, taskcast_core::WorkerStatus::Draining);

    ws.close().await;
}

#[tokio::test]
async fn ws_update_after_register_changes_worker() {
    let (_engine, manager, server) = make_worker_ws_server();

    let mut ws = server.get_websocket("/workers/ws").await.into_websocket().await;
    ws.send_json(&json!({
        "type": "register",
        "matchRule": {},
        "capacity": 5,
        "workerId": "w1"
    }))
    .await;
    let _: serde_json::Value = ws.receive_json().await;

    // Update weight and capacity
    ws.send_json(&json!({
        "type": "update",
        "weight": 90,
        "capacity": 10
    }))
    .await;

    // Give time for processing
    tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

    let worker = manager.get_worker("w1").await.unwrap().unwrap();
    assert_eq!(worker.weight, 90);
    assert_eq!(worker.capacity, 10);

    ws.close().await;
}

#[tokio::test]
async fn ws_claim_pending_task_returns_claimed_success() {
    let (engine, _manager, server) = make_worker_ws_server();

    engine
        .create_task(taskcast_core::engine::CreateTaskInput {
            id: Some("t1".to_string()),
            ..Default::default()
        })
        .await
        .unwrap();

    let mut ws = server.get_websocket("/workers/ws").await.into_websocket().await;
    ws.send_json(&json!({
        "type": "register",
        "matchRule": {},
        "capacity": 5,
        "workerId": "w1"
    }))
    .await;
    let _: serde_json::Value = ws.receive_json().await;

    ws.send_json(&json!({
        "type": "claim",
        "taskId": "t1"
    }))
    .await;

    let resp: serde_json::Value = ws.receive_json().await;
    assert_eq!(resp["type"], "claimed");
    assert_eq!(resp["taskId"], "t1");
    assert_eq!(resp["success"], true);

    ws.close().await;
}

#[tokio::test]
async fn ws_accept_pending_task_returns_assigned_v2() {
    let (engine, _manager, server) = make_worker_ws_server();

    engine
        .create_task(taskcast_core::engine::CreateTaskInput {
            id: Some("t1".to_string()),
            ..Default::default()
        })
        .await
        .unwrap();

    let mut ws = server.get_websocket("/workers/ws").await.into_websocket().await;
    ws.send_json(&json!({
        "type": "register",
        "matchRule": {},
        "capacity": 5,
        "workerId": "w1"
    }))
    .await;
    let _: serde_json::Value = ws.receive_json().await;

    ws.send_json(&json!({
        "type": "accept",
        "taskId": "t1"
    }))
    .await;

    let resp: serde_json::Value = ws.receive_json().await;
    assert_eq!(resp["type"], "assigned");
    assert_eq!(resp["taskId"], "t1");

    ws.close().await;
}

// ─── Workers REST: pull_task NO_CONTENT and error paths ──────────────────

#[tokio::test]
async fn pull_task_returns_no_content_when_no_tasks() {
    let (_engine, manager, server) = make_worker_server();
    register_test_worker(&manager, "w1").await;

    // Use short timeout=100ms so this test doesn't wait 30s
    let response = server.get("/workers/pull?workerId=w1&timeout=100").await;
    response.assert_status(axum_test::http::StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn pull_task_returns_400_when_worker_not_registered() {
    let (_engine, _manager, server) = make_worker_server();

    // Don't register any worker — pull with a non-existent workerId triggers
    // manager_error (WorkerManager returns error "Worker not found").
    // heartbeat is a no-op for missing workers, but wait_for_task errors.
    let response = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        server.get("/workers/pull?workerId=nonexistent"),
    )
    .await
    .expect("pull request timed out");
    response.assert_status(axum_test::http::StatusCode::BAD_REQUEST);
}

// ─── OpenAPI / Scalar UI ────────────────────────────────────────────────────

#[tokio::test]
async fn openapi_json_returns_valid_spec() {
    let (_engine, server) = make_no_auth_server();

    let response = server.get("/openapi.json").await;
    response.assert_status(axum_test::http::StatusCode::OK);

    let body: serde_json::Value = response.json();

    // Must be OpenAPI 3.1.x
    assert_eq!(body["openapi"], "3.1.0");

    // Must have correct title
    assert_eq!(body["info"]["title"], "Taskcast API");

    // Must contain a paths object with /tasks
    assert!(body["paths"].is_object(), "paths should be an object");
    assert!(
        body["paths"]["/tasks"].is_object(),
        "paths should contain /tasks"
    );
}

#[tokio::test]
async fn docs_returns_html() {
    let (_engine, server) = make_no_auth_server();

    let response = server.get("/docs").await;
    response.assert_status(axum_test::http::StatusCode::OK);

    let header_value = response.header("content-type");
    let content_type = header_value
        .to_str()
        .expect("content-type header should be valid UTF-8");
    assert!(
        content_type.contains("text/html"),
        "expected text/html content type, got: {content_type}"
    );
}
