use std::sync::Arc;

use axum_test::TestServer;
use serde_json::json;
use taskcast_core::worker_manager::{WorkerManager, WorkerManagerOptions};
use taskcast_core::{
    BroadcastProvider, MemoryBroadcastProvider, MemoryShortTermStore, ShortTermStore, TaskEngine,
    TaskEngineOptions,
};
use taskcast_server::{create_app, AuthMode, CorsConfig};

fn make_ws_server() -> (Arc<TaskEngine>, Arc<WorkerManager>, TestServer) {
    let short_term_store = Arc::new(MemoryShortTermStore::new());
    let broadcast = Arc::new(MemoryBroadcastProvider::new());
    let engine = Arc::new(TaskEngine::new(TaskEngineOptions {
        short_term_store: Arc::clone(&short_term_store) as Arc<dyn ShortTermStore>,
        broadcast: Arc::clone(&broadcast) as Arc<dyn BroadcastProvider>,
        long_term_store: None,
        hooks: None,
    }));
    let manager = Arc::new(WorkerManager::new(WorkerManagerOptions {
        engine: Arc::clone(&engine),
        short_term_store: short_term_store as Arc<dyn ShortTermStore>,
        broadcast: broadcast as Arc<dyn BroadcastProvider>,
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

async fn register_ws(
    server: &TestServer,
) -> axum_test::TestWebSocket {
    let mut ws = server
        .get_websocket("/workers/ws")
        .await
        .into_websocket()
        .await;

    ws.send_json(&json!({
        "type": "register",
        "matchRule": {},
        "capacity": 5
    }))
    .await;

    let reg: serde_json::Value = ws.receive_json().await;
    assert_eq!(reg["type"], "registered");

    ws
}

// ─── Wrong types for taskId ────────────────────────────────────────────────

#[tokio::test]
async fn ws_claim_with_numeric_task_id_returns_parse_error() {
    let (_engine, _manager, server) = make_ws_server();
    let mut ws = register_ws(&server).await;

    ws.send_json(&json!({
        "type": "claim",
        "taskId": 123
    }))
    .await;

    let response: serde_json::Value = ws.receive_json().await;
    assert_eq!(response["type"], "error");
    assert_eq!(response["code"], "PARSE_ERROR");

    ws.close().await;
}

#[tokio::test]
async fn ws_accept_with_boolean_task_id_returns_parse_error() {
    let (_engine, _manager, server) = make_ws_server();
    let mut ws = register_ws(&server).await;

    ws.send_json(&json!({
        "type": "accept",
        "taskId": true
    }))
    .await;

    let response: serde_json::Value = ws.receive_json().await;
    assert_eq!(response["type"], "error");
    assert_eq!(response["code"], "PARSE_ERROR");

    ws.close().await;
}

#[tokio::test]
async fn ws_decline_with_null_task_id_returns_parse_error() {
    let (_engine, _manager, server) = make_ws_server();
    let mut ws = register_ws(&server).await;

    ws.send_json(&json!({
        "type": "decline",
        "taskId": null
    }))
    .await;

    let response: serde_json::Value = ws.receive_json().await;
    assert_eq!(response["type"], "error");
    assert_eq!(response["code"], "PARSE_ERROR");

    ws.close().await;
}

// ─── Unknown message types ─────────────────────────────────────────────────

#[tokio::test]
async fn ws_unknown_message_type_returns_parse_error() {
    let (_engine, _manager, server) = make_ws_server();
    let mut ws = register_ws(&server).await;

    ws.send_json(&json!({ "type": "fly_to_moon" })).await;

    let response: serde_json::Value = ws.receive_json().await;
    assert_eq!(response["type"], "error");
    assert_eq!(response["code"], "PARSE_ERROR");
    assert!(response["message"]
        .as_str()
        .unwrap()
        .contains("Invalid message"));

    ws.close().await;
}

#[tokio::test]
async fn ws_empty_type_string_returns_parse_error() {
    let (_engine, _manager, server) = make_ws_server();
    let mut ws = server
        .get_websocket("/workers/ws")
        .await
        .into_websocket()
        .await;

    ws.send_json(&json!({ "type": "" })).await;

    let response: serde_json::Value = ws.receive_json().await;
    assert_eq!(response["type"], "error");
    assert_eq!(response["code"], "PARSE_ERROR");

    ws.close().await;
}

#[tokio::test]
async fn ws_numeric_type_returns_parse_error() {
    let (_engine, _manager, server) = make_ws_server();
    let mut ws = server
        .get_websocket("/workers/ws")
        .await
        .into_websocket()
        .await;

    ws.send_json(&json!({ "type": 42 })).await;

    let response: serde_json::Value = ws.receive_json().await;
    assert_eq!(response["type"], "error");
    assert_eq!(response["code"], "PARSE_ERROR");

    ws.close().await;
}

// ─── Missing required fields ───────────────────────────────────────────────

#[tokio::test]
async fn ws_claim_missing_task_id_returns_parse_error() {
    let (_engine, _manager, server) = make_ws_server();
    let mut ws = register_ws(&server).await;

    ws.send_json(&json!({ "type": "claim" })).await;

    let response: serde_json::Value = ws.receive_json().await;
    assert_eq!(response["type"], "error");
    assert_eq!(response["code"], "PARSE_ERROR");

    ws.close().await;
}

#[tokio::test]
async fn ws_register_missing_capacity_returns_parse_error() {
    let (_engine, _manager, server) = make_ws_server();
    let mut ws = server
        .get_websocket("/workers/ws")
        .await
        .into_websocket()
        .await;

    ws.send_json(&json!({
        "type": "register",
        "matchRule": {}
    }))
    .await;

    let response: serde_json::Value = ws.receive_json().await;
    assert_eq!(response["type"], "error");
    assert_eq!(response["code"], "PARSE_ERROR");

    ws.close().await;
}
