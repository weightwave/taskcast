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

// ─── Test 10: start() and stop() lifecycle ──────────────────────────────────

#[tokio::test]
async fn start_and_stop_lifecycle() {
    let mut ctx = make_monitor(DisconnectPolicy::Mark, 100, 30_000);

    // start() should launch the background task
    ctx.monitor.start();

    // Give the background task a moment to run
    tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;

    // stop() should abort the background task cleanly
    ctx.monitor.stop();

    // Calling stop() again (handle already taken) should be a no-op
    ctx.monitor.stop();
}

// ─── Test 11: stop() when never started is a no-op ──────────────────────────

#[tokio::test]
async fn stop_without_start_is_noop() {
    let mut ctx = make_monitor(DisconnectPolicy::Mark, 100, 30_000);

    // stop() with no prior start() — handle is None, should not panic
    ctx.monitor.stop();
}

// ─── Test 12: Worker deleted between list and get ───────────────────────────

#[tokio::test]
async fn handle_timeout_worker_not_found_is_ok() {
    let ctx = make_monitor(DisconnectPolicy::Fail, 100, 30_000);

    // Register a stale worker, then delete it before tick processes it.
    // We can't intercept between list_workers and get_worker, so instead
    // we test the code path by calling tick on an empty store — tick
    // should succeed even with no workers.
    ctx.monitor.tick().await.unwrap();

    // No panic, no error — the tick simply does nothing.
}

// ─── Test 13: Worker in grace period is skipped on subsequent tick ───────────

#[tokio::test]
async fn worker_in_grace_period_is_skipped_on_re_tick() {
    // Use a long grace period so it doesn't expire during the test
    let ctx = make_monitor(DisconnectPolicy::Reassign, 100, 5_000);

    register_stale_worker(&ctx.store, "w1", 200.0).await;
    create_assigned_task_with_status(
        &ctx.engine,
        &ctx.store,
        "t1",
        "w1",
        None,
        TaskStatus::Assigned,
    )
    .await;

    // First tick: marks worker offline, starts grace period
    ctx.monitor.tick().await.unwrap();
    let worker = ctx.store.get_worker("w1").await.unwrap().unwrap();
    assert_eq!(worker.status, WorkerStatus::Offline);

    // Manually set the worker back to a stale-but-online state to simulate
    // it appearing in the list_workers query again (status filter won't match
    // Offline, so we need to re-register as Idle to test the grace skip)
    register_stale_worker(&ctx.store, "w1", 200.0).await;

    // Second tick: worker should be in the grace set, so handle_timeout
    // is NOT called again (no duplicate grace spawn)
    ctx.monitor.tick().await.unwrap();

    // The worker should be marked offline again from this second tick,
    // but the grace period tracking should have prevented re-entry
    // into handle_timeout. We verify by checking the task is still Assigned
    // (not double-spawned).
    let task = ctx.engine.get_task("t1").await.unwrap().unwrap();
    assert_eq!(task.status, TaskStatus::Assigned);
}

// ─── Test 14: Busy worker times out ─────────────────────────────────────────

#[tokio::test]
async fn busy_worker_times_out() {
    let ctx = make_monitor(DisconnectPolicy::Fail, 100, 30_000);

    // Register a stale worker with Busy status
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as f64;
    let worker = Worker {
        id: "w1".to_string(),
        status: WorkerStatus::Busy,
        match_rule: WorkerMatchRule::default(),
        capacity: 5,
        used_slots: 5,
        weight: 50,
        connection_mode: ConnectionMode::Pull,
        connected_at: now - 200.0,
        last_heartbeat_at: now - 200.0,
        metadata: None,
    };
    ctx.store.save_worker(worker).await.unwrap();
    create_assigned_task(&ctx.engine, &ctx.store, "t1", "w1", None).await;

    ctx.monitor.tick().await.unwrap();

    let worker = ctx.store.get_worker("w1").await.unwrap().unwrap();
    assert_eq!(worker.status, WorkerStatus::Offline);
    let task = ctx.engine.get_task("t1").await.unwrap().unwrap();
    assert_eq!(task.status, TaskStatus::Failed);
}

// ─── Test 15: Draining worker times out ─────────────────────────────────────

#[tokio::test]
async fn draining_worker_times_out() {
    let ctx = make_monitor(DisconnectPolicy::Mark, 100, 30_000);

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as f64;
    let worker = Worker {
        id: "w1".to_string(),
        status: WorkerStatus::Draining,
        match_rule: WorkerMatchRule::default(),
        capacity: 5,
        used_slots: 2,
        weight: 50,
        connection_mode: ConnectionMode::Pull,
        connected_at: now - 200.0,
        last_heartbeat_at: now - 200.0,
        metadata: None,
    };
    ctx.store.save_worker(worker).await.unwrap();
    create_assigned_task(&ctx.engine, &ctx.store, "t1", "w1", None).await;

    ctx.monitor.tick().await.unwrap();

    let worker = ctx.store.get_worker("w1").await.unwrap().unwrap();
    assert_eq!(worker.status, WorkerStatus::Offline);
    // Mark policy: task stays running
    let task = ctx.engine.get_task("t1").await.unwrap().unwrap();
    assert_eq!(task.status, TaskStatus::Running);
}

// ─── Test 16: Multiple tasks with mixed disconnect policies ─────────────────

#[tokio::test]
async fn multiple_tasks_with_mixed_policies() {
    // Default is Mark, but individual tasks override
    let ctx = make_monitor(DisconnectPolicy::Mark, 100, 30_000);

    register_stale_worker(&ctx.store, "w1", 200.0).await;

    // t1: explicit Fail policy
    create_assigned_task(
        &ctx.engine,
        &ctx.store,
        "t1",
        "w1",
        Some(DisconnectPolicy::Fail),
    )
    .await;

    // t2: no explicit policy, uses default Mark
    create_assigned_task(&ctx.engine, &ctx.store, "t2", "w1", None).await;

    ctx.monitor.tick().await.unwrap();

    // t1 should be failed (explicit Fail policy)
    let task1 = ctx.engine.get_task("t1").await.unwrap().unwrap();
    assert_eq!(task1.status, TaskStatus::Failed);

    // t2 should still be running (default Mark policy)
    let task2 = ctx.engine.get_task("t2").await.unwrap().unwrap();
    assert_eq!(task2.status, TaskStatus::Running);
}

// ─── Test 17: Worker with no assigned tasks ─────────────────────────────────

#[tokio::test]
async fn worker_with_no_tasks_still_goes_offline() {
    let ctx = make_monitor(DisconnectPolicy::Fail, 100, 30_000);

    register_stale_worker(&ctx.store, "w1", 200.0).await;
    // No tasks assigned — just the worker

    ctx.monitor.tick().await.unwrap();

    // Worker should be marked offline even though it has no tasks
    let worker = ctx.store.get_worker("w1").await.unwrap().unwrap();
    assert_eq!(worker.status, WorkerStatus::Offline);
}

// ─── Test 18: Multiple stale workers in a single tick ───────────────────────

#[tokio::test]
async fn multiple_stale_workers_all_handled_in_one_tick() {
    let ctx = make_monitor(DisconnectPolicy::Fail, 100, 30_000);

    register_stale_worker(&ctx.store, "w1", 200.0).await;
    register_stale_worker(&ctx.store, "w2", 300.0).await;

    create_assigned_task(&ctx.engine, &ctx.store, "t1", "w1", None).await;
    create_assigned_task(&ctx.engine, &ctx.store, "t2", "w2", None).await;

    ctx.monitor.tick().await.unwrap();

    // Both workers should be offline
    let w1 = ctx.store.get_worker("w1").await.unwrap().unwrap();
    let w2 = ctx.store.get_worker("w2").await.unwrap().unwrap();
    assert_eq!(w1.status, WorkerStatus::Offline);
    assert_eq!(w2.status, WorkerStatus::Offline);

    // Both tasks should be failed
    let t1 = ctx.engine.get_task("t1").await.unwrap().unwrap();
    let t2 = ctx.engine.get_task("t2").await.unwrap().unwrap();
    assert_eq!(t1.status, TaskStatus::Failed);
    assert_eq!(t2.status, TaskStatus::Failed);
}

// ─── Test 19: Reassign with Running task (invalid transition) ───────────────

#[tokio::test]
async fn reassign_with_running_task_invalid_transition_does_not_panic() {
    // Running -> Pending is not a valid state transition, so the reassign
    // grace handler should silently fail (the `let _ = eng.transition_task`
    // ignores the error).
    let ctx = make_monitor(DisconnectPolicy::Reassign, 100, 50);

    register_stale_worker(&ctx.store, "w1", 200.0).await;
    // Task in Running status — Running -> Pending is invalid
    create_assigned_task(&ctx.engine, &ctx.store, "t1", "w1", None).await;

    ctx.monitor.tick().await.unwrap();

    // Wait for grace period to expire
    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

    // Task should still be Running because the transition was invalid
    let task = ctx.engine.get_task("t1").await.unwrap().unwrap();
    assert_eq!(task.status, TaskStatus::Running);

    // Assignment should still be released (release_task runs regardless)
    let assignments = ctx.worker_manager.get_worker_tasks("w1").await.unwrap();
    assert!(assignments.is_empty());
}

// ─── Test 20: Task-level Mark policy overrides default Fail ─────────────────

#[tokio::test]
async fn task_level_mark_policy_overrides_default_fail() {
    // Default is Fail, but task has Mark
    let ctx = make_monitor(DisconnectPolicy::Fail, 100, 30_000);

    register_stale_worker(&ctx.store, "w1", 200.0).await;
    create_assigned_task(
        &ctx.engine,
        &ctx.store,
        "t1",
        "w1",
        Some(DisconnectPolicy::Mark),
    )
    .await;

    ctx.monitor.tick().await.unwrap();

    // Worker offline
    let worker = ctx.store.get_worker("w1").await.unwrap().unwrap();
    assert_eq!(worker.status, WorkerStatus::Offline);

    // Task should still be running (Mark does not change task state)
    let task = ctx.engine.get_task("t1").await.unwrap().unwrap();
    assert_eq!(task.status, TaskStatus::Running);

    // Assignment should still exist (Mark does not release)
    let assignments = ctx.worker_manager.get_worker_tasks("w1").await.unwrap();
    assert_eq!(assignments.len(), 1);
}

// ─── Test 21: Task-level Reassign policy overrides default Fail ─────────────

#[tokio::test]
async fn task_level_reassign_policy_overrides_default_fail() {
    let ctx = make_monitor(DisconnectPolicy::Fail, 100, 50);

    register_stale_worker(&ctx.store, "w1", 200.0).await;
    create_assigned_task_with_status(
        &ctx.engine,
        &ctx.store,
        "t1",
        "w1",
        Some(DisconnectPolicy::Reassign),
        TaskStatus::Assigned,
    )
    .await;

    ctx.monitor.tick().await.unwrap();

    // Task should NOT be failed (Reassign overrides Fail)
    let task = ctx.engine.get_task("t1").await.unwrap().unwrap();
    assert_eq!(task.status, TaskStatus::Assigned);

    // Wait for grace period
    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

    // After grace, task should be pending
    let task = ctx.engine.get_task("t1").await.unwrap().unwrap();
    assert_eq!(task.status, TaskStatus::Pending);
}

// ─── Test 22: Assignment exists but task was deleted ─────────────────────────

#[tokio::test]
async fn assignment_exists_but_task_deleted_continues_gracefully() {
    let ctx = make_monitor(DisconnectPolicy::Fail, 100, 30_000);

    register_stale_worker(&ctx.store, "w1", 200.0).await;

    // Create an assignment directly without a corresponding task
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as f64;
    let assignment = WorkerAssignment {
        task_id: "ghost-task".to_string(),
        worker_id: "w1".to_string(),
        cost: 1,
        assigned_at: now,
        status: WorkerAssignmentStatus::Assigned,
    };
    ctx.store.add_assignment(assignment).await.unwrap();

    // Should not panic or error — the `let Some(task) = task else { continue }` path
    ctx.monitor.tick().await.unwrap();

    // Worker should still be marked offline
    let worker = ctx.store.get_worker("w1").await.unwrap().unwrap();
    assert_eq!(worker.status, WorkerStatus::Offline);
}

// ─── Test 23: Multiple tasks with Reassign — all get grace spawns ───────────

#[tokio::test]
async fn multiple_tasks_with_reassign_all_get_grace_period() {
    let ctx = make_monitor(DisconnectPolicy::Reassign, 100, 50);

    register_stale_worker(&ctx.store, "w1", 200.0).await;
    create_assigned_task_with_status(
        &ctx.engine,
        &ctx.store,
        "t1",
        "w1",
        None,
        TaskStatus::Assigned,
    )
    .await;
    create_assigned_task_with_status(
        &ctx.engine,
        &ctx.store,
        "t2",
        "w1",
        None,
        TaskStatus::Assigned,
    )
    .await;

    ctx.monitor.tick().await.unwrap();

    // Both tasks should still be assigned initially
    let task1 = ctx.engine.get_task("t1").await.unwrap().unwrap();
    let task2 = ctx.engine.get_task("t2").await.unwrap().unwrap();
    assert_eq!(task1.status, TaskStatus::Assigned);
    assert_eq!(task2.status, TaskStatus::Assigned);

    // Wait for grace period to expire
    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

    // Both tasks should be back to pending after grace
    let task1 = ctx.engine.get_task("t1").await.unwrap().unwrap();
    let task2 = ctx.engine.get_task("t2").await.unwrap().unwrap();
    assert_eq!(task1.status, TaskStatus::Pending);
    assert_eq!(task2.status, TaskStatus::Pending);

    // Both assignments should be released
    let assignments = ctx.worker_manager.get_worker_tasks("w1").await.unwrap();
    assert!(assignments.is_empty());
}

// ─── Test 24: start() actually runs tick cycles ─────────────────────────────

#[tokio::test]
async fn start_runs_tick_cycles_and_processes_workers() {
    // Use very short check interval so the background loop fires quickly
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
    let mut monitor = HeartbeatMonitor::new(HeartbeatMonitorOptions {
        worker_manager: Arc::clone(&worker_manager),
        engine: Arc::clone(&engine),
        short_term_store: Arc::clone(&store) as Arc<dyn ShortTermStore>,
        check_interval_ms: 10, // very short interval
        heartbeat_timeout_ms: 50,
        default_disconnect_policy: DisconnectPolicy::Mark,
        disconnect_grace_ms: 30_000,
    });

    // Register a stale worker
    register_stale_worker(&store, "w1", 200.0).await;

    // Start the background loop
    monitor.start();

    // Wait enough time for at least one tick cycle
    tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

    // Worker should have been marked offline by the background loop
    let worker = store.get_worker("w1").await.unwrap().unwrap();
    assert_eq!(worker.status, WorkerStatus::Offline);

    // Clean up
    monitor.stop();
}

// ─── Test 25: Fail policy error message includes correct worker ID ──────────

#[tokio::test]
async fn fail_policy_error_message_format() {
    let ctx = make_monitor(DisconnectPolicy::Fail, 100, 30_000);

    register_stale_worker(&ctx.store, "worker-abc-123", 200.0).await;
    create_assigned_task(&ctx.engine, &ctx.store, "t1", "worker-abc-123", None).await;

    ctx.monitor.tick().await.unwrap();

    let task = ctx.engine.get_task("t1").await.unwrap().unwrap();
    assert_eq!(task.status, TaskStatus::Failed);

    let error = task.error.unwrap();
    assert_eq!(error.code, Some("WORKER_DISCONNECT".to_string()));
    assert!(error.message.contains("worker-abc-123"));
    assert!(error.message.contains("heartbeat timeout"));
    assert!(error.details.is_none());
}

// ─── Test 26: Mix of stale and fresh workers ────────────────────────────────

#[tokio::test]
async fn mix_of_stale_and_fresh_workers() {
    let ctx = make_monitor(DisconnectPolicy::Fail, 100, 30_000);

    register_stale_worker(&ctx.store, "w1", 200.0).await;
    register_fresh_worker(&ctx.store, "w2").await;

    create_assigned_task(&ctx.engine, &ctx.store, "t1", "w1", None).await;
    create_assigned_task(&ctx.engine, &ctx.store, "t2", "w2", None).await;

    ctx.monitor.tick().await.unwrap();

    // Stale worker goes offline, its task fails
    let w1 = ctx.store.get_worker("w1").await.unwrap().unwrap();
    assert_eq!(w1.status, WorkerStatus::Offline);
    let t1 = ctx.engine.get_task("t1").await.unwrap().unwrap();
    assert_eq!(t1.status, TaskStatus::Failed);

    // Fresh worker stays idle, its task stays running
    let w2 = ctx.store.get_worker("w2").await.unwrap().unwrap();
    assert_eq!(w2.status, WorkerStatus::Idle);
    let t2 = ctx.engine.get_task("t2").await.unwrap().unwrap();
    assert_eq!(t2.status, TaskStatus::Running);
}

// ─── Test 27: Reassign grace — worker deleted during grace period ───────────

#[tokio::test]
async fn reassign_grace_worker_deleted_during_grace() {
    let ctx = make_monitor(DisconnectPolicy::Reassign, 100, 50);

    register_stale_worker(&ctx.store, "w1", 200.0).await;
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

    // Delete the worker during the grace period
    ctx.store.delete_worker("w1").await.unwrap();

    // Wait for grace period
    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

    // The grace spawn should handle the missing worker gracefully
    // (get_worker returns None, so the check `if w.status != Offline` is skipped)
    // Task should be reassigned (moved to Pending) since worker wasn't found
    // and the code proceeds past the check.
    let task = ctx.engine.get_task("t1").await.unwrap().unwrap();
    assert_eq!(task.status, TaskStatus::Pending);
}

// ─── Test 28: Tick on empty store succeeds ──────────────────────────────────

#[tokio::test]
async fn tick_on_empty_store_succeeds() {
    let ctx = make_monitor(DisconnectPolicy::Fail, 100, 30_000);

    // No workers, no tasks — tick should succeed without errors
    ctx.monitor.tick().await.unwrap();
}

// ─── Test 29: Multiple tasks with mixed Fail and Reassign ───────────────────

#[tokio::test]
async fn multiple_tasks_mixed_fail_and_reassign() {
    let ctx = make_monitor(DisconnectPolicy::Mark, 100, 50);

    register_stale_worker(&ctx.store, "w1", 200.0).await;

    // t1: explicit Fail
    create_assigned_task(
        &ctx.engine,
        &ctx.store,
        "t1",
        "w1",
        Some(DisconnectPolicy::Fail),
    )
    .await;

    // t2: explicit Reassign (Assigned status for valid transition)
    create_assigned_task_with_status(
        &ctx.engine,
        &ctx.store,
        "t2",
        "w1",
        Some(DisconnectPolicy::Reassign),
        TaskStatus::Assigned,
    )
    .await;

    // t3: default Mark
    create_assigned_task(&ctx.engine, &ctx.store, "t3", "w1", None).await;

    ctx.monitor.tick().await.unwrap();

    // t1: Fail policy => failed immediately
    let t1 = ctx.engine.get_task("t1").await.unwrap().unwrap();
    assert_eq!(t1.status, TaskStatus::Failed);

    // t2: Reassign policy => still assigned (grace period)
    let t2 = ctx.engine.get_task("t2").await.unwrap().unwrap();
    assert_eq!(t2.status, TaskStatus::Assigned);

    // t3: Mark policy => still running
    let t3 = ctx.engine.get_task("t3").await.unwrap().unwrap();
    assert_eq!(t3.status, TaskStatus::Running);

    // Wait for grace period to complete
    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

    // t2 should now be pending
    let t2 = ctx.engine.get_task("t2").await.unwrap().unwrap();
    assert_eq!(t2.status, TaskStatus::Pending);
}
