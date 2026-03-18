use std::sync::Arc;

use axum_test::TestServer;
use taskcast_core::worker_manager::{WorkerManager, WorkerManagerOptions, WorkerRegistration, WorkerUpdate, WorkerUpdateStatus};
use taskcast_core::{
    AssignMode, ConnectionMode, CreateTaskInput, MemoryBroadcastProvider, MemoryShortTermStore,
    TaskEngine, TaskEngineOptions, WorkerMatchRule,
};
use taskcast_server::{
    auto_release_worker, create_app, dispatch_ws_offer, dispatch_ws_race,
    start_background_services, AuthMode, CorsConfig, WsRegistry,
};

fn make_engine() -> Arc<TaskEngine> {
    Arc::new(TaskEngine::new(TaskEngineOptions {
        short_term_store: Arc::new(MemoryShortTermStore::new()),
        broadcast: Arc::new(MemoryBroadcastProvider::new()),
        long_term_store: None,
        hooks: None,
    }))
}

fn make_worker_manager(engine: &Arc<TaskEngine>) -> Arc<WorkerManager> {
    Arc::new(WorkerManager::new(WorkerManagerOptions {
        engine: Arc::clone(engine),
        short_term_store: Arc::new(MemoryShortTermStore::new()),
        broadcast: Arc::new(MemoryBroadcastProvider::new()),
        long_term_store: None,
        hooks: None,
        defaults: None,
    }))
}

// ─── auto_release_worker ──────────────────────────────────────────────────────

#[tokio::test]
async fn auto_release_worker_ignores_unknown_task() {
    let engine = make_engine();
    let wm = make_worker_manager(&engine);
    // Should not panic or error — unknown task release is intentionally ignored
    auto_release_worker(&wm, "nonexistent-task-id").await;
}

#[tokio::test]
async fn auto_release_worker_releases_assigned_task() {
    let engine = make_engine();
    let wm = make_worker_manager(&engine);

    // Register a worker
    let worker = wm
        .register_worker(WorkerRegistration {
            worker_id: None,
            match_rule: WorkerMatchRule {
                task_types: None,
                tags: None,
            },
            capacity: 1,
            weight: None,
            connection_mode: ConnectionMode::Pull,
            metadata: None,
        })
        .await
        .unwrap();

    // Create and dispatch a task
    let task = engine
        .create_task(CreateTaskInput {
            r#type: Some("test".to_string()),
            assign_mode: Some(AssignMode::Pull),
            ..Default::default()
        })
        .await
        .unwrap();

    let _ = wm.dispatch_task(&task.id).await;

    // Release should succeed silently
    auto_release_worker(&wm, &task.id).await;

    // Verify worker has 0 used slots after release
    let workers = wm.list_workers(None).await.unwrap();
    let w = workers.iter().find(|w| w.id == worker.id).unwrap();
    assert_eq!(w.used_slots, 0);
}

// ─── dispatch_ws_offer ────────────────────────────────────────────────────────

#[tokio::test]
async fn dispatch_ws_offer_sends_offer_to_matched_worker() {
    let engine = make_engine();
    let wm = make_worker_manager(&engine);
    let registry = WsRegistry::new();

    // Register a WebSocket worker
    let worker = wm
        .register_worker(WorkerRegistration {
            worker_id: None,
            match_rule: WorkerMatchRule {
                task_types: None,
                tags: None,
            },
            capacity: 1,
            weight: None,
            connection_mode: ConnectionMode::Websocket,
            metadata: None,
        })
        .await
        .unwrap();

    // Register the worker in WsRegistry
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    registry.register(&worker.id, tx);

    // Create a task
    let task = engine
        .create_task(CreateTaskInput {
            r#type: Some("test".to_string()),
            assign_mode: Some(AssignMode::WsOffer),
            ..Default::default()
        })
        .await
        .unwrap();

    dispatch_ws_offer(&wm, &registry, &task).await;

    // Should receive an Offer command
    let cmd = rx.try_recv();
    assert!(cmd.is_ok(), "should receive an offer command");
}

#[tokio::test]
async fn dispatch_ws_offer_no_worker_does_not_panic() {
    let engine = make_engine();
    let wm = make_worker_manager(&engine);
    let registry = WsRegistry::new();

    let task = engine
        .create_task(CreateTaskInput {
            r#type: Some("test".to_string()),
            assign_mode: Some(AssignMode::WsOffer),
            ..Default::default()
        })
        .await
        .unwrap();

    // Should not panic when no workers are available
    dispatch_ws_offer(&wm, &registry, &task).await;
}

// ─── dispatch_ws_race ─────────────────────────────────────────────────────────

#[tokio::test]
async fn dispatch_ws_race_sends_available_to_eligible_workers() {
    let engine = make_engine();
    let wm = make_worker_manager(&engine);
    let registry = WsRegistry::new();

    // Register two WebSocket workers
    let w1 = wm
        .register_worker(WorkerRegistration {
            worker_id: None,
            match_rule: WorkerMatchRule {
                task_types: None,
                tags: None,
            },
            capacity: 1,
            weight: None,
            connection_mode: ConnectionMode::Websocket,
            metadata: None,
        })
        .await
        .unwrap();

    let w2 = wm
        .register_worker(WorkerRegistration {
            worker_id: None,
            match_rule: WorkerMatchRule {
                task_types: None,
                tags: None,
            },
            capacity: 1,
            weight: None,
            connection_mode: ConnectionMode::Websocket,
            metadata: None,
        })
        .await
        .unwrap();

    let (tx1, mut rx1) = tokio::sync::mpsc::unbounded_channel();
    let (tx2, mut rx2) = tokio::sync::mpsc::unbounded_channel();
    registry.register(&w1.id, tx1);
    registry.register(&w2.id, tx2);

    let task = engine
        .create_task(CreateTaskInput {
            r#type: Some("test".to_string()),
            assign_mode: Some(AssignMode::WsRace),
            ..Default::default()
        })
        .await
        .unwrap();

    dispatch_ws_race(&wm, &registry, &task).await;

    // Both workers should receive Available commands
    assert!(rx1.try_recv().is_ok(), "worker 1 should receive available");
    assert!(rx2.try_recv().is_ok(), "worker 2 should receive available");
}

#[tokio::test]
async fn dispatch_ws_race_skips_pull_workers() {
    let engine = make_engine();
    let wm = make_worker_manager(&engine);
    let registry = WsRegistry::new();

    // Register a Pull worker (should be skipped)
    let pull_worker = wm
        .register_worker(WorkerRegistration {
            worker_id: None,
            match_rule: WorkerMatchRule {
                task_types: None,
                tags: None,
            },
            capacity: 1,
            weight: None,
            connection_mode: ConnectionMode::Pull,
            metadata: None,
        })
        .await
        .unwrap();

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    registry.register(&pull_worker.id, tx);

    let task = engine
        .create_task(CreateTaskInput {
            r#type: Some("test".to_string()),
            assign_mode: Some(AssignMode::WsRace),
            ..Default::default()
        })
        .await
        .unwrap();

    dispatch_ws_race(&wm, &registry, &task).await;

    // Pull worker should NOT receive any command
    assert!(rx.try_recv().is_err(), "pull worker should not receive race command");
}

#[tokio::test]
async fn dispatch_ws_race_skips_draining_workers() {
    let engine = make_engine();
    let wm = make_worker_manager(&engine);
    let registry = WsRegistry::new();

    // Register a WebSocket worker and set it to draining
    let worker = wm
        .register_worker(WorkerRegistration {
            worker_id: None,
            match_rule: WorkerMatchRule {
                task_types: None,
                tags: None,
            },
            capacity: 1,
            weight: None,
            connection_mode: ConnectionMode::Websocket,
            metadata: None,
        })
        .await
        .unwrap();

    // Set to draining
    wm.update_worker(
        &worker.id,
        WorkerUpdate {
            status: Some(WorkerUpdateStatus::Draining),
            ..Default::default()
        },
    )
    .await
    .unwrap();

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    registry.register(&worker.id, tx);

    let task = engine
        .create_task(CreateTaskInput {
            r#type: Some("test".to_string()),
            assign_mode: Some(AssignMode::WsRace),
            ..Default::default()
        })
        .await
        .unwrap();

    dispatch_ws_race(&wm, &registry, &task).await;

    // Draining worker should NOT receive the command
    assert!(
        rx.try_recv().is_err(),
        "draining worker should not receive race command"
    );
}

#[tokio::test]
async fn dispatch_ws_race_no_workers_does_not_panic() {
    let engine = make_engine();
    let wm = make_worker_manager(&engine);
    let registry = WsRegistry::new();

    let task = engine
        .create_task(CreateTaskInput {
            r#type: Some("test".to_string()),
            assign_mode: Some(AssignMode::WsRace),
            ..Default::default()
        })
        .await
        .unwrap();

    // Should not panic
    dispatch_ws_race(&wm, &registry, &task).await;
}

// ─── BackgroundServices ───────────────────────────────────────────────────────

#[tokio::test]
async fn background_services_stop_without_worker_manager() {
    let engine = make_engine();
    let store: Arc<dyn taskcast_core::ShortTermStore> = Arc::new(MemoryShortTermStore::new());

    let mut services = start_background_services(Arc::clone(&engine), store, None);
    assert!(services.scheduler.is_some());
    assert!(services.heartbeat_monitor.is_none());

    services.stop();
}

#[tokio::test]
async fn background_services_stop_with_worker_manager() {
    let engine = make_engine();
    let store: Arc<dyn taskcast_core::ShortTermStore> = Arc::new(MemoryShortTermStore::new());
    let wm = make_worker_manager(&engine);

    let mut services = start_background_services(Arc::clone(&engine), store, Some(wm));
    assert!(services.scheduler.is_some());
    assert!(services.heartbeat_monitor.is_some());

    services.stop();
}

// ─── CorsConfig ───────────────────────────────────────────────────────────────

#[tokio::test]
async fn cors_allow_all_allows_any_origin() {
    let engine = make_engine();
    let (app, _) = create_app(
        Arc::clone(&engine),
        AuthMode::None,
        None,
        None,
        CorsConfig::AllowAll,
    );
    let server = TestServer::new(app);

    let res = server
        .get("/health")
        .add_header(
            axum_test::http::header::ORIGIN,
            axum_test::http::HeaderValue::from_static("http://example.com"),
        )
        .await;
    res.assert_status_ok();

    let acao = res.headers().get("access-control-allow-origin");
    assert!(acao.is_some(), "should have CORS header");
    assert_eq!(acao.unwrap().to_str().unwrap(), "*");
}

#[tokio::test]
async fn cors_allow_origins_allows_specified_origin() {
    let engine = make_engine();
    let (app, _) = create_app(
        Arc::clone(&engine),
        AuthMode::None,
        None,
        None,
        CorsConfig::AllowOrigins(vec!["http://localhost:3000".to_string()]),
    );
    let server = TestServer::new(app);

    let res = server
        .get("/health")
        .add_header(
            axum_test::http::header::ORIGIN,
            axum_test::http::HeaderValue::from_static("http://localhost:3000"),
        )
        .await;
    res.assert_status_ok();

    let acao = res.headers().get("access-control-allow-origin");
    assert!(acao.is_some(), "should have CORS header for allowed origin");
    assert_eq!(acao.unwrap().to_str().unwrap(), "http://localhost:3000");
}

#[tokio::test]
async fn cors_disabled_has_no_cors_headers() {
    let engine = make_engine();
    let (app, _) = create_app(
        Arc::clone(&engine),
        AuthMode::None,
        None,
        None,
        CorsConfig::Disabled,
    );
    let server = TestServer::new(app);

    let res = server
        .get("/health")
        .add_header(
            axum_test::http::header::ORIGIN,
            axum_test::http::HeaderValue::from_static("http://example.com"),
        )
        .await;
    res.assert_status_ok();

    let acao = res.headers().get("access-control-allow-origin");
    assert!(acao.is_none(), "disabled CORS should not have CORS headers");
}
