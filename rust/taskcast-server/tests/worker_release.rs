//! Server integration tests for the worker capacity release feature.
//!
//! These tests verify that the `create_app()` wiring automatically calls
//! `release_task()` on terminal transitions via the transition listener,
//! so that worker capacity is restored when tasks reach terminal status
//! through HTTP PATCH endpoints.

use std::sync::Arc;

use axum_test::TestServer;
use serde_json::json;
use taskcast_core::worker_manager::{WorkerManager, WorkerManagerOptions, WorkerRegistration};
use taskcast_core::{
    BroadcastProvider, ConnectionMode, MemoryBroadcastProvider, MemoryShortTermStore,
    ShortTermStore, TaskEngine, TaskEngineOptions, WorkerMatchRule, WorkerStatus,
};
use taskcast_server::{create_app, AuthMode};

// ─── Test Helpers ────────────────────────────────────────────────────────────

/// Create an engine + worker manager + TestServer with worker routes enabled.
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
    );
    let server = TestServer::new(router);
    (engine, manager, server)
}

/// Register a worker directly via the manager for REST endpoint tests.
async fn register_test_worker(manager: &WorkerManager, worker_id: &str, capacity: u32) {
    manager
        .register_worker(WorkerRegistration {
            worker_id: Some(worker_id.to_string()),
            match_rule: WorkerMatchRule::default(),
            capacity,
            weight: None,
            connection_mode: ConnectionMode::Pull,
            metadata: None,
        })
        .await
        .expect("register_worker failed");
}

/// Create a task via the engine directly.
async fn create_task(engine: &TaskEngine, task_id: &str) {
    engine
        .create_task(taskcast_core::engine::CreateTaskInput {
            id: Some(task_id.to_string()),
            r#type: Some("test".to_string()),
            ..Default::default()
        })
        .await
        .expect("create_task failed");
}

// =============================================================================
// Tests
// =============================================================================

/// 11. PATCH to completed releases worker capacity.
///
/// Claim a task, transition to running, then PATCH to completed via HTTP.
/// The transition listener wired in create_app should auto-call release_task,
/// restoring the worker's capacity.
#[tokio::test]
async fn patch_to_completed_releases_worker_capacity() {
    let (engine, manager, server) = make_worker_server();

    register_test_worker(&manager, "rel-w1", 5).await;
    create_task(&engine, "rel-t1").await;

    // Worker claims the task
    let claim = manager.claim_task("rel-t1", "rel-w1").await.unwrap();
    assert_eq!(claim, taskcast_core::ClaimResult::Claimed);

    let worker = manager.get_worker("rel-w1").await.unwrap().unwrap();
    assert_eq!(worker.used_slots, 1);

    // Transition to running via HTTP
    let response = server
        .patch("/tasks/rel-t1/status")
        .json(&json!({ "status": "running" }))
        .await;
    response.assert_status(axum_test::http::StatusCode::OK);

    // Worker still holds the slot (running is not terminal)
    let worker = manager.get_worker("rel-w1").await.unwrap().unwrap();
    assert_eq!(worker.used_slots, 1);

    // Transition to completed via HTTP
    let response = server
        .patch("/tasks/rel-t1/status")
        .json(&json!({ "status": "completed" }))
        .await;
    response.assert_status(axum_test::http::StatusCode::OK);

    // Allow the spawned release_task to run
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    // Worker capacity should be restored
    let worker = manager.get_worker("rel-w1").await.unwrap().unwrap();
    assert_eq!(
        worker.used_slots, 0,
        "Worker capacity should be restored after task completed"
    );
    assert_eq!(worker.status, WorkerStatus::Idle);
}

/// 12. PATCH to failed releases worker capacity.
#[tokio::test]
async fn patch_to_failed_releases_capacity() {
    let (engine, manager, server) = make_worker_server();

    register_test_worker(&manager, "rel-w2", 5).await;
    create_task(&engine, "rel-t2").await;

    // Claim and transition to running
    manager.claim_task("rel-t2", "rel-w2").await.unwrap();

    server
        .patch("/tasks/rel-t2/status")
        .json(&json!({ "status": "running" }))
        .await
        .assert_status(axum_test::http::StatusCode::OK);

    let worker = manager.get_worker("rel-w2").await.unwrap().unwrap();
    assert_eq!(worker.used_slots, 1);

    // Transition to failed via HTTP
    let response = server
        .patch("/tasks/rel-t2/status")
        .json(&json!({ "status": "failed" }))
        .await;
    response.assert_status(axum_test::http::StatusCode::OK);

    // Allow the spawned release_task to run
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    let worker = manager.get_worker("rel-w2").await.unwrap().unwrap();
    assert_eq!(
        worker.used_slots, 0,
        "Worker capacity should be restored after task failed"
    );
    assert_eq!(worker.status, WorkerStatus::Idle);
}

/// 13. PATCH to cancelled releases worker capacity.
#[tokio::test]
async fn patch_to_cancelled_releases_capacity() {
    let (engine, manager, server) = make_worker_server();

    register_test_worker(&manager, "rel-w3", 5).await;
    create_task(&engine, "rel-t3").await;

    // Claim the task (pending -> assigned)
    manager.claim_task("rel-t3", "rel-w3").await.unwrap();

    let worker = manager.get_worker("rel-w3").await.unwrap().unwrap();
    assert_eq!(worker.used_slots, 1);

    // Cancel the task directly (assigned -> cancelled)
    let response = server
        .patch("/tasks/rel-t3/status")
        .json(&json!({ "status": "cancelled" }))
        .await;
    response.assert_status(axum_test::http::StatusCode::OK);

    // Allow the spawned release_task to run
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    let worker = manager.get_worker("rel-w3").await.unwrap().unwrap();
    assert_eq!(
        worker.used_slots, 0,
        "Worker capacity should be restored after task cancelled"
    );
    assert_eq!(worker.status, WorkerStatus::Idle);
}

/// 14. PATCH to running does NOT release worker capacity — only terminal
///     transitions trigger release_task.
#[tokio::test]
async fn patch_to_running_does_not_release() {
    let (engine, manager, server) = make_worker_server();

    register_test_worker(&manager, "rel-w4", 5).await;
    create_task(&engine, "rel-t4").await;

    // Claim the task
    manager.claim_task("rel-t4", "rel-w4").await.unwrap();

    let worker = manager.get_worker("rel-w4").await.unwrap().unwrap();
    assert_eq!(worker.used_slots, 1);

    // Transition to running via HTTP (not terminal)
    let response = server
        .patch("/tasks/rel-t4/status")
        .json(&json!({ "status": "running" }))
        .await;
    response.assert_status(axum_test::http::StatusCode::OK);

    // Give time for any async operations
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    // Worker should still hold the slot
    let worker = manager.get_worker("rel-w4").await.unwrap().unwrap();
    assert_eq!(
        worker.used_slots, 1,
        "Running is not terminal — worker should still hold the slot"
    );
}

/// 15. Full flow: claim -> run -> complete -> worker becomes idle -> claim new task.
///
/// End-to-end test of the worker lifecycle with automatic capacity release.
#[tokio::test]
async fn full_flow_claim_run_complete_idle_new_task() {
    let (engine, manager, server) = make_worker_server();

    // Worker with capacity 1 — can only handle one task at a time
    register_test_worker(&manager, "rel-w5", 1).await;

    // Create first task
    create_task(&engine, "rel-t5a").await;

    // Claim the first task — worker should be busy
    let claim = manager.claim_task("rel-t5a", "rel-w5").await.unwrap();
    assert_eq!(claim, taskcast_core::ClaimResult::Claimed);

    let worker = manager.get_worker("rel-w5").await.unwrap().unwrap();
    assert_eq!(worker.status, WorkerStatus::Busy);
    assert_eq!(worker.used_slots, 1);

    // Create second task — worker cannot claim it yet (at capacity)
    create_task(&engine, "rel-t5b").await;

    let claim2 = manager.claim_task("rel-t5b", "rel-w5").await.unwrap();
    assert!(
        matches!(claim2, taskcast_core::ClaimResult::Failed { .. }),
        "Worker at capacity should not be able to claim another task"
    );

    // Complete the first task via HTTP: assigned -> running -> completed
    server
        .patch("/tasks/rel-t5a/status")
        .json(&json!({ "status": "running" }))
        .await
        .assert_status(axum_test::http::StatusCode::OK);

    server
        .patch("/tasks/rel-t5a/status")
        .json(&json!({ "status": "completed" }))
        .await
        .assert_status(axum_test::http::StatusCode::OK);

    // Allow the spawned release_task to run
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    // Worker should now be idle and able to take a new task
    let worker = manager.get_worker("rel-w5").await.unwrap().unwrap();
    assert_eq!(worker.status, WorkerStatus::Idle);
    assert_eq!(worker.used_slots, 0);

    // Now the worker should be able to claim the second task
    let claim3 = manager.claim_task("rel-t5b", "rel-w5").await.unwrap();
    assert_eq!(
        claim3,
        taskcast_core::ClaimResult::Claimed,
        "Worker should be able to claim a new task after the first one completed and capacity was released"
    );

    let worker = manager.get_worker("rel-w5").await.unwrap().unwrap();
    assert_eq!(worker.status, WorkerStatus::Busy);
    assert_eq!(worker.used_slots, 1);
}
