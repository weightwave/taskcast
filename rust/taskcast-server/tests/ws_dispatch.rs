use std::sync::Arc;

use taskcast_core::worker_manager::{WorkerManager, WorkerManagerOptions, WorkerRegistration};
use taskcast_core::{
    AssignMode, BroadcastProvider, ConnectionMode, CreateTaskInput, MemoryBroadcastProvider,
    MemoryShortTermStore, ShortTermStore, TaskEngine, TaskEngineOptions, TaskStatus,
    WorkerMatchRule,
};
use taskcast_server::{create_app, AuthMode, WorkerCommand, WsRegistry};

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
