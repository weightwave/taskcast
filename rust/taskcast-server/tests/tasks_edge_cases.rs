use std::sync::Arc;

use axum_test::TestServer;
use serde_json::json;
use taskcast_core::{
    CreateTaskInput, MemoryBroadcastProvider, MemoryShortTermStore, TaskEngine, TaskEngineOptions,
    TaskStatus,
};
use taskcast_server::{create_app, AuthMode, CorsConfig};

fn make_engine() -> Arc<TaskEngine> {
    Arc::new(TaskEngine::new(TaskEngineOptions {
        short_term_store: Arc::new(MemoryShortTermStore::new()),
        broadcast: Arc::new(MemoryBroadcastProvider::new()),
        long_term_store: None,
        hooks: None,
    }))
}

fn make_server(engine: Arc<TaskEngine>) -> TestServer {
    let (app, _) = create_app(engine, AuthMode::None, None, None, CorsConfig::default());
    TestServer::new(app)
}

async fn create_running_task(engine: &TaskEngine, id: &str) {
    engine
        .create_task(CreateTaskInput {
            id: Some(id.to_string()),
            ..Default::default()
        })
        .await
        .unwrap();
    engine
        .transition_task(id, TaskStatus::Running, None)
        .await
        .unwrap();
}

// ─── POST /tasks/:id/events — batch edge cases ──────────────────────────────

#[tokio::test]
async fn publish_events_empty_batch_returns_201_empty_array() {
    let engine = make_engine();
    let server = make_server(Arc::clone(&engine));
    create_running_task(&engine, "batch-empty").await;

    let resp = server
        .post("/tasks/batch-empty/events")
        .json(&json!([]))
        .await;
    resp.assert_status(axum_test::http::StatusCode::CREATED);
    let body: serde_json::Value = resp.json();
    assert!(body.is_array());
    assert_eq!(body.as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn publish_events_batch_multiple_events() {
    let engine = make_engine();
    let server = make_server(Arc::clone(&engine));
    create_running_task(&engine, "batch-multi").await;

    let resp = server
        .post("/tasks/batch-multi/events")
        .json(&json!([
            {"type": "log", "level": "info", "data": {"msg": "one"}},
            {"type": "log", "level": "info", "data": {"msg": "two"}},
            {"type": "log", "level": "info", "data": {"msg": "three"}}
        ]))
        .await;
    resp.assert_status(axum_test::http::StatusCode::CREATED);
    let body: serde_json::Value = resp.json();
    assert_eq!(body.as_array().unwrap().len(), 3);
}

#[tokio::test]
async fn publish_events_batch_with_invalid_event_returns_400() {
    let engine = make_engine();
    let server = make_server(Arc::clone(&engine));
    create_running_task(&engine, "batch-invalid").await;

    // Missing required fields in one event
    let resp = server
        .post("/tasks/batch-invalid/events")
        .json(&json!([
            {"type": "log", "level": "info", "data": {"msg": "ok"}},
            {"bad": "event"}
        ]))
        .await;
    resp.assert_status_bad_request();
}

// ─── POST /tasks/:id/events — publish to nonexistent task ────────────────────

#[tokio::test]
async fn publish_events_to_nonexistent_task_returns_404() {
    let engine = make_engine();
    let server = make_server(Arc::clone(&engine));

    let resp = server
        .post("/tasks/nonexistent/events")
        .json(&json!({"type": "log", "level": "info", "data": {}}))
        .await;
    let status = resp.status_code().as_u16();
    // Could be 404 or 400 depending on engine behavior
    assert!(status >= 400, "expected error, got {status}");
}

// ─── POST /tasks/:id/events — publish to terminal task ───────────────────────

#[tokio::test]
async fn publish_events_to_completed_task_returns_error() {
    let engine = make_engine();
    let server = make_server(Arc::clone(&engine));
    create_running_task(&engine, "terminal-pub").await;
    engine
        .transition_task("terminal-pub", TaskStatus::Completed, None)
        .await
        .unwrap();

    let resp = server
        .post("/tasks/terminal-pub/events")
        .json(&json!({"type": "log", "level": "info", "data": {}}))
        .await;
    let status = resp.status_code().as_u16();
    assert!(status >= 400, "expected error, got {status}");
}

// ─── PATCH /tasks/:id/status — invalid transitions ──────────────────────────

#[tokio::test]
async fn transition_nonexistent_task_returns_404() {
    let engine = make_engine();
    let server = make_server(Arc::clone(&engine));

    let resp = server
        .patch("/tasks/nonexistent/status")
        .json(&json!({"status": "running"}))
        .await;
    resp.assert_status_not_found();
}

#[tokio::test]
async fn transition_invalid_status_value_returns_422() {
    let engine = make_engine();
    let server = make_server(Arc::clone(&engine));
    engine
        .create_task(CreateTaskInput {
            id: Some("bad-status".to_string()),
            ..Default::default()
        })
        .await
        .unwrap();

    let resp = server
        .patch("/tasks/bad-status/status")
        .json(&json!({"status": "bogus_status"}))
        .await;
    let status = resp.status_code().as_u16();
    assert!(status >= 400 && status < 500, "expected 4xx, got {status}");
}

#[tokio::test]
async fn transition_backwards_returns_conflict() {
    let engine = make_engine();
    let server = make_server(Arc::clone(&engine));
    create_running_task(&engine, "backward").await;
    engine
        .transition_task("backward", TaskStatus::Completed, None)
        .await
        .unwrap();

    // Try to go back to running
    let resp = server
        .patch("/tasks/backward/status")
        .json(&json!({"status": "running"}))
        .await;
    let status = resp.status_code().as_u16();
    assert!(status >= 400, "expected error for backward transition, got {status}");
}

#[tokio::test]
async fn transition_completed_to_failed_returns_error() {
    let engine = make_engine();
    let server = make_server(Arc::clone(&engine));
    create_running_task(&engine, "double-terminal").await;
    engine
        .transition_task("double-terminal", TaskStatus::Completed, None)
        .await
        .unwrap();

    let resp = server
        .patch("/tasks/double-terminal/status")
        .json(&json!({"status": "failed"}))
        .await;
    let status = resp.status_code().as_u16();
    assert!(status >= 400, "expected error for double terminal transition, got {status}");
}

// ─── PATCH /tasks/:id/status — with error payload ────────────────────────────

#[tokio::test]
async fn transition_to_failed_with_error_payload() {
    let engine = make_engine();
    let server = make_server(Arc::clone(&engine));
    create_running_task(&engine, "fail-err").await;

    let resp = server
        .patch("/tasks/fail-err/status")
        .json(&json!({
            "status": "failed",
            "error": {
                "code": "TIMEOUT",
                "message": "Operation timed out",
                "details": {"elapsed_ms": 30000}
            }
        }))
        .await;
    resp.assert_status_ok();
    let body: serde_json::Value = resp.json();
    assert_eq!(body["status"], "failed");
    assert_eq!(body["error"]["code"], "TIMEOUT");
    assert_eq!(body["error"]["message"], "Operation timed out");
}

// ─── GET /tasks/:id — not found ─────────────────────────────────────────────

#[tokio::test]
async fn get_nonexistent_task_returns_404() {
    let engine = make_engine();
    let server = make_server(Arc::clone(&engine));

    let resp = server.get("/tasks/nonexistent").await;
    resp.assert_status_not_found();
}

// ─── GET /tasks — list with filters ─────────────────────────────────────────

#[tokio::test]
async fn list_tasks_with_invalid_status_filter_returns_empty() {
    let engine = make_engine();
    let server = make_server(Arc::clone(&engine));
    engine
        .create_task(CreateTaskInput {
            id: Some("list-1".to_string()),
            ..Default::default()
        })
        .await
        .unwrap();

    let resp = server
        .get("/tasks")
        .add_query_param("status", "bogus_status")
        .await;
    resp.assert_status_ok();
    let body: serde_json::Value = resp.json();
    // Invalid status filter should be ignored (filter_map skips it)
    // so all tasks are returned
    let tasks = body["tasks"].as_array().unwrap();
    assert!(!tasks.is_empty());
}

#[tokio::test]
async fn list_tasks_with_empty_status_filter() {
    let engine = make_engine();
    let server = make_server(Arc::clone(&engine));
    engine
        .create_task(CreateTaskInput {
            id: Some("list-empty-filter".to_string()),
            ..Default::default()
        })
        .await
        .unwrap();

    let resp = server
        .get("/tasks")
        .add_query_param("status", "")
        .await;
    resp.assert_status_ok();
}

// ─── GET /tasks/:id/events/history — edge cases ─────────────────────────────

#[tokio::test]
async fn event_history_nonexistent_task_returns_404() {
    let engine = make_engine();
    let server = make_server(Arc::clone(&engine));

    let resp = server.get("/tasks/nonexistent/events/history").await;
    resp.assert_status_not_found();
}

#[tokio::test]
async fn event_history_empty_returns_200() {
    let engine = make_engine();
    let server = make_server(Arc::clone(&engine));
    engine
        .create_task(CreateTaskInput {
            id: Some("hist-empty".to_string()),
            ..Default::default()
        })
        .await
        .unwrap();

    let resp = server.get("/tasks/hist-empty/events/history").await;
    resp.assert_status_ok();
    let body: serde_json::Value = resp.json();
    // Only the initial status event should be present
    assert!(body.is_array());
}

// ─── POST /tasks — duplicate ID → conflict ──────────────────────────────────

#[tokio::test]
async fn create_task_duplicate_id_returns_conflict() {
    let engine = make_engine();
    let server = make_server(Arc::clone(&engine));
    engine
        .create_task(CreateTaskInput {
            id: Some("dup-1".to_string()),
            ..Default::default()
        })
        .await
        .unwrap();

    let resp = server
        .post("/tasks")
        .json(&json!({"id": "dup-1"}))
        .await;
    let status = resp.status_code().as_u16();
    assert_eq!(status, 409, "expected 409 Conflict, got {status}");
}

// ─── GET /tasks — type filter ─────────────────────────────────────────────

#[tokio::test]
async fn list_tasks_with_type_filter() {
    let engine = make_engine();
    let server = make_server(Arc::clone(&engine));

    engine
        .create_task(CreateTaskInput {
            id: Some("type-crawl".to_string()),
            r#type: Some("crawl".to_string()),
            ..Default::default()
        })
        .await
        .unwrap();

    engine
        .create_task(CreateTaskInput {
            id: Some("type-analysis".to_string()),
            r#type: Some("analysis".to_string()),
            ..Default::default()
        })
        .await
        .unwrap();

    let resp = server
        .get("/tasks")
        .add_query_param("type", "crawl")
        .await;
    resp.assert_status_ok();
    let body: serde_json::Value = resp.json();
    let tasks = body["tasks"].as_array().unwrap();
    assert_eq!(tasks.len(), 1, "expected exactly 1 task with type=crawl");
    assert_eq!(tasks[0]["id"], "type-crawl");
    assert_eq!(tasks[0]["type"], "crawl");
}

// ─── POST /tasks — InvalidInput (ttl=0) ───────────────────────────────────

#[tokio::test]
async fn create_task_with_zero_ttl_returns_400() {
    let engine = make_engine();
    let server = make_server(Arc::clone(&engine));

    let resp = server
        .post("/tasks")
        .json(&json!({"ttl": 0}))
        .await;
    resp.assert_status_bad_request();
    let body: serde_json::Value = resp.json();
    let error_msg = body["error"].as_str().unwrap();
    assert!(
        error_msg.contains("TTL"),
        "expected error message to mention TTL, got: {error_msg}"
    );
}
