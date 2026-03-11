//! Tests targeting uncovered lines in `worker_ws.rs`.
//!
//! Covers:
//! - Line 167: WS permission check (JWT scope check returns 403)
//! - Lines 196-197: Ping interval tick
//! - Lines 204-205: Available command (ws-race dispatch)
//! - Lines 247-259: Register with worker_id mismatch against JWT
//! - Line 498: Disconnect unregister

use std::sync::Arc;

use axum_test::http::HeaderValue;
use axum_test::TestServer;
use jsonwebtoken::{encode, EncodingKey, Header};
use serde_json::json;
use taskcast_core::worker_manager::{WorkerManager, WorkerManagerDefaults, WorkerManagerOptions};
use taskcast_core::{
    AssignMode, BroadcastProvider, MemoryBroadcastProvider, MemoryShortTermStore, ShortTermStore,
    TaskEngine, TaskEngineOptions,
};
use taskcast_server::{create_app, AuthMode, CorsConfig, JwtConfig};

// ─── Constants ───────────────────────────────────────────────────────────────

const JWT_SECRET: &str = "test-secret-key-for-jwt-signing-needs-to-be-long-enough";

// ─── Helpers ─────────────────────────────────────────────────────────────────

fn make_ws_server() -> (Arc<TaskEngine>, Arc<WorkerManager>, TestServer) {
    let store = Arc::new(MemoryShortTermStore::new());
    let broadcast = Arc::new(MemoryBroadcastProvider::new());
    let engine = Arc::new(TaskEngine::new(TaskEngineOptions {
        short_term_store: Arc::clone(&store) as Arc<dyn ShortTermStore>,
        broadcast: Arc::clone(&broadcast) as Arc<dyn BroadcastProvider>,
        long_term_store: None,
        hooks: None,
    }));
    let manager = Arc::new(WorkerManager::new(WorkerManagerOptions {
        engine: Arc::clone(&engine),
        short_term_store: Arc::clone(&store) as Arc<dyn ShortTermStore>,
        broadcast: Arc::clone(&broadcast) as Arc<dyn BroadcastProvider>,
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
    let server = TestServer::builder().http_transport().build(app);
    (engine, manager, server)
}

fn make_ws_server_with_fast_heartbeat() -> (Arc<TaskEngine>, Arc<WorkerManager>, TestServer) {
    let store = Arc::new(MemoryShortTermStore::new());
    let broadcast = Arc::new(MemoryBroadcastProvider::new());
    let engine = Arc::new(TaskEngine::new(TaskEngineOptions {
        short_term_store: Arc::clone(&store) as Arc<dyn ShortTermStore>,
        broadcast: Arc::clone(&broadcast) as Arc<dyn BroadcastProvider>,
        long_term_store: None,
        hooks: None,
    }));
    let manager = Arc::new(WorkerManager::new(WorkerManagerOptions {
        engine: Arc::clone(&engine),
        short_term_store: Arc::clone(&store) as Arc<dyn ShortTermStore>,
        broadcast: Arc::clone(&broadcast) as Arc<dyn BroadcastProvider>,
        long_term_store: None,
        hooks: None,
        defaults: Some(WorkerManagerDefaults {
            heartbeat_interval_ms: Some(100),
            heartbeat_timeout_ms: Some(5000),
            assign_mode: None,
            offer_timeout_ms: None,
            disconnect_policy: None,
            disconnect_grace_ms: None,
        }),
    }));
    let (app, _) = create_app(
        Arc::clone(&engine),
        AuthMode::None,
        Some(Arc::clone(&manager)),
        None,
        CorsConfig::default(),
    );
    let server = TestServer::builder().http_transport().build(app);
    (engine, manager, server)
}

fn make_jwt_ws_server() -> (Arc<TaskEngine>, Arc<WorkerManager>, TestServer) {
    let store = Arc::new(MemoryShortTermStore::new());
    let broadcast = Arc::new(MemoryBroadcastProvider::new());
    let engine = Arc::new(TaskEngine::new(TaskEngineOptions {
        short_term_store: Arc::clone(&store) as Arc<dyn ShortTermStore>,
        broadcast: Arc::clone(&broadcast) as Arc<dyn BroadcastProvider>,
        long_term_store: None,
        hooks: None,
    }));
    let manager = Arc::new(WorkerManager::new(WorkerManagerOptions {
        engine: Arc::clone(&engine),
        short_term_store: Arc::clone(&store) as Arc<dyn ShortTermStore>,
        broadcast: Arc::clone(&broadcast) as Arc<dyn BroadcastProvider>,
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
    let server = TestServer::builder().http_transport().build(app);
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

async fn ws_register(ws: &mut axum_test::TestWebSocket, worker_id: &str, capacity: u32) -> String {
    ws.send_json(&json!({
        "type": "register",
        "matchRule": {},
        "capacity": capacity,
        "workerId": worker_id
    }))
    .await;
    let resp: serde_json::Value = ws.receive_json().await;
    assert_eq!(resp["type"], "registered");
    resp["workerId"].as_str().unwrap().to_string()
}

// ─── Test 1: Ping interval (lines 196-197) ──────────────────────────────────

#[tokio::test]
async fn ws_ping_interval_sends_ping_message() {
    let (_engine, _manager, server) = make_ws_server_with_fast_heartbeat();

    let mut ws = server
        .get_websocket("/workers/ws")
        .await
        .into_websocket()
        .await;

    // Register so we enter the `registered` branch that uses tokio::select!
    ws_register(&mut ws, "ping-w1", 5).await;

    // Wait for the heartbeat interval to fire (100ms configured, wait 200ms to be safe)
    tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;

    // Should receive a ping message
    let msg: serde_json::Value = ws.receive_json().await;
    assert_eq!(msg["type"], "ping");

    ws.close().await;
}

// ─── Test 2: Available command via ws-race (lines 204-205) ──────────────────

#[tokio::test]
async fn ws_race_dispatch_sends_available_message() {
    let (engine, _manager, server) = make_ws_server();

    let mut ws = server
        .get_websocket("/workers/ws")
        .await
        .into_websocket()
        .await;

    // Register worker
    ws_register(&mut ws, "race-avail-w1", 5).await;

    // Create a task with WsRace assign mode. The transition listener in
    // create_app will automatically call dispatch_ws_race, which sends
    // Available to all eligible WebSocket workers.
    engine
        .create_task(taskcast_core::engine::CreateTaskInput {
            id: Some("race-avail-t1".to_string()),
            r#type: Some("test".to_string()),
            assign_mode: Some(AssignMode::WsRace),
            ..Default::default()
        })
        .await
        .expect("create_task failed");

    // Give the async dispatch a moment to propagate
    tokio::time::sleep(tokio::time::Duration::from_millis(150)).await;

    // Worker should receive an Available message
    let msg: serde_json::Value = ws.receive_json().await;
    assert_eq!(msg["type"], "available");
    assert_eq!(msg["taskId"], "race-avail-t1");
    assert!(msg["task"].is_object(), "task should be a summary object");
    assert_eq!(msg["task"]["id"], "race-avail-t1");

    ws.close().await;
}

// ─── Test 3: Register worker_id mismatch (lines 247-259) ────────────────────

#[tokio::test]
async fn ws_register_worker_id_mismatch_returns_forbidden_error() {
    let (_engine, _manager, server) = make_jwt_ws_server();

    // Token has workerId "w1" but we will register with "w2"
    let token = make_token(json!({
        "sub": "worker",
        "scope": ["worker:connect"],
        "taskIds": "*",
        "workerId": "w1",
        "exp": 9999999999u64
    }));

    let mut ws = server
        .get_websocket("/workers/ws")
        .add_header(
            axum_test::http::header::AUTHORIZATION,
            bearer_header(&token),
        )
        .await
        .into_websocket()
        .await;

    // Try to register with a different worker_id than the JWT claims
    ws.send_json(&json!({
        "type": "register",
        "matchRule": {},
        "capacity": 5,
        "workerId": "w2"
    }))
    .await;

    let resp: serde_json::Value = ws.receive_json().await;
    assert_eq!(resp["type"], "error");
    assert_eq!(resp["code"], "FORBIDDEN");
    assert_eq!(resp["message"], "Forbidden: worker ID mismatch");

    ws.close().await;
}

// ─── Test 4: WS forbidden — wrong scope (line 167) ──────────────────────────

#[tokio::test]
async fn ws_forbidden_without_worker_connect_scope() {
    let (_engine, _manager, server) = make_jwt_ws_server();

    // Token with task:create scope only, no worker:connect
    let token = make_token(json!({
        "sub": "user",
        "scope": ["task:create"],
        "taskIds": "*",
        "exp": 9999999999u64
    }));

    // The WS handler checks scope BEFORE upgrading. Using get_websocket
    // ensures the Upgrade/Connection headers are present so the
    // WebSocketUpgrade extractor succeeds, letting the handler run its
    // scope check and return 403.
    let resp = server
        .get_websocket("/workers/ws")
        .add_header(
            axum_test::http::header::AUTHORIZATION,
            bearer_header(&token),
        )
        .await;
    resp.assert_status(axum_test::http::StatusCode::FORBIDDEN);
}

// ─── Test 5: Disconnect unregister (line 498) ────────────────────────────────

#[tokio::test]
async fn ws_disconnect_unregisters_worker() {
    let (_engine, manager, server) = make_ws_server();

    let mut ws = server
        .get_websocket("/workers/ws")
        .await
        .into_websocket()
        .await;

    // Register worker
    ws_register(&mut ws, "disconnect-w1", 5).await;

    // Verify the worker exists
    let worker = manager.get_worker("disconnect-w1").await.unwrap();
    assert!(worker.is_some(), "worker should exist after registration");

    // Close the WebSocket connection
    ws.close().await;

    // Wait for the disconnect handler to process
    tokio::time::sleep(tokio::time::Duration::from_millis(150)).await;

    // Verify the worker has been unregistered
    let worker = manager.get_worker("disconnect-w1").await.unwrap();
    assert!(
        worker.is_none(),
        "worker should be unregistered after WS disconnect"
    );
}
