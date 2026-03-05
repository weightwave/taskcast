use std::sync::Arc;

use taskcast_core::{
    BroadcastProvider, CreateTaskInput, MemoryBroadcastProvider, MemoryShortTermStore,
    ShortTermStore, TaskEngine, TaskEngineOptions, TaskScheduler, TaskSchedulerOptions, TaskStatus,
    TransitionPayload,
};

// ─── Helpers ────────────────────────────────────────────────────────────────

#[allow(dead_code)]
struct TestContext {
    engine: Arc<TaskEngine>,
    store: Arc<MemoryShortTermStore>,
    scheduler: TaskScheduler,
}

fn make_scheduler(
    paused_cold_after_ms: Option<u64>,
    blocked_cold_after_ms: Option<u64>,
) -> TestContext {
    let store = Arc::new(MemoryShortTermStore::new());
    let broadcast = Arc::new(MemoryBroadcastProvider::new());
    let engine = Arc::new(TaskEngine::new(TaskEngineOptions {
        short_term_store: Arc::clone(&store) as Arc<dyn ShortTermStore>,
        broadcast: Arc::clone(&broadcast) as Arc<dyn BroadcastProvider>,
        long_term_store: None,
        hooks: None,
    }));
    let scheduler = TaskScheduler::new(TaskSchedulerOptions {
        engine: Arc::clone(&engine),
        short_term_store: Arc::clone(&store) as Arc<dyn ShortTermStore>,
        check_interval_ms: 60_000,
        paused_cold_after_ms,
        blocked_cold_after_ms,
    });
    TestContext {
        engine,
        store,
        scheduler,
    }
}

/// Create a task and move it to Running status.
async fn create_running_task(engine: &TaskEngine, task_id: &str) {
    engine
        .create_task(CreateTaskInput {
            id: Some(task_id.to_string()),
            ttl: Some(3600),
            ..Default::default()
        })
        .await
        .unwrap();
    engine
        .transition_task(task_id, TaskStatus::Running, None)
        .await
        .unwrap();
}

/// Create a running task and block it with a specific resume_at time.
async fn create_blocked_task_with_resume(engine: &TaskEngine, task_id: &str, resume_after_ms: f64) {
    create_running_task(engine, task_id).await;
    engine
        .transition_task(
            task_id,
            TaskStatus::Blocked,
            Some(TransitionPayload {
                reason: Some("waiting".to_string()),
                resume_after_ms: Some(resume_after_ms),
                ..Default::default()
            }),
        )
        .await
        .unwrap();
}

// ─── Test 1: tick() auto-resumes blocked tasks with expired resume_at ───────

#[tokio::test]
async fn tick_resumes_blocked_task_with_expired_resume_at() {
    let ctx = make_scheduler(None, None);

    // Create a blocked task with resume_after_ms = 0 (expires immediately)
    create_blocked_task_with_resume(&ctx.engine, "t1", 0.0).await;

    // Verify task is blocked
    let task = ctx.engine.get_task("t1").await.unwrap().unwrap();
    assert_eq!(task.status, TaskStatus::Blocked);

    // Wait a tiny bit to ensure the resume_at is in the past
    tokio::time::sleep(tokio::time::Duration::from_millis(5)).await;

    // Run scheduler tick
    ctx.scheduler.tick().await.unwrap();

    // Task should now be running
    let task = ctx.engine.get_task("t1").await.unwrap().unwrap();
    assert_eq!(task.status, TaskStatus::Running);
}

// ─── Test 2: tick() does not resume blocked tasks with future resume_at ─────

#[tokio::test]
async fn tick_does_not_resume_blocked_task_with_future_resume_at() {
    let ctx = make_scheduler(None, None);

    // Create a blocked task with resume_after_ms far in the future
    create_blocked_task_with_resume(&ctx.engine, "t1", 999_999_999.0).await;

    let task = ctx.engine.get_task("t1").await.unwrap().unwrap();
    assert_eq!(task.status, TaskStatus::Blocked);
    assert!(task.resume_at.is_some());

    // Run scheduler tick
    ctx.scheduler.tick().await.unwrap();

    // Task should still be blocked
    let task = ctx.engine.get_task("t1").await.unwrap().unwrap();
    assert_eq!(task.status, TaskStatus::Blocked);
}

// ─── Test 3: tick() does not resume blocked tasks without resume_at ─────────

#[tokio::test]
async fn tick_does_not_resume_blocked_task_without_resume_at() {
    let ctx = make_scheduler(None, None);

    // Create a blocked task without resume_after_ms
    create_running_task(&ctx.engine, "t1").await;
    ctx.engine
        .transition_task(
            "t1",
            TaskStatus::Blocked,
            Some(TransitionPayload {
                reason: Some("waiting".to_string()),
                ..Default::default()
            }),
        )
        .await
        .unwrap();

    let task = ctx.engine.get_task("t1").await.unwrap().unwrap();
    assert_eq!(task.status, TaskStatus::Blocked);
    assert!(task.resume_at.is_none());

    // Run scheduler tick
    ctx.scheduler.tick().await.unwrap();

    // Task should still be blocked
    let task = ctx.engine.get_task("t1").await.unwrap().unwrap();
    assert_eq!(task.status, TaskStatus::Blocked);
}

// ─── Test 4: tick() handles transition errors gracefully ────────────────────

#[tokio::test]
async fn tick_handles_transition_errors_gracefully() {
    let ctx = make_scheduler(None, None);

    // Create a blocked task with expired resume_at
    create_blocked_task_with_resume(&ctx.engine, "t1", 0.0).await;

    // Wait for resume_at to pass
    tokio::time::sleep(tokio::time::Duration::from_millis(5)).await;

    // Manually transition the task to a terminal state, so the scheduler's
    // attempt to transition it to Running will fail.
    // Blocked -> Failed is a valid transition.
    ctx.engine
        .transition_task("t1", TaskStatus::Failed, None)
        .await
        .unwrap();

    let task = ctx.engine.get_task("t1").await.unwrap().unwrap();
    assert_eq!(task.status, TaskStatus::Failed);

    // tick() should not panic or return error — it silently ignores
    // transition failures for individual tasks
    let result = ctx.scheduler.tick().await;
    assert!(result.is_ok());

    // Task should remain failed
    let task = ctx.engine.get_task("t1").await.unwrap().unwrap();
    assert_eq!(task.status, TaskStatus::Failed);
}

// ─── Test 5: Cold demotion emits taskcast:cold for old paused tasks ─────────

#[tokio::test]
async fn cold_demotion_emits_cold_event_for_old_paused_task() {
    // Set paused_cold_after_ms to 0 so any paused task is considered "cold"
    let ctx = make_scheduler(Some(0), None);

    // Create a task and pause it
    create_running_task(&ctx.engine, "t1").await;
    ctx.engine
        .transition_task(
            "t1",
            TaskStatus::Paused,
            Some(TransitionPayload {
                reason: Some("user pause".to_string()),
                ..Default::default()
            }),
        )
        .await
        .unwrap();

    // Wait a tiny bit so now - updated_at >= 0
    tokio::time::sleep(tokio::time::Duration::from_millis(5)).await;

    // Run scheduler tick
    ctx.scheduler.tick().await.unwrap();

    // The task should still be paused (cold demotion only emits an event)
    let task = ctx.engine.get_task("t1").await.unwrap().unwrap();
    assert_eq!(task.status, TaskStatus::Paused);

    // Check that a taskcast:cold event was emitted
    let events = ctx.engine.get_events("t1", None).await.unwrap();
    let cold_events: Vec<_> = events.iter().filter(|e| e.r#type == "taskcast:cold").collect();
    assert!(
        !cold_events.is_empty(),
        "Expected at least one taskcast:cold event, got none. Events: {:?}",
        events.iter().map(|e| &e.r#type).collect::<Vec<_>>()
    );
}

// ─── Test 6: Cold demotion does not emit when thresholds are None ───────────

#[tokio::test]
async fn cold_demotion_does_nothing_when_thresholds_are_none() {
    let ctx = make_scheduler(None, None);

    // Create a task and pause it
    create_running_task(&ctx.engine, "t1").await;
    ctx.engine
        .transition_task(
            "t1",
            TaskStatus::Paused,
            Some(TransitionPayload {
                reason: Some("user pause".to_string()),
                ..Default::default()
            }),
        )
        .await
        .unwrap();

    tokio::time::sleep(tokio::time::Duration::from_millis(5)).await;

    ctx.scheduler.tick().await.unwrap();

    // No taskcast:cold event should exist
    let events = ctx.engine.get_events("t1", None).await.unwrap();
    let cold_events: Vec<_> = events.iter().filter(|e| e.r#type == "taskcast:cold").collect();
    assert!(
        cold_events.is_empty(),
        "Expected no taskcast:cold events, got {}",
        cold_events.len()
    );
}

// ─── Test 7: Cold demotion for blocked tasks ────────────────────────────────

#[tokio::test]
async fn cold_demotion_emits_cold_event_for_old_blocked_task() {
    // Set blocked_cold_after_ms to 0
    let ctx = make_scheduler(None, Some(0));

    // Create a blocked task (no resume timer — it stays blocked)
    create_running_task(&ctx.engine, "t1").await;
    ctx.engine
        .transition_task(
            "t1",
            TaskStatus::Blocked,
            Some(TransitionPayload {
                reason: Some("waiting for input".to_string()),
                ..Default::default()
            }),
        )
        .await
        .unwrap();

    tokio::time::sleep(tokio::time::Duration::from_millis(5)).await;

    ctx.scheduler.tick().await.unwrap();

    let events = ctx.engine.get_events("t1", None).await.unwrap();
    let cold_events: Vec<_> = events.iter().filter(|e| e.r#type == "taskcast:cold").collect();
    assert!(
        !cold_events.is_empty(),
        "Expected at least one taskcast:cold event for blocked task"
    );
}

// ─── Test 8: Cold demotion does not emit for recent tasks ───────────────────

#[tokio::test]
async fn cold_demotion_does_not_emit_for_recent_paused_task() {
    // Threshold is very high — task was just paused, so it should not be cold
    let ctx = make_scheduler(Some(999_999_999), None);

    create_running_task(&ctx.engine, "t1").await;
    ctx.engine
        .transition_task(
            "t1",
            TaskStatus::Paused,
            Some(TransitionPayload {
                reason: Some("user pause".to_string()),
                ..Default::default()
            }),
        )
        .await
        .unwrap();

    ctx.scheduler.tick().await.unwrap();

    let events = ctx.engine.get_events("t1", None).await.unwrap();
    let cold_events: Vec<_> = events.iter().filter(|e| e.r#type == "taskcast:cold").collect();
    assert!(
        cold_events.is_empty(),
        "Expected no taskcast:cold events for recently paused task"
    );
}
