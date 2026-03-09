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

// ─── "Bearer " with empty token after space ────────────────────────────────

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

// ─── "Bearer" with no space at all ─────────────────────────────────────────

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
    let body: serde_json::Value = response.json();
    assert_eq!(body["error"], "Missing Bearer token");
}

// ─── lowercase "bearer" ────────────────────────────────────────────────────

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
    let body: serde_json::Value = response.json();
    assert_eq!(body["error"], "Missing Bearer token");
}

// ─── "Basic" auth scheme ───────────────────────────────────────────────────

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
    let body: serde_json::Value = response.json();
    assert_eq!(body["error"], "Missing Bearer token");
}

// ─── "Bearer   " with extra whitespace ─────────────────────────────────────

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
    let body: serde_json::Value = response.json();
    assert_eq!(body["error"], "Invalid or expired token");
}

// ─── No Authorization header at all ────────────────────────────────────────

#[tokio::test]
async fn no_authorization_header_returns_401_with_message() {
    let server = make_jwt_server();
    let response = server.post("/tasks").json(&json!({})).await;
    response.assert_status(axum_test::http::StatusCode::UNAUTHORIZED);
    let body: serde_json::Value = response.json();
    assert_eq!(body["error"], "Missing Bearer token");
}

// ─── Garbled token ─────────────────────────────────────────────────────────

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
