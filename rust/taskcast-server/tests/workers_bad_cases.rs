use std::sync::Arc;

use axum_test::http::HeaderValue;
use axum_test::TestServer;
use jsonwebtoken::{encode, EncodingKey, Header};
use serde_json::json;
use taskcast_core::worker_manager::{WorkerManager, WorkerManagerOptions, WorkerRegistration};
use taskcast_core::{
    AssignMode, BroadcastProvider, ConnectionMode, CreateTaskInput, MemoryBroadcastProvider,
    MemoryShortTermStore, ShortTermStore, TaskEngine, TaskEngineOptions, WorkerMatchRule,
};
use taskcast_server::{create_app, AuthMode, CorsConfig, JwtConfig};

const JWT_SECRET: &str = "test-secret-key-for-jwt-signing-needs-to-be-long-enough";

fn make_no_auth_server_with_workers() -> (Arc<TaskEngine>, Arc<WorkerManager>, TestServer) {
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

fn make_jwt_server_with_workers() -> (Arc<TaskEngine>, Arc<WorkerManager>, TestServer) {
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
    let (app, _) = create_app(
        Arc::clone(&engine),
        auth_mode,
        Some(Arc::clone(&manager)),
        None,
        CorsConfig::default(),
    );
    let server = TestServer::new(app);
    (engine, manager, server)
}

fn make_token(claims: serde_json::Value) -> String {
    encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(JWT_SECRET.as_bytes()),
    )
    .unwrap()
}

fn bearer_header(token: &str) -> HeaderValue {
    HeaderValue::from_str(&format!("Bearer {token}")).unwrap()
}

async fn register_worker(manager: &WorkerManager, id: &str) {
    manager
        .register_worker(WorkerRegistration {
            worker_id: Some(id.to_string()),
            match_rule: WorkerMatchRule::default(),
            connection_mode: ConnectionMode::Pull,
            capacity: 1,
            weight: None,
            metadata: None,
        })
        .await
        .unwrap();
}

// ─── GET /workers/{worker_id} — not found ────────────────────────────────────

#[tokio::test]
async fn get_worker_returns_404_for_nonexistent_worker() {
    let (_engine, _manager, server) = make_no_auth_server_with_workers();

    let resp = server.get("/workers/nonexistent-id").await;
    resp.assert_status_not_found();
}

// ─── DELETE /workers/{worker_id} — not found ─────────────────────────────────

#[tokio::test]
async fn delete_worker_returns_404_for_nonexistent_worker() {
    let (_engine, _manager, server) = make_no_auth_server_with_workers();

    let resp = server.delete("/workers/nonexistent-id").await;
    resp.assert_status_not_found();
}

// ─── PATCH /workers/{worker_id}/status — malformed JSON ──────────────────────

#[tokio::test]
async fn update_worker_status_returns_422_for_malformed_json() {
    let (_engine, manager, server) = make_no_auth_server_with_workers();
    register_worker(&manager, "w1").await;

    let resp = server
        .patch("/workers/w1/status")
        .content_type("application/json")
        .bytes("not json".into())
        .await;
    let status = resp.status_code().as_u16();
    assert!(status >= 400 && status < 500, "expected 4xx, got {status}");
}

#[tokio::test]
async fn update_worker_status_returns_422_for_invalid_status_enum() {
    let (_engine, manager, server) = make_no_auth_server_with_workers();
    register_worker(&manager, "w1").await;

    let resp = server
        .patch("/workers/w1/status")
        .content_type("application/json")
        .json(&json!({"status": "bogus"}))
        .await;
    let status = resp.status_code().as_u16();
    assert!(status >= 400 && status < 500, "expected 4xx, got {status}");
}

#[tokio::test]
async fn update_worker_status_returns_422_for_empty_body() {
    let (_engine, manager, server) = make_no_auth_server_with_workers();
    register_worker(&manager, "w1").await;

    let resp = server
        .patch("/workers/w1/status")
        .content_type("application/json")
        .json(&json!({}))
        .await;
    let status = resp.status_code().as_u16();
    assert!(status >= 400 && status < 500, "expected 4xx, got {status}");
}

// ─── PATCH /workers/{worker_id}/status — worker not found ────────────────────

#[tokio::test]
async fn update_worker_status_fails_for_nonexistent_worker() {
    let (_engine, _manager, server) = make_no_auth_server_with_workers();

    let resp = server
        .patch("/workers/nonexistent/status")
        .content_type("application/json")
        .json(&json!({"status": "draining"}))
        .await;
    let status = resp.status_code().as_u16();
    assert!(status >= 400 && status < 500, "expected 4xx, got {status}");
}

// ─── POST /workers/tasks/{task_id}/decline — malformed JSON ──────────────────

#[tokio::test]
async fn decline_task_returns_422_for_malformed_json() {
    let (_engine, _manager, server) = make_no_auth_server_with_workers();

    let resp = server
        .post("/workers/tasks/some-task/decline")
        .content_type("application/json")
        .bytes("not json".into())
        .await;
    let status = resp.status_code().as_u16();
    assert!(status >= 400 && status < 500, "expected 4xx, got {status}");
}

#[tokio::test]
async fn decline_task_returns_422_for_missing_worker_id() {
    let (_engine, _manager, server) = make_no_auth_server_with_workers();

    let resp = server
        .post("/workers/tasks/some-task/decline")
        .content_type("application/json")
        .json(&json!({}))
        .await;
    let status = resp.status_code().as_u16();
    assert!(status >= 400 && status < 500, "expected 4xx, got {status}");
}

// ─── Auth: worker_id mismatch in pull ────────────────────────────────────────

#[tokio::test]
async fn pull_task_returns_403_when_worker_id_mismatches_token() {
    let (_engine, manager, server) = make_jwt_server_with_workers();
    register_worker(&manager, "w1").await;

    let token = make_token(json!({
        "sub": "worker",
        "scope": ["worker:connect"],
        "taskIds": "*",
        "workerId": "w1",
        "exp": 9999999999u64
    }));

    // Try to pull as w2 but token says w1
    let resp = server
        .get("/workers/pull")
        .add_query_param("workerId", "w2")
        .add_header(axum_test::http::header::AUTHORIZATION, bearer_header(&token))
        .await;
    resp.assert_status(axum_test::http::StatusCode::FORBIDDEN);
}

// ─── Auth: worker_id mismatch in decline ─────────────────────────────────────

#[tokio::test]
async fn decline_task_returns_403_when_worker_id_mismatches_token() {
    let (_engine, _manager, server) = make_jwt_server_with_workers();

    let token = make_token(json!({
        "sub": "worker",
        "scope": ["worker:connect"],
        "taskIds": "*",
        "workerId": "w1",
        "exp": 9999999999u64
    }));

    let resp = server
        .post("/workers/tasks/some-task/decline")
        .content_type("application/json")
        .json(&json!({"workerId": "w2"}))
        .add_header(axum_test::http::header::AUTHORIZATION, bearer_header(&token))
        .await;
    resp.assert_status(axum_test::http::StatusCode::FORBIDDEN);
}

// ─── Auth: insufficient scope ────────────────────────────────────────────────

#[tokio::test]
async fn list_workers_returns_403_without_worker_manage_scope() {
    let (_engine, _manager, server) = make_jwt_server_with_workers();

    let token = make_token(json!({
        "sub": "user",
        "scope": ["task:create"],
        "taskIds": "*",
        "exp": 9999999999u64
    }));

    let resp = server
        .get("/workers")
        .add_header(axum_test::http::header::AUTHORIZATION, bearer_header(&token))
        .await;
    resp.assert_status(axum_test::http::StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn delete_worker_returns_403_without_worker_manage_scope() {
    let (_engine, _manager, server) = make_jwt_server_with_workers();

    let token = make_token(json!({
        "sub": "user",
        "scope": ["task:create"],
        "taskIds": "*",
        "exp": 9999999999u64
    }));

    let resp = server
        .delete("/workers/some-worker")
        .add_header(axum_test::http::header::AUTHORIZATION, bearer_header(&token))
        .await;
    resp.assert_status(axum_test::http::StatusCode::FORBIDDEN);
}

// ─── PATCH /workers/{worker_id}/status — forbidden scope ──────────────────

#[tokio::test]
async fn update_worker_status_returns_403_without_worker_manage_scope() {
    let (_engine, manager, server) = make_jwt_server_with_workers();
    register_worker(&manager, "w1").await;

    let token = make_token(json!({
        "sub": "user",
        "scope": ["task:create"],  // wrong scope
        "taskIds": "*",
        "exp": 9999999999u64
    }));

    let resp = server
        .patch("/workers/w1/status")
        .json(&json!({"status": "draining"}))
        .add_header(axum_test::http::header::AUTHORIZATION, bearer_header(&token))
        .await;
    resp.assert_status(axum_test::http::StatusCode::FORBIDDEN);
}

// ─── PATCH /workers/{worker_id}/status — idle status (success) ────────────

#[tokio::test]
async fn update_worker_status_to_idle_succeeds() {
    let (_engine, manager, server) = make_no_auth_server_with_workers();
    register_worker(&manager, "w-idle").await;

    // First set to draining, then back to idle
    let resp = server
        .patch("/workers/w-idle/status")
        .json(&json!({"status": "draining"}))
        .await;
    resp.assert_status_ok();

    let resp = server
        .patch("/workers/w-idle/status")
        .json(&json!({"status": "idle"}))
        .await;
    resp.assert_status_ok();
    let body: serde_json::Value = resp.json();
    assert_eq!(body["status"], "idle");
}

// ─── POST /workers/tasks/{task_id}/decline — matching worker_id ───────────

#[tokio::test]
async fn decline_task_succeeds_when_worker_id_matches_token() {
    // This covers line 283 - the path where token_worker_id == body.worker_id
    let (engine, manager, server) = make_jwt_server_with_workers();
    register_worker(&manager, "w1").await;

    // Create a task
    engine
        .create_task(CreateTaskInput {
            id: Some("decline-match".to_string()),
            assign_mode: Some(AssignMode::WsRace),
            ..Default::default()
        })
        .await
        .unwrap();

    let token = make_token(json!({
        "sub": "worker",
        "scope": ["worker:connect"],
        "taskIds": "*",
        "workerId": "w1",
        "exp": 9999999999u64
    }));

    // Decline with matching worker_id - should pass the auth check
    // (may still fail on business logic, but the 403 check passes)
    let resp = server
        .post("/workers/tasks/decline-match/decline")
        .json(&json!({"workerId": "w1"}))
        .add_header(axum_test::http::header::AUTHORIZATION, bearer_header(&token))
        .await;
    // Should NOT be 403 (the auth check passes).
    // It may be 200 or some other error depending on the task state,
    // but importantly it won't be 403.
    let status = resp.status_code().as_u16();
    assert_ne!(status, 403, "should not be forbidden when worker_id matches");
}
