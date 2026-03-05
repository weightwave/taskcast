use std::sync::Arc;

use axum_test::TestServer;
use serde_json::json;
use taskcast_core::{
    BlockedRequest, MemoryBroadcastProvider, MemoryShortTermStore, TaskEngine, TaskEngineOptions,
    TaskStatus, TransitionPayload,
};
use taskcast_server::{create_app, AuthMode};

fn make_engine() -> Arc<TaskEngine> {
    Arc::new(TaskEngine::new(TaskEngineOptions {
        short_term_store: Arc::new(MemoryShortTermStore::new()),
        broadcast: Arc::new(MemoryBroadcastProvider::new()),
        long_term_store: None,
        hooks: None,
    }))
}

fn make_server(engine: Arc<TaskEngine>) -> TestServer {
    let (app, _) = create_app(engine, AuthMode::None, None);
    TestServer::new(app)
}

/// Helper: create a task, move it to running, then to blocked with an optional blocked_request.
async fn setup_blocked_task(
    engine: &Arc<TaskEngine>,
    task_id: &str,
    blocked_request: Option<BlockedRequest>,
) {
    engine
        .create_task(taskcast_core::CreateTaskInput {
            id: Some(task_id.to_string()),
            ..Default::default()
        })
        .await
        .unwrap();

    engine
        .transition_task(task_id, TaskStatus::Running, None)
        .await
        .unwrap();

    engine
        .transition_task(
            task_id,
            TaskStatus::Blocked,
            Some(TransitionPayload {
                blocked_request,
                ..Default::default()
            }),
        )
        .await
        .unwrap();
}

// ─── POST /tasks/:id/resolve ────────────────────────────────────────────────

#[tokio::test]
async fn resolve_transitions_blocked_task_to_running() {
    let engine = make_engine();
    let server = make_server(Arc::clone(&engine));

    setup_blocked_task(
        &engine,
        "resolve-1",
        Some(BlockedRequest {
            request_type: "approval".to_string(),
            data: json!({"prompt": "approve?"}),
        }),
    )
    .await;

    let response = server
        .post("/tasks/resolve-1/resolve")
        .json(&json!({ "data": { "approved": true } }))
        .await;

    response.assert_status(axum_test::http::StatusCode::OK);
    let body: serde_json::Value = response.json();
    assert_eq!(body["status"], "running");
    assert_eq!(body["id"], "resolve-1");
    // The resolution data should be stored as result
    assert_eq!(body["result"]["approved"], true);
}

#[tokio::test]
async fn resolve_with_non_object_data_wraps_in_resolution() {
    let engine = make_engine();
    let server = make_server(Arc::clone(&engine));

    setup_blocked_task(&engine, "resolve-wrap", None).await;

    let response = server
        .post("/tasks/resolve-wrap/resolve")
        .json(&json!({ "data": "yes" }))
        .await;

    response.assert_status(axum_test::http::StatusCode::OK);
    let body: serde_json::Value = response.json();
    assert_eq!(body["status"], "running");
    assert_eq!(body["result"]["resolution"], "yes");
}

#[tokio::test]
async fn resolve_returns_400_for_non_blocked_task() {
    let engine = make_engine();
    let server = make_server(Arc::clone(&engine));

    // Create a task in running state (not blocked)
    engine
        .create_task(taskcast_core::CreateTaskInput {
            id: Some("resolve-not-blocked".to_string()),
            ..Default::default()
        })
        .await
        .unwrap();
    engine
        .transition_task("resolve-not-blocked", TaskStatus::Running, None)
        .await
        .unwrap();

    let response = server
        .post("/tasks/resolve-not-blocked/resolve")
        .json(&json!({ "data": {} }))
        .await;

    response.assert_status(axum_test::http::StatusCode::BAD_REQUEST);
    let body: serde_json::Value = response.json();
    assert!(body["error"].as_str().unwrap().contains("not blocked"));
}

#[tokio::test]
async fn resolve_returns_404_for_nonexistent_task() {
    let engine = make_engine();
    let server = make_server(Arc::clone(&engine));

    let response = server
        .post("/tasks/nonexistent/resolve")
        .json(&json!({ "data": {} }))
        .await;

    response.assert_status(axum_test::http::StatusCode::NOT_FOUND);
}

// ─── GET /tasks/:id/request ─────────────────────────────────────────────────

#[tokio::test]
async fn get_request_returns_blocked_request() {
    let engine = make_engine();
    let server = make_server(Arc::clone(&engine));

    setup_blocked_task(
        &engine,
        "req-1",
        Some(BlockedRequest {
            request_type: "user_input".to_string(),
            data: json!({"fields": ["name", "email"]}),
        }),
    )
    .await;

    let response = server.get("/tasks/req-1/request").await;

    response.assert_status(axum_test::http::StatusCode::OK);
    let body: serde_json::Value = response.json();
    assert_eq!(body["type"], "user_input");
    assert_eq!(body["data"]["fields"][0], "name");
    assert_eq!(body["data"]["fields"][1], "email");
}

#[tokio::test]
async fn get_request_returns_404_for_non_blocked_task() {
    let engine = make_engine();
    let server = make_server(Arc::clone(&engine));

    // Create a task in running state
    engine
        .create_task(taskcast_core::CreateTaskInput {
            id: Some("req-not-blocked".to_string()),
            ..Default::default()
        })
        .await
        .unwrap();
    engine
        .transition_task("req-not-blocked", TaskStatus::Running, None)
        .await
        .unwrap();

    let response = server.get("/tasks/req-not-blocked/request").await;

    response.assert_status(axum_test::http::StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn get_request_returns_404_for_blocked_task_without_request() {
    let engine = make_engine();
    let server = make_server(Arc::clone(&engine));

    // Blocked task but no blocked_request
    setup_blocked_task(&engine, "req-no-body", None).await;

    let response = server.get("/tasks/req-no-body/request").await;

    response.assert_status(axum_test::http::StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn get_request_returns_404_for_nonexistent_task() {
    let engine = make_engine();
    let server = make_server(Arc::clone(&engine));

    let response = server.get("/tasks/nonexistent/request").await;

    response.assert_status(axum_test::http::StatusCode::NOT_FOUND);
}

// ─── PATCH /tasks/:id/status with new fields ───────────────────────────────

#[tokio::test]
async fn transition_with_reason_stores_it() {
    let engine = make_engine();
    let server = make_server(Arc::clone(&engine));

    engine
        .create_task(taskcast_core::CreateTaskInput {
            id: Some("reason-1".to_string()),
            ..Default::default()
        })
        .await
        .unwrap();
    engine
        .transition_task("reason-1", TaskStatus::Running, None)
        .await
        .unwrap();

    let response = server
        .patch("/tasks/reason-1/status")
        .json(&json!({
            "status": "paused",
            "reason": "waiting for external resource"
        }))
        .await;

    response.assert_status(axum_test::http::StatusCode::OK);
    let body: serde_json::Value = response.json();
    assert_eq!(body["status"], "paused");
    assert_eq!(body["reason"], "waiting for external resource");
}

#[tokio::test]
async fn transition_with_blocked_request_stores_it() {
    let engine = make_engine();
    let server = make_server(Arc::clone(&engine));

    engine
        .create_task(taskcast_core::CreateTaskInput {
            id: Some("br-1".to_string()),
            ..Default::default()
        })
        .await
        .unwrap();
    engine
        .transition_task("br-1", TaskStatus::Running, None)
        .await
        .unwrap();

    let response = server
        .patch("/tasks/br-1/status")
        .json(&json!({
            "status": "blocked",
            "blockedRequest": {
                "type": "approval",
                "data": { "question": "proceed?" }
            }
        }))
        .await;

    response.assert_status(axum_test::http::StatusCode::OK);
    let body: serde_json::Value = response.json();
    assert_eq!(body["status"], "blocked");
    assert_eq!(body["blockedRequest"]["type"], "approval");
    assert_eq!(body["blockedRequest"]["data"]["question"], "proceed?");
}

#[tokio::test]
async fn transition_with_resume_after_ms_sets_resume_at() {
    let engine = make_engine();
    let server = make_server(Arc::clone(&engine));

    engine
        .create_task(taskcast_core::CreateTaskInput {
            id: Some("resume-1".to_string()),
            ..Default::default()
        })
        .await
        .unwrap();
    engine
        .transition_task("resume-1", TaskStatus::Running, None)
        .await
        .unwrap();

    let response = server
        .patch("/tasks/resume-1/status")
        .json(&json!({
            "status": "blocked",
            "resumeAfterMs": 5000.0
        }))
        .await;

    response.assert_status(axum_test::http::StatusCode::OK);
    let body: serde_json::Value = response.json();
    assert_eq!(body["status"], "blocked");
    // resume_at should be set (some positive value)
    assert!(body["resumeAt"].as_f64().unwrap() > 0.0);
}
