//! Cross-cutting integration tests between the SSE streaming system and the
//! worker assignment system.
//!
//! These tests verify that SSE subscribers correctly observe task lifecycle
//! events when tasks flow through the worker assignment pipeline (claim,
//! decline, assigned → running → completed, etc.).

use std::sync::Arc;

use serde_json::json;
use taskcast_core::worker_manager::{WorkerManager, WorkerManagerOptions, WorkerRegistration};
use taskcast_core::{
    BroadcastProvider, ConnectionMode, Level, MemoryBroadcastProvider, MemoryShortTermStore,
    ShortTermStore, TaskEngine, TaskEngineOptions, TaskStatus, WorkerMatchRule,
};
use taskcast_server::{create_app, AuthMode, CorsConfig};

// ─── Test Helpers ────────────────────────────────────────────────────────────

/// Create engine + worker manager + real HTTP server for SSE streaming tests.
/// Returns (engine, manager, address) where address is the `host:port` to
/// connect to.
fn make_sse_worker_app() -> (Arc<TaskEngine>, Arc<WorkerManager>, axum::Router) {
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
    (engine, manager, router)
}

/// Spin up a real TCP listener so we can use reqwest for SSE streaming.
async fn serve_app(app: axum::Router) -> std::net::SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    addr
}

/// Register a worker directly via the manager.
async fn register_worker(manager: &WorkerManager, worker_id: &str) -> taskcast_core::Worker {
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

/// Create a task via the engine directly.
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

/// Publish a single event via the engine.
async fn publish_event(engine: &TaskEngine, task_id: &str, event_type: &str, data: serde_json::Value) {
    engine
        .publish_event(
            task_id,
            taskcast_core::PublishEventInput {
                r#type: event_type.to_string(),
                level: Level::Info,
                data,
                series_id: None,
                series_mode: None,
                series_acc_field: None,
            },
        )
        .await
        .expect("publish_event failed");
}

// =============================================================================
// 1. SSE on Assigned Task — subscriber can connect and receive events
// =============================================================================

#[tokio::test]
async fn sse_on_assigned_task_replays_history() {
    let (engine, manager, app) = make_sse_worker_app();
    let addr = serve_app(app).await;
    let client = reqwest::Client::new();

    // Create a task and have a worker claim it (pending → assigned)
    create_task(&engine, "sse-w-assigned-1", None).await;
    register_worker(&manager, "w-assigned-1").await;
    let claim = manager.claim_task("sse-w-assigned-1", "w-assigned-1").await.unwrap();
    assert_eq!(claim, taskcast_core::worker_manager::ClaimResult::Claimed);

    // Verify task is assigned
    let task = engine.get_task("sse-w-assigned-1").await.unwrap().unwrap();
    assert_eq!(task.status, TaskStatus::Assigned);

    // Publish an event while assigned
    publish_event(&engine, "sse-w-assigned-1", "progress", json!({ "step": "init" })).await;

    // Now transition assigned → running → completed so SSE stream closes
    engine
        .transition_task("sse-w-assigned-1", TaskStatus::Running, None)
        .await
        .unwrap();
    engine
        .transition_task("sse-w-assigned-1", TaskStatus::Completed, None)
        .await
        .unwrap();

    // Connect SSE — task is terminal, so it replays history and closes
    let response = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        client.get(format!("http://{addr}/tasks/sse-w-assigned-1/events")).send(),
    )
    .await
    .expect("SSE connect timed out")
    .unwrap();
    assert_eq!(response.status(), 200);

    let text = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        response.text(),
    )
    .await
    .expect("SSE stream timed out")
    .unwrap();

    // Should have events including the status transitions and the progress event
    assert!(
        text.contains("event: taskcast.event"),
        "should have event lines. Got:\n{text}"
    );
    assert!(
        text.contains("init"),
        "should contain the progress event data. Got:\n{text}"
    );
    assert!(
        text.contains("event: taskcast.done"),
        "should have done event. Got:\n{text}"
    );
    assert!(
        text.contains("completed"),
        "done reason should be completed. Got:\n{text}"
    );
}

// =============================================================================
// 2. SSE pending → assigned → running flow: live streaming
// =============================================================================

#[tokio::test]
async fn sse_pending_to_assigned_to_running_live_stream() {
    let (engine, manager, app) = make_sse_worker_app();
    let addr = serve_app(app).await;
    let client = reqwest::Client::new();

    // Create a pending task
    create_task(&engine, "sse-w-flow-2", None).await;

    // Spawn a background task that will claim, run, then complete after SSE connects
    let engine_clone = Arc::clone(&engine);
    let manager_clone = Arc::clone(&manager);
    tokio::spawn(async move {
        // Wait for SSE subscription to establish
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;

        // Register worker and claim task (pending → assigned)
        register_worker(&manager_clone, "w-flow-2").await;
        let claim = manager_clone.claim_task("sse-w-flow-2", "w-flow-2").await.unwrap();
        assert_eq!(claim, taskcast_core::worker_manager::ClaimResult::Claimed);

        // assigned → running
        engine_clone
            .transition_task("sse-w-flow-2", TaskStatus::Running, None)
            .await
            .unwrap();

        // Publish a progress event
        publish_event(&engine_clone, "sse-w-flow-2", "progress", json!({ "step": 1 })).await;

        // running → completed (closes stream)
        engine_clone
            .transition_task("sse-w-flow-2", TaskStatus::Completed, None)
            .await
            .unwrap();
    });

    // Connect SSE on pending task — should hold then stream events
    let response = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        client.get(format!("http://{addr}/tasks/sse-w-flow-2/events")).send(),
    )
    .await
    .expect("SSE connect timed out")
    .unwrap();
    assert_eq!(response.status(), 200);

    let text = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        response.text(),
    )
    .await
    .expect("SSE stream timed out")
    .unwrap();

    // Should see: status events (assigned, running, completed) + progress event + done
    assert!(
        text.contains("event: taskcast.event"),
        "should have event lines. Got:\n{text}"
    );
    assert!(
        text.contains("event: taskcast.done"),
        "should have done event. Got:\n{text}"
    );
    assert!(
        text.contains("completed"),
        "done reason should be completed. Got:\n{text}"
    );
}

// =============================================================================
// 3. SSE on assigned → running → events → completed: full flow
// =============================================================================

#[tokio::test]
async fn sse_assigned_to_running_to_events_to_completed() {
    let (engine, manager, app) = make_sse_worker_app();
    let addr = serve_app(app).await;
    let client = reqwest::Client::new();

    // Create task, claim it (pending → assigned)
    create_task(&engine, "sse-w-full-3", None).await;
    register_worker(&manager, "w-full-3").await;
    let claim = manager.claim_task("sse-w-full-3", "w-full-3").await.unwrap();
    assert_eq!(claim, taskcast_core::worker_manager::ClaimResult::Claimed);

    // Verify assigned
    let task = engine.get_task("sse-w-full-3").await.unwrap().unwrap();
    assert_eq!(task.status, TaskStatus::Assigned);

    // Spawn background work: assigned → running → events → completed
    let engine_clone = Arc::clone(&engine);
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;

        // assigned → running
        engine_clone
            .transition_task("sse-w-full-3", TaskStatus::Running, None)
            .await
            .unwrap();

        // Publish multiple events while running
        publish_event(&engine_clone, "sse-w-full-3", "progress", json!({ "pct": 25 })).await;
        publish_event(&engine_clone, "sse-w-full-3", "progress", json!({ "pct": 50 })).await;
        publish_event(&engine_clone, "sse-w-full-3", "progress", json!({ "pct": 100 })).await;

        // Complete
        engine_clone
            .transition_task("sse-w-full-3", TaskStatus::Completed, None)
            .await
            .unwrap();
    });

    // Connect SSE while task is assigned (non-terminal, non-pending — should stream live)
    let response = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        client.get(format!("http://{addr}/tasks/sse-w-full-3/events")).send(),
    )
    .await
    .expect("SSE connect timed out")
    .unwrap();
    assert_eq!(response.status(), 200);

    let text = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        response.text(),
    )
    .await
    .expect("SSE stream timed out")
    .unwrap();

    // Count taskcast.event occurrences — should have status events + progress events
    let event_count = text.matches("event: taskcast.event").count();
    assert!(
        event_count >= 4,
        "should have at least 4 events (status:running + 3 progress), got {event_count}. Full text:\n{text}"
    );
    assert!(
        text.contains("event: taskcast.done"),
        "should have done event. Got:\n{text}"
    );
    assert!(
        text.contains("completed"),
        "done reason should be completed. Got:\n{text}"
    );
}

// =============================================================================
// 4. SSE on assigned → pending (decline): subscriber sees status revert
// =============================================================================

#[tokio::test]
async fn sse_assigned_to_pending_on_decline() {
    let (engine, manager, app) = make_sse_worker_app();
    let addr = serve_app(app).await;
    let client = reqwest::Client::new();

    // Create task, claim it (pending → assigned)
    create_task(&engine, "sse-w-decline-4", None).await;
    register_worker(&manager, "w-decline-4").await;
    let claim = manager.claim_task("sse-w-decline-4", "w-decline-4").await.unwrap();
    assert_eq!(claim, taskcast_core::worker_manager::ClaimResult::Claimed);

    let task = engine.get_task("sse-w-decline-4").await.unwrap().unwrap();
    assert_eq!(task.status, TaskStatus::Assigned);

    // Spawn background: decline (assigned → pending), then re-claim → running → completed
    let engine_clone = Arc::clone(&engine);
    let manager_clone = Arc::clone(&manager);
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;

        // Decline — task goes back to pending
        manager_clone
            .decline_task("sse-w-decline-4", "w-decline-4", None)
            .await
            .unwrap();

        // Small delay, then re-claim
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        // Re-claim
        let claim2 = manager_clone.claim_task("sse-w-decline-4", "w-decline-4").await.unwrap();
        assert_eq!(claim2, taskcast_core::worker_manager::ClaimResult::Claimed);

        // assigned → running → completed
        engine_clone
            .transition_task("sse-w-decline-4", TaskStatus::Running, None)
            .await
            .unwrap();
        engine_clone
            .transition_task("sse-w-decline-4", TaskStatus::Completed, None)
            .await
            .unwrap();
    });

    // Connect SSE while assigned
    let response = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        client.get(format!("http://{addr}/tasks/sse-w-decline-4/events")).send(),
    )
    .await
    .expect("SSE connect timed out")
    .unwrap();
    assert_eq!(response.status(), 200);

    let text = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        response.text(),
    )
    .await
    .expect("SSE stream timed out")
    .unwrap();

    // Should see the decline (status → pending) and re-assignment flow
    // The stream should contain status events showing the lifecycle
    assert!(
        text.contains("event: taskcast.event"),
        "should have event lines. Got:\n{text}"
    );
    assert!(
        text.contains("event: taskcast.done"),
        "should have done event. Got:\n{text}"
    );
    assert!(
        text.contains("completed"),
        "done reason should be completed. Got:\n{text}"
    );
    // Verify we see the pending status revert in the event stream
    assert!(
        text.contains("pending"),
        "should see pending status from decline. Got:\n{text}"
    );
}

// =============================================================================
// 5. SSE on assigned → cancelled: terminal event, stream closes
// =============================================================================

#[tokio::test]
async fn sse_assigned_to_cancelled_terminal_closes_stream() {
    let (engine, manager, app) = make_sse_worker_app();
    let addr = serve_app(app).await;
    let client = reqwest::Client::new();

    // Create task, claim it (pending → assigned)
    create_task(&engine, "sse-w-cancel-5", None).await;
    register_worker(&manager, "w-cancel-5").await;
    let claim = manager.claim_task("sse-w-cancel-5", "w-cancel-5").await.unwrap();
    assert_eq!(claim, taskcast_core::worker_manager::ClaimResult::Claimed);

    let task = engine.get_task("sse-w-cancel-5").await.unwrap().unwrap();
    assert_eq!(task.status, TaskStatus::Assigned);

    // Spawn background: cancel the task from assigned state
    let engine_clone = Arc::clone(&engine);
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;

        // assigned → cancelled (valid transition per state machine)
        engine_clone
            .transition_task("sse-w-cancel-5", TaskStatus::Cancelled, None)
            .await
            .unwrap();
    });

    // Connect SSE while assigned
    let response = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        client.get(format!("http://{addr}/tasks/sse-w-cancel-5/events")).send(),
    )
    .await
    .expect("SSE connect timed out")
    .unwrap();
    assert_eq!(response.status(), 200);

    let text = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        response.text(),
    )
    .await
    .expect("SSE stream timed out")
    .unwrap();

    // Should have a done event with reason "cancelled"
    assert!(
        text.contains("event: taskcast.done"),
        "should have done event. Got:\n{text}"
    );
    assert!(
        text.contains("cancelled"),
        "done reason should be cancelled. Got:\n{text}"
    );
}

// =============================================================================
// 6. Publish event via API while task is assigned — SSE subscriber sees it
// =============================================================================

#[tokio::test]
async fn publish_event_while_assigned_visible_to_sse() {
    let (engine, manager, app) = make_sse_worker_app();
    let addr = serve_app(app).await;
    let client = reqwest::Client::new();

    // Create task, claim it
    create_task(&engine, "sse-w-pub-6", None).await;
    register_worker(&manager, "w-pub-6").await;
    let claim = manager.claim_task("sse-w-pub-6", "w-pub-6").await.unwrap();
    assert_eq!(claim, taskcast_core::worker_manager::ClaimResult::Claimed);

    // Verify task is assigned
    let task = engine.get_task("sse-w-pub-6").await.unwrap().unwrap();
    assert_eq!(task.status, TaskStatus::Assigned);

    // Spawn background: publish events via HTTP API while assigned, then complete
    let addr_clone = addr;
    let engine_clone = Arc::clone(&engine);
    tokio::spawn(async move {
        let bg_client = reqwest::Client::new();
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;

        // Publish event via HTTP API while task is assigned
        let resp = bg_client
            .post(format!("http://{addr_clone}/tasks/sse-w-pub-6/events"))
            .json(&json!({
                "type": "worker.log",
                "level": "info",
                "data": { "msg": "working while assigned" }
            }))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 201);

        // Transition to running, then completed
        engine_clone
            .transition_task("sse-w-pub-6", TaskStatus::Running, None)
            .await
            .unwrap();
        engine_clone
            .transition_task("sse-w-pub-6", TaskStatus::Completed, None)
            .await
            .unwrap();
    });

    // Connect SSE
    let response = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        client.get(format!("http://{addr}/tasks/sse-w-pub-6/events")).send(),
    )
    .await
    .expect("SSE connect timed out")
    .unwrap();
    assert_eq!(response.status(), 200);

    let text = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        response.text(),
    )
    .await
    .expect("SSE stream timed out")
    .unwrap();

    // Should see the event published while assigned
    assert!(
        text.contains("working while assigned"),
        "SSE subscriber should see event published while task was assigned. Got:\n{text}"
    );
    assert!(
        text.contains("event: taskcast.done"),
        "should have done event. Got:\n{text}"
    );
}

// =============================================================================
// 7. Event ordering across assignment lifecycle (claim → events → decline →
//    re-claim → events)
// =============================================================================

#[tokio::test]
async fn event_ordering_across_claim_decline_reclaim() {
    let (engine, manager, app) = make_sse_worker_app();
    let addr = serve_app(app).await;
    let client = reqwest::Client::new();

    // Create task, claim it
    create_task(&engine, "sse-w-order-7", None).await;
    register_worker(&manager, "w-order-7a").await;
    register_worker(&manager, "w-order-7b").await;

    let claim = manager.claim_task("sse-w-order-7", "w-order-7a").await.unwrap();
    assert_eq!(claim, taskcast_core::worker_manager::ClaimResult::Claimed);

    // Spawn background lifecycle
    let engine_clone = Arc::clone(&engine);
    let manager_clone = Arc::clone(&manager);
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;

        // Phase 1: publish event while first worker is assigned
        publish_event(&engine_clone, "sse-w-order-7", "log", json!("phase1-event")).await;

        // Decline — task goes back to pending
        manager_clone
            .decline_task("sse-w-order-7", "w-order-7a", None)
            .await
            .unwrap();

        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        // Phase 2: second worker claims
        let claim2 = manager_clone.claim_task("sse-w-order-7", "w-order-7b").await.unwrap();
        assert_eq!(claim2, taskcast_core::worker_manager::ClaimResult::Claimed);

        // Publish event while second worker is assigned
        publish_event(&engine_clone, "sse-w-order-7", "log", json!("phase2-event")).await;

        // assigned → running → completed
        engine_clone
            .transition_task("sse-w-order-7", TaskStatus::Running, None)
            .await
            .unwrap();
        engine_clone
            .transition_task("sse-w-order-7", TaskStatus::Completed, None)
            .await
            .unwrap();
    });

    // Connect SSE while task is assigned to first worker
    let response = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        client.get(format!("http://{addr}/tasks/sse-w-order-7/events")).send(),
    )
    .await
    .expect("SSE connect timed out")
    .unwrap();
    assert_eq!(response.status(), 200);

    let text = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        response.text(),
    )
    .await
    .expect("SSE stream timed out")
    .unwrap();

    // Both phases' events should appear in order
    assert!(
        text.contains("phase1-event"),
        "should see phase 1 event. Got:\n{text}"
    );
    assert!(
        text.contains("phase2-event"),
        "should see phase 2 event. Got:\n{text}"
    );

    // Verify ordering: phase1 before phase2
    let pos1 = text.find("phase1-event").unwrap();
    let pos2 = text.find("phase2-event").unwrap();
    assert!(
        pos1 < pos2,
        "phase1-event should appear before phase2-event in the stream"
    );

    assert!(
        text.contains("event: taskcast.done"),
        "should have done event. Got:\n{text}"
    );
    assert!(
        text.contains("completed"),
        "done reason should be completed. Got:\n{text}"
    );
}

// =============================================================================
// 8. Full end-to-end: create → claim → events while assigned → running →
//    more events → complete → SSE subscriber sees everything in order
// =============================================================================

#[tokio::test]
async fn full_end_to_end_worker_sse_lifecycle() {
    let (engine, manager, app) = make_sse_worker_app();
    let addr = serve_app(app).await;
    let client = reqwest::Client::new();

    // Create task via HTTP
    let resp = client
        .post(format!("http://{addr}/tasks"))
        .json(&json!({ "id": "sse-w-e2e-8", "type": "llm.generate" }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 201);

    // Register worker and claim task
    register_worker(&manager, "w-e2e-8").await;
    let claim = manager.claim_task("sse-w-e2e-8", "w-e2e-8").await.unwrap();
    assert_eq!(claim, taskcast_core::worker_manager::ClaimResult::Claimed);

    // Verify assigned
    let task = engine.get_task("sse-w-e2e-8").await.unwrap().unwrap();
    assert_eq!(task.status, TaskStatus::Assigned);
    assert_eq!(task.assigned_worker.as_deref(), Some("w-e2e-8"));

    // Spawn the full lifecycle in background
    let engine_clone = Arc::clone(&engine);
    let addr_clone = addr;
    tokio::spawn(async move {
        let bg_client = reqwest::Client::new();
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;

        // Publish events while assigned
        publish_event(&engine_clone, "sse-w-e2e-8", "log", json!("assigned-log-1")).await;
        publish_event(&engine_clone, "sse-w-e2e-8", "log", json!("assigned-log-2")).await;

        // Transition assigned → running
        engine_clone
            .transition_task("sse-w-e2e-8", TaskStatus::Running, None)
            .await
            .unwrap();

        // Publish events while running via HTTP API
        bg_client
            .post(format!("http://{addr_clone}/tasks/sse-w-e2e-8/events"))
            .json(&json!([
                { "type": "progress", "level": "info", "data": { "pct": 33 } },
                { "type": "progress", "level": "info", "data": { "pct": 66 } },
                { "type": "progress", "level": "info", "data": { "pct": 100 } }
            ]))
            .send()
            .await
            .unwrap();

        // Complete
        bg_client
            .patch(format!("http://{addr_clone}/tasks/sse-w-e2e-8/status"))
            .json(&json!({
                "status": "completed",
                "result": { "output": "generated text" }
            }))
            .send()
            .await
            .unwrap();
    });

    // Connect SSE on the assigned task
    let response = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        client.get(format!("http://{addr}/tasks/sse-w-e2e-8/events")).send(),
    )
    .await
    .expect("SSE connect timed out")
    .unwrap();
    assert_eq!(response.status(), 200);

    let text = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        response.text(),
    )
    .await
    .expect("SSE stream timed out")
    .unwrap();

    // Verify all events are present
    assert!(
        text.contains("assigned-log-1"),
        "should see first assigned-phase log. Got:\n{text}"
    );
    assert!(
        text.contains("assigned-log-2"),
        "should see second assigned-phase log. Got:\n{text}"
    );

    // Verify ordering of assigned-phase logs
    let pos_log1 = text.find("assigned-log-1").unwrap();
    let pos_log2 = text.find("assigned-log-2").unwrap();
    assert!(pos_log1 < pos_log2, "assigned-log-1 should come before assigned-log-2");

    // Verify running-phase progress events
    assert!(
        text.contains("\"pct\":33") || text.contains("\"pct\": 33"),
        "should see progress 33%. Got:\n{text}"
    );
    assert!(
        text.contains("\"pct\":100") || text.contains("\"pct\": 100"),
        "should see progress 100%. Got:\n{text}"
    );

    // Verify ordering: assigned events before progress events
    let pos_assigned = text.find("assigned-log-2").unwrap();
    // Find the first occurrence of pct:33 (progress during running phase)
    let pos_progress = text.find("pct").unwrap();
    assert!(
        pos_assigned < pos_progress,
        "assigned-phase events should appear before running-phase progress events"
    );

    // Verify done event
    assert!(
        text.contains("event: taskcast.done"),
        "should have done event. Got:\n{text}"
    );
    assert!(
        text.contains("completed"),
        "done reason should be completed. Got:\n{text}"
    );

    // Verify we received a healthy number of events
    let event_count = text.matches("event: taskcast.event").count();
    // At minimum: audit:claim + log1 + log2 + status:running + progress*3 + status:completed = 8
    // (audit events from worker manager may add more)
    assert!(
        event_count >= 5,
        "should have at least 5 events, got {event_count}. Full text:\n{text}"
    );
}
