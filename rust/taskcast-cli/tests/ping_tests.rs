use std::net::SocketAddr;

use axum::{routing::get, Json, Router};
use serde_json::json;
use tokio::net::TcpListener;

use taskcast_cli::commands::ping::ping_server;

// ─── Helpers ─────────────────────────────────────────────────────────────────

async fn start_mock_server(app: Router) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr: SocketAddr = listener.local_addr().unwrap();
    let base_url = format!("http://127.0.0.1:{}", addr.port());

    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    base_url
}

// ─── ping_server: success ────────────────────────────────────────────────────

#[tokio::test]
async fn ping_success_returns_ok_with_latency() {
    let app = Router::new().route(
        "/health",
        get(|| async { Json(json!({ "ok": true })) }),
    );
    let base_url = start_mock_server(app).await;

    let result = ping_server(&base_url).await;
    assert!(result.ok, "ping should succeed");
    assert!(result.latency_ms.is_some(), "latency should be measured");
    assert!(result.error.is_none(), "no error expected");
    // Latency should be a small number (< 5 seconds for a local server)
    assert!(result.latency_ms.unwrap() < 5000);
}

// ─── ping_server: non-200 response ──────────────────────────────────────────

#[tokio::test]
async fn ping_non_200_returns_not_ok() {
    let app = Router::new().route(
        "/health",
        get(|| async { (axum::http::StatusCode::SERVICE_UNAVAILABLE, "down") }),
    );
    let base_url = start_mock_server(app).await;

    let result = ping_server(&base_url).await;
    assert!(!result.ok, "ping should fail for non-200");
    assert!(result.latency_ms.is_none());
    let error = result.error.unwrap();
    assert!(
        error.contains("503"),
        "error should contain status code, got: {error}"
    );
}

// ─── ping_server: connection refused ─────────────────────────────────────────

#[tokio::test]
async fn ping_connection_refused() {
    // Use a port that is almost certainly not listening
    let result = ping_server("http://127.0.0.1:19997").await;
    assert!(!result.ok);
    assert!(result.latency_ms.is_none());
    assert!(result.error.is_some());
    let err = result.error.unwrap();
    assert!(
        err.contains("error") || err.contains("connect") || err.contains("Connection"),
        "expected connection error, got: {err}"
    );
}

// ─── ping_server: invalid URL ────────────────────────────────────────────────

#[tokio::test]
async fn ping_invalid_url_returns_error() {
    let result = ping_server("not-a-valid-url").await;
    assert!(!result.ok);
    assert!(result.latency_ms.is_none());
    assert!(result.error.is_some());
}

// ─── ping_server: 500 status ─────────────────────────────────────────────────

#[tokio::test]
async fn ping_500_returns_not_ok() {
    let app = Router::new().route(
        "/health",
        get(|| async { (axum::http::StatusCode::INTERNAL_SERVER_ERROR, "error") }),
    );
    let base_url = start_mock_server(app).await;

    let result = ping_server(&base_url).await;
    assert!(!result.ok);
    assert!(result.latency_ms.is_none());
    let error = result.error.unwrap();
    assert!(
        error.contains("500"),
        "error should contain 500, got: {error}"
    );
}

// ─── ping_server: 404 is still a failure ─────────────────────────────────────

#[tokio::test]
async fn ping_404_returns_not_ok() {
    // Server exists but /health is not routed
    let app = Router::new().route("/other", get(|| async { "hello" }));
    let base_url = start_mock_server(app).await;

    let result = ping_server(&base_url).await;
    assert!(!result.ok);
    assert!(result.latency_ms.is_none());
    let error = result.error.unwrap();
    assert!(
        error.contains("404"),
        "error should contain 404, got: {error}"
    );
}
