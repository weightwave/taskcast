use std::sync::Arc;

use axum_test::TestServer;
use taskcast_core::{MemoryBroadcastProvider, MemoryShortTermStore, TaskEngine, TaskEngineOptions};
use taskcast_server::{create_app, AuthMode, CorsConfig};

fn make_engine() -> Arc<TaskEngine> {
    Arc::new(TaskEngine::new(TaskEngineOptions {
        short_term_store: Arc::new(MemoryShortTermStore::new()),
        broadcast: Arc::new(MemoryBroadcastProvider::new()),
        long_term_store: None,
        hooks: None,
    }))
}

fn make_server_with_cors(cors_config: CorsConfig) -> TestServer {
    let engine = make_engine();
    let (app, _) = create_app(engine, AuthMode::None, None, None, cors_config);
    TestServer::new(app)
}

// ─── CORS: AllowAll ─────────────────────────────────────────────────────────

#[tokio::test]
async fn cors_allow_all_returns_wildcard_origin() {
    let server = make_server_with_cors(CorsConfig::AllowAll);

    let resp = server
        .get("/health")
        .add_header(
            axum_test::http::header::ORIGIN,
            axum_test::http::HeaderValue::from_static("http://example.com"),
        )
        .await;

    resp.assert_status_ok();
    let acao = resp
        .headers()
        .get("access-control-allow-origin")
        .expect("expected access-control-allow-origin header");
    assert_eq!(acao, "*");
}

// ─── CORS: AllowOrigins ─────────────────────────────────────────────────────

#[tokio::test]
async fn cors_allow_origins_returns_matching_origin() {
    let server = make_server_with_cors(CorsConfig::AllowOrigins(vec![
        "http://example.com".to_string(),
    ]));

    let resp = server
        .get("/health")
        .add_header(
            axum_test::http::header::ORIGIN,
            axum_test::http::HeaderValue::from_static("http://example.com"),
        )
        .await;

    resp.assert_status_ok();
    let acao = resp
        .headers()
        .get("access-control-allow-origin")
        .expect("expected access-control-allow-origin header");
    assert_eq!(acao, "http://example.com");
}

// ─── CORS: Disabled ─────────────────────────────────────────────────────────

#[tokio::test]
async fn cors_disabled_does_not_return_cors_headers() {
    let server = make_server_with_cors(CorsConfig::Disabled);

    let resp = server
        .get("/health")
        .add_header(
            axum_test::http::header::ORIGIN,
            axum_test::http::HeaderValue::from_static("http://example.com"),
        )
        .await;

    resp.assert_status_ok();
    assert!(
        resp.headers().get("access-control-allow-origin").is_none(),
        "expected no access-control-allow-origin header when CORS is disabled"
    );
}
