use std::net::SocketAddr;

use axum::{routing::get, Json, Router};
use serde_json::json;
use tokio::net::TcpListener;

use taskcast_cli::commands::doctor::run_doctor;
use taskcast_cli::node_config::{NodeEntry, TokenType};

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

fn make_node(url: &str, token: Option<&str>, token_type: Option<TokenType>) -> NodeEntry {
    NodeEntry {
        url: url.to_string(),
        token: token.map(|s| s.to_string()),
        token_type,
    }
}

// ─── run_doctor: healthy server ──────────────────────────────────────────────

#[tokio::test]
async fn doctor_healthy_server() {
    let app = Router::new().route(
        "/health/detail",
        get(|| async {
            Json(json!({
                "ok": true,
                "uptime": 3600,
                "auth": { "mode": "none" },
                "adapters": {
                    "broadcast": { "provider": "memory", "status": "ok" },
                    "shortTermStore": { "provider": "memory", "status": "ok" }
                }
            }))
        }),
    );
    let base_url = start_mock_server(app).await;
    let node = make_node(&base_url, None, None);

    let result = run_doctor(&node).await;

    assert!(result.server.ok);
    assert_eq!(result.server.url, base_url);
    assert_eq!(result.server.uptime, Some(3600));
    assert!(result.server.error.is_none());

    assert_eq!(result.auth.status, "ok");
    assert_eq!(result.auth.mode, Some("none".to_string()));
    assert!(result.auth.message.is_none());

    assert_eq!(result.adapters.len(), 2);
    assert_eq!(result.adapters[0].name, "broadcast");
    assert_eq!(result.adapters[0].provider, "memory");
    assert_eq!(result.adapters[0].status, "ok");
    assert_eq!(result.adapters[1].name, "shortTermStore");
    assert_eq!(result.adapters[1].provider, "memory");
    assert_eq!(result.adapters[1].status, "ok");
}

// ─── run_doctor: healthy server with all three adapters ──────────────────────

#[tokio::test]
async fn doctor_healthy_server_with_long_term_store() {
    let app = Router::new().route(
        "/health/detail",
        get(|| async {
            Json(json!({
                "ok": true,
                "uptime": 7200,
                "auth": { "mode": "jwt" },
                "adapters": {
                    "broadcast": { "provider": "redis", "status": "ok" },
                    "shortTermStore": { "provider": "redis", "status": "ok" },
                    "longTermStore": { "provider": "postgres", "status": "ok" }
                }
            }))
        }),
    );
    let base_url = start_mock_server(app).await;
    let node = make_node(&base_url, Some("jwt-token"), Some(TokenType::Jwt));

    let result = run_doctor(&node).await;

    assert!(result.server.ok);
    assert_eq!(result.server.uptime, Some(7200));
    assert_eq!(result.auth.status, "ok");
    assert_eq!(result.auth.mode, Some("jwt".to_string()));

    assert_eq!(result.adapters.len(), 3);
    assert_eq!(result.adapters[0].name, "broadcast");
    assert_eq!(result.adapters[0].provider, "redis");
    assert_eq!(result.adapters[1].name, "shortTermStore");
    assert_eq!(result.adapters[1].provider, "redis");
    assert_eq!(result.adapters[2].name, "longTermStore");
    assert_eq!(result.adapters[2].provider, "postgres");
}

// ─── run_doctor: connection refused ──────────────────────────────────────────

#[tokio::test]
async fn doctor_connection_refused() {
    let node = make_node("http://127.0.0.1:19996", None, None);

    let result = run_doctor(&node).await;

    assert!(!result.server.ok);
    assert!(result.server.uptime.is_none());
    assert!(result.server.error.is_some());
    let err = result.server.error.unwrap();
    assert!(
        err.contains("error") || err.contains("connect") || err.contains("Connection"),
        "expected connection error, got: {err}"
    );

    // Auth and adapters should be in degraded state
    assert_eq!(result.auth.status, "warn");
    assert!(result.adapters.is_empty());
}

// ─── run_doctor: non-200 response ────────────────────────────────────────────

#[tokio::test]
async fn doctor_non_200_response() {
    let app = Router::new().route(
        "/health/detail",
        get(|| async { (axum::http::StatusCode::SERVICE_UNAVAILABLE, "down") }),
    );
    let base_url = start_mock_server(app).await;
    let node = make_node(&base_url, None, None);

    let result = run_doctor(&node).await;

    assert!(!result.server.ok);
    assert_eq!(result.server.url, base_url);
    assert!(result.server.uptime.is_none());
    let err = result.server.error.unwrap();
    assert!(
        err.contains("503"),
        "expected HTTP 503 in error, got: {err}"
    );

    assert_eq!(result.auth.status, "warn");
    assert!(result.adapters.is_empty());
}

// ─── run_doctor: 500 response ────────────────────────────────────────────────

#[tokio::test]
async fn doctor_500_response() {
    let app = Router::new().route(
        "/health/detail",
        get(|| async {
            (
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                "internal error",
            )
        }),
    );
    let base_url = start_mock_server(app).await;
    let node = make_node(&base_url, None, None);

    let result = run_doctor(&node).await;

    assert!(!result.server.ok);
    let err = result.server.error.unwrap();
    assert!(
        err.contains("500"),
        "expected HTTP 500 in error, got: {err}"
    );
}

// ─── run_doctor: invalid JSON response ───────────────────────────────────────

#[tokio::test]
async fn doctor_invalid_json_response() {
    let app = Router::new().route(
        "/health/detail",
        get(|| async { (axum::http::StatusCode::OK, "this is not json") }),
    );
    let base_url = start_mock_server(app).await;
    let node = make_node(&base_url, None, None);

    let result = run_doctor(&node).await;

    assert!(!result.server.ok);
    let err = result.server.error.unwrap();
    assert!(
        err.contains("failed to parse response"),
        "expected parse error, got: {err}"
    );
}

// ─── run_doctor: auth warning when no token but server requires auth ─────────

#[tokio::test]
async fn doctor_auth_warn_when_no_token_and_jwt_mode() {
    let app = Router::new().route(
        "/health/detail",
        get(|| async {
            Json(json!({
                "ok": true,
                "uptime": 100,
                "auth": { "mode": "jwt" },
                "adapters": {
                    "broadcast": { "provider": "memory", "status": "ok" },
                    "shortTermStore": { "provider": "memory", "status": "ok" }
                }
            }))
        }),
    );
    let base_url = start_mock_server(app).await;
    // Node has no token, but server uses JWT auth
    let node = make_node(&base_url, None, None);

    let result = run_doctor(&node).await;

    assert!(result.server.ok);
    assert_eq!(result.auth.status, "warn");
    assert_eq!(result.auth.mode, Some("jwt".to_string()));
    assert_eq!(
        result.auth.message,
        Some("no token configured for this node".to_string())
    );
}

// ─── run_doctor: auth OK when token present and server requires auth ─────────

#[tokio::test]
async fn doctor_auth_ok_when_token_configured() {
    let app = Router::new().route(
        "/health/detail",
        get(|| async {
            Json(json!({
                "ok": true,
                "uptime": 100,
                "auth": { "mode": "jwt" },
                "adapters": {
                    "broadcast": { "provider": "memory", "status": "ok" },
                    "shortTermStore": { "provider": "memory", "status": "ok" }
                }
            }))
        }),
    );
    let base_url = start_mock_server(app).await;
    let node = make_node(&base_url, Some("my-jwt-token"), Some(TokenType::Jwt));

    let result = run_doctor(&node).await;

    assert!(result.server.ok);
    assert_eq!(result.auth.status, "ok");
    assert_eq!(result.auth.mode, Some("jwt".to_string()));
    assert!(result.auth.message.is_none());
}

// ─── run_doctor: auth OK when mode is "none" even without token ──────────────

#[tokio::test]
async fn doctor_auth_ok_when_mode_none_no_token() {
    let app = Router::new().route(
        "/health/detail",
        get(|| async {
            Json(json!({
                "ok": true,
                "uptime": 50,
                "auth": { "mode": "none" },
                "adapters": {
                    "broadcast": { "provider": "memory", "status": "ok" },
                    "shortTermStore": { "provider": "memory", "status": "ok" }
                }
            }))
        }),
    );
    let base_url = start_mock_server(app).await;
    let node = make_node(&base_url, None, None);

    let result = run_doctor(&node).await;

    assert!(result.server.ok);
    assert_eq!(result.auth.status, "ok");
    assert!(result.auth.message.is_none());
}

// ─── run_doctor: trailing slash is stripped from URL ──────────────────────────

#[tokio::test]
async fn doctor_strips_trailing_slash_from_url() {
    let app = Router::new().route(
        "/health/detail",
        get(|| async {
            Json(json!({
                "ok": true,
                "uptime": 10,
                "auth": { "mode": "none" },
                "adapters": {
                    "broadcast": { "provider": "memory", "status": "ok" },
                    "shortTermStore": { "provider": "memory", "status": "ok" }
                }
            }))
        }),
    );
    let base_url = start_mock_server(app).await;
    let url_with_slash = format!("{base_url}/");
    let node = make_node(&url_with_slash, None, None);

    let result = run_doctor(&node).await;

    assert!(result.server.ok);
    assert_eq!(result.server.url, base_url);
}

// ─── run_doctor: adapters in canonical order ─────────────────────────────────

#[tokio::test]
async fn doctor_adapters_returned_in_canonical_order() {
    // Return adapters in a different order than canonical
    let app = Router::new().route(
        "/health/detail",
        get(|| async {
            Json(json!({
                "ok": true,
                "uptime": 10,
                "auth": { "mode": "none" },
                "adapters": {
                    "longTermStore": { "provider": "postgres", "status": "ok" },
                    "shortTermStore": { "provider": "redis", "status": "ok" },
                    "broadcast": { "provider": "redis", "status": "ok" }
                }
            }))
        }),
    );
    let base_url = start_mock_server(app).await;
    let node = make_node(&base_url, None, None);

    let result = run_doctor(&node).await;

    // Should be in canonical order: broadcast, shortTermStore, longTermStore
    assert_eq!(result.adapters.len(), 3);
    assert_eq!(result.adapters[0].name, "broadcast");
    assert_eq!(result.adapters[1].name, "shortTermStore");
    assert_eq!(result.adapters[2].name, "longTermStore");
}

// ─── run_doctor: adapter with failed status ──────────────────────────────────

#[tokio::test]
async fn doctor_adapter_with_failed_status() {
    let app = Router::new().route(
        "/health/detail",
        get(|| async {
            Json(json!({
                "ok": true,
                "uptime": 10,
                "auth": { "mode": "none" },
                "adapters": {
                    "broadcast": { "provider": "redis", "status": "ok" },
                    "shortTermStore": { "provider": "redis", "status": "fail" },
                    "longTermStore": { "provider": "postgres", "status": "fail" }
                }
            }))
        }),
    );
    let base_url = start_mock_server(app).await;
    let node = make_node(&base_url, None, None);

    let result = run_doctor(&node).await;

    assert!(result.server.ok);
    assert_eq!(result.adapters[0].status, "ok");
    assert_eq!(result.adapters[1].status, "fail");
    assert_eq!(result.adapters[2].status, "fail");
}
