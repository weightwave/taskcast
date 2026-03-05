//! Tests for `WorkerManager::release_task()` and `TaskEngine::add_transition_listener()`.
//!
//! These tests verify:
//! - release_task removes assignment and restores worker capacity
//! - release_task edge cases (no assignment, deleted worker, concurrent calls)
//! - release_task preserves draining status
//! - release_task does not mutate task status
//! - add_transition_listener callback invocation

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

use taskcast_core::{
    BroadcastProvider, ClaimResult, ConnectionMode, CreateTaskInput, MemoryBroadcastProvider,
    MemoryShortTermStore, ShortTermStore, TaskEngine, TaskEngineOptions, TaskStatus,
    WorkerManager, WorkerManagerOptions, WorkerMatchRule, WorkerRegistration, WorkerStatus,
    WorkerUpdate, WorkerUpdateStatus,
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
// release_task
// =============================================================================

/// 1. release_task removes the assignment and restores worker capacity.
#[tokio::test]
async fn release_task_removes_assignment_and_restores_capacity() {
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
    assert_eq!(worker.used_slots, 1);

    // Verify assignment exists
    let assignments = ctx.manager.get_worker_tasks("w1").await.unwrap();
    assert_eq!(assignments.len(), 1);

    // Release the task
    ctx.manager.release_task("t1").await.unwrap();

    // Verify assignment is gone
    let assignments = ctx.manager.get_worker_tasks("w1").await.unwrap();
    assert_eq!(assignments.len(), 0, "Assignment should be removed after release");

    // Verify capacity is restored
    let worker = ctx.manager.get_worker("w1").await.unwrap().unwrap();
    assert_eq!(worker.used_slots, 0, "used_slots should be 0 after release");
}

/// 2. release_task is a no-op when there is no assignment for the given task.
#[tokio::test]
async fn release_task_noop_when_no_assignment() {
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

    // Call release on a task that has no assignment — should not error
    let result = ctx.manager.release_task("t1").await;
    assert!(result.is_ok(), "release_task should succeed as no-op when no assignment");

    // Also try a completely non-existent task ID
    let result = ctx.manager.release_task("nonexistent").await;
    assert!(result.is_ok(), "release_task should succeed for non-existent task");
}

/// 3. release_task sets worker to idle when used_slots reaches zero.
#[tokio::test]
async fn release_task_sets_idle_when_used_slots_zero() {
    let ctx = make_context();

    // Worker with capacity 1 so claiming one task makes it busy
    ctx.manager
        .register_worker(make_registration("w1", 1))
        .await
        .unwrap();

    ctx.engine
        .create_task(CreateTaskInput {
            id: Some("t1".to_string()),
            ..Default::default()
        })
        .await
        .unwrap();

    // Claim the task — worker should become busy (used_slots == capacity)
    ctx.manager.claim_task("t1", "w1").await.unwrap();

    let worker = ctx.manager.get_worker("w1").await.unwrap().unwrap();
    assert_eq!(worker.status, WorkerStatus::Busy);
    assert_eq!(worker.used_slots, 1);

    // Release the task
    ctx.manager.release_task("t1").await.unwrap();

    let worker = ctx.manager.get_worker("w1").await.unwrap().unwrap();
    assert_eq!(worker.used_slots, 0);
    assert_eq!(
        worker.status,
        WorkerStatus::Idle,
        "Worker should be idle after releasing its only task"
    );
}

/// 4. release_task preserves draining status even when used_slots reaches zero.
#[tokio::test]
async fn release_task_preserves_draining_status() {
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

    // Set worker to draining
    ctx.manager
        .update_worker(
            "w1",
            WorkerUpdate {
                status: Some(WorkerUpdateStatus::Draining),
                ..Default::default()
            },
        )
        .await
        .unwrap();

    let worker = ctx.manager.get_worker("w1").await.unwrap().unwrap();
    assert_eq!(worker.status, WorkerStatus::Draining);

    // Release the task
    ctx.manager.release_task("t1").await.unwrap();

    let worker = ctx.manager.get_worker("w1").await.unwrap().unwrap();
    assert_eq!(worker.used_slots, 0);
    assert_eq!(
        worker.status,
        WorkerStatus::Draining,
        "Draining status should be preserved after release even when used_slots is zero"
    );
}

/// 5. release_task handles a deleted worker gracefully (worker no longer exists).
#[tokio::test]
async fn release_task_handles_deleted_worker() {
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

    // Delete the worker
    ctx.manager.unregister_worker("w1").await.unwrap();

    // Release task — worker no longer exists, should not error
    let result = ctx.manager.release_task("t1").await;
    assert!(
        result.is_ok(),
        "release_task should succeed even when worker has been deleted"
    );

    // Verify assignment was still removed
    let assignments = ctx.manager.get_worker_tasks("w1").await.unwrap();
    assert_eq!(
        assignments.len(),
        0,
        "Assignment should be removed even when worker is deleted"
    );
}

/// 6. Concurrent double release is idempotent — both calls succeed, worker
///    capacity is only restored once.
#[tokio::test]
async fn release_task_concurrent_double_release_idempotent() {
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

    let worker = ctx.manager.get_worker("w1").await.unwrap().unwrap();
    assert_eq!(worker.used_slots, 1);

    // Spawn two concurrent releases
    let m1 = Arc::clone(&ctx.manager);
    let m2 = Arc::clone(&ctx.manager);

    let h1 = tokio::spawn(async move { m1.release_task("t1").await });
    let h2 = tokio::spawn(async move { m2.release_task("t1").await });

    let (r1, r2) = tokio::join!(h1, h2);
    assert!(r1.unwrap().is_ok(), "First release should succeed");
    assert!(r2.unwrap().is_ok(), "Second release should succeed (no-op)");

    // Worker used_slots should be 0, not negative (saturating_sub prevents underflow)
    let worker = ctx.manager.get_worker("w1").await.unwrap().unwrap();
    assert_eq!(
        worker.used_slots, 0,
        "used_slots should be 0 after concurrent double release"
    );
    assert_eq!(worker.status, WorkerStatus::Idle);
}

/// 7. release_task does not change the task's status — it only affects the
///    assignment and worker capacity.
#[tokio::test]
async fn release_task_does_not_change_task_state() {
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

    // Transition to running (assigned -> running)
    ctx.engine
        .transition_task("t1", TaskStatus::Running, None)
        .await
        .unwrap();

    // Transition to completed (running -> completed)
    ctx.engine
        .transition_task("t1", TaskStatus::Completed, None)
        .await
        .unwrap();

    let task_before = ctx.engine.get_task("t1").await.unwrap().unwrap();
    assert_eq!(task_before.status, TaskStatus::Completed);

    // Release the task
    ctx.manager.release_task("t1").await.unwrap();

    // Task status should still be completed — release_task doesn't touch task state
    let task_after = ctx.engine.get_task("t1").await.unwrap().unwrap();
    assert_eq!(
        task_after.status,
        TaskStatus::Completed,
        "release_task should not change the task's status"
    );
    assert_eq!(
        task_after.completed_at, task_before.completed_at,
        "completed_at should remain unchanged"
    );
}

/// 8. release_task correctly restores capacity for tasks with different costs.
#[tokio::test]
async fn release_task_multiple_costs_correct_math() {
    let ctx = make_context();

    ctx.manager
        .register_worker(make_registration("w1", 10))
        .await
        .unwrap();

    // Create tasks with different costs
    ctx.engine
        .create_task(CreateTaskInput {
            id: Some("t1".to_string()),
            cost: Some(3),
            ..Default::default()
        })
        .await
        .unwrap();

    ctx.engine
        .create_task(CreateTaskInput {
            id: Some("t2".to_string()),
            cost: Some(5),
            ..Default::default()
        })
        .await
        .unwrap();

    // Claim both tasks
    let r1 = ctx.manager.claim_task("t1", "w1").await.unwrap();
    assert_eq!(r1, ClaimResult::Claimed);
    let r2 = ctx.manager.claim_task("t2", "w1").await.unwrap();
    assert_eq!(r2, ClaimResult::Claimed);

    let worker = ctx.manager.get_worker("w1").await.unwrap().unwrap();
    assert_eq!(worker.used_slots, 8); // 3 + 5

    // Release t1 (cost 3)
    ctx.manager.release_task("t1").await.unwrap();

    let worker = ctx.manager.get_worker("w1").await.unwrap().unwrap();
    assert_eq!(
        worker.used_slots, 5,
        "After releasing t1 (cost=3), used_slots should drop from 8 to 5"
    );

    // Release t2 (cost 5)
    ctx.manager.release_task("t2").await.unwrap();

    let worker = ctx.manager.get_worker("w1").await.unwrap().unwrap();
    assert_eq!(
        worker.used_slots, 0,
        "After releasing t2 (cost=5), used_slots should be 0"
    );
    assert_eq!(worker.status, WorkerStatus::Idle);
}

// =============================================================================
// add_transition_listener
// =============================================================================

/// 9. add_transition_listener callback is called on every task transition.
#[tokio::test]
async fn add_transition_listener_called_on_transition() {
    let ctx = make_context();

    let call_count = Arc::new(AtomicU32::new(0));
    let call_count_clone = Arc::clone(&call_count);

    // Track the last transition seen
    let last_from = Arc::new(std::sync::Mutex::new(None::<TaskStatus>));
    let last_to = Arc::new(std::sync::Mutex::new(None::<TaskStatus>));
    let last_from_clone = Arc::clone(&last_from);
    let last_to_clone = Arc::clone(&last_to);

    ctx.engine
        .add_transition_listener(Box::new(move |_task, from, to| {
            call_count_clone.fetch_add(1, Ordering::SeqCst);
            *last_from_clone.lock().unwrap() = Some(from.clone());
            *last_to_clone.lock().unwrap() = Some(to.clone());
        }));

    ctx.engine
        .create_task(CreateTaskInput {
            id: Some("t1".to_string()),
            ..Default::default()
        })
        .await
        .unwrap();

    // Transition pending -> running
    ctx.engine
        .transition_task("t1", TaskStatus::Running, None)
        .await
        .unwrap();

    assert_eq!(call_count.load(Ordering::SeqCst), 1);
    assert_eq!(*last_from.lock().unwrap(), Some(TaskStatus::Pending));
    assert_eq!(*last_to.lock().unwrap(), Some(TaskStatus::Running));

    // Transition running -> completed
    ctx.engine
        .transition_task("t1", TaskStatus::Completed, None)
        .await
        .unwrap();

    assert_eq!(call_count.load(Ordering::SeqCst), 2);
    assert_eq!(*last_from.lock().unwrap(), Some(TaskStatus::Running));
    assert_eq!(*last_to.lock().unwrap(), Some(TaskStatus::Completed));
}

/// 10. Multiple transition listeners are all called on each transition.
#[tokio::test]
async fn add_transition_listener_multiple_listeners() {
    let ctx = make_context();

    let counter_a = Arc::new(AtomicU32::new(0));
    let counter_b = Arc::new(AtomicU32::new(0));
    let counter_c = Arc::new(AtomicU32::new(0));

    let a = Arc::clone(&counter_a);
    let b = Arc::clone(&counter_b);
    let c = Arc::clone(&counter_c);

    ctx.engine
        .add_transition_listener(Box::new(move |_task, _from, _to| {
            a.fetch_add(1, Ordering::SeqCst);
        }));
    ctx.engine
        .add_transition_listener(Box::new(move |_task, _from, _to| {
            b.fetch_add(1, Ordering::SeqCst);
        }));
    ctx.engine
        .add_transition_listener(Box::new(move |_task, _from, _to| {
            c.fetch_add(1, Ordering::SeqCst);
        }));

    ctx.engine
        .create_task(CreateTaskInput {
            id: Some("t1".to_string()),
            ..Default::default()
        })
        .await
        .unwrap();

    // One transition: pending -> running
    ctx.engine
        .transition_task("t1", TaskStatus::Running, None)
        .await
        .unwrap();

    assert_eq!(counter_a.load(Ordering::SeqCst), 1, "Listener A should be called once");
    assert_eq!(counter_b.load(Ordering::SeqCst), 1, "Listener B should be called once");
    assert_eq!(counter_c.load(Ordering::SeqCst), 1, "Listener C should be called once");

    // Another transition: running -> completed
    ctx.engine
        .transition_task("t1", TaskStatus::Completed, None)
        .await
        .unwrap();

    assert_eq!(counter_a.load(Ordering::SeqCst), 2, "Listener A should be called twice");
    assert_eq!(counter_b.load(Ordering::SeqCst), 2, "Listener B should be called twice");
    assert_eq!(counter_c.load(Ordering::SeqCst), 2, "Listener C should be called twice");
}
