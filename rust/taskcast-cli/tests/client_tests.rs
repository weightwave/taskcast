use std::net::SocketAddr;

use axum::{
    extract::Request,
    routing::{get, patch, post},
    Json, Router,
};
use serde_json::json;
use tokio::net::TcpListener;

use taskcast_cli::client::TaskcastClient;
use taskcast_cli::node_config::{NodeEntry, TokenType};

// ─── Helpers ─────────────────────────────────────────────────────────────────

/// Start an axum server on a random port and return its base URL.
async fn start_mock_server(app: Router) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr: SocketAddr = listener.local_addr().unwrap();
    let base_url = format!("http://127.0.0.1:{}", addr.port());

    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    base_url
}

/// Build a mock server that serves `/admin/token` returning a JWT,
/// and echo endpoints for GET/POST/PATCH that return the Authorization header.
fn mock_app() -> Router {
    Router::new()
        .route(
            "/admin/token",
            post(|Json(body): Json<serde_json::Value>| async move {
                let admin_token = body["adminToken"].as_str().unwrap_or("");
                if admin_token == "valid-admin-token" {
                    Json(json!({ "token": "exchanged-jwt-token", "expiresAt": 9999999999u64 }))
                } else {
                    // Return an error for invalid admin tokens
                    // (In real code the status code would be non-200, but we handle that separately)
                    Json(json!({ "token": "exchanged-jwt-token", "expiresAt": 9999999999u64 }))
                }
            }),
        )
        .route(
            "/echo",
            get(|req: Request| async move {
                let auth = req
                    .headers()
                    .get("Authorization")
                    .map(|v| v.to_str().unwrap().to_string());
                Json(json!({ "method": "GET", "auth": auth }))
            }),
        )
        .route(
            "/echo",
            post(|req: Request| async move {
                let auth = req
                    .headers()
                    .get("Authorization")
                    .map(|v| v.to_str().unwrap().to_string());
                Json(json!({ "method": "POST", "auth": auth }))
            }),
        )
        .route(
            "/echo",
            patch(|req: Request| async move {
                let auth = req
                    .headers()
                    .get("Authorization")
                    .map(|v| v.to_str().unwrap().to_string());
                Json(json!({ "method": "PATCH", "auth": auth }))
            }),
        )
}

/// Build a mock server whose `/admin/token` returns a non-200 status with an error body.
fn mock_app_admin_token_error() -> Router {
    Router::new().route(
        "/admin/token",
        post(|| async {
            (
                axum::http::StatusCode::FORBIDDEN,
                Json(json!({ "error": "invalid admin token" })),
            )
        }),
    )
}

/// Build a mock server whose `/admin/token` returns a non-200 status with a non-JSON body.
fn mock_app_admin_token_error_no_json() -> Router {
    Router::new().route(
        "/admin/token",
        post(|| async { (axum::http::StatusCode::INTERNAL_SERVER_ERROR, "oops") }),
    )
}

// ─── from_node: admin token exchange ─────────────────────────────────────────

#[tokio::test]
async fn from_node_admin_token_exchanges_for_jwt() {
    let base_url = start_mock_server(mock_app()).await;

    let node = NodeEntry {
        url: base_url.clone(),
        token: Some("valid-admin-token".to_string()),
        token_type: Some(TokenType::Admin),
    };

    let client = TaskcastClient::from_node(&node).await.unwrap();
    assert_eq!(client.base_url(), base_url);
    // The token should be the exchanged JWT, not the original admin token
    assert_eq!(client.token(), Some("exchanged-jwt-token"));
}

#[tokio::test]
async fn from_node_admin_token_error_returns_error_message() {
    let base_url = start_mock_server(mock_app_admin_token_error()).await;

    let node = NodeEntry {
        url: base_url,
        token: Some("bad-admin-token".to_string()),
        token_type: Some(TokenType::Admin),
    };

    let result = TaskcastClient::from_node(&node).await;
    assert!(result.is_err(), "expected error for invalid admin token");
    let msg = result.err().unwrap().to_string();
    assert!(
        msg.contains("invalid admin token"),
        "expected error message about invalid admin token, got: {msg}"
    );
}

#[tokio::test]
async fn from_node_admin_token_error_without_json_body() {
    let base_url = start_mock_server(mock_app_admin_token_error_no_json()).await;

    let node = NodeEntry {
        url: base_url,
        token: Some("bad-admin-token".to_string()),
        token_type: Some(TokenType::Admin),
    };

    let result = TaskcastClient::from_node(&node).await;
    assert!(result.is_err(), "expected error for non-JSON error body");
    let msg = result.err().unwrap().to_string();
    // Should fall back to "HTTP 500" since there's no JSON error body
    assert!(
        msg.contains("HTTP 500"),
        "expected HTTP status fallback, got: {msg}"
    );
}

// ─── from_node: JWT token (no exchange) ──────────────────────────────────────

#[tokio::test]
async fn from_node_jwt_uses_token_directly() {
    // No mock server needed — JWT tokens don't trigger an HTTP call
    let node = NodeEntry {
        url: "http://127.0.0.1:1".to_string(), // doesn't matter, no HTTP call
        token: Some("my-jwt-token".to_string()),
        token_type: Some(TokenType::Jwt),
    };

    let client = TaskcastClient::from_node(&node).await.unwrap();
    assert_eq!(client.token(), Some("my-jwt-token"));
}

#[tokio::test]
async fn from_node_no_token() {
    let node = NodeEntry {
        url: "http://127.0.0.1:1".to_string(),
        token: None,
        token_type: None,
    };

    let client = TaskcastClient::from_node(&node).await.unwrap();
    assert_eq!(client.token(), None);
}

// ─── from_node: admin type but no token value ────────────────────────────────

#[tokio::test]
async fn from_node_admin_type_but_no_token_skips_exchange() {
    // token_type is Admin but token is None — should skip the exchange
    let node = NodeEntry {
        url: "http://127.0.0.1:1".to_string(),
        token: None,
        token_type: Some(TokenType::Admin),
    };

    let client = TaskcastClient::from_node(&node).await.unwrap();
    assert_eq!(client.token(), None);
}

// ─── from_node: admin token with unreachable server ──────────────────────────

#[tokio::test]
async fn from_node_admin_token_connection_refused() {
    let node = NodeEntry {
        url: "http://127.0.0.1:19998".to_string(),
        token: Some("admin-token".to_string()),
        token_type: Some(TokenType::Admin),
    };

    let result = TaskcastClient::from_node(&node).await;
    assert!(result.is_err(), "expected connection error");
    let msg = result.err().unwrap().to_string();
    assert!(
        msg.contains("error") || msg.contains("connect") || msg.contains("Connection"),
        "expected connection error, got: {msg}"
    );
}

// ─── get() / post() / patch(): auth header ───────────────────────────────────

#[tokio::test]
async fn get_sends_bearer_token() {
    let base_url = start_mock_server(mock_app()).await;
    let client = TaskcastClient::new(base_url, Some("test-token".to_string()));

    let res = client.get("/echo").await.unwrap();
    let body: serde_json::Value = res.json().await.unwrap();
    assert_eq!(body["method"], "GET");
    assert_eq!(body["auth"], "Bearer test-token");
}

#[tokio::test]
async fn get_without_token_sends_no_auth_header() {
    let base_url = start_mock_server(mock_app()).await;
    let client = TaskcastClient::new(base_url, None);

    let res = client.get("/echo").await.unwrap();
    let body: serde_json::Value = res.json().await.unwrap();
    assert_eq!(body["method"], "GET");
    assert!(body["auth"].is_null(), "expected no auth header");
}

#[tokio::test]
async fn post_sends_bearer_token() {
    let base_url = start_mock_server(mock_app()).await;
    let client = TaskcastClient::new(base_url, Some("post-token".to_string()));

    let payload = json!({ "key": "value" });
    let res = client.post("/echo", &payload).await.unwrap();
    let body: serde_json::Value = res.json().await.unwrap();
    assert_eq!(body["method"], "POST");
    assert_eq!(body["auth"], "Bearer post-token");
}

#[tokio::test]
async fn post_without_token_sends_no_auth_header() {
    let base_url = start_mock_server(mock_app()).await;
    let client = TaskcastClient::new(base_url, None);

    let payload = json!({ "key": "value" });
    let res = client.post("/echo", &payload).await.unwrap();
    let body: serde_json::Value = res.json().await.unwrap();
    assert_eq!(body["method"], "POST");
    assert!(body["auth"].is_null());
}

#[tokio::test]
async fn patch_sends_bearer_token() {
    let base_url = start_mock_server(mock_app()).await;
    let client = TaskcastClient::new(base_url, Some("patch-token".to_string()));

    let payload = json!({ "update": true });
    let res = client.patch("/echo", &payload).await.unwrap();
    let body: serde_json::Value = res.json().await.unwrap();
    assert_eq!(body["method"], "PATCH");
    assert_eq!(body["auth"], "Bearer patch-token");
}

#[tokio::test]
async fn patch_without_token_sends_no_auth_header() {
    let base_url = start_mock_server(mock_app()).await;
    let client = TaskcastClient::new(base_url, None);

    let payload = json!({ "update": true });
    let res = client.patch("/echo", &payload).await.unwrap();
    let body: serde_json::Value = res.json().await.unwrap();
    assert_eq!(body["method"], "PATCH");
    assert!(body["auth"].is_null());
}

// ─── new() and base_url() edge cases ─────────────────────────────────────────

#[test]
fn new_strips_trailing_slash() {
    let client = TaskcastClient::new("http://localhost:3721/".to_string(), None);
    assert_eq!(client.base_url(), "http://localhost:3721");
}

#[test]
fn new_preserves_url_without_trailing_slash() {
    let client = TaskcastClient::new("http://localhost:3721".to_string(), None);
    assert_eq!(client.base_url(), "http://localhost:3721");
}

#[tokio::test]
async fn from_node_strips_trailing_slash() {
    let base_url = start_mock_server(mock_app()).await;
    let url_with_slash = format!("{base_url}/");

    let node = NodeEntry {
        url: url_with_slash,
        token: Some("valid-admin-token".to_string()),
        token_type: Some(TokenType::Admin),
    };

    let client = TaskcastClient::from_node(&node).await.unwrap();
    assert_eq!(client.base_url(), base_url);
}
