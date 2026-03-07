use std::sync::Arc;

use axum_test::TestServer;
use taskcast_core::{
    MemoryBroadcastProvider, MemoryShortTermStore, TaskEngine, TaskEngineOptions,
};
use taskcast_server::{create_app, AuthMode};

fn make_server() -> TestServer {
    let engine = Arc::new(TaskEngine::new(TaskEngineOptions {
        short_term_store: Arc::new(MemoryShortTermStore::new()),
        broadcast: Arc::new(MemoryBroadcastProvider::new()),
        long_term_store: None,
        hooks: None,
    }));
    let (app, _) = create_app(engine, AuthMode::None, None, None);
    TestServer::new(app)
}

#[tokio::test]
async fn health_detail_returns_ok_and_uptime() {
    let server = make_server();
    let res = server.get("/health/detail").await;
    res.assert_status_ok();
    let body: serde_json::Value = res.json();
    assert_eq!(body["ok"], true);
    assert!(body["uptime"].is_number());
    // Uptime should parse as a valid u64 (non-negative by type)
    let _uptime = body["uptime"].as_u64().expect("uptime should be a valid u64");
}

#[tokio::test]
async fn health_detail_reports_auth_mode() {
    let server = make_server();
    let res = server.get("/health/detail").await;
    let body: serde_json::Value = res.json();
    assert_eq!(body["auth"]["mode"], "none");
}

#[tokio::test]
async fn health_detail_reports_memory_adapters_by_default() {
    let server = make_server();
    let res = server.get("/health/detail").await;
    let body: serde_json::Value = res.json();
    assert_eq!(body["adapters"]["broadcast"]["provider"], "memory");
    assert_eq!(body["adapters"]["broadcast"]["status"], "ok");
    assert_eq!(body["adapters"]["shortTermStore"]["provider"], "memory");
    assert_eq!(body["adapters"]["shortTermStore"]["status"], "ok");
}
