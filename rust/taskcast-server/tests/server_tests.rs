use std::sync::Arc;

use axum_test::http::HeaderValue;
use axum_test::TestServer;
use jsonwebtoken::{encode, EncodingKey, Header};
use serde_json::json;
use taskcast_core::{
    MemoryBroadcastProvider, MemoryShortTermStore, TaskEngine, TaskEngineOptions,
};
use taskcast_server::{create_app, AuthMode, JwtConfig};

fn make_engine() -> Arc<TaskEngine> {
    Arc::new(TaskEngine::new(TaskEngineOptions {
        short_term: Arc::new(MemoryShortTermStore::new()),
        broadcast: Arc::new(MemoryBroadcastProvider::new()),
        long_term: None,
        hooks: None,
    }))
}

fn make_server(engine: Arc<TaskEngine>, auth_mode: AuthMode) -> TestServer {
    let app = create_app(engine, auth_mode);
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
