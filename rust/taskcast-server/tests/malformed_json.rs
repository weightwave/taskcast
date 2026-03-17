use std::sync::Arc;

use axum_test::TestServer;
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
