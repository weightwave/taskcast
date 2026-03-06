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

// ─── Test 9: stop() aborts a running scheduler ──────────────────────────────

#[tokio::test]
async fn stop_aborts_running_scheduler() {
    let store = Arc::new(MemoryShortTermStore::new());
    let broadcast = Arc::new(MemoryBroadcastProvider::new());
    let engine = Arc::new(TaskEngine::new(TaskEngineOptions {
        short_term_store: Arc::clone(&store) as Arc<dyn ShortTermStore>,
        broadcast: Arc::clone(&broadcast) as Arc<dyn BroadcastProvider>,
        long_term_store: None,
        hooks: None,
    }));
    let mut scheduler = TaskScheduler::new(TaskSchedulerOptions {
        engine: Arc::clone(&engine),
        short_term_store: Arc::clone(&store) as Arc<dyn ShortTermStore>,
        check_interval_ms: 60_000,
        paused_cold_after_ms: None,
        blocked_cold_after_ms: None,
    });

    // Start the scheduler (spawns a background task)
    scheduler.start();

    // Give the background task a moment to begin
    tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;

    // Stop should abort the background task without panicking
    scheduler.stop();

    // Calling stop again when there is no handle should be a no-op
    scheduler.stop();
}

// ─── Test 10: stop() is a no-op when scheduler was never started ────────────

#[tokio::test]
async fn stop_is_noop_when_never_started() {
    let store = Arc::new(MemoryShortTermStore::new());
    let broadcast = Arc::new(MemoryBroadcastProvider::new());
    let engine = Arc::new(TaskEngine::new(TaskEngineOptions {
        short_term_store: Arc::clone(&store) as Arc<dyn ShortTermStore>,
        broadcast: Arc::clone(&broadcast) as Arc<dyn BroadcastProvider>,
        long_term_store: None,
        hooks: None,
    }));
    let mut scheduler = TaskScheduler::new(TaskSchedulerOptions {
        engine,
        short_term_store: Arc::clone(&store) as Arc<dyn ShortTermStore>,
        check_interval_ms: 60_000,
        paused_cold_after_ms: None,
        blocked_cold_after_ms: None,
    });

    // Calling stop without start should not panic
    scheduler.stop();
}

// ─── Test 11: start() runs tick automatically ───────────────────────────────

#[tokio::test]
async fn start_runs_tick_automatically() {
    let store = Arc::new(MemoryShortTermStore::new());
    let broadcast = Arc::new(MemoryBroadcastProvider::new());
    let engine = Arc::new(TaskEngine::new(TaskEngineOptions {
        short_term_store: Arc::clone(&store) as Arc<dyn ShortTermStore>,
        broadcast: Arc::clone(&broadcast) as Arc<dyn BroadcastProvider>,
        long_term_store: None,
        hooks: None,
    }));

    // Create a blocked task with resume_after_ms = 0 (expires immediately)
    create_blocked_task_with_resume(&engine, "t-auto", 0.0).await;

    let mut scheduler = TaskScheduler::new(TaskSchedulerOptions {
        engine: Arc::clone(&engine),
        short_term_store: Arc::clone(&store) as Arc<dyn ShortTermStore>,
        // Very short interval so the tick fires quickly
        check_interval_ms: 10,
        paused_cold_after_ms: None,
        blocked_cold_after_ms: None,
    });

    scheduler.start();

    // Wait enough for at least one tick to fire
    tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

    // The scheduler's tick should have auto-resumed the task
    let task = engine.get_task("t-auto").await.unwrap().unwrap();
    assert_eq!(task.status, TaskStatus::Running);

    scheduler.stop();
}

// ─── Test 12: Cold demotion with both paused and blocked thresholds ─────────

#[tokio::test]
async fn cold_demotion_both_thresholds_with_mixed_tasks() {
    // Both thresholds set to 0 — all suspended tasks are immediately cold
    let ctx = make_scheduler(Some(0), Some(0));

    // Create a paused task
    create_running_task(&ctx.engine, "t-paused").await;
    ctx.engine
        .transition_task(
            "t-paused",
            TaskStatus::Paused,
            Some(TransitionPayload {
                reason: Some("user pause".to_string()),
                ..Default::default()
            }),
        )
        .await
        .unwrap();

    // Create a blocked task (no resume timer)
    create_running_task(&ctx.engine, "t-blocked").await;
    ctx.engine
        .transition_task(
            "t-blocked",
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

    // Both tasks should have received a taskcast:cold event
    let paused_events = ctx.engine.get_events("t-paused", None).await.unwrap();
    let paused_cold: Vec<_> = paused_events
        .iter()
        .filter(|e| e.r#type == "taskcast:cold")
        .collect();
    assert!(
        !paused_cold.is_empty(),
        "Expected taskcast:cold event for paused task"
    );

    let blocked_events = ctx.engine.get_events("t-blocked", None).await.unwrap();
    let blocked_cold: Vec<_> = blocked_events
        .iter()
        .filter(|e| e.r#type == "taskcast:cold")
        .collect();
    assert!(
        !blocked_cold.is_empty(),
        "Expected taskcast:cold event for blocked task"
    );
}

// ─── Test 13: Cold demotion does not emit for recent blocked task ───────────

#[tokio::test]
async fn cold_demotion_does_not_emit_for_recent_blocked_task() {
    // blocked_cold threshold is very high — recently blocked task is not cold
    let ctx = make_scheduler(None, Some(999_999_999));

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

    ctx.scheduler.tick().await.unwrap();

    let events = ctx.engine.get_events("t1", None).await.unwrap();
    let cold_events: Vec<_> = events.iter().filter(|e| e.r#type == "taskcast:cold").collect();
    assert!(
        cold_events.is_empty(),
        "Expected no taskcast:cold events for recently blocked task"
    );
}

// ─── Test 14: Multiple blocked tasks resumed in a single tick ───────────────

#[tokio::test]
async fn tick_resumes_multiple_blocked_tasks() {
    let ctx = make_scheduler(None, None);

    // Create several blocked tasks with expired resume_at
    for i in 0..5 {
        let id = format!("t-multi-{}", i);
        create_blocked_task_with_resume(&ctx.engine, &id, 0.0).await;
    }

    tokio::time::sleep(tokio::time::Duration::from_millis(5)).await;

    ctx.scheduler.tick().await.unwrap();

    // All tasks should now be running
    for i in 0..5 {
        let id = format!("t-multi-{}", i);
        let task = ctx.engine.get_task(&id).await.unwrap().unwrap();
        assert_eq!(
            task.status,
            TaskStatus::Running,
            "Task {} should be running after tick",
            id
        );
    }
}

// ─── Test 15: Only paused_cold set, blocked task is not affected ────────────

#[tokio::test]
async fn cold_demotion_only_paused_threshold_ignores_blocked() {
    // Only paused_cold is set; blocked_cold is None
    let ctx = make_scheduler(Some(0), None);

    // Create a blocked task (should NOT get cold event since blocked_cold is None)
    create_running_task(&ctx.engine, "t-blocked").await;
    ctx.engine
        .transition_task(
            "t-blocked",
            TaskStatus::Blocked,
            Some(TransitionPayload {
                reason: Some("waiting".to_string()),
                ..Default::default()
            }),
        )
        .await
        .unwrap();

    tokio::time::sleep(tokio::time::Duration::from_millis(5)).await;

    ctx.scheduler.tick().await.unwrap();

    let events = ctx.engine.get_events("t-blocked", None).await.unwrap();
    let cold_events: Vec<_> = events.iter().filter(|e| e.r#type == "taskcast:cold").collect();
    assert!(
        cold_events.is_empty(),
        "Expected no taskcast:cold events for blocked task when only paused_cold is set"
    );
}

// ─── Test 16: Only blocked_cold set, paused task is not affected ────────────

#[tokio::test]
async fn cold_demotion_only_blocked_threshold_ignores_paused() {
    // Only blocked_cold is set; paused_cold is None
    let ctx = make_scheduler(None, Some(0));

    // Create a paused task (should NOT get cold event since paused_cold is None)
    create_running_task(&ctx.engine, "t-paused").await;
    ctx.engine
        .transition_task(
            "t-paused",
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

    let events = ctx.engine.get_events("t-paused", None).await.unwrap();
    let cold_events: Vec<_> = events.iter().filter(|e| e.r#type == "taskcast:cold").collect();
    assert!(
        cold_events.is_empty(),
        "Expected no taskcast:cold events for paused task when only blocked_cold is set"
    );
}

// ─── Test 17: Tick with no tasks is a no-op ─────────────────────────────────

#[tokio::test]
async fn tick_with_no_tasks_is_noop() {
    let ctx = make_scheduler(Some(0), Some(0));

    // tick on an empty store should succeed without error
    let result = ctx.scheduler.tick().await;
    assert!(result.is_ok());
}

// ─── Test 18: Cold demotion with mixed thresholds — selective emit ──────────

#[tokio::test]
async fn cold_demotion_selective_with_different_thresholds() {
    // paused_cold = 0 (immediate), blocked_cold = very high (never triggers)
    let ctx = make_scheduler(Some(0), Some(999_999_999));

    // Create a paused task (should get cold event)
    create_running_task(&ctx.engine, "t-paused").await;
    ctx.engine
        .transition_task(
            "t-paused",
            TaskStatus::Paused,
            Some(TransitionPayload {
                reason: Some("pause".to_string()),
                ..Default::default()
            }),
        )
        .await
        .unwrap();

    // Create a blocked task (should NOT get cold event — threshold too high)
    create_running_task(&ctx.engine, "t-blocked").await;
    ctx.engine
        .transition_task(
            "t-blocked",
            TaskStatus::Blocked,
            Some(TransitionPayload {
                reason: Some("waiting".to_string()),
                ..Default::default()
            }),
        )
        .await
        .unwrap();

    tokio::time::sleep(tokio::time::Duration::from_millis(5)).await;

    ctx.scheduler.tick().await.unwrap();

    // Paused task should have cold event
    let paused_events = ctx.engine.get_events("t-paused", None).await.unwrap();
    let paused_cold: Vec<_> = paused_events
        .iter()
        .filter(|e| e.r#type == "taskcast:cold")
        .collect();
    assert!(
        !paused_cold.is_empty(),
        "Expected taskcast:cold event for paused task with threshold 0"
    );

    // Blocked task should NOT have cold event
    let blocked_events = ctx.engine.get_events("t-blocked", None).await.unwrap();
    let blocked_cold: Vec<_> = blocked_events
        .iter()
        .filter(|e| e.r#type == "taskcast:cold")
        .collect();
    assert!(
        blocked_cold.is_empty(),
        "Expected no taskcast:cold events for blocked task with high threshold"
    );
}

// ─── Test 19: Blocked task with resume_at AND cold demotion ─────────────────

#[tokio::test]
async fn blocked_task_with_expired_resume_and_cold_demotion() {
    // Both wake-up and cold demotion configured
    let ctx = make_scheduler(None, Some(0));

    // Create a blocked task with resume_after_ms = 0 (expires immediately)
    create_blocked_task_with_resume(&ctx.engine, "t1", 0.0).await;

    tokio::time::sleep(tokio::time::Duration::from_millis(5)).await;

    ctx.scheduler.tick().await.unwrap();

    // The task should be resumed to Running (wake-up runs before cold demotion)
    let task = ctx.engine.get_task("t1").await.unwrap().unwrap();
    assert_eq!(task.status, TaskStatus::Running);
}

// ─── Test 20: start() then stop() then tick() still works ───────────────────

#[tokio::test]
async fn tick_works_after_start_and_stop() {
    let store = Arc::new(MemoryShortTermStore::new());
    let broadcast = Arc::new(MemoryBroadcastProvider::new());
    let engine = Arc::new(TaskEngine::new(TaskEngineOptions {
        short_term_store: Arc::clone(&store) as Arc<dyn ShortTermStore>,
        broadcast: Arc::clone(&broadcast) as Arc<dyn BroadcastProvider>,
        long_term_store: None,
        hooks: None,
    }));
    let mut scheduler = TaskScheduler::new(TaskSchedulerOptions {
        engine: Arc::clone(&engine),
        short_term_store: Arc::clone(&store) as Arc<dyn ShortTermStore>,
        check_interval_ms: 60_000,
        paused_cold_after_ms: None,
        blocked_cold_after_ms: None,
    });

    // Start and immediately stop
    scheduler.start();
    scheduler.stop();

    // Create a blocked task with expired resume_at
    create_blocked_task_with_resume(&engine, "t-after-stop", 0.0).await;
    tokio::time::sleep(tokio::time::Duration::from_millis(5)).await;

    // Manual tick should still work after stop
    let result = scheduler.tick().await;
    assert!(result.is_ok());

    let task = engine.get_task("t-after-stop").await.unwrap().unwrap();
    assert_eq!(task.status, TaskStatus::Running);
}
