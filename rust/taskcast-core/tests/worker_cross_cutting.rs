//! Cross-cutting integration tests between the worker system, task state machine,
//! TTL, and events in the Rust implementation.
//!
//! These tests verify behavior at the intersection of multiple subsystems:
//! - State machine transitions involving `Assigned` status
//! - TTL behavior during worker assignment
//! - Event publishing and ordering during worker lifecycle
//! - Worker capacity tracking across state transitions

use std::sync::Arc;

use taskcast_core::{
    BroadcastProvider, ClaimResult, ConnectionMode, CreateTaskInput, Level,
    MemoryBroadcastProvider, MemoryShortTermStore, PublishEventInput, ShortTermStore, TaskEngine,
    TaskEngineOptions, TaskStatus, WorkerManager, WorkerManagerOptions, WorkerMatchRule,
    WorkerRegistration, WorkerStatus,
};

// ─── Test Helpers ───────────────────────────────────────────────────────────

struct TestContext {
    manager: Arc<WorkerManager>,
    engine: Arc<TaskEngine>,
}

fn make_context() -> TestContext {
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
    TestContext { manager, engine }
}

fn make_registration(id: &str, capacity: u32) -> WorkerRegistration {
    WorkerRegistration {
        worker_id: Some(id.to_string()),
        match_rule: WorkerMatchRule::default(),
        capacity,
        weight: None,
        connection_mode: ConnectionMode::Pull,
        metadata: None,
    }
}

// =============================================================================
// State Machine + Worker Lifecycle
// =============================================================================

/// 1. assigned -> cancelled: admin cancels an assigned task.
///    Verifies the state transition succeeds and worker capacity is restored
///    after cancellation (via decline, since cancellation doesn't auto-release).
#[tokio::test]
async fn assigned_to_cancelled_restores_worker_capacity() {
    let ctx = make_context();

    ctx.manager
        .register_worker(make_registration("w1", 5))
        .await
        .unwrap();

    ctx.engine
        .create_task(CreateTaskInput {
            id: Some("t1".to_string()),
            ..Default::default()
        })
        .await
        .unwrap();

    // Claim the task (pending -> assigned)
    let result = ctx.manager.claim_task("t1", "w1").await.unwrap();
    assert_eq!(result, ClaimResult::Claimed);

    let worker = ctx.manager.get_worker("w1").await.unwrap().unwrap();
    assert_eq!(worker.used_slots, 1, "Worker should have 1 used slot after claim");

    // Admin cancels the assigned task (assigned -> cancelled)
    let task = ctx
        .engine
        .transition_task("t1", TaskStatus::Cancelled, None)
        .await
        .unwrap();
    assert_eq!(task.status, TaskStatus::Cancelled);

    // NOTE: The engine transition_task does NOT automatically release worker slots.
    // In the real system, the server layer or a hook would call decline_task or
    // release the slot. Here we verify the raw engine behavior: the worker still
    // holds the slot after a direct engine transition.
    let worker = ctx.manager.get_worker("w1").await.unwrap().unwrap();
    assert_eq!(
        worker.used_slots, 1,
        "Engine-level cancellation does not auto-release worker slots; \
         the orchestration layer must handle this"
    );

    // When decline_task is called (simulating the orchestration layer cleanup),
    // it should be a no-op or gracefully handle the already-cancelled task.
    // The assignment record still exists, so decline should release the slot.
    ctx.manager.decline_task("t1", "w1", None).await.unwrap();

    let worker = ctx.manager.get_worker("w1").await.unwrap().unwrap();
    assert_eq!(
        worker.used_slots, 0,
        "Worker capacity should be restored after decline on cancelled task"
    );
    assert_eq!(worker.status, WorkerStatus::Idle);
}

/// 2. assigned -> running -> completed: full worker lifecycle.
///    Task is claimed by a worker, transitions to running, then completes.
#[tokio::test]
async fn assigned_to_running_to_completed_full_lifecycle() {
    let ctx = make_context();

    ctx.manager
        .register_worker(make_registration("w1", 5))
        .await
        .unwrap();

    ctx.engine
        .create_task(CreateTaskInput {
            id: Some("t1".to_string()),
            ..Default::default()
        })
        .await
        .unwrap();

    // Step 1: Claim (pending -> assigned)
    let result = ctx.manager.claim_task("t1", "w1").await.unwrap();
    assert_eq!(result, ClaimResult::Claimed);

    let task = ctx.engine.get_task("t1").await.unwrap().unwrap();
    assert_eq!(task.status, TaskStatus::Assigned);
    assert_eq!(task.assigned_worker, Some("w1".to_string()));

    // Step 2: Transition to running (assigned -> running)
    let task = ctx
        .engine
        .transition_task("t1", TaskStatus::Running, None)
        .await
        .unwrap();
    assert_eq!(task.status, TaskStatus::Running);
    assert_eq!(
        task.assigned_worker,
        Some("w1".to_string()),
        "assigned_worker should persist through running"
    );

    // Worker should still have the slot consumed
    let worker = ctx.manager.get_worker("w1").await.unwrap().unwrap();
    assert_eq!(worker.used_slots, 1);

    // Step 3: Complete (running -> completed)
    let task = ctx
        .engine
        .transition_task("t1", TaskStatus::Completed, None)
        .await
        .unwrap();
    assert_eq!(task.status, TaskStatus::Completed);
    assert!(task.completed_at.is_some(), "completed_at should be set");
}

/// 3. assigned -> pending (decline) -> assigned (re-claim by different worker).
///    Worker declines, task returns to pending, then another worker picks it up.
#[tokio::test]
async fn decline_and_reclaim_by_different_worker() {
    let ctx = make_context();

    ctx.manager
        .register_worker(make_registration("w1", 5))
        .await
        .unwrap();
    ctx.manager
        .register_worker(make_registration("w2", 5))
        .await
        .unwrap();

    ctx.engine
        .create_task(CreateTaskInput {
            id: Some("t1".to_string()),
            ..Default::default()
        })
        .await
        .unwrap();

    // Worker 1 claims
    let result = ctx.manager.claim_task("t1", "w1").await.unwrap();
    assert_eq!(result, ClaimResult::Claimed);

    let task = ctx.engine.get_task("t1").await.unwrap().unwrap();
    assert_eq!(task.status, TaskStatus::Assigned);
    assert_eq!(task.assigned_worker, Some("w1".to_string()));

    // Worker 1 declines -> task goes back to pending
    ctx.manager.decline_task("t1", "w1", None).await.unwrap();

    let task = ctx.engine.get_task("t1").await.unwrap().unwrap();
    assert_eq!(task.status, TaskStatus::Pending);
    assert_eq!(
        task.assigned_worker, None,
        "assigned_worker should be cleared after decline"
    );

    let w1 = ctx.manager.get_worker("w1").await.unwrap().unwrap();
    assert_eq!(w1.used_slots, 0, "w1 should have 0 used slots after decline");

    // Worker 2 claims the same task
    let result = ctx.manager.claim_task("t1", "w2").await.unwrap();
    assert_eq!(result, ClaimResult::Claimed);

    let task = ctx.engine.get_task("t1").await.unwrap().unwrap();
    assert_eq!(task.status, TaskStatus::Assigned);
    assert_eq!(task.assigned_worker, Some("w2".to_string()));

    let w2 = ctx.manager.get_worker("w2").await.unwrap().unwrap();
    assert_eq!(w2.used_slots, 1, "w2 should have 1 used slot after claim");
}

/// 4. assigned -> paused: verify the transition is valid.
///    The state machine allows assigned -> paused. Verify the engine accepts it.
#[tokio::test]
async fn assigned_to_paused_is_valid_transition() {
    let ctx = make_context();

    ctx.manager
        .register_worker(make_registration("w1", 5))
        .await
        .unwrap();

    ctx.engine
        .create_task(CreateTaskInput {
            id: Some("t1".to_string()),
            ..Default::default()
        })
        .await
        .unwrap();

    // Claim the task (pending -> assigned)
    let result = ctx.manager.claim_task("t1", "w1").await.unwrap();
    assert_eq!(result, ClaimResult::Claimed);

    // Transition to paused (assigned -> paused)
    let task = ctx
        .engine
        .transition_task("t1", TaskStatus::Paused, None)
        .await
        .unwrap();
    assert_eq!(task.status, TaskStatus::Paused);

    // Worker should still hold the slot (pausing doesn't release capacity)
    let worker = ctx.manager.get_worker("w1").await.unwrap().unwrap();
    assert_eq!(
        worker.used_slots, 1,
        "Pausing should not release worker slots"
    );

    // Should be able to resume: paused -> assigned is valid
    let task = ctx
        .engine
        .transition_task("t1", TaskStatus::Assigned, None)
        .await
        .unwrap();
    assert_eq!(task.status, TaskStatus::Assigned);
}

// =============================================================================
// TTL + Worker
// =============================================================================

/// 5. Task with TTL, claimed by worker -- TTL is set at creation time.
///    Verify that the TTL field persists through the claim.
#[tokio::test]
async fn task_with_ttl_claimed_by_worker_preserves_ttl() {
    let ctx = make_context();

    ctx.manager
        .register_worker(make_registration("w1", 5))
        .await
        .unwrap();

    ctx.engine
        .create_task(CreateTaskInput {
            id: Some("t1".to_string()),
            ttl: Some(60),
            ..Default::default()
        })
        .await
        .unwrap();

    // Verify TTL is set on creation
    let task = ctx.engine.get_task("t1").await.unwrap().unwrap();
    assert_eq!(task.ttl, Some(60));

    // Claim the task
    let result = ctx.manager.claim_task("t1", "w1").await.unwrap();
    assert_eq!(result, ClaimResult::Claimed);

    // TTL should still be present on the task after claim
    let task = ctx.engine.get_task("t1").await.unwrap().unwrap();
    assert_eq!(task.ttl, Some(60), "TTL should persist through claim");
    assert_eq!(task.status, TaskStatus::Assigned);
}

/// 6. TTL expiry during assigned status.
///    The state machine does NOT allow assigned -> timeout (it's not in the
///    allowed transitions). Verify this is rejected.
#[tokio::test]
async fn ttl_expiry_during_assigned_is_invalid_transition() {
    let ctx = make_context();

    ctx.manager
        .register_worker(make_registration("w1", 5))
        .await
        .unwrap();

    ctx.engine
        .create_task(CreateTaskInput {
            id: Some("t1".to_string()),
            ttl: Some(1), // very short TTL
            ..Default::default()
        })
        .await
        .unwrap();

    // Claim the task
    ctx.manager.claim_task("t1", "w1").await.unwrap();

    let task = ctx.engine.get_task("t1").await.unwrap().unwrap();
    assert_eq!(task.status, TaskStatus::Assigned);

    // Attempt to transition directly to timeout -- should fail
    let result = ctx
        .engine
        .transition_task("t1", TaskStatus::Timeout, None)
        .await;
    assert!(
        result.is_err(),
        "assigned -> timeout should be an invalid transition"
    );

    // The task should still be assigned
    let task = ctx.engine.get_task("t1").await.unwrap().unwrap();
    assert_eq!(task.status, TaskStatus::Assigned);

    // The valid path for TTL expiry on an assigned task would be:
    // assigned -> cancelled (admin cancellation)
    let task = ctx
        .engine
        .transition_task("t1", TaskStatus::Cancelled, None)
        .await
        .unwrap();
    assert_eq!(
        task.status,
        TaskStatus::Cancelled,
        "assigned -> cancelled should work as TTL expiry fallback"
    );
}

/// 7. Task assigned, transitions to running, then TTL-triggered timeout.
///    running -> timeout IS valid. Verify the full path.
#[tokio::test]
async fn assigned_to_running_then_timeout_succeeds() {
    let ctx = make_context();

    ctx.manager
        .register_worker(make_registration("w1", 5))
        .await
        .unwrap();

    ctx.engine
        .create_task(CreateTaskInput {
            id: Some("t1".to_string()),
            ttl: Some(5),
            ..Default::default()
        })
        .await
        .unwrap();

    // Claim the task (pending -> assigned)
    ctx.manager.claim_task("t1", "w1").await.unwrap();

    // Transition to running (assigned -> running)
    ctx.engine
        .transition_task("t1", TaskStatus::Running, None)
        .await
        .unwrap();

    // TTL triggers timeout (running -> timeout)
    let task = ctx
        .engine
        .transition_task("t1", TaskStatus::Timeout, None)
        .await
        .unwrap();
    assert_eq!(task.status, TaskStatus::Timeout);
    assert!(task.completed_at.is_some(), "completed_at should be set on timeout");

    // Verify worker still holds the slot (engine doesn't auto-release)
    let worker = ctx.manager.get_worker("w1").await.unwrap().unwrap();
    assert_eq!(
        worker.used_slots, 1,
        "Worker slot is not released automatically by engine timeout"
    );
}

// =============================================================================
// Events + Worker
// =============================================================================

/// 8. publish_event while task is assigned -- should succeed.
///    The engine allows publishing events to non-terminal tasks. Assigned is
///    not terminal, so events should be publishable.
#[tokio::test]
async fn publish_event_while_task_is_assigned_succeeds() {
    let ctx = make_context();

    ctx.manager
        .register_worker(make_registration("w1", 5))
        .await
        .unwrap();

    ctx.engine
        .create_task(CreateTaskInput {
            id: Some("t1".to_string()),
            ..Default::default()
        })
        .await
        .unwrap();

    // Claim the task (pending -> assigned)
    ctx.manager.claim_task("t1", "w1").await.unwrap();

    let task = ctx.engine.get_task("t1").await.unwrap().unwrap();
    assert_eq!(task.status, TaskStatus::Assigned);

    // Publish an event while assigned
    let event = ctx
        .engine
        .publish_event(
            "t1",
            PublishEventInput {
                r#type: "worker.progress".to_string(),
                level: Level::Info,
                data: serde_json::json!({"progress": 0.1}),
                series_id: None,
                series_mode: None,
                series_acc_field: None,
            },
        )
        .await
        .unwrap();

    assert_eq!(event.task_id, "t1");
    assert_eq!(event.r#type, "worker.progress");
    assert_eq!(event.level, Level::Info);

    // Verify the event is retrievable
    let events = ctx.engine.get_events("t1", None).await.unwrap();
    // There should be the claim audit event(s) plus our published event
    let user_events: Vec<_> = events
        .iter()
        .filter(|e| e.r#type == "worker.progress")
        .collect();
    assert_eq!(user_events.len(), 1, "Should have exactly 1 user event");
}

/// 9. Multiple events during assigned, verify event history ordering.
///    Events should be ordered by index, monotonically increasing.
#[tokio::test]
async fn multiple_events_during_assigned_maintain_order() {
    let ctx = make_context();

    ctx.manager
        .register_worker(make_registration("w1", 5))
        .await
        .unwrap();

    ctx.engine
        .create_task(CreateTaskInput {
            id: Some("t1".to_string()),
            ..Default::default()
        })
        .await
        .unwrap();

    // Claim the task
    ctx.manager.claim_task("t1", "w1").await.unwrap();

    // Publish multiple events in sequence
    for i in 0..5 {
        ctx.engine
            .publish_event(
                "t1",
                PublishEventInput {
                    r#type: format!("step.{}", i),
                    level: Level::Info,
                    data: serde_json::json!({"step": i}),
                    series_id: None,
                    series_mode: None,
                    series_acc_field: None,
                },
            )
            .await
            .unwrap();
    }

    let events = ctx.engine.get_events("t1", None).await.unwrap();

    // Filter to our step events
    let step_events: Vec<_> = events
        .iter()
        .filter(|e| e.r#type.starts_with("step."))
        .collect();
    assert_eq!(step_events.len(), 5, "Should have 5 step events");

    // Verify monotonic index ordering
    for i in 1..step_events.len() {
        assert!(
            step_events[i].index > step_events[i - 1].index,
            "Event indices should be strictly increasing: {} vs {}",
            step_events[i - 1].index,
            step_events[i].index
        );
    }

    // Verify monotonic timestamp ordering
    for i in 1..step_events.len() {
        assert!(
            step_events[i].timestamp >= step_events[i - 1].timestamp,
            "Event timestamps should be non-decreasing"
        );
    }
}

/// 10. Event ordering across claim -> events -> decline -> re-claim -> events.
///     Verify the full event history is preserved and ordered after a claim
///     cycle with events interspersed.
#[tokio::test]
async fn event_ordering_across_claim_decline_reclaim_cycle() {
    let ctx = make_context();

    ctx.manager
        .register_worker(make_registration("w1", 5))
        .await
        .unwrap();
    ctx.manager
        .register_worker(make_registration("w2", 5))
        .await
        .unwrap();

    ctx.engine
        .create_task(CreateTaskInput {
            id: Some("t1".to_string()),
            ..Default::default()
        })
        .await
        .unwrap();

    // Phase 1: w1 claims, publishes events
    ctx.manager.claim_task("t1", "w1").await.unwrap();

    ctx.engine
        .publish_event(
            "t1",
            PublishEventInput {
                r#type: "phase1.event1".to_string(),
                level: Level::Info,
                data: serde_json::json!({"phase": 1, "event": 1}),
                series_id: None,
                series_mode: None,
                series_acc_field: None,
            },
        )
        .await
        .unwrap();

    ctx.engine
        .publish_event(
            "t1",
            PublishEventInput {
                r#type: "phase1.event2".to_string(),
                level: Level::Info,
                data: serde_json::json!({"phase": 1, "event": 2}),
                series_id: None,
                series_mode: None,
                series_acc_field: None,
            },
        )
        .await
        .unwrap();

    // Phase 2: w1 declines
    ctx.manager.decline_task("t1", "w1", None).await.unwrap();

    // Phase 3: w2 claims, publishes events
    ctx.manager.claim_task("t1", "w2").await.unwrap();

    ctx.engine
        .publish_event(
            "t1",
            PublishEventInput {
                r#type: "phase3.event1".to_string(),
                level: Level::Info,
                data: serde_json::json!({"phase": 3, "event": 1}),
                series_id: None,
                series_mode: None,
                series_acc_field: None,
            },
        )
        .await
        .unwrap();

    ctx.engine
        .publish_event(
            "t1",
            PublishEventInput {
                r#type: "phase3.event2".to_string(),
                level: Level::Info,
                data: serde_json::json!({"phase": 3, "event": 2}),
                series_id: None,
                series_mode: None,
                series_acc_field: None,
            },
        )
        .await
        .unwrap();

    // Verify the full event history
    let events = ctx.engine.get_events("t1", None).await.unwrap();

    // Collect user-published events (excluding audit and status events)
    let user_events: Vec<_> = events
        .iter()
        .filter(|e| e.r#type.starts_with("phase"))
        .collect();
    assert_eq!(
        user_events.len(),
        4,
        "Should have 4 user events across both phases"
    );

    // Verify ordering: phase1 events come before phase3 events
    assert_eq!(user_events[0].r#type, "phase1.event1");
    assert_eq!(user_events[1].r#type, "phase1.event2");
    assert_eq!(user_events[2].r#type, "phase3.event1");
    assert_eq!(user_events[3].r#type, "phase3.event2");

    // Verify all indices are strictly increasing across the full history
    for i in 1..events.len() {
        assert!(
            events[i].index > events[i - 1].index,
            "All event indices should be strictly increasing across the full history: \
             event[{}].index={} vs event[{}].index={}",
            i - 1,
            events[i - 1].index,
            i,
            events[i].index
        );
    }

    // Verify audit events are also present (claim, decline, re-claim)
    let audit_events: Vec<_> = events
        .iter()
        .filter(|e| e.r#type == "taskcast:audit")
        .collect();
    assert!(
        audit_events.len() >= 2,
        "Should have audit events for claim/decline operations, got {}",
        audit_events.len()
    );

    // Verify status transition events are present
    let status_events: Vec<_> = events
        .iter()
        .filter(|e| e.r#type == "taskcast:status")
        .collect();
    assert!(
        !status_events.is_empty(),
        "Should have at least one status transition event (for the decline -> pending transition)"
    );
}

// =============================================================================
// Worker Capacity + State Transitions
// =============================================================================

/// 11. When task transitions assigned -> cancelled via engine, worker.used_slots
///     is NOT automatically decremented (engine doesn't know about workers).
///     After explicit decline, used_slots should be decremented.
#[tokio::test]
async fn assigned_to_cancelled_worker_slots_require_explicit_release() {
    let ctx = make_context();

    ctx.manager
        .register_worker(make_registration("w1", 3))
        .await
        .unwrap();

    // Create and claim two tasks
    for tid in &["t1", "t2"] {
        ctx.engine
            .create_task(CreateTaskInput {
                id: Some(tid.to_string()),
                ..Default::default()
            })
            .await
            .unwrap();
        ctx.manager.claim_task(tid, "w1").await.unwrap();
    }

    let worker = ctx.manager.get_worker("w1").await.unwrap().unwrap();
    assert_eq!(worker.used_slots, 2, "Should have 2 slots used");

    // Cancel t1 via engine (assigned -> cancelled)
    ctx.engine
        .transition_task("t1", TaskStatus::Cancelled, None)
        .await
        .unwrap();

    // Worker still holds the slot -- engine transitions don't release worker capacity
    let worker = ctx.manager.get_worker("w1").await.unwrap().unwrap();
    assert_eq!(
        worker.used_slots, 2,
        "used_slots should still be 2 after engine-level cancel (no auto-release)"
    );

    // Explicitly decline to release the slot
    ctx.manager.decline_task("t1", "w1", None).await.unwrap();

    let worker = ctx.manager.get_worker("w1").await.unwrap().unwrap();
    assert_eq!(
        worker.used_slots, 1,
        "used_slots should be 1 after declining the cancelled task"
    );
}

/// 12. When task transitions assigned -> running, worker.used_slots should remain
///     unchanged (the worker is still working on the task).
#[tokio::test]
async fn assigned_to_running_does_not_change_worker_slots() {
    let ctx = make_context();

    ctx.manager
        .register_worker(make_registration("w1", 5))
        .await
        .unwrap();

    ctx.engine
        .create_task(CreateTaskInput {
            id: Some("t1".to_string()),
            cost: Some(2),
            ..Default::default()
        })
        .await
        .unwrap();

    // Claim (pending -> assigned)
    ctx.manager.claim_task("t1", "w1").await.unwrap();

    let worker = ctx.manager.get_worker("w1").await.unwrap().unwrap();
    assert_eq!(worker.used_slots, 2, "Should reflect cost=2 after claim");

    // Transition to running (assigned -> running)
    ctx.engine
        .transition_task("t1", TaskStatus::Running, None)
        .await
        .unwrap();

    let worker = ctx.manager.get_worker("w1").await.unwrap().unwrap();
    assert_eq!(
        worker.used_slots, 2,
        "used_slots should remain 2 after assigned -> running (worker still owns the task)"
    );
}

/// 13. When task completes (running -> completed), worker.used_slots is NOT
///     automatically decremented by the engine. The orchestration layer must
///     handle slot release. This test verifies that behavior and shows how
///     decline_task can clean up after completion.
#[tokio::test]
async fn running_to_completed_worker_slots_require_explicit_release() {
    let ctx = make_context();

    ctx.manager
        .register_worker(make_registration("w1", 5))
        .await
        .unwrap();

    ctx.engine
        .create_task(CreateTaskInput {
            id: Some("t1".to_string()),
            cost: Some(3),
            ..Default::default()
        })
        .await
        .unwrap();

    // Claim and transition to running
    ctx.manager.claim_task("t1", "w1").await.unwrap();
    ctx.engine
        .transition_task("t1", TaskStatus::Running, None)
        .await
        .unwrap();

    let worker = ctx.manager.get_worker("w1").await.unwrap().unwrap();
    assert_eq!(worker.used_slots, 3, "Should reflect cost=3");

    // Complete the task (running -> completed)
    let task = ctx
        .engine
        .transition_task("t1", TaskStatus::Completed, None)
        .await
        .unwrap();
    assert_eq!(task.status, TaskStatus::Completed);

    // Engine doesn't auto-release worker slots
    let worker = ctx.manager.get_worker("w1").await.unwrap().unwrap();
    assert_eq!(
        worker.used_slots, 3,
        "used_slots should still be 3 after engine-level completion (no auto-release)"
    );

    // The decline_task call (simulating orchestration-layer cleanup) should
    // release the slot since the assignment record still exists
    ctx.manager.decline_task("t1", "w1", None).await.unwrap();

    let worker = ctx.manager.get_worker("w1").await.unwrap().unwrap();
    assert_eq!(
        worker.used_slots, 0,
        "used_slots should be 0 after explicit release via decline"
    );
    assert_eq!(worker.status, WorkerStatus::Idle);
}
