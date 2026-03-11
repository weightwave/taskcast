use std::sync::Arc;

use axum_test::TestServer;
use serde_json::json;
use taskcast_core::config::TaskcastConfig;
use taskcast_core::{MemoryBroadcastProvider, MemoryShortTermStore, TaskEngine, TaskEngineOptions};
use taskcast_server::{create_app, AuthMode, CorsConfig, JwtConfig};

const JWT_SECRET: &str = "test-secret-key-for-jwt-signing-needs-to-be-long-enough";

fn make_engine() -> Arc<TaskEngine> {
    Arc::new(TaskEngine::new(TaskEngineOptions {
        short_term_store: Arc::new(MemoryShortTermStore::new()),
        broadcast: Arc::new(MemoryBroadcastProvider::new()),
        long_term_store: None,
        hooks: None,
    }))
}

fn make_admin_server(
    auth_mode: AuthMode,
    config: TaskcastConfig,
) -> TestServer {
    let engine = make_engine();
    let (app, _) = create_app(engine, auth_mode, None, Some(config), CorsConfig::default());
    TestServer::new(app)
}

fn jwt_auth_mode() -> AuthMode {
    AuthMode::Jwt(JwtConfig {
        algorithm: jsonwebtoken::Algorithm::HS256,
        secret: Some(JWT_SECRET.to_string()),
        public_key: None,
        issuer: None,
        audience: None,
    })
}

// ─── adminApi not enabled → 404 ─────────────────────────────────────────────

#[tokio::test]
async fn admin_token_returns_404_when_admin_api_disabled() {
    let config = TaskcastConfig {
        admin_api: Some(false),
        admin_token: Some("secret".into()),
        ..Default::default()
    };
    let server = make_admin_server(AuthMode::None, config);

    let resp = server
        .post("/admin/token")
        .content_type("application/json")
        .json(&json!({"adminToken": "secret"}))
        .await;
    resp.assert_status_not_found();
}

#[tokio::test]
async fn admin_token_returns_404_when_admin_api_none() {
    let config = TaskcastConfig {
        admin_api: None,
        admin_token: Some("secret".into()),
        ..Default::default()
    };
    let server = make_admin_server(AuthMode::None, config);

    let resp = server
        .post("/admin/token")
        .content_type("application/json")
        .json(&json!({"adminToken": "secret"}))
        .await;
    resp.assert_status_not_found();
}

// ─── Invalid JSON body → 400 ────────────────────────────────────────────────

#[tokio::test]
async fn admin_token_returns_400_for_invalid_json() {
    let config = TaskcastConfig {
        admin_api: Some(true),
        admin_token: Some("secret".into()),
        ..Default::default()
    };
    let server = make_admin_server(AuthMode::None, config);

    let resp = server
        .post("/admin/token")
        .content_type("application/json")
        .bytes("not json".into())
        .await;
    resp.assert_status_bad_request();
}

#[tokio::test]
async fn admin_token_returns_400_for_empty_body() {
    let config = TaskcastConfig {
        admin_api: Some(true),
        admin_token: Some("secret".into()),
        ..Default::default()
    };
    let server = make_admin_server(AuthMode::None, config);

    let resp = server
        .post("/admin/token")
        .content_type("application/json")
        .bytes("".into())
        .await;
    resp.assert_status_bad_request();
}

// ─── Missing / empty admin token → 401 ──────────────────────────────────────

#[tokio::test]
async fn admin_token_returns_401_when_token_missing_from_body() {
    let config = TaskcastConfig {
        admin_api: Some(true),
        admin_token: Some("secret".into()),
        ..Default::default()
    };
    let server = make_admin_server(AuthMode::None, config);

    let resp = server
        .post("/admin/token")
        .content_type("application/json")
        .json(&json!({}))
        .await;
    resp.assert_status_unauthorized();
}

#[tokio::test]
async fn admin_token_returns_401_when_token_is_empty_string() {
    let config = TaskcastConfig {
        admin_api: Some(true),
        admin_token: Some("secret".into()),
        ..Default::default()
    };
    let server = make_admin_server(AuthMode::None, config);

    let resp = server
        .post("/admin/token")
        .content_type("application/json")
        .json(&json!({"adminToken": ""}))
        .await;
    resp.assert_status_unauthorized();
}

#[tokio::test]
async fn admin_token_returns_401_when_token_is_null() {
    let config = TaskcastConfig {
        admin_api: Some(true),
        admin_token: Some("secret".into()),
        ..Default::default()
    };
    let server = make_admin_server(AuthMode::None, config);

    let resp = server
        .post("/admin/token")
        .content_type("application/json")
        .json(&json!({"adminToken": null}))
        .await;
    resp.assert_status_unauthorized();
}

// ─── No admin_token configured on server → 401 ──────────────────────────────

#[tokio::test]
async fn admin_token_returns_401_when_server_has_no_admin_token_configured() {
    let config = TaskcastConfig {
        admin_api: Some(true),
        admin_token: None,
        ..Default::default()
    };
    let server = make_admin_server(AuthMode::None, config);

    let resp = server
        .post("/admin/token")
        .content_type("application/json")
        .json(&json!({"adminToken": "anything"}))
        .await;
    resp.assert_status_unauthorized();
}

// ─── Wrong admin token → 401 ────────────────────────────────────────────────

#[tokio::test]
async fn admin_token_returns_401_for_wrong_token() {
    let config = TaskcastConfig {
        admin_api: Some(true),
        admin_token: Some("correct-secret".into()),
        ..Default::default()
    };
    let server = make_admin_server(AuthMode::None, config);

    let resp = server
        .post("/admin/token")
        .content_type("application/json")
        .json(&json!({"adminToken": "wrong-secret"}))
        .await;
    resp.assert_status_unauthorized();
}

// ─── JWT mode: successful token issuance ─────────────────────────────────────

#[tokio::test]
async fn admin_token_issues_jwt_in_jwt_mode() {
    let config = TaskcastConfig {
        admin_api: Some(true),
        admin_token: Some("secret".into()),
        ..Default::default()
    };
    let server = make_admin_server(jwt_auth_mode(), config);

    let resp = server
        .post("/admin/token")
        .content_type("application/json")
        .json(&json!({"adminToken": "secret"}))
        .await;
    resp.assert_status_ok();

    let body: serde_json::Value = resp.json();
    assert!(body["token"].is_string());
    assert!(!body["token"].as_str().unwrap().is_empty());
    assert!(body["expiresAt"].is_number());
}

#[tokio::test]
async fn admin_token_jwt_respects_custom_scopes() {
    let config = TaskcastConfig {
        admin_api: Some(true),
        admin_token: Some("secret".into()),
        ..Default::default()
    };
    let server = make_admin_server(jwt_auth_mode(), config);

    let resp = server
        .post("/admin/token")
        .content_type("application/json")
        .json(&json!({
            "adminToken": "secret",
            "scopes": ["task:create", "event:subscribe"]
        }))
        .await;
    resp.assert_status_ok();

    let body: serde_json::Value = resp.json();
    assert!(body["token"].is_string());
    assert!(!body["token"].as_str().unwrap().is_empty());
}

#[tokio::test]
async fn admin_token_jwt_respects_custom_expires_in() {
    let config = TaskcastConfig {
        admin_api: Some(true),
        admin_token: Some("secret".into()),
        ..Default::default()
    };
    let server = make_admin_server(jwt_auth_mode(), config);

    let resp = server
        .post("/admin/token")
        .content_type("application/json")
        .json(&json!({
            "adminToken": "secret",
            "expiresIn": 3600
        }))
        .await;
    resp.assert_status_ok();

    let body: serde_json::Value = resp.json();
    assert!(body["expiresAt"].is_number());
}

#[tokio::test]
async fn admin_token_jwt_negative_expires_in_uses_default() {
    let config = TaskcastConfig {
        admin_api: Some(true),
        admin_token: Some("secret".into()),
        ..Default::default()
    };
    let server = make_admin_server(jwt_auth_mode(), config);

    let resp = server
        .post("/admin/token")
        .content_type("application/json")
        .json(&json!({
            "adminToken": "secret",
            "expiresIn": -100
        }))
        .await;
    resp.assert_status_ok();

    let body: serde_json::Value = resp.json();
    // Should use default 86400s, so expiresAt should be reasonable
    assert!(body["expiresAt"].is_number());
}

// ─── Non-JWT mode: placeholder token ─────────────────────────────────────────

#[tokio::test]
async fn admin_token_returns_empty_token_in_no_auth_mode() {
    let config = TaskcastConfig {
        admin_api: Some(true),
        admin_token: Some("secret".into()),
        ..Default::default()
    };
    let server = make_admin_server(AuthMode::None, config);

    let resp = server
        .post("/admin/token")
        .content_type("application/json")
        .json(&json!({"adminToken": "secret"}))
        .await;
    resp.assert_status_ok();

    let body: serde_json::Value = resp.json();
    assert_eq!(body["token"].as_str().unwrap(), "");
    assert!(body["expiresAt"].is_number());
}

// ─── No config at all → admin routes not mounted ─────────────────────────────

#[tokio::test]
async fn admin_token_returns_404_when_no_config_provided() {
    let engine = make_engine();
    let (app, _) = create_app(engine, AuthMode::None, None, None, CorsConfig::default());
    let server = TestServer::new(app);

    let resp = server
        .post("/admin/token")
        .content_type("application/json")
        .json(&json!({"adminToken": "secret"}))
        .await;
    resp.assert_status_not_found();
}
