use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use axum_test::http::HeaderValue;
use axum_test::TestServer;
use serde_json::json;
use taskcast_core::{
    BlockedRequest, CreateTaskInput, MemoryBroadcastProvider, MemoryShortTermStore, TaskEngine,
    TaskEngineOptions, TaskStatus, TransitionPayload,
};
use taskcast_server::{create_app, AuthMode, CorsConfig, JwtConfig};

const JWT_SECRET: &str = "test-secret-key-for-jwt-signing-needs-to-be-long-enough";

fn make_jwt_server() -> (Arc<TaskEngine>, TestServer) {
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
    let (app, _) = create_app(Arc::clone(&engine), auth_mode, None, None, CorsConfig::default());
    (engine, TestServer::new(app))
}

fn make_token(claims: serde_json::Value) -> String {
    jsonwebtoken::encode(
        &jsonwebtoken::Header::default(),
        &claims,
        &jsonwebtoken::EncodingKey::from_secret(JWT_SECRET.as_bytes()),
    )
    .unwrap()
}

fn bearer_header(token: &str) -> HeaderValue {
    HeaderValue::from_str(&format!("Bearer {token}")).unwrap()
}

/// Helper: create a task, move it to running, then to blocked with a blocked_request.
async fn setup_blocked_task(engine: &TaskEngine, task_id: &str) {
    engine
        .create_task(CreateTaskInput {
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
                blocked_request: Some(BlockedRequest {
                    request_type: "approval".to_string(),
                    data: json!({"prompt": "approve?"}),
                }),
                ..Default::default()
            }),
        )
        .await
        .unwrap();
}

fn future_exp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
        + 3600
}

// ─── POST /tasks/:id/resolve — 403 without task:resolve scope ─────────────

#[tokio::test]
async fn resolve_task_returns_403_without_task_resolve_scope() {
    let (engine, server) = make_jwt_server();
    setup_blocked_task(&engine, "auth-resolve-1").await;

    // Token with only task:create scope — no task:resolve
    let token = make_token(json!({
        "sub": "test-user",
        "scope": ["task:create"],
        "exp": future_exp()
    }));

    let resp = server
        .post("/tasks/auth-resolve-1/resolve")
        .add_header(axum_test::http::header::AUTHORIZATION, bearer_header(&token))
        .json(&json!({ "data": { "approved": true } }))
        .await;

    resp.assert_status(axum_test::http::StatusCode::FORBIDDEN);
    let body: serde_json::Value = resp.json();
    assert_eq!(body["error"], "Forbidden");
}

// ─── GET /tasks/:id/request — 403 without task:resolve scope ──────────────

#[tokio::test]
async fn get_blocked_request_returns_403_without_task_resolve_scope() {
    let (engine, server) = make_jwt_server();
    setup_blocked_task(&engine, "auth-request-1").await;

    // Token with only task:create scope — no task:resolve
    let token = make_token(json!({
        "sub": "test-user",
        "scope": ["task:create"],
        "exp": future_exp()
    }));

    let resp = server
        .get("/tasks/auth-request-1/request")
        .add_header(axum_test::http::header::AUTHORIZATION, bearer_header(&token))
        .await;

    resp.assert_status(axum_test::http::StatusCode::FORBIDDEN);
    let body: serde_json::Value = resp.json();
    assert_eq!(body["error"], "Forbidden");
}
