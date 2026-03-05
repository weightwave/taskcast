use std::sync::Arc;

use taskcast_core::{
    BroadcastProvider, ConnectionMode, CreateTaskInput, DisconnectPolicy, HeartbeatMonitor,
    HeartbeatMonitorOptions, MemoryBroadcastProvider, MemoryShortTermStore, ShortTermStore,
    TaskEngine, TaskEngineOptions, TaskStatus, Worker, WorkerAssignment, WorkerAssignmentStatus,
    WorkerManager, WorkerManagerOptions, WorkerMatchRule, WorkerStatus,
};

// ─── Helpers ────────────────────────────────────────────────────────────────

struct TestContext {
    engine: Arc<TaskEngine>,
    store: Arc<MemoryShortTermStore>,
    worker_manager: Arc<WorkerManager>,
    monitor: HeartbeatMonitor,
}

fn make_monitor(
    default_policy: DisconnectPolicy,
    timeout_ms: u64,
    grace_ms: u64,
) -> TestContext {
    let store = Arc::new(MemoryShortTermStore::new());
    let broadcast = Arc::new(MemoryBroadcastProvider::new());
    let engine = Arc::new(TaskEngine::new(TaskEngineOptions {
        short_term_store: Arc::clone(&store) as Arc<dyn ShortTermStore>,
        broadcast: Arc::clone(&broadcast) as Arc<dyn BroadcastProvider>,
        long_term_store: None,
        hooks: None,
    }));
    let worker_manager = Arc::new(WorkerManager::new(WorkerManagerOptions {
        engine: Arc::clone(&engine),
        short_term_store: Arc::clone(&store) as Arc<dyn ShortTermStore>,
        broadcast: Arc::clone(&broadcast) as Arc<dyn BroadcastProvider>,
        long_term_store: None,
        hooks: None,
        defaults: None,
    }));
    let monitor = HeartbeatMonitor::new(HeartbeatMonitorOptions {
        worker_manager: Arc::clone(&worker_manager),
        engine: Arc::clone(&engine),
        short_term_store: Arc::clone(&store) as Arc<dyn ShortTermStore>,
        check_interval_ms: 30_000,
        heartbeat_timeout_ms: timeout_ms,
        default_disconnect_policy: default_policy,
        disconnect_grace_ms: grace_ms,
    });
    TestContext {
        engine,
        store,
        worker_manager,
        monitor,
    }
}

/// Register a worker with a very old heartbeat (simulating timeout).
async fn register_stale_worker(
    store: &Arc<MemoryShortTermStore>,
    worker_id: &str,
    staleness_ms: f64,
) {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as f64;
    let worker = Worker {
        id: worker_id.to_string(),
        status: WorkerStatus::Idle,
        match_rule: WorkerMatchRule::default(),
        capacity: 5,
        used_slots: 1,
        weight: 50,
        connection_mode: ConnectionMode::Pull,
        connected_at: now - staleness_ms,
        last_heartbeat_at: now - staleness_ms,
        metadata: None,
    };
    store.save_worker(worker).await.unwrap();
}

/// Register a worker with a fresh heartbeat.
async fn register_fresh_worker(
    store: &Arc<MemoryShortTermStore>,
    worker_id: &str,
) {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as f64;
    let worker = Worker {
        id: worker_id.to_string(),
        status: WorkerStatus::Idle,
        match_rule: WorkerMatchRule::default(),
        capacity: 5,
        used_slots: 0,
        weight: 50,
        connection_mode: ConnectionMode::Pull,
        connected_at: now,
        last_heartbeat_at: now,
        metadata: None,
    };
    store.save_worker(worker).await.unwrap();
}

/// Create a task in Running status with an assignment record.
async fn create_assigned_task(
    engine: &Arc<TaskEngine>,
    store: &Arc<MemoryShortTermStore>,
    task_id: &str,
    worker_id: &str,
    disconnect_policy: Option<DisconnectPolicy>,
) {
    create_assigned_task_with_status(
        engine,
        store,
        task_id,
        worker_id,
        disconnect_policy,
        TaskStatus::Running,
    )
    .await;
}

/// Create a task in the given status with an assignment record.
async fn create_assigned_task_with_status(
    engine: &Arc<TaskEngine>,
    store: &Arc<MemoryShortTermStore>,
    task_id: &str,
    worker_id: &str,
    disconnect_policy: Option<DisconnectPolicy>,
    status: TaskStatus,
) {
    engine
        .create_task(CreateTaskInput {
            id: Some(task_id.to_string()),
            disconnect_policy,
            ..Default::default()
        })
        .await
        .unwrap();

    // Walk through valid transitions to reach the desired status
    match status {
        TaskStatus::Running => {
            engine
                .transition_task(task_id, TaskStatus::Running, None)
                .await
                .unwrap();
        }
        TaskStatus::Assigned => {
            engine
                .transition_task(task_id, TaskStatus::Assigned, None)
                .await
                .unwrap();
        }
        _ => {
            engine
                .transition_task(task_id, TaskStatus::Running, None)
                .await
                .unwrap();
        }
    }

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as f64;
    let assignment = WorkerAssignment {
        task_id: task_id.to_string(),
        worker_id: worker_id.to_string(),
        cost: 1,
        assigned_at: now,
        status: WorkerAssignmentStatus::Assigned,
    };
    store.add_assignment(assignment).await.unwrap();
}

// ─── Test 1: tick() marks worker offline when heartbeat times out ───────────

#[tokio::test]
async fn tick_marks_worker_offline_on_heartbeat_timeout() {
    let ctx = make_monitor(DisconnectPolicy::Mark, 100, 30_000);

    // Register a worker with a very stale heartbeat (200ms ago, timeout is 100ms)
    register_stale_worker(&ctx.store, "w1", 200.0).await;

    ctx.monitor.tick().await.unwrap();

    let worker = ctx.store.get_worker("w1").await.unwrap().unwrap();
    assert_eq!(worker.status, WorkerStatus::Offline);
}

// ─── Test 2: tick() does not affect workers with recent heartbeat ───────────

#[tokio::test]
async fn tick_does_not_affect_workers_with_recent_heartbeat() {
    let ctx = make_monitor(DisconnectPolicy::Mark, 100, 30_000);

    register_fresh_worker(&ctx.store, "w1").await;

    ctx.monitor.tick().await.unwrap();

    let worker = ctx.store.get_worker("w1").await.unwrap().unwrap();
    assert_eq!(worker.status, WorkerStatus::Idle);
}

// ─── Test 3: tick() with Fail policy transitions task to failed ─────────────

#[tokio::test]
async fn tick_with_fail_policy_transitions_task_to_failed() {
    let ctx = make_monitor(DisconnectPolicy::Fail, 100, 30_000);

    register_stale_worker(&ctx.store, "w1", 200.0).await;
    create_assigned_task(&ctx.engine, &ctx.store, "t1", "w1", None).await;

    ctx.monitor.tick().await.unwrap();

    // Worker should be offline
    let worker = ctx.store.get_worker("w1").await.unwrap().unwrap();
    assert_eq!(worker.status, WorkerStatus::Offline);

    // Task should be failed
    let task = ctx.engine.get_task("t1").await.unwrap().unwrap();
    assert_eq!(task.status, TaskStatus::Failed);

    // Error should indicate worker disconnect
    assert!(task.error.is_some());
    let error = task.error.unwrap();
    assert_eq!(error.code, Some("WORKER_DISCONNECT".to_string()));
    assert!(error.message.contains("w1"));

    // Assignment should be released
    let assignments = ctx.worker_manager.get_worker_tasks("w1").await.unwrap();
    assert!(assignments.is_empty());
}

// ─── Test 4: tick() with Mark policy only marks offline ─────────────────────

#[tokio::test]
async fn tick_with_mark_policy_only_marks_offline() {
    let ctx = make_monitor(DisconnectPolicy::Mark, 100, 30_000);

    register_stale_worker(&ctx.store, "w1", 200.0).await;
    create_assigned_task(&ctx.engine, &ctx.store, "t1", "w1", None).await;

    ctx.monitor.tick().await.unwrap();

    // Worker should be offline
    let worker = ctx.store.get_worker("w1").await.unwrap().unwrap();
    assert_eq!(worker.status, WorkerStatus::Offline);

    // Task should still be running (mark policy does not change task state)
    let task = ctx.engine.get_task("t1").await.unwrap().unwrap();
    assert_eq!(task.status, TaskStatus::Running);

    // Assignment should still exist
    let assignments = ctx.worker_manager.get_worker_tasks("w1").await.unwrap();
    assert_eq!(assignments.len(), 1);
}

// ─── Test 5: tick() with Reassign policy starts grace → after grace, reassigns

#[tokio::test]
async fn tick_with_reassign_policy_reassigns_after_grace_period() {
    // Use a very short grace period (50ms)
    let ctx = make_monitor(DisconnectPolicy::Reassign, 100, 50);

    register_stale_worker(&ctx.store, "w1", 200.0).await;
    // Use Assigned status so Assigned -> Pending is a valid transition
    create_assigned_task_with_status(
        &ctx.engine,
        &ctx.store,
        "t1",
        "w1",
        None,
        TaskStatus::Assigned,
    )
    .await;

    ctx.monitor.tick().await.unwrap();

    // Worker should be offline immediately
    let worker = ctx.store.get_worker("w1").await.unwrap().unwrap();
    assert_eq!(worker.status, WorkerStatus::Offline);

    // Task should still be assigned (grace period not elapsed yet)
    let task = ctx.engine.get_task("t1").await.unwrap().unwrap();
    assert_eq!(task.status, TaskStatus::Assigned);

    // Wait for grace period to expire
    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

    // After grace period, task should be back to pending
    let task = ctx.engine.get_task("t1").await.unwrap().unwrap();
    assert_eq!(task.status, TaskStatus::Pending);

    // Assignment should be released
    let assignments = ctx.worker_manager.get_worker_tasks("w1").await.unwrap();
    assert!(assignments.is_empty());
}

// ─── Test 6: Reassign grace — worker comes back during grace period ─────────

#[tokio::test]
async fn reassign_grace_cancelled_when_worker_comes_back() {
    // Use a slightly longer grace period
    let ctx = make_monitor(DisconnectPolicy::Reassign, 100, 100);

    register_stale_worker(&ctx.store, "w1", 200.0).await;
    // Use Assigned status so Assigned -> Pending is a valid transition
    create_assigned_task_with_status(
        &ctx.engine,
        &ctx.store,
        "t1",
        "w1",
        None,
        TaskStatus::Assigned,
    )
    .await;

    ctx.monitor.tick().await.unwrap();

    // Worker is offline
    let worker = ctx.store.get_worker("w1").await.unwrap().unwrap();
    assert_eq!(worker.status, WorkerStatus::Offline);

    // Simulate the worker reconnecting during the grace period
    let mut worker = ctx.store.get_worker("w1").await.unwrap().unwrap();
    worker.status = WorkerStatus::Idle;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as f64;
    worker.last_heartbeat_at = now;
    ctx.store.save_worker(worker).await.unwrap();

    // Wait for grace period to expire
    tokio::time::sleep(tokio::time::Duration::from_millis(150)).await;

    // Task should still be assigned — worker came back, so reassignment was cancelled
    let task = ctx.engine.get_task("t1").await.unwrap().unwrap();
    assert_eq!(task.status, TaskStatus::Assigned);
}

// ─── Test 7: Task-level disconnect_policy overrides default ─────────────────

#[tokio::test]
async fn task_level_disconnect_policy_overrides_default() {
    // Default is Reassign, but task has Fail
    let ctx = make_monitor(DisconnectPolicy::Reassign, 100, 30_000);

    register_stale_worker(&ctx.store, "w1", 200.0).await;
    create_assigned_task(
        &ctx.engine,
        &ctx.store,
        "t1",
        "w1",
        Some(DisconnectPolicy::Fail),
    )
    .await;

    ctx.monitor.tick().await.unwrap();

    // Task should be failed (task-level Fail overrides default Reassign)
    let task = ctx.engine.get_task("t1").await.unwrap().unwrap();
    assert_eq!(task.status, TaskStatus::Failed);
    assert!(task.error.is_some());
}

// ─── Test 8: Multiple tasks for same worker are all handled ─────────────────

#[tokio::test]
async fn multiple_tasks_for_timed_out_worker_are_all_failed() {
    let ctx = make_monitor(DisconnectPolicy::Fail, 100, 30_000);

    register_stale_worker(&ctx.store, "w1", 200.0).await;
    create_assigned_task(&ctx.engine, &ctx.store, "t1", "w1", None).await;
    create_assigned_task(&ctx.engine, &ctx.store, "t2", "w1", None).await;

    ctx.monitor.tick().await.unwrap();

    let task1 = ctx.engine.get_task("t1").await.unwrap().unwrap();
    let task2 = ctx.engine.get_task("t2").await.unwrap().unwrap();
    assert_eq!(task1.status, TaskStatus::Failed);
    assert_eq!(task2.status, TaskStatus::Failed);
}

// ─── Test 9: Offline workers are not re-checked ─────────────────────────────

#[tokio::test]
async fn offline_workers_are_not_checked() {
    let ctx = make_monitor(DisconnectPolicy::Fail, 100, 30_000);

    // Register a worker already offline
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as f64;
    let worker = Worker {
        id: "w1".to_string(),
        status: WorkerStatus::Offline,
        match_rule: WorkerMatchRule::default(),
        capacity: 5,
        used_slots: 0,
        weight: 50,
        connection_mode: ConnectionMode::Pull,
        connected_at: now - 200.0,
        last_heartbeat_at: now - 200.0,
        metadata: None,
    };
    ctx.store.save_worker(worker).await.unwrap();

    // Create a task assignment (to verify it's not touched)
    create_assigned_task(&ctx.engine, &ctx.store, "t1", "w1", None).await;

    ctx.monitor.tick().await.unwrap();

    // Task should remain running — offline workers are filtered out by WorkerFilter
    let task = ctx.engine.get_task("t1").await.unwrap().unwrap();
    assert_eq!(task.status, TaskStatus::Running);
}
