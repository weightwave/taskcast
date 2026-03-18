use std::sync::Arc;

use axum_test::TestServer;
use taskcast_core::{
    MemoryBroadcastProvider, MemoryShortTermStore, TaskEngine, TaskEngineOptions,
};
use taskcast_server::{create_app, AuthMode, CorsConfig, JwtConfig};

fn make_server() -> TestServer {
    let engine = Arc::new(TaskEngine::new(TaskEngineOptions {
        short_term_store: Arc::new(MemoryShortTermStore::new()),
        broadcast: Arc::new(MemoryBroadcastProvider::new()),
        long_term_store: None,
        hooks: None,
    }));
    let (app, _) = create_app(engine, AuthMode::None, None, None, CorsConfig::default());
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

fn make_jwt_server() -> TestServer {
    let engine = Arc::new(TaskEngine::new(TaskEngineOptions {
        short_term_store: Arc::new(MemoryShortTermStore::new()),
        broadcast: Arc::new(MemoryBroadcastProvider::new()),
        long_term_store: None,
        hooks: None,
    }));
    let auth = AuthMode::Jwt(JwtConfig {
        algorithm: jsonwebtoken::Algorithm::HS256,
        secret: Some("test-secret-key-for-jwt-signing".to_string()),
        public_key: None,
        issuer: None,
        audience: None,
    });
    let (app, _) = create_app(engine, auth, None, None, CorsConfig::default());
    TestServer::new(app)
}

#[tokio::test]
async fn health_bypasses_jwt_auth() {
    let server = make_jwt_server();
    // No Bearer token — should still return 200
    let res = server.get("/health").await;
    res.assert_status_ok();
    let body: serde_json::Value = res.json();
    assert_eq!(body["ok"], true);
}

#[tokio::test]
async fn health_detail_bypasses_jwt_auth() {
    let server = make_jwt_server();
    // No Bearer token — should still return 200
    let res = server.get("/health/detail").await;
    res.assert_status_ok();
    let body: serde_json::Value = res.json();
    assert_eq!(body["ok"], true);
    assert!(body["uptime"].is_number());
    assert_eq!(body["auth"]["mode"], "jwt");
}

#[tokio::test]
async fn authenticated_routes_still_require_jwt() {
    let server = make_jwt_server();
    // No Bearer token — should return 401 for task routes
    let res = server.get("/tasks").await;
    res.assert_status(axum_test::http::StatusCode::UNAUTHORIZED);
}

// ─── health_detail with config adapter overrides ────────────────────────────

use taskcast_core::config::{AdapterEntry, AdaptersConfig, TaskcastConfig};

fn make_server_with_config(config: TaskcastConfig) -> TestServer {
    let engine = Arc::new(TaskEngine::new(TaskEngineOptions {
        short_term_store: Arc::new(MemoryShortTermStore::new()),
        broadcast: Arc::new(MemoryBroadcastProvider::new()),
        long_term_store: None,
        hooks: None,
    }));
    let (app, _) = create_app(engine, AuthMode::None, None, Some(config), CorsConfig::default());
    TestServer::new(app)
}

#[tokio::test]
async fn health_detail_with_config_broadcast_override() {
    let config = TaskcastConfig {
        adapters: Some(AdaptersConfig {
            broadcast: Some(AdapterEntry {
                provider: "redis".to_string(),
                url: Some("redis://localhost:6379".to_string()),
            }),
            short_term_store: None,
            long_term_store: None,
        }),
        ..Default::default()
    };
    let server = make_server_with_config(config);
    let res = server.get("/health/detail").await;
    res.assert_status_ok();
    let body: serde_json::Value = res.json();
    assert_eq!(body["adapters"]["broadcast"]["provider"], "redis");
    // shortTermStore should still default to "memory"
    assert_eq!(body["adapters"]["shortTermStore"]["provider"], "memory");
    // No longTermStore configured
    assert!(body["adapters"]["longTermStore"].is_null());
}

#[tokio::test]
async fn health_detail_with_config_short_term_store_override() {
    let config = TaskcastConfig {
        adapters: Some(AdaptersConfig {
            broadcast: None,
            short_term_store: Some(AdapterEntry {
                provider: "redis".to_string(),
                url: None,
            }),
            long_term_store: None,
        }),
        ..Default::default()
    };
    let server = make_server_with_config(config);
    let res = server.get("/health/detail").await;
    res.assert_status_ok();
    let body: serde_json::Value = res.json();
    assert_eq!(body["adapters"]["shortTermStore"]["provider"], "redis");
    // broadcast should still default to "memory"
    assert_eq!(body["adapters"]["broadcast"]["provider"], "memory");
}

#[tokio::test]
async fn health_detail_with_config_long_term_store() {
    let config = TaskcastConfig {
        adapters: Some(AdaptersConfig {
            broadcast: None,
            short_term_store: None,
            long_term_store: Some(AdapterEntry {
                provider: "postgres".to_string(),
                url: Some("postgresql://localhost/taskcast".to_string()),
            }),
        }),
        ..Default::default()
    };
    let server = make_server_with_config(config);
    let res = server.get("/health/detail").await;
    res.assert_status_ok();
    let body: serde_json::Value = res.json();
    assert_eq!(body["adapters"]["longTermStore"]["provider"], "postgres");
    assert_eq!(body["adapters"]["longTermStore"]["status"], "ok");
}

#[tokio::test]
async fn health_detail_with_all_adapters_configured() {
    let config = TaskcastConfig {
        adapters: Some(AdaptersConfig {
            broadcast: Some(AdapterEntry {
                provider: "redis".to_string(),
                url: Some("redis://localhost:6379".to_string()),
            }),
            short_term_store: Some(AdapterEntry {
                provider: "redis".to_string(),
                url: Some("redis://localhost:6379".to_string()),
            }),
            long_term_store: Some(AdapterEntry {
                provider: "postgres".to_string(),
                url: Some("postgresql://localhost/taskcast".to_string()),
            }),
        }),
        ..Default::default()
    };
    let server = make_server_with_config(config);
    let res = server.get("/health/detail").await;
    res.assert_status_ok();
    let body: serde_json::Value = res.json();
    assert_eq!(body["adapters"]["broadcast"]["provider"], "redis");
    assert_eq!(body["adapters"]["shortTermStore"]["provider"], "redis");
    assert_eq!(body["adapters"]["longTermStore"]["provider"], "postgres");
    assert_eq!(body["adapters"]["longTermStore"]["status"], "ok");
}

#[tokio::test]
async fn health_detail_with_config_but_no_adapters_section() {
    // Config exists but adapters is None -- defaults should be used
    let config = TaskcastConfig {
        adapters: None,
        ..Default::default()
    };
    let server = make_server_with_config(config);
    let res = server.get("/health/detail").await;
    res.assert_status_ok();
    let body: serde_json::Value = res.json();
    assert_eq!(body["adapters"]["broadcast"]["provider"], "memory");
    assert_eq!(body["adapters"]["shortTermStore"]["provider"], "memory");
}

#[tokio::test]
async fn health_detail_with_config_empty_adapters() {
    // Config has adapters section but all fields are None
    let config = TaskcastConfig {
        adapters: Some(AdaptersConfig {
            broadcast: None,
            short_term_store: None,
            long_term_store: None,
        }),
        ..Default::default()
    };
    let server = make_server_with_config(config);
    let res = server.get("/health/detail").await;
    res.assert_status_ok();
    let body: serde_json::Value = res.json();
    // All defaults
    assert_eq!(body["adapters"]["broadcast"]["provider"], "memory");
    assert_eq!(body["adapters"]["shortTermStore"]["provider"], "memory");
    assert!(body["adapters"]["longTermStore"].is_null());
}
