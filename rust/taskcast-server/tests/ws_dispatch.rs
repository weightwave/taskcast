use std::sync::Arc;

use taskcast_core::worker_manager::{WorkerManager, WorkerManagerOptions, WorkerRegistration};
use taskcast_core::{
    AssignMode, BroadcastProvider, ConnectionMode, CreateTaskInput, MemoryBroadcastProvider,
    MemoryShortTermStore, ShortTermStore, TaskEngine, TaskEngineOptions, TaskStatus,
    WorkerMatchRule,
};
use taskcast_server::{
    auto_release_worker, create_app, dispatch_ws_offer, dispatch_ws_race,
    start_background_services, AuthMode, BackgroundServices, WorkerCommand, WsRegistry,
};

// ─── WsRegistry Unit Tests ──────────────────────────────────────────────────

#[test]
fn ws_registry_register_and_worker_ids() {
    let registry = WsRegistry::new();
    let (tx1, _rx1) = tokio::sync::mpsc::unbounded_channel();
    let (tx2, _rx2) = tokio::sync::mpsc::unbounded_channel();

    registry.register("w1", tx1);
    registry.register("w2", tx2);

    let mut ids = registry.worker_ids();
    ids.sort();
    assert_eq!(ids, vec!["w1".to_string(), "w2".to_string()]);
}

#[test]
fn ws_registry_unregister_removes_worker() {
    let registry = WsRegistry::new();
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();

    registry.register("w1", tx);
    assert_eq!(registry.worker_ids().len(), 1);

    registry.unregister("w1");
    assert!(registry.worker_ids().is_empty());
}

#[test]
fn ws_registry_unregister_nonexistent_is_noop() {
    let registry = WsRegistry::new();
    registry.unregister("nonexistent"); // Should not panic
    assert!(registry.worker_ids().is_empty());
}

#[test]
fn ws_registry_send_to_known_worker_returns_true() {
    let registry = WsRegistry::new();
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();

    registry.register("w1", tx);

    let summary = taskcast_server::TaskSummary {
        id: "task-1".to_string(),
        r#type: Some("llm.generate".to_string()),
        tags: None,
        cost: None,
        params: None,
    };

    let result = registry.send(
        "w1",
        WorkerCommand::Offer {
            task_id: "task-1".to_string(),
            task: summary,
        },
    );
    assert!(result);

    // Verify the message was received
    let cmd = rx.try_recv().unwrap();
    match cmd {
        WorkerCommand::Offer { task_id, task } => {
            assert_eq!(task_id, "task-1");
            assert_eq!(task.id, "task-1");
            assert_eq!(task.r#type, Some("llm.generate".to_string()));
        }
        _ => panic!("Expected Offer command"),
    }
}

#[test]
fn ws_registry_send_to_unknown_worker_returns_false() {
    let registry = WsRegistry::new();

    let summary = taskcast_server::TaskSummary {
        id: "task-1".to_string(),
        r#type: None,
        tags: None,
        cost: None,
        params: None,
    };

    let result = registry.send(
        "nonexistent",
        WorkerCommand::Available {
            task_id: "task-1".to_string(),
            task: summary,
        },
    );
    assert!(!result);
}

#[test]
fn ws_registry_send_returns_false_when_receiver_dropped() {
    let registry = WsRegistry::new();
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();

    registry.register("w1", tx);
    drop(rx); // Drop the receiver

    let summary = taskcast_server::TaskSummary {
        id: "task-1".to_string(),
        r#type: None,
        tags: None,
        cost: None,
        params: None,
    };

    let result = registry.send(
        "w1",
        WorkerCommand::Offer {
            task_id: "task-1".to_string(),
            task: summary,
        },
    );
    assert!(!result);
}

#[test]
fn ws_registry_clone_shares_state() {
    let registry = WsRegistry::new();
    let registry2 = registry.clone();
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();

    registry.register("w1", tx);
    assert_eq!(registry2.worker_ids(), vec!["w1".to_string()]);

    registry2.unregister("w1");
    assert!(registry.worker_ids().is_empty());
}

#[test]
fn ws_registry_send_available_command() {
    let registry = WsRegistry::new();
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();

    registry.register("w1", tx);

    let summary = taskcast_server::TaskSummary {
        id: "task-2".to_string(),
        r#type: Some("render.video".to_string()),
        tags: Some(vec!["gpu".to_string()]),
        cost: Some(5),
        params: None,
    };

    let result = registry.send(
        "w1",
        WorkerCommand::Available {
            task_id: "task-2".to_string(),
            task: summary,
        },
    );
    assert!(result);

    let cmd = rx.try_recv().unwrap();
    match cmd {
        WorkerCommand::Available { task_id, task } => {
            assert_eq!(task_id, "task-2");
            assert_eq!(task.id, "task-2");
            assert_eq!(task.r#type, Some("render.video".to_string()));
            assert_eq!(task.tags, Some(vec!["gpu".to_string()]));
            assert_eq!(task.cost, Some(5));
        }
        _ => panic!("Expected Available command"),
    }
}

// ─── Integration: create_app compiles and starts with WorkerManager ─────────

fn make_engine_and_store() -> (Arc<TaskEngine>, Arc<MemoryShortTermStore>) {
    let store = Arc::new(MemoryShortTermStore::new());
    let broadcast = Arc::new(MemoryBroadcastProvider::new());
    let engine = Arc::new(TaskEngine::new(TaskEngineOptions {
        short_term_store: Arc::clone(&store) as Arc<dyn ShortTermStore>,
        broadcast: broadcast as Arc<dyn BroadcastProvider>,
        long_term_store: None,
        hooks: None,
    }));
    (engine, store)
}

fn make_worker_manager(
    engine: &Arc<TaskEngine>,
    store: &Arc<MemoryShortTermStore>,
) -> Arc<WorkerManager> {
    let broadcast = Arc::new(MemoryBroadcastProvider::new());
    Arc::new(WorkerManager::new(WorkerManagerOptions {
        engine: Arc::clone(engine),
        short_term_store: Arc::clone(store) as Arc<dyn ShortTermStore>,
        broadcast: broadcast as Arc<dyn BroadcastProvider>,
        long_term_store: None,
        hooks: None,
        defaults: None,
    }))
}

#[test]
fn create_app_with_worker_manager_compiles() {
    let (engine, store) = make_engine_and_store();
    let manager = make_worker_manager(&engine, &store);
    let (_app, registry) = create_app(engine, AuthMode::None, Some(manager));
    // If this compiles and runs, the routing/state wiring is correct.
    assert!(registry.is_some());
}

#[test]
fn create_app_without_worker_manager_compiles() {
    let (engine, _store) = make_engine_and_store();
    let (_app, registry) = create_app(engine, AuthMode::None, None);
    assert!(registry.is_none());
}

// ─── Integration: transition listener fires for ws-offer task creation ──────

#[tokio::test]
async fn transition_listener_fires_on_task_create() {
    let (engine, _store) = make_engine_and_store();

    let fired = Arc::new(std::sync::Mutex::new(Vec::new()));
    let fired_clone = Arc::clone(&fired);

    engine.add_transition_listener(Box::new(move |task, from, to| {
        fired_clone.lock().unwrap().push((
            task.id.clone(),
            from.clone(),
            to.clone(),
        ));
    }));

    engine
        .create_task(CreateTaskInput {
            id: Some("t1".to_string()),
            assign_mode: Some(AssignMode::WsOffer),
            ..Default::default()
        })
        .await
        .unwrap();

    let transitions = fired.lock().unwrap();
    assert_eq!(transitions.len(), 1);
    assert_eq!(transitions[0].0, "t1");
    assert_eq!(transitions[0].1, TaskStatus::Pending);
    assert_eq!(transitions[0].2, TaskStatus::Pending);
}

#[tokio::test]
async fn transition_listener_fires_on_status_transition() {
    let (engine, _store) = make_engine_and_store();

    let fired = Arc::new(std::sync::Mutex::new(Vec::new()));
    let fired_clone = Arc::clone(&fired);

    engine.add_transition_listener(Box::new(move |task, from, to| {
        fired_clone.lock().unwrap().push((
            task.id.clone(),
            from.clone(),
            to.clone(),
        ));
    }));

    engine
        .create_task(CreateTaskInput {
            id: Some("t2".to_string()),
            ..Default::default()
        })
        .await
        .unwrap();

    engine
        .transition_task("t2", TaskStatus::Running, None)
        .await
        .unwrap();

    let transitions = fired.lock().unwrap();
    assert_eq!(transitions.len(), 2);
    // First: creation (pending -> pending)
    assert_eq!(transitions[0].1, TaskStatus::Pending);
    assert_eq!(transitions[0].2, TaskStatus::Pending);
    // Second: transition (pending -> running)
    assert_eq!(transitions[1].1, TaskStatus::Pending);
    assert_eq!(transitions[1].2, TaskStatus::Running);
}

#[tokio::test]
async fn ws_offer_dispatch_sends_offer_to_best_worker() {
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
        broadcast: broadcast as Arc<dyn BroadcastProvider>,
        long_term_store: None,
        hooks: None,
        defaults: None,
    }));

    // Register a WebSocket worker
    manager
        .register_worker(WorkerRegistration {
            worker_id: Some("ws-worker".to_string()),
            match_rule: WorkerMatchRule::default(),
            capacity: 5,
            weight: Some(50),
            connection_mode: ConnectionMode::Websocket,
            metadata: None,
        })
        .await
        .unwrap();

    // Create app with worker manager — get back the WsRegistry
    let (_app, ws_registry) = create_app(
        Arc::clone(&engine),
        AuthMode::None,
        Some(Arc::clone(&manager)),
    );
    let ws_registry = ws_registry.unwrap();

    // Register a channel for the worker in the returned registry
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    ws_registry.register("ws-worker", tx);

    // Create a task with ws-offer mode — the transition listener should dispatch
    engine
        .create_task(CreateTaskInput {
            id: Some("offer-task".to_string()),
            r#type: Some("llm.generate".to_string()),
            assign_mode: Some(AssignMode::WsOffer),
            ..Default::default()
        })
        .await
        .unwrap();

    // Give the spawned task a moment to complete
    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

    // The worker should have received an Offer command
    let cmd = rx.try_recv().unwrap();
    match cmd {
        WorkerCommand::Offer { task_id, task } => {
            assert_eq!(task_id, "offer-task");
            assert_eq!(task.id, "offer-task");
            assert_eq!(task.r#type, Some("llm.generate".to_string()));
        }
        _ => panic!("Expected Offer command, got: {:?}", cmd),
    }
}

#[tokio::test]
async fn ws_race_dispatch_broadcasts_to_all_ws_workers() {
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
        broadcast: broadcast as Arc<dyn BroadcastProvider>,
        long_term_store: None,
        hooks: None,
        defaults: None,
    }));

    // Register two WebSocket workers
    manager
        .register_worker(WorkerRegistration {
            worker_id: Some("ws1".to_string()),
            match_rule: WorkerMatchRule::default(),
            capacity: 5,
            weight: Some(50),
            connection_mode: ConnectionMode::Websocket,
            metadata: None,
        })
        .await
        .unwrap();
    manager
        .register_worker(WorkerRegistration {
            worker_id: Some("ws2".to_string()),
            match_rule: WorkerMatchRule::default(),
            capacity: 5,
            weight: Some(50),
            connection_mode: ConnectionMode::Websocket,
            metadata: None,
        })
        .await
        .unwrap();

    // Register a Pull worker (should NOT receive the broadcast)
    manager
        .register_worker(WorkerRegistration {
            worker_id: Some("pull1".to_string()),
            match_rule: WorkerMatchRule::default(),
            capacity: 5,
            weight: Some(50),
            connection_mode: ConnectionMode::Pull,
            metadata: None,
        })
        .await
        .unwrap();

    // Create app — get back the WsRegistry
    let (_app, ws_registry) = create_app(
        Arc::clone(&engine),
        AuthMode::None,
        Some(Arc::clone(&manager)),
    );
    let ws_registry = ws_registry.unwrap();

    // Register channels for all workers in the returned registry
    let (tx1, mut rx1) = tokio::sync::mpsc::unbounded_channel();
    let (tx2, mut rx2) = tokio::sync::mpsc::unbounded_channel();
    let (tx_pull, mut rx_pull) = tokio::sync::mpsc::unbounded_channel();
    ws_registry.register("ws1", tx1);
    ws_registry.register("ws2", tx2);
    ws_registry.register("pull1", tx_pull);

    // Create a task with ws-race mode
    engine
        .create_task(CreateTaskInput {
            id: Some("race-task".to_string()),
            r#type: Some("render.video".to_string()),
            assign_mode: Some(AssignMode::WsRace),
            ..Default::default()
        })
        .await
        .unwrap();

    // Give the spawned task time to complete
    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

    // Both WS workers should receive Available
    let cmd1 = rx1.try_recv().unwrap();
    match cmd1 {
        WorkerCommand::Available { task_id, .. } => {
            assert_eq!(task_id, "race-task");
        }
        _ => panic!("Expected Available command for ws1"),
    }

    let cmd2 = rx2.try_recv().unwrap();
    match cmd2 {
        WorkerCommand::Available { task_id, .. } => {
            assert_eq!(task_id, "race-task");
        }
        _ => panic!("Expected Available command for ws2"),
    }

    // Pull worker should NOT receive anything (ws-race only broadcasts
    // to workers with connection_mode == Websocket)
    assert!(rx_pull.try_recv().is_err());
}

#[tokio::test]
async fn external_assign_mode_does_not_trigger_ws_dispatch() {
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
        broadcast: broadcast as Arc<dyn BroadcastProvider>,
        long_term_store: None,
        hooks: None,
        defaults: None,
    }));

    manager
        .register_worker(WorkerRegistration {
            worker_id: Some("ws-worker".to_string()),
            match_rule: WorkerMatchRule::default(),
            capacity: 5,
            weight: Some(50),
            connection_mode: ConnectionMode::Websocket,
            metadata: None,
        })
        .await
        .unwrap();

    // Create app — get back the WsRegistry
    let (_app, ws_registry) = create_app(
        Arc::clone(&engine),
        AuthMode::None,
        Some(Arc::clone(&manager)),
    );
    let ws_registry = ws_registry.unwrap();

    // Register a channel for the worker
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    ws_registry.register("ws-worker", tx);

    // Create a task with external assign mode
    engine
        .create_task(CreateTaskInput {
            id: Some("ext-task".to_string()),
            assign_mode: Some(AssignMode::External),
            ..Default::default()
        })
        .await
        .unwrap();

    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

    // No command should be sent
    assert!(rx.try_recv().is_err());
}

// ─── Integration: auto-release on terminal transition ───────────────────────

#[tokio::test]
async fn auto_release_fires_on_terminal_transition() {
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
        broadcast: broadcast as Arc<dyn BroadcastProvider>,
        long_term_store: None,
        hooks: None,
        defaults: None,
    }));

    // Register a worker and create the app (which wires up the auto-release listener)
    manager
        .register_worker(WorkerRegistration {
            worker_id: Some("release-worker".to_string()),
            match_rule: WorkerMatchRule::default(),
            capacity: 5,
            weight: Some(50),
            connection_mode: ConnectionMode::Websocket,
            metadata: None,
        })
        .await
        .unwrap();

    let (_app, _ws_registry) = create_app(
        Arc::clone(&engine),
        AuthMode::None,
        Some(Arc::clone(&manager)),
    );

    // Create a task and claim it (which creates an assignment)
    engine
        .create_task(CreateTaskInput {
            id: Some("release-task".to_string()),
            assign_mode: Some(AssignMode::WsOffer),
            ..Default::default()
        })
        .await
        .unwrap();

    manager
        .claim_task("release-task", "release-worker")
        .await
        .unwrap();

    // Verify assignment exists
    let assignment = store
        .get_task_assignment("release-task")
        .await
        .unwrap();
    assert!(assignment.is_some(), "Assignment should exist after claim");

    // Check worker used_slots increased
    let worker = store.get_worker("release-worker").await.unwrap().unwrap();
    assert_eq!(worker.used_slots, 1);

    // Transition to running, then to completed (terminal)
    engine
        .transition_task("release-task", TaskStatus::Running, None)
        .await
        .unwrap();
    engine
        .transition_task("release-task", TaskStatus::Completed, None)
        .await
        .unwrap();

    // Give the spawned auto-release task time to complete
    tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;

    // Verify the assignment was released
    let assignment = store
        .get_task_assignment("release-task")
        .await
        .unwrap();
    assert!(
        assignment.is_none(),
        "Assignment should be released after terminal transition"
    );

    // Verify worker capacity was restored
    let worker = store.get_worker("release-worker").await.unwrap().unwrap();
    assert_eq!(
        worker.used_slots, 0,
        "Worker used_slots should be restored after auto-release"
    );
}

#[tokio::test]
async fn auto_release_fires_on_failed_transition() {
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
        broadcast: broadcast as Arc<dyn BroadcastProvider>,
        long_term_store: None,
        hooks: None,
        defaults: None,
    }));

    manager
        .register_worker(WorkerRegistration {
            worker_id: Some("w-fail".to_string()),
            match_rule: WorkerMatchRule::default(),
            capacity: 5,
            weight: Some(50),
            connection_mode: ConnectionMode::Websocket,
            metadata: None,
        })
        .await
        .unwrap();

    let (_app, _ws_registry) = create_app(
        Arc::clone(&engine),
        AuthMode::None,
        Some(Arc::clone(&manager)),
    );

    engine
        .create_task(CreateTaskInput {
            id: Some("fail-task".to_string()),
            assign_mode: Some(AssignMode::WsOffer),
            ..Default::default()
        })
        .await
        .unwrap();

    manager.claim_task("fail-task", "w-fail").await.unwrap();

    // Transition to running, then to failed (terminal)
    engine
        .transition_task("fail-task", TaskStatus::Running, None)
        .await
        .unwrap();
    engine
        .transition_task("fail-task", TaskStatus::Failed, None)
        .await
        .unwrap();

    tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;

    let assignment = store.get_task_assignment("fail-task").await.unwrap();
    assert!(
        assignment.is_none(),
        "Assignment should be released after failed transition"
    );
}

#[tokio::test]
async fn auto_release_does_not_fire_on_non_terminal_transition() {
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
        broadcast: broadcast as Arc<dyn BroadcastProvider>,
        long_term_store: None,
        hooks: None,
        defaults: None,
    }));

    manager
        .register_worker(WorkerRegistration {
            worker_id: Some("w-keep".to_string()),
            match_rule: WorkerMatchRule::default(),
            capacity: 5,
            weight: Some(50),
            connection_mode: ConnectionMode::Websocket,
            metadata: None,
        })
        .await
        .unwrap();

    let (_app, _ws_registry) = create_app(
        Arc::clone(&engine),
        AuthMode::None,
        Some(Arc::clone(&manager)),
    );

    engine
        .create_task(CreateTaskInput {
            id: Some("keep-task".to_string()),
            assign_mode: Some(AssignMode::WsOffer),
            ..Default::default()
        })
        .await
        .unwrap();

    manager.claim_task("keep-task", "w-keep").await.unwrap();

    // Transition to running (non-terminal) — assignment should remain
    engine
        .transition_task("keep-task", TaskStatus::Running, None)
        .await
        .unwrap();

    tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;

    let assignment = store.get_task_assignment("keep-task").await.unwrap();
    assert!(
        assignment.is_some(),
        "Assignment should NOT be released on non-terminal transition"
    );
}

// ─── Integration: ws-offer dispatch when no workers match ───────────────────

#[tokio::test]
async fn ws_offer_no_available_worker_does_not_send() {
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
        broadcast: broadcast as Arc<dyn BroadcastProvider>,
        long_term_store: None,
        hooks: None,
        defaults: None,
    }));

    // No workers registered at all — dispatch should result in NoMatch
    let (_app, ws_registry) = create_app(
        Arc::clone(&engine),
        AuthMode::None,
        Some(Arc::clone(&manager)),
    );
    let ws_registry = ws_registry.unwrap();

    // Register a channel even though no worker is registered in the manager
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    ws_registry.register("phantom-worker", tx);

    engine
        .create_task(CreateTaskInput {
            id: Some("no-match-task".to_string()),
            assign_mode: Some(AssignMode::WsOffer),
            ..Default::default()
        })
        .await
        .unwrap();

    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

    // No offer should be sent because dispatch_task returns NoMatch
    assert!(
        rx.try_recv().is_err(),
        "No offer should be sent when no workers are available"
    );
}

// ─── Integration: ws-race skips draining and offline workers ────────────────

#[tokio::test]
async fn ws_race_skips_draining_and_offline_workers() {
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
        broadcast: broadcast as Arc<dyn BroadcastProvider>,
        long_term_store: None,
        hooks: None,
        defaults: None,
    }));

    // Register one idle worker and one that will be set to draining
    manager
        .register_worker(WorkerRegistration {
            worker_id: Some("ws-idle".to_string()),
            match_rule: WorkerMatchRule::default(),
            capacity: 5,
            weight: Some(50),
            connection_mode: ConnectionMode::Websocket,
            metadata: None,
        })
        .await
        .unwrap();

    manager
        .register_worker(WorkerRegistration {
            worker_id: Some("ws-draining".to_string()),
            match_rule: WorkerMatchRule::default(),
            capacity: 5,
            weight: Some(50),
            connection_mode: ConnectionMode::Websocket,
            metadata: None,
        })
        .await
        .unwrap();

    // Update the second worker to draining status
    manager
        .update_worker(
            "ws-draining",
            taskcast_core::worker_manager::WorkerUpdate {
                status: Some(taskcast_core::worker_manager::WorkerUpdateStatus::Draining),
                ..Default::default()
            },
        )
        .await
        .unwrap();

    let (_app, ws_registry) = create_app(
        Arc::clone(&engine),
        AuthMode::None,
        Some(Arc::clone(&manager)),
    );
    let ws_registry = ws_registry.unwrap();

    let (tx_idle, mut rx_idle) = tokio::sync::mpsc::unbounded_channel();
    let (tx_draining, mut rx_draining) = tokio::sync::mpsc::unbounded_channel();
    ws_registry.register("ws-idle", tx_idle);
    ws_registry.register("ws-draining", tx_draining);

    engine
        .create_task(CreateTaskInput {
            id: Some("race-skip-task".to_string()),
            assign_mode: Some(AssignMode::WsRace),
            ..Default::default()
        })
        .await
        .unwrap();

    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

    // Idle worker should receive Available
    let cmd = rx_idle.try_recv().unwrap();
    match cmd {
        WorkerCommand::Available { task_id, .. } => {
            assert_eq!(task_id, "race-skip-task");
        }
        _ => panic!("Expected Available command for idle worker"),
    }

    // Draining worker should NOT receive Available
    assert!(
        rx_draining.try_recv().is_err(),
        "Draining worker should not receive ws-race broadcast"
    );
}

// ─── Integration: task with no assign_mode does not trigger dispatch ────────

#[tokio::test]
async fn task_without_assign_mode_does_not_trigger_dispatch() {
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
        broadcast: broadcast as Arc<dyn BroadcastProvider>,
        long_term_store: None,
        hooks: None,
        defaults: None,
    }));

    manager
        .register_worker(WorkerRegistration {
            worker_id: Some("ws-noop".to_string()),
            match_rule: WorkerMatchRule::default(),
            capacity: 5,
            weight: Some(50),
            connection_mode: ConnectionMode::Websocket,
            metadata: None,
        })
        .await
        .unwrap();

    let (_app, ws_registry) = create_app(
        Arc::clone(&engine),
        AuthMode::None,
        Some(Arc::clone(&manager)),
    );
    let ws_registry = ws_registry.unwrap();

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    ws_registry.register("ws-noop", tx);

    // Create a task with NO assign_mode
    engine
        .create_task(CreateTaskInput {
            id: Some("no-mode-task".to_string()),
            assign_mode: None,
            ..Default::default()
        })
        .await
        .unwrap();

    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

    assert!(
        rx.try_recv().is_err(),
        "No dispatch should happen when assign_mode is None"
    );
}

// ─── Integration: non-pending transition does not trigger ws dispatch ───────

#[tokio::test]
async fn non_pending_transition_does_not_trigger_ws_dispatch() {
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
        broadcast: broadcast as Arc<dyn BroadcastProvider>,
        long_term_store: None,
        hooks: None,
        defaults: None,
    }));

    manager
        .register_worker(WorkerRegistration {
            worker_id: Some("ws-w".to_string()),
            match_rule: WorkerMatchRule::default(),
            capacity: 5,
            weight: Some(50),
            connection_mode: ConnectionMode::Websocket,
            metadata: None,
        })
        .await
        .unwrap();

    let (_app, ws_registry) = create_app(
        Arc::clone(&engine),
        AuthMode::None,
        Some(Arc::clone(&manager)),
    );
    let ws_registry = ws_registry.unwrap();

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    ws_registry.register("ws-w", tx);

    // Create with ws-offer — this will fire one dispatch on creation (pending->pending)
    engine
        .create_task(CreateTaskInput {
            id: Some("trans-task".to_string()),
            assign_mode: Some(AssignMode::WsOffer),
            ..Default::default()
        })
        .await
        .unwrap();

    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

    // Drain the initial offer from creation
    let _ = rx.try_recv();

    // Now transition to running — this is a non-pending transition, so no new dispatch
    engine
        .transition_task("trans-task", TaskStatus::Running, None)
        .await
        .unwrap();

    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

    assert!(
        rx.try_recv().is_err(),
        "No dispatch should happen on non-pending transitions"
    );
}

// ─── BackgroundServices and start_background_services tests ─────────────────

#[tokio::test]
async fn start_background_services_without_worker_manager() {
    let store = Arc::new(MemoryShortTermStore::new());
    let broadcast = Arc::new(MemoryBroadcastProvider::new());
    let engine = Arc::new(TaskEngine::new(TaskEngineOptions {
        short_term_store: Arc::clone(&store) as Arc<dyn ShortTermStore>,
        broadcast: broadcast as Arc<dyn BroadcastProvider>,
        long_term_store: None,
        hooks: None,
    }));

    let mut services = start_background_services(
        Arc::clone(&engine),
        Arc::clone(&store) as Arc<dyn ShortTermStore>,
        None,
    );

    // Scheduler should be created
    assert!(services.scheduler.is_some());
    // Heartbeat monitor should NOT be created (no worker manager)
    assert!(services.heartbeat_monitor.is_none());

    // Stop should work without panicking
    services.stop();

    // Scheduler should be consumed (taken) after stop
    // A second stop should be harmless (handles are already taken)
    services.stop();
}

#[tokio::test]
async fn start_background_services_with_worker_manager() {
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
        broadcast: broadcast as Arc<dyn BroadcastProvider>,
        long_term_store: None,
        hooks: None,
        defaults: None,
    }));

    let mut services = start_background_services(
        Arc::clone(&engine),
        Arc::clone(&store) as Arc<dyn ShortTermStore>,
        Some(manager),
    );

    // Both scheduler and heartbeat monitor should be created
    assert!(services.scheduler.is_some());
    assert!(services.heartbeat_monitor.is_some());

    // Stop should work without panicking
    services.stop();
}

#[test]
fn background_services_stop_with_empty_fields() {
    // Test that stop() works safely when both fields are None
    let mut services = BackgroundServices {
        scheduler: None,
        heartbeat_monitor: None,
    };
    // Should not panic
    services.stop();
}

// ─── Integration: ws-offer task summary includes all fields ─────────────────

#[tokio::test]
async fn ws_offer_dispatch_includes_task_metadata_in_summary() {
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
        broadcast: broadcast as Arc<dyn BroadcastProvider>,
        long_term_store: None,
        hooks: None,
        defaults: None,
    }));

    manager
        .register_worker(WorkerRegistration {
            worker_id: Some("meta-worker".to_string()),
            match_rule: WorkerMatchRule::default(),
            capacity: 5,
            weight: Some(50),
            connection_mode: ConnectionMode::Websocket,
            metadata: None,
        })
        .await
        .unwrap();

    let (_app, ws_registry) = create_app(
        Arc::clone(&engine),
        AuthMode::None,
        Some(Arc::clone(&manager)),
    );
    let ws_registry = ws_registry.unwrap();

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    ws_registry.register("meta-worker", tx);

    // Create a task with type, tags, and cost
    engine
        .create_task(CreateTaskInput {
            id: Some("meta-task".to_string()),
            r#type: Some("llm.embed".to_string()),
            tags: Some(vec!["gpu".to_string(), "priority".to_string()]),
            cost: Some(3),
            assign_mode: Some(AssignMode::WsOffer),
            ..Default::default()
        })
        .await
        .unwrap();

    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

    let cmd = rx.try_recv().unwrap();
    match cmd {
        WorkerCommand::Offer { task_id, task } => {
            assert_eq!(task_id, "meta-task");
            assert_eq!(task.id, "meta-task");
            assert_eq!(task.r#type, Some("llm.embed".to_string()));
            assert_eq!(
                task.tags,
                Some(vec!["gpu".to_string(), "priority".to_string()])
            );
            assert_eq!(task.cost, Some(3));
        }
        _ => panic!("Expected Offer command"),
    }
}

// ─── Integration: ws-race task summary includes metadata ────────────────────

#[tokio::test]
async fn ws_race_dispatch_includes_task_metadata_in_summary() {
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
        broadcast: broadcast as Arc<dyn BroadcastProvider>,
        long_term_store: None,
        hooks: None,
        defaults: None,
    }));

    manager
        .register_worker(WorkerRegistration {
            worker_id: Some("race-meta-w".to_string()),
            match_rule: WorkerMatchRule::default(),
            capacity: 5,
            weight: Some(50),
            connection_mode: ConnectionMode::Websocket,
            metadata: None,
        })
        .await
        .unwrap();

    let (_app, ws_registry) = create_app(
        Arc::clone(&engine),
        AuthMode::None,
        Some(Arc::clone(&manager)),
    );
    let ws_registry = ws_registry.unwrap();

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    ws_registry.register("race-meta-w", tx);

    engine
        .create_task(CreateTaskInput {
            id: Some("race-meta-task".to_string()),
            r#type: Some("render.3d".to_string()),
            tags: Some(vec!["gpu".to_string()]),
            cost: Some(2),
            assign_mode: Some(AssignMode::WsRace),
            ..Default::default()
        })
        .await
        .unwrap();

    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

    let cmd = rx.try_recv().unwrap();
    match cmd {
        WorkerCommand::Available { task_id, task } => {
            assert_eq!(task_id, "race-meta-task");
            assert_eq!(task.id, "race-meta-task");
            assert_eq!(task.r#type, Some("render.3d".to_string()));
            assert_eq!(task.tags, Some(vec!["gpu".to_string()]));
            assert_eq!(task.cost, Some(2));
        }
        _ => panic!("Expected Available command"),
    }
}

// ─── Integration: multiple terminal statuses trigger auto-release ───────────

#[tokio::test]
async fn auto_release_fires_on_cancelled_transition() {
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
        broadcast: broadcast as Arc<dyn BroadcastProvider>,
        long_term_store: None,
        hooks: None,
        defaults: None,
    }));

    manager
        .register_worker(WorkerRegistration {
            worker_id: Some("w-cancel".to_string()),
            match_rule: WorkerMatchRule::default(),
            capacity: 5,
            weight: Some(50),
            connection_mode: ConnectionMode::Websocket,
            metadata: None,
        })
        .await
        .unwrap();

    let (_app, _ws_registry) = create_app(
        Arc::clone(&engine),
        AuthMode::None,
        Some(Arc::clone(&manager)),
    );

    engine
        .create_task(CreateTaskInput {
            id: Some("cancel-task".to_string()),
            assign_mode: Some(AssignMode::WsOffer),
            ..Default::default()
        })
        .await
        .unwrap();

    manager
        .claim_task("cancel-task", "w-cancel")
        .await
        .unwrap();

    // Cancel directly from pending->cancelled (valid transition)
    // But we already claimed, so task is in assigned state. Let's transition:
    // assigned -> running -> cancelled
    engine
        .transition_task("cancel-task", TaskStatus::Running, None)
        .await
        .unwrap();
    engine
        .transition_task("cancel-task", TaskStatus::Cancelled, None)
        .await
        .unwrap();

    tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;

    let assignment = store.get_task_assignment("cancel-task").await.unwrap();
    assert!(
        assignment.is_none(),
        "Assignment should be released after cancelled transition"
    );
}

#[tokio::test]
async fn auto_release_fires_on_timeout_transition() {
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
        broadcast: broadcast as Arc<dyn BroadcastProvider>,
        long_term_store: None,
        hooks: None,
        defaults: None,
    }));

    manager
        .register_worker(WorkerRegistration {
            worker_id: Some("w-timeout".to_string()),
            match_rule: WorkerMatchRule::default(),
            capacity: 5,
            weight: Some(50),
            connection_mode: ConnectionMode::Websocket,
            metadata: None,
        })
        .await
        .unwrap();

    let (_app, _ws_registry) = create_app(
        Arc::clone(&engine),
        AuthMode::None,
        Some(Arc::clone(&manager)),
    );

    engine
        .create_task(CreateTaskInput {
            id: Some("timeout-task".to_string()),
            assign_mode: Some(AssignMode::WsOffer),
            ..Default::default()
        })
        .await
        .unwrap();

    manager
        .claim_task("timeout-task", "w-timeout")
        .await
        .unwrap();

    engine
        .transition_task("timeout-task", TaskStatus::Running, None)
        .await
        .unwrap();
    engine
        .transition_task("timeout-task", TaskStatus::Timeout, None)
        .await
        .unwrap();

    tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;

    let assignment = store.get_task_assignment("timeout-task").await.unwrap();
    assert!(
        assignment.is_none(),
        "Assignment should be released after timeout transition"
    );
}

// ─── Direct tests for extracted dispatch functions ───────────────────────────

fn make_test_infra() -> (
    Arc<MemoryShortTermStore>,
    Arc<TaskEngine>,
    Arc<WorkerManager>,
) {
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
        broadcast: broadcast as Arc<dyn BroadcastProvider>,
        long_term_store: None,
        hooks: None,
        defaults: None,
    }));
    (store, engine, manager)
}

#[tokio::test]
async fn direct_auto_release_worker_releases_assignment() {
    let (store, engine, manager) = make_test_infra();

    manager
        .register_worker(WorkerRegistration {
            worker_id: Some("direct-w".to_string()),
            match_rule: WorkerMatchRule::default(),
            capacity: 5,
            weight: Some(50),
            connection_mode: ConnectionMode::Websocket,
            metadata: None,
        })
        .await
        .unwrap();

    engine
        .create_task(CreateTaskInput {
            id: Some("direct-t".to_string()),
            ..Default::default()
        })
        .await
        .unwrap();

    manager.claim_task("direct-t", "direct-w").await.unwrap();

    // Verify assignment exists
    let assignment = store.get_task_assignment("direct-t").await.unwrap();
    assert!(assignment.is_some());

    // Directly call the extracted function
    auto_release_worker(&manager, "direct-t").await;

    // Verify released
    let assignment = store.get_task_assignment("direct-t").await.unwrap();
    assert!(assignment.is_none());

    // Calling again on non-existent assignment is a no-op
    auto_release_worker(&manager, "direct-t").await;
}

#[tokio::test]
async fn direct_auto_release_worker_noop_for_unknown_task() {
    let (_store, _engine, manager) = make_test_infra();
    // Should not panic or error for unknown task
    auto_release_worker(&manager, "nonexistent").await;
}

#[tokio::test]
async fn direct_dispatch_ws_offer_dispatches_to_worker() {
    let (_store, engine, manager) = make_test_infra();
    let registry = WsRegistry::new();

    manager
        .register_worker(WorkerRegistration {
            worker_id: Some("offer-w".to_string()),
            match_rule: WorkerMatchRule::default(),
            capacity: 5,
            weight: Some(50),
            connection_mode: ConnectionMode::Websocket,
            metadata: None,
        })
        .await
        .unwrap();

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    registry.register("offer-w", tx);

    let task = engine
        .create_task(CreateTaskInput {
            id: Some("offer-t".to_string()),
            assign_mode: Some(AssignMode::WsOffer),
            ..Default::default()
        })
        .await
        .unwrap();

    // Directly call dispatch
    dispatch_ws_offer(&manager, &registry, &task).await;

    let cmd = rx.try_recv().unwrap();
    match cmd {
        WorkerCommand::Offer { task_id, task: summary } => {
            assert_eq!(task_id, "offer-t");
            assert_eq!(summary.id, "offer-t");
        }
        _ => panic!("Expected Offer command"),
    }
}

#[tokio::test]
async fn direct_dispatch_ws_offer_no_match_sends_nothing() {
    let (_store, engine, manager) = make_test_infra();
    let registry = WsRegistry::new();

    // No workers registered
    let task = engine
        .create_task(CreateTaskInput {
            id: Some("offer-no-w".to_string()),
            assign_mode: Some(AssignMode::WsOffer),
            ..Default::default()
        })
        .await
        .unwrap();

    // Should not panic
    dispatch_ws_offer(&manager, &registry, &task).await;
}

#[tokio::test]
async fn direct_dispatch_ws_race_broadcasts_to_eligible() {
    let (_store, engine, manager) = make_test_infra();
    let registry = WsRegistry::new();

    // Register two WS workers (idle) and one pull worker
    for (id, mode) in [
        ("race-w1", ConnectionMode::Websocket),
        ("race-w2", ConnectionMode::Websocket),
        ("race-pull", ConnectionMode::Pull),
    ] {
        manager
            .register_worker(WorkerRegistration {
                worker_id: Some(id.to_string()),
                match_rule: WorkerMatchRule::default(),
                capacity: 5,
                weight: Some(50),
                connection_mode: mode,
                metadata: None,
            })
            .await
            .unwrap();
    }

    let (tx1, mut rx1) = tokio::sync::mpsc::unbounded_channel();
    let (tx2, mut rx2) = tokio::sync::mpsc::unbounded_channel();
    registry.register("race-w1", tx1);
    registry.register("race-w2", tx2);

    let task = engine
        .create_task(CreateTaskInput {
            id: Some("race-t".to_string()),
            assign_mode: Some(AssignMode::WsRace),
            ..Default::default()
        })
        .await
        .unwrap();

    dispatch_ws_race(&manager, &registry, &task).await;

    // Both WS workers should receive Available
    let cmd1 = rx1.try_recv().unwrap();
    let cmd2 = rx2.try_recv().unwrap();
    match cmd1 {
        WorkerCommand::Available { task_id, .. } => assert_eq!(task_id, "race-t"),
        _ => panic!("Expected Available"),
    }
    match cmd2 {
        WorkerCommand::Available { task_id, .. } => assert_eq!(task_id, "race-t"),
        _ => panic!("Expected Available"),
    }
}

#[tokio::test]
async fn direct_dispatch_ws_race_skips_pull_workers() {
    let (_store, engine, manager) = make_test_infra();
    let registry = WsRegistry::new();

    // Register only a pull worker (should be skipped by ws-race)
    manager
        .register_worker(WorkerRegistration {
            worker_id: Some("race-pull-only".to_string()),
            match_rule: WorkerMatchRule::default(),
            capacity: 5,
            weight: Some(50),
            connection_mode: ConnectionMode::Pull,
            metadata: None,
        })
        .await
        .unwrap();

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    registry.register("race-pull-only", tx);

    let task = engine
        .create_task(CreateTaskInput {
            id: Some("race-pull-skip-t".to_string()),
            assign_mode: Some(AssignMode::WsRace),
            ..Default::default()
        })
        .await
        .unwrap();

    dispatch_ws_race(&manager, &registry, &task).await;

    // Pull worker should not receive command
    assert!(rx.try_recv().is_err(), "Pull worker should not receive ws-race command");
}

#[tokio::test]
async fn direct_dispatch_ws_race_no_workers_is_noop() {
    let (_store, engine, manager) = make_test_infra();
    let registry = WsRegistry::new();

    let task = engine
        .create_task(CreateTaskInput {
            id: Some("race-empty-t".to_string()),
            assign_mode: Some(AssignMode::WsRace),
            ..Default::default()
        })
        .await
        .unwrap();

    // Should not panic
    dispatch_ws_race(&manager, &registry, &task).await;
}
