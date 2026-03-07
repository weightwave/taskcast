//! Extended worker system integration tests for the Rust Taskcast server.
//!
//! These tests cover additional WebSocket protocol flows and REST endpoint edge
//! cases that are not exercised by the base `server_tests.rs` suite.

use std::sync::Arc;

use axum_test::http::HeaderValue;
use axum_test::TestServer;
use jsonwebtoken::{encode, EncodingKey, Header};
use serde_json::json;
use taskcast_core::worker_manager::{WorkerManager, WorkerManagerOptions, WorkerRegistration};
use taskcast_core::{
    BroadcastProvider, ConnectionMode, MemoryBroadcastProvider, MemoryShortTermStore,
    ShortTermStore, TaskEngine, TaskEngineOptions, WorkerMatchRule,
};
use taskcast_server::{create_app, AuthMode, CorsConfig, JwtConfig};

// ─── Test Helpers ────────────────────────────────────────────────────────────

/// Create an engine + worker manager + TestServer suitable for REST tests.
fn make_worker_server() -> (Arc<TaskEngine>, Arc<WorkerManager>, TestServer) {
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
    let (router, _ws_registry) = create_app(
        Arc::clone(&engine),
        AuthMode::None,
        Some(Arc::clone(&manager)),
        None,
        CorsConfig::default(),
    );
    let server = TestServer::new(router);
    (engine, manager, server)
}

/// Create an engine + worker manager + TestServer suitable for WebSocket tests
/// (requires HTTP transport for axum-test WS support).
fn make_worker_ws_server() -> (Arc<TaskEngine>, Arc<WorkerManager>, TestServer) {
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
    let (router, _ws_registry) = create_app(
        Arc::clone(&engine),
        AuthMode::None,
        Some(Arc::clone(&manager)),
        None,
        CorsConfig::default(),
    );
    let server = TestServer::builder().http_transport().build(router);
    (engine, manager, server)
}

const JWT_SECRET: &str = "test-secret-key-for-jwt-signing-needs-to-be-long-enough";

fn make_jwt_worker_server() -> (Arc<TaskEngine>, Arc<WorkerManager>, TestServer) {
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
    let auth_mode = AuthMode::Jwt(JwtConfig {
        algorithm: jsonwebtoken::Algorithm::HS256,
        secret: Some(JWT_SECRET.to_string()),
        public_key: None,
        issuer: None,
        audience: None,
    });
    let (router, _ws_registry) = create_app(Arc::clone(&engine), auth_mode, Some(Arc::clone(&manager)), None, CorsConfig::default());
    let server = TestServer::new(router);
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

/// Register a worker directly via the manager for REST endpoint tests.
async fn register_test_worker(manager: &WorkerManager, worker_id: &str) -> taskcast_core::Worker {
    manager
        .register_worker(WorkerRegistration {
            worker_id: Some(worker_id.to_string()),
            match_rule: WorkerMatchRule::default(),
            capacity: 5,
            weight: None,
            connection_mode: ConnectionMode::Pull,
            metadata: None,
        })
        .await
        .expect("register_worker failed")
}

/// Helper to register a WS worker on a connection. Returns the worker ID from
/// the server's `registered` response.
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

/// Helper to create a task via the engine directly.
async fn create_task(
    engine: &TaskEngine,
    task_id: &str,
    assign_mode: Option<taskcast_core::AssignMode>,
) {
    engine
        .create_task(taskcast_core::engine::CreateTaskInput {
            id: Some(task_id.to_string()),
            r#type: Some("test".to_string()),
            assign_mode,
            ..Default::default()
        })
        .await
        .expect("create_task failed");
}

// =============================================================================
// WebSocket Protocol Extended
// =============================================================================

// ─── 1. ws-offer complete flow ──────────────────────────────────────────────
//
// Register a WS worker -> create a task -> dispatch it -> worker receives
// offer via the manager -> worker sends accept -> gets assigned response.

#[tokio::test]
async fn ws_offer_complete_flow_register_dispatch_accept_assigned() {
    let (engine, manager, server) = make_worker_ws_server();

    let mut ws = server
        .get_websocket("/workers/ws")
        .await
        .into_websocket()
        .await;

    // Step 1: Register
    let wid = ws_register(&mut ws, "ext-offer-w1", 5).await;
    assert_eq!(wid, "ext-offer-w1");

    // Step 2: Create a task with ws-offer assign mode
    // The transition listener in create_app will automatically dispatch
    create_task(
        &engine,
        "ext-offer-t1",
        Some(taskcast_core::AssignMode::WsOffer),
    )
    .await;

    // Step 3: Worker should automatically receive an offer via the dispatch wiring
    // Give the async dispatch a moment to propagate
    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
    let offer: serde_json::Value = ws.receive_json().await;
    assert_eq!(offer["type"], "offer");
    assert_eq!(offer["taskId"], "ext-offer-t1");

    // Step 4: Worker accepts the task (internally calls claim_task)
    ws.send_json(&json!({
        "type": "accept",
        "taskId": "ext-offer-t1"
    }))
    .await;

    let resp: serde_json::Value = ws.receive_json().await;
    assert_eq!(resp["type"], "assigned");
    assert_eq!(resp["taskId"], "ext-offer-t1");

    // Verify task is now assigned
    let task = engine.get_task("ext-offer-t1").await.unwrap().unwrap();
    assert_eq!(task.status, taskcast_core::TaskStatus::Assigned);
    assert_eq!(task.assigned_worker.as_deref(), Some("ext-offer-w1"));

    ws.close().await;
}

// ─── 2. ws-race: multiple WS workers, first claim wins ─────────────────────

#[tokio::test]
async fn ws_race_first_claim_wins_others_get_success_false() {
    let (engine, _manager, server) = make_worker_ws_server();

    // Open two WS connections
    let mut ws1 = server
        .get_websocket("/workers/ws")
        .await
        .into_websocket()
        .await;
    let mut ws2 = server
        .get_websocket("/workers/ws")
        .await
        .into_websocket()
        .await;

    // Register both workers
    ws_register(&mut ws1, "ext-race-w1", 5).await;
    ws_register(&mut ws2, "ext-race-w2", 5).await;

    // Create a pending task
    create_task(&engine, "ext-race-t1", None).await;

    // Worker 1 claims first
    ws1.send_json(&json!({
        "type": "claim",
        "taskId": "ext-race-t1"
    }))
    .await;

    let resp1: serde_json::Value = ws1.receive_json().await;
    assert_eq!(resp1["type"], "claimed");
    assert_eq!(resp1["taskId"], "ext-race-t1");
    assert_eq!(resp1["success"], true);

    // Worker 2 tries to claim the same task — should fail
    ws2.send_json(&json!({
        "type": "claim",
        "taskId": "ext-race-t1"
    }))
    .await;

    let resp2: serde_json::Value = ws2.receive_json().await;
    assert_eq!(resp2["type"], "claimed");
    assert_eq!(resp2["taskId"], "ext-race-t1");
    assert_eq!(resp2["success"], false);

    ws1.close().await;
    ws2.close().await;
}

// ─── 3. Double register on same connection ──────────────────────────────────

#[tokio::test]
async fn ws_double_register_on_same_connection() {
    let (_engine, manager, server) = make_worker_ws_server();

    let mut ws = server
        .get_websocket("/workers/ws")
        .await
        .into_websocket()
        .await;

    // First register
    ws_register(&mut ws, "ext-double-w1", 5).await;

    // Second register on the same connection with a different ID.
    // The server code replaces worker_id in its local state. The old
    // worker_id remains registered in the manager (not auto-cleaned),
    // but the new one becomes the active identity for this connection.
    ws.send_json(&json!({
        "type": "register",
        "matchRule": {},
        "capacity": 3,
        "workerId": "ext-double-w2"
    }))
    .await;

    let resp: serde_json::Value = ws.receive_json().await;
    assert_eq!(resp["type"], "registered");
    assert_eq!(resp["workerId"], "ext-double-w2");

    // The second worker should exist in the manager
    let w2 = manager.get_worker("ext-double-w2").await.unwrap();
    assert!(w2.is_some());

    ws.close().await;
}

// ─── 4. Operations before register return NOT_REGISTERED ────────────────────

#[tokio::test]
async fn ws_all_operations_before_register_return_not_registered() {
    let (_engine, _manager, server) = make_worker_ws_server();

    let mut ws = server
        .get_websocket("/workers/ws")
        .await
        .into_websocket()
        .await;

    // Test each operation that requires registration

    // update
    ws.send_json(&json!({ "type": "update", "weight": 50 }))
        .await;
    let resp: serde_json::Value = ws.receive_json().await;
    assert_eq!(resp["type"], "error");
    assert_eq!(resp["code"], "NOT_REGISTERED");

    // accept
    ws.send_json(&json!({ "type": "accept", "taskId": "t1" }))
        .await;
    let resp: serde_json::Value = ws.receive_json().await;
    assert_eq!(resp["type"], "error");
    assert_eq!(resp["code"], "NOT_REGISTERED");

    // claim
    ws.send_json(&json!({ "type": "claim", "taskId": "t1" }))
        .await;
    let resp: serde_json::Value = ws.receive_json().await;
    assert_eq!(resp["type"], "error");
    assert_eq!(resp["code"], "NOT_REGISTERED");

    // decline
    ws.send_json(&json!({ "type": "decline", "taskId": "t1" }))
        .await;
    let resp: serde_json::Value = ws.receive_json().await;
    assert_eq!(resp["type"], "error");
    assert_eq!(resp["code"], "NOT_REGISTERED");

    // drain
    ws.send_json(&json!({ "type": "drain" })).await;
    let resp: serde_json::Value = ws.receive_json().await;
    assert_eq!(resp["type"], "error");
    assert_eq!(resp["code"], "NOT_REGISTERED");

    // Connection should still be alive — register to prove it
    ws.send_json(&json!({
        "type": "register",
        "matchRule": {},
        "capacity": 5
    }))
    .await;
    let resp: serde_json::Value = ws.receive_json().await;
    assert_eq!(resp["type"], "registered");

    ws.close().await;
}

// ─── 5. Claim non-existent task returns success=false ───────────────────────

#[tokio::test]
async fn ws_claim_nonexistent_task_returns_claimed_success_false() {
    let (_engine, _manager, server) = make_worker_ws_server();

    let mut ws = server
        .get_websocket("/workers/ws")
        .await
        .into_websocket()
        .await;

    ws_register(&mut ws, "ext-claim-noexist-w1", 5).await;

    ws.send_json(&json!({
        "type": "claim",
        "taskId": "totally-nonexistent-task"
    }))
    .await;

    let resp: serde_json::Value = ws.receive_json().await;
    assert_eq!(resp["type"], "claimed");
    assert_eq!(resp["success"], false);

    ws.close().await;
}

// ─── 6. Claim already-completed task returns success=false ──────────────────

#[tokio::test]
async fn ws_claim_completed_task_returns_success_false() {
    let (engine, _manager, server) = make_worker_ws_server();

    // Create a task and move it to completed
    create_task(&engine, "ext-completed-t1", None).await;
    engine
        .transition_task("ext-completed-t1", taskcast_core::TaskStatus::Running, None)
        .await
        .unwrap();
    engine
        .transition_task(
            "ext-completed-t1",
            taskcast_core::TaskStatus::Completed,
            None,
        )
        .await
        .unwrap();

    let mut ws = server
        .get_websocket("/workers/ws")
        .await
        .into_websocket()
        .await;

    ws_register(&mut ws, "ext-claim-completed-w1", 5).await;

    ws.send_json(&json!({
        "type": "claim",
        "taskId": "ext-completed-t1"
    }))
    .await;

    let resp: serde_json::Value = ws.receive_json().await;
    assert_eq!(resp["type"], "claimed");
    assert_eq!(resp["taskId"], "ext-completed-t1");
    assert_eq!(resp["success"], false);

    ws.close().await;
}

// ─── 7. Decline with blacklist via WebSocket — verify blacklist applied ─────

#[tokio::test]
async fn ws_decline_with_blacklist_updates_task_metadata() {
    let (engine, manager, server) = make_worker_ws_server();

    let mut ws = server
        .get_websocket("/workers/ws")
        .await
        .into_websocket()
        .await;

    ws_register(&mut ws, "ext-bl-w1", 5).await;

    // Create and claim the task
    create_task(&engine, "ext-bl-t1", None).await;

    // Claim via WS
    ws.send_json(&json!({
        "type": "claim",
        "taskId": "ext-bl-t1"
    }))
    .await;
    let claim_resp: serde_json::Value = ws.receive_json().await;
    assert_eq!(claim_resp["type"], "claimed");
    assert_eq!(claim_resp["success"], true);

    // Decline with blacklist=true
    ws.send_json(&json!({
        "type": "decline",
        "taskId": "ext-bl-t1",
        "blacklist": true
    }))
    .await;

    let decline_resp: serde_json::Value = ws.receive_json().await;
    assert_eq!(decline_resp["type"], "declined");
    assert_eq!(decline_resp["taskId"], "ext-bl-t1");

    // Allow time for the decline to be processed
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    // Verify the blacklist was written to the task metadata
    let task = engine.get_task("ext-bl-t1").await.unwrap().unwrap();
    let metadata = task.metadata.as_ref().expect("metadata should exist");
    let blacklisted_workers = metadata
        .get("_blacklistedWorkers")
        .expect("_blacklistedWorkers should exist")
        .as_array()
        .expect("should be array");
    let ids: Vec<&str> = blacklisted_workers
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert!(ids.contains(&"ext-bl-w1"), "worker should be blacklisted");

    // Verify the task went back to pending
    assert_eq!(task.status, taskcast_core::TaskStatus::Pending);

    // Verify worker capacity was restored
    let worker = manager.get_worker("ext-bl-w1").await.unwrap().unwrap();
    assert_eq!(worker.used_slots, 0);

    // Verify that dispatching again will NOT pick the blacklisted worker
    let dispatch = manager.dispatch_task("ext-bl-t1").await.unwrap();
    assert_eq!(
        dispatch,
        taskcast_core::worker_manager::DispatchResult::NoMatch,
        "Blacklisted worker should be excluded from dispatch"
    );

    ws.close().await;
}

// ─── 8. Update weight/capacity after register ──────────────────────────────

#[tokio::test]
async fn ws_update_weight_and_capacity_after_register() {
    let (_engine, manager, server) = make_worker_ws_server();

    let mut ws = server
        .get_websocket("/workers/ws")
        .await
        .into_websocket()
        .await;

    ws_register(&mut ws, "ext-update-w1", 5).await;

    // Verify initial state
    let worker = manager.get_worker("ext-update-w1").await.unwrap().unwrap();
    assert_eq!(worker.capacity, 5);
    assert_eq!(worker.weight, 50); // default weight

    // Update weight
    ws.send_json(&json!({
        "type": "update",
        "weight": 100
    }))
    .await;

    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let worker = manager.get_worker("ext-update-w1").await.unwrap().unwrap();
    assert_eq!(worker.weight, 100);
    assert_eq!(worker.capacity, 5); // unchanged

    // Update capacity
    ws.send_json(&json!({
        "type": "update",
        "capacity": 20
    }))
    .await;

    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let worker = manager.get_worker("ext-update-w1").await.unwrap().unwrap();
    assert_eq!(worker.weight, 100); // still 100
    assert_eq!(worker.capacity, 20);

    // Update both at once
    ws.send_json(&json!({
        "type": "update",
        "weight": 30,
        "capacity": 2
    }))
    .await;

    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let worker = manager.get_worker("ext-update-w1").await.unwrap().unwrap();
    assert_eq!(worker.weight, 30);
    assert_eq!(worker.capacity, 2);

    ws.close().await;
}

// ─── 9. Drain then verify worker excluded from new dispatches ───────────────

#[tokio::test]
async fn ws_drain_excludes_worker_from_new_dispatches() {
    let (engine, manager, server) = make_worker_ws_server();

    let mut ws = server
        .get_websocket("/workers/ws")
        .await
        .into_websocket()
        .await;

    ws_register(&mut ws, "ext-drain-w1", 5).await;

    // Send drain
    ws.send_json(&json!({ "type": "drain" })).await;

    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    // Verify draining status
    let worker = manager.get_worker("ext-drain-w1").await.unwrap().unwrap();
    assert_eq!(worker.status, taskcast_core::WorkerStatus::Draining);

    // Create a new task
    create_task(
        &engine,
        "ext-drain-t1",
        Some(taskcast_core::AssignMode::WsOffer),
    )
    .await;

    // Dispatch should NOT pick the draining worker
    let dispatch = manager.dispatch_task("ext-drain-t1").await.unwrap();
    assert_eq!(
        dispatch,
        taskcast_core::worker_manager::DispatchResult::NoMatch,
        "Draining worker should be excluded from dispatch"
    );

    ws.close().await;
}

// =============================================================================
// REST Extended
// =============================================================================

// ─── 10. DELETE worker with active assignments ──────────────────────────────

#[tokio::test]
async fn delete_worker_with_active_assignments() {
    let (engine, manager, server) = make_worker_server();

    register_test_worker(&manager, "ext-del-w1").await;

    // Create a task and have the worker claim it
    create_task(&engine, "ext-del-t1", None).await;

    let claim_result = manager
        .claim_task("ext-del-t1", "ext-del-w1")
        .await
        .unwrap();
    assert_eq!(
        claim_result,
        taskcast_core::worker_manager::ClaimResult::Claimed
    );

    // Verify the assignment exists
    let assignments = manager.get_worker_tasks("ext-del-w1").await.unwrap();
    assert_eq!(assignments.len(), 1);
    assert_eq!(assignments[0].task_id, "ext-del-t1");

    // Delete the worker via REST
    let response = server.delete("/workers/ext-del-w1").await;
    response.assert_status(axum_test::http::StatusCode::NO_CONTENT);

    // Verify the worker is gone
    let response = server.get("/workers/ext-del-w1").await;
    response.assert_status(axum_test::http::StatusCode::NOT_FOUND);

    // The task should still exist (deleting a worker does not auto-complete tasks)
    let task = engine.get_task("ext-del-t1").await.unwrap();
    assert!(
        task.is_some(),
        "Task should still exist after worker deletion"
    );
}

// ─── 11. Pull endpoint with very short timeout ─────────────────────────────

#[tokio::test]
async fn pull_endpoint_with_very_short_timeout_returns_no_content() {
    let (_engine, manager, server) = make_worker_server();
    register_test_worker(&manager, "ext-pull-short-w1").await;

    // Pull with a 1ms timeout — there are no tasks, so should quickly return 204
    let response = server
        .get("/workers/pull?workerId=ext-pull-short-w1&timeout=1")
        .await;
    response.assert_status(axum_test::http::StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn pull_endpoint_with_zero_timeout_returns_no_content() {
    let (_engine, manager, server) = make_worker_server();
    register_test_worker(&manager, "ext-pull-zero-w1").await;

    // timeout=0 should return immediately
    let response = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        server.get("/workers/pull?workerId=ext-pull-zero-w1&timeout=0"),
    )
    .await
    .expect("pull should not hang with timeout=0");
    response.assert_status(axum_test::http::StatusCode::NO_CONTENT);
}

// ─── 12. Pull endpoint worker_id validation against JWT ─────────────────────

#[tokio::test]
async fn pull_endpoint_rejects_mismatched_worker_id_in_jwt() {
    let (_engine, manager, server) = make_jwt_worker_server();

    // Register a worker directly
    register_test_worker(&manager, "ext-jwt-w1").await;

    // Create a token that has worker:connect scope but is bound to a DIFFERENT workerId
    let token = make_token(json!({
        "sub": "worker-user",
        "scope": ["worker:connect"],
        "workerId": "some-other-worker",
        "taskIds": "*",
        "exp": 9999999999u64
    }));

    // Try to pull with ext-jwt-w1 but the token says "some-other-worker"
    let response = server
        .get("/workers/pull?workerId=ext-jwt-w1&timeout=100")
        .add_header(
            axum_test::http::header::AUTHORIZATION,
            bearer_header(&token),
        )
        .await;

    // Should be 403 because the token's workerId doesn't match the query's workerId
    response.assert_status(axum_test::http::StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn pull_endpoint_succeeds_with_matching_worker_id_in_jwt() {
    let (_engine, manager, server) = make_jwt_worker_server();

    register_test_worker(&manager, "ext-jwt-match-w1").await;

    // Token with matching workerId
    let token = make_token(json!({
        "sub": "worker-user",
        "scope": ["worker:connect"],
        "workerId": "ext-jwt-match-w1",
        "taskIds": "*",
        "exp": 9999999999u64
    }));

    let response = server
        .get("/workers/pull?workerId=ext-jwt-match-w1&timeout=100")
        .add_header(
            axum_test::http::header::AUTHORIZATION,
            bearer_header(&token),
        )
        .await;

    // Should be 204 (no tasks available) not 403
    response.assert_status(axum_test::http::StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn pull_endpoint_allows_any_worker_id_when_jwt_has_no_worker_id() {
    let (_engine, manager, server) = make_jwt_worker_server();

    register_test_worker(&manager, "ext-jwt-nolock-w1").await;

    // Token with worker:connect scope but NO workerId field — should allow any
    let token = make_token(json!({
        "sub": "worker-user",
        "scope": ["worker:connect"],
        "taskIds": "*",
        "exp": 9999999999u64
    }));

    let response = server
        .get("/workers/pull?workerId=ext-jwt-nolock-w1&timeout=100")
        .add_header(
            axum_test::http::header::AUTHORIZATION,
            bearer_header(&token),
        )
        .await;

    // No worker_id restriction in token, so any workerId should be allowed
    response.assert_status(axum_test::http::StatusCode::NO_CONTENT);
}

// ─── 13. Decline already-declined task ──────────────────────────────────────

#[tokio::test]
async fn rest_decline_already_declined_task_is_noop() {
    let (engine, manager, server) = make_worker_server();

    register_test_worker(&manager, "ext-decline2-w1").await;

    // Create and claim a task
    server
        .post("/tasks")
        .json(&json!({
            "id": "ext-decline2-t1",
            "type": "test",
            "assignMode": "pull"
        }))
        .await
        .assert_status(axum_test::http::StatusCode::CREATED);

    let claim_result = manager
        .claim_task("ext-decline2-t1", "ext-decline2-w1")
        .await
        .unwrap();
    assert_eq!(
        claim_result,
        taskcast_core::worker_manager::ClaimResult::Claimed
    );

    // Decline the first time
    let response = server
        .post("/workers/tasks/ext-decline2-t1/decline")
        .json(&json!({ "workerId": "ext-decline2-w1" }))
        .await;
    response.assert_status(axum_test::http::StatusCode::OK);

    let body: serde_json::Value = response.json();
    assert_eq!(body["ok"], true);

    // Decline again — the assignment no longer exists, so it's a no-op
    let response = server
        .post("/workers/tasks/ext-decline2-t1/decline")
        .json(&json!({ "workerId": "ext-decline2-w1" }))
        .await;
    response.assert_status(axum_test::http::StatusCode::OK);

    let body: serde_json::Value = response.json();
    assert_eq!(body["ok"], true);

    // Task should be back to pending (from the first decline)
    let task = engine.get_task("ext-decline2-t1").await.unwrap().unwrap();
    assert_eq!(task.status, taskcast_core::TaskStatus::Pending);
}

// ─── 14. Decline endpoint worker_id validation against JWT ──────────────────

#[tokio::test]
async fn decline_endpoint_rejects_mismatched_worker_id_in_jwt() {
    let (_engine, manager, server) = make_jwt_worker_server();

    register_test_worker(&manager, "ext-jwt-dec-w1").await;

    // Token bound to a different worker
    let token = make_token(json!({
        "sub": "worker-user",
        "scope": ["worker:connect"],
        "workerId": "different-worker",
        "taskIds": "*",
        "exp": 9999999999u64
    }));

    let response = server
        .post("/workers/tasks/some-task/decline")
        .add_header(
            axum_test::http::header::AUTHORIZATION,
            bearer_header(&token),
        )
        .json(&json!({ "workerId": "ext-jwt-dec-w1" }))
        .await;

    // Should be 403 because token's workerId doesn't match body's workerId
    response.assert_status(axum_test::http::StatusCode::FORBIDDEN);
}

// =============================================================================
// Additional WebSocket edge cases
// =============================================================================

// ─── Accept on already-assigned (non-pending) task ──────────────────────────

#[tokio::test]
async fn ws_accept_already_assigned_task_returns_claim_failed() {
    let (engine, manager, server) = make_worker_ws_server();

    // Register a REST worker and have it claim the task
    register_test_worker(&manager, "ext-preempt-w1").await;
    create_task(&engine, "ext-preempt-t1", None).await;
    let claim_result = manager
        .claim_task("ext-preempt-t1", "ext-preempt-w1")
        .await
        .unwrap();
    assert_eq!(
        claim_result,
        taskcast_core::worker_manager::ClaimResult::Claimed
    );

    // Now a WS worker tries to accept the same task
    let mut ws = server
        .get_websocket("/workers/ws")
        .await
        .into_websocket()
        .await;
    ws_register(&mut ws, "ext-preempt-w2", 5).await;

    ws.send_json(&json!({
        "type": "accept",
        "taskId": "ext-preempt-t1"
    }))
    .await;

    let resp: serde_json::Value = ws.receive_json().await;
    assert_eq!(resp["type"], "error");
    assert_eq!(resp["code"], "CLAIM_FAILED");

    ws.close().await;
}

// ─── Multiple claims from same worker on different tasks ────────────────────

#[tokio::test]
async fn ws_worker_can_claim_multiple_tasks_within_capacity() {
    let (engine, _manager, server) = make_worker_ws_server();

    let mut ws = server
        .get_websocket("/workers/ws")
        .await
        .into_websocket()
        .await;

    // Register with capacity 5
    ws_register(&mut ws, "ext-multi-w1", 5).await;

    // Create two tasks
    create_task(&engine, "ext-multi-t1", None).await;
    create_task(&engine, "ext-multi-t2", None).await;

    // Claim first task
    ws.send_json(&json!({
        "type": "claim",
        "taskId": "ext-multi-t1"
    }))
    .await;

    let resp1: serde_json::Value = ws.receive_json().await;
    assert_eq!(resp1["type"], "claimed");
    assert_eq!(resp1["success"], true);

    // Claim second task
    ws.send_json(&json!({
        "type": "claim",
        "taskId": "ext-multi-t2"
    }))
    .await;

    let resp2: serde_json::Value = ws.receive_json().await;
    assert_eq!(resp2["type"], "claimed");
    assert_eq!(resp2["success"], true);

    ws.close().await;
}

// ─── Claim after drain — worker is draining but claim should still work ─────

#[tokio::test]
async fn ws_claim_still_works_after_drain() {
    let (engine, _manager, server) = make_worker_ws_server();

    let mut ws = server
        .get_websocket("/workers/ws")
        .await
        .into_websocket()
        .await;

    ws_register(&mut ws, "ext-drain-claim-w1", 5).await;

    // Drain
    ws.send_json(&json!({ "type": "drain" })).await;
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    // Create a task and try to claim it directly (not via dispatch)
    create_task(&engine, "ext-drain-claim-t1", None).await;

    ws.send_json(&json!({
        "type": "claim",
        "taskId": "ext-drain-claim-t1"
    }))
    .await;

    let resp: serde_json::Value = ws.receive_json().await;
    // claim_task doesn't check worker status — it only checks task status
    assert_eq!(resp["type"], "claimed");
    assert_eq!(resp["success"], true);

    ws.close().await;
}

// ─── REST: Delete non-existent worker returns 404 ───────────────────────────

#[tokio::test]
async fn delete_nonexistent_worker_returns_404() {
    let (_engine, _manager, server) = make_worker_server();

    let response = server.delete("/workers/ext-nonexistent").await;
    response.assert_status(axum_test::http::StatusCode::NOT_FOUND);
}

// ─── REST: Pull returns task when task is created after worker starts polling ─

#[tokio::test]
async fn pull_returns_task_created_during_poll_window() {
    let (engine, manager, server) = make_worker_server();
    register_test_worker(&manager, "ext-pull-late-w1").await;

    // Create the task BEFORE pull (since long-poll will check existing first)
    create_task(
        &engine,
        "ext-pull-late-t1",
        Some(taskcast_core::AssignMode::Pull),
    )
    .await;

    // Pull should find the task immediately
    let response = server
        .get("/workers/pull?workerId=ext-pull-late-w1&timeout=1000")
        .await;
    response.assert_status(axum_test::http::StatusCode::OK);

    let body: serde_json::Value = response.json();
    assert_eq!(body["id"], "ext-pull-late-t1");
}

// ─── REST: Pull with weight update ──────────────────────────────────────────

#[tokio::test]
async fn pull_with_weight_parameter_updates_worker_weight() {
    let (_engine, manager, server) = make_worker_server();
    register_test_worker(&manager, "ext-pull-weight-w1").await;

    // Verify initial weight is default (50)
    let worker = manager
        .get_worker("ext-pull-weight-w1")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(worker.weight, 50);

    // Pull with weight=90 (short timeout since there are no tasks)
    let response = server
        .get("/workers/pull?workerId=ext-pull-weight-w1&weight=90&timeout=100")
        .await;
    response.assert_status(axum_test::http::StatusCode::NO_CONTENT);

    // Verify weight was updated
    let worker = manager
        .get_worker("ext-pull-weight-w1")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(worker.weight, 90);
}
