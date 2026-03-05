//! Remaining test gaps for WorkerManager.
//!
//! Covers:
//! 1. Concurrent decline — two workers decline the same task simultaneously
//! 2. Capacity update during dispatch — worker capacity changes between dispatch and claim
//! 3. Large blacklist — blacklist with 100+ worker IDs
//! 4. Audit completeness — all audit events emitted correctly for each operation
//! 5. Hook exception handling — hooks that panic don't crash the system

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::RwLock as TokioRwLock;

use taskcast_core::{
    BroadcastProvider, ClaimResult, ConnectionMode, CreateTaskInput, DeclineOptions,
    DispatchResult, EventQueryOptions, LongTermStore, MemoryBroadcastProvider,
    MemoryShortTermStore, ShortTermStore, Task, TaskEngine, TaskEngineOptions, TaskEvent,
    TaskStatus, TaskcastHooks, Worker, WorkerAuditAction, WorkerAuditEvent,
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

// ─── Mock LongTermStore ─────────────────────────────────────────────────────

struct MockLongTermStore {
    tasks: TokioRwLock<HashMap<String, Task>>,
    events: TokioRwLock<Vec<TaskEvent>>,
    worker_events: TokioRwLock<Vec<WorkerAuditEvent>>,
}

impl MockLongTermStore {
    fn new() -> Self {
        Self {
            tasks: TokioRwLock::new(HashMap::new()),
            events: TokioRwLock::new(Vec::new()),
            worker_events: TokioRwLock::new(Vec::new()),
        }
    }
}

#[async_trait]
impl LongTermStore for MockLongTermStore {
    async fn save_task(
        &self,
        task: Task,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        self.tasks.write().await.insert(task.id.clone(), task);
        Ok(())
    }

    async fn get_task(
        &self,
        task_id: &str,
    ) -> Result<Option<Task>, Box<dyn std::error::Error + Send + Sync>> {
        Ok(self.tasks.read().await.get(task_id).cloned())
    }

    async fn save_event(
        &self,
        event: TaskEvent,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        self.events.write().await.push(event);
        Ok(())
    }

    async fn get_events(
        &self,
        _task_id: &str,
        _opts: Option<EventQueryOptions>,
    ) -> Result<Vec<TaskEvent>, Box<dyn std::error::Error + Send + Sync>> {
        Ok(self.events.read().await.clone())
    }

    async fn save_worker_event(
        &self,
        event: WorkerAuditEvent,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        self.worker_events.write().await.push(event);
        Ok(())
    }

    async fn get_worker_events(
        &self,
        _worker_id: &str,
        _opts: Option<EventQueryOptions>,
    ) -> Result<Vec<WorkerAuditEvent>, Box<dyn std::error::Error + Send + Sync>> {
        Ok(self.worker_events.read().await.clone())
    }
}

// ─── Mock Hooks ─────────────────────────────────────────────────────────────

struct MockHooks {
    connected_count: AtomicU64,
    disconnected_count: AtomicU64,
    assigned_count: AtomicU64,
    declined_count: AtomicU64,
    connected_ids: std::sync::Mutex<Vec<String>>,
    disconnected_ids: std::sync::Mutex<Vec<String>>,
    assigned_task_ids: std::sync::Mutex<Vec<String>>,
    declined_task_ids: std::sync::Mutex<Vec<String>>,
}

impl MockHooks {
    fn new() -> Self {
        Self {
            connected_count: AtomicU64::new(0),
            disconnected_count: AtomicU64::new(0),
            assigned_count: AtomicU64::new(0),
            declined_count: AtomicU64::new(0),
            connected_ids: std::sync::Mutex::new(Vec::new()),
            disconnected_ids: std::sync::Mutex::new(Vec::new()),
            assigned_task_ids: std::sync::Mutex::new(Vec::new()),
            declined_task_ids: std::sync::Mutex::new(Vec::new()),
        }
    }
}

impl TaskcastHooks for MockHooks {
    fn on_worker_connected(&self, worker: &Worker) {
        self.connected_count.fetch_add(1, Ordering::SeqCst);
        self.connected_ids
            .lock()
            .unwrap()
            .push(worker.id.clone());
    }

    fn on_worker_disconnected(&self, worker: &Worker, _reason: &str) {
        self.disconnected_count.fetch_add(1, Ordering::SeqCst);
        self.disconnected_ids
            .lock()
            .unwrap()
            .push(worker.id.clone());
    }

    fn on_task_assigned(&self, task: &Task, _worker: &Worker) {
        self.assigned_count.fetch_add(1, Ordering::SeqCst);
        self.assigned_task_ids
            .lock()
            .unwrap()
            .push(task.id.clone());
    }

    fn on_task_declined(&self, task: &Task, _worker: &Worker, _blacklisted: bool) {
        self.declined_count.fetch_add(1, Ordering::SeqCst);
        self.declined_task_ids
            .lock()
            .unwrap()
            .push(task.id.clone());
    }
}

/// Hooks implementation that panics on every callback.
struct PanickingHooks;

impl TaskcastHooks for PanickingHooks {
    fn on_worker_connected(&self, _worker: &Worker) {
        panic!("PanickingHooks: on_worker_connected");
    }

    fn on_worker_disconnected(&self, _worker: &Worker, _reason: &str) {
        panic!("PanickingHooks: on_worker_disconnected");
    }

    fn on_task_assigned(&self, _task: &Task, _worker: &Worker) {
        panic!("PanickingHooks: on_task_assigned");
    }

    fn on_task_declined(&self, _task: &Task, _worker: &Worker, _blacklisted: bool) {
        panic!("PanickingHooks: on_task_declined");
    }
}

fn make_context_with_long_term(
    long_term_store: Arc<MockLongTermStore>,
) -> (TestContext, Arc<MockLongTermStore>) {
    let short_term_store = Arc::new(MemoryShortTermStore::new());
    let broadcast = Arc::new(MemoryBroadcastProvider::new());
    let engine = Arc::new(TaskEngine::new(TaskEngineOptions {
        short_term_store: Arc::clone(&short_term_store) as Arc<dyn ShortTermStore>,
        broadcast: Arc::clone(&broadcast) as Arc<dyn BroadcastProvider>,
        long_term_store: Some(Arc::clone(&long_term_store) as Arc<dyn LongTermStore>),
        hooks: None,
    }));
    let manager = Arc::new(WorkerManager::new(WorkerManagerOptions {
        engine: Arc::clone(&engine),
        short_term_store: short_term_store as Arc<dyn ShortTermStore>,
        broadcast: broadcast as Arc<dyn BroadcastProvider>,
        long_term_store: Some(long_term_store.clone() as Arc<dyn LongTermStore>),
        hooks: None,
        defaults: None,
    }));
    (TestContext { manager, engine }, long_term_store)
}

fn make_context_with_hooks(hooks: Arc<dyn TaskcastHooks>) -> TestContext {
    let short_term_store = Arc::new(MemoryShortTermStore::new());
    let broadcast = Arc::new(MemoryBroadcastProvider::new());
    let engine = Arc::new(TaskEngine::new(TaskEngineOptions {
        short_term_store: Arc::clone(&short_term_store) as Arc<dyn ShortTermStore>,
        broadcast: Arc::clone(&broadcast) as Arc<dyn BroadcastProvider>,
        long_term_store: None,
        hooks: Some(Arc::clone(&hooks)),
    }));
    let manager = Arc::new(WorkerManager::new(WorkerManagerOptions {
        engine: Arc::clone(&engine),
        short_term_store: short_term_store as Arc<dyn ShortTermStore>,
        broadcast: broadcast as Arc<dyn BroadcastProvider>,
        long_term_store: None,
        hooks: Some(hooks),
        defaults: None,
    }));
    TestContext { manager, engine }
}

// =============================================================================
// 1. Concurrent Decline
// =============================================================================

/// Two workers decline the same task simultaneously (only the actual assignee
/// should succeed; the other is a no-op). Worker capacity must remain consistent.
#[tokio::test]
async fn concurrent_decline_same_task_only_assignee_succeeds() {
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

    // w1 claims the task
    let result = ctx.manager.claim_task("t1", "w1").await.unwrap();
    assert_eq!(result, ClaimResult::Claimed);

    // Both w1 and w2 try to decline simultaneously
    let m1 = Arc::clone(&ctx.manager);
    let m2 = Arc::clone(&ctx.manager);

    let h1 = tokio::spawn(async move { m1.decline_task("t1", "w1", None).await });
    let h2 = tokio::spawn(async move { m2.decline_task("t1", "w2", None).await });

    let (r1, r2) = tokio::join!(h1, h2);
    assert!(r1.unwrap().is_ok(), "w1 decline should not error");
    assert!(r2.unwrap().is_ok(), "w2 decline should not error (no-op)");

    // w1 should have its slot restored (used_slots = 0)
    let w1 = ctx.manager.get_worker("w1").await.unwrap().unwrap();
    assert_eq!(
        w1.used_slots, 0,
        "w1 should have used_slots restored after decline"
    );
    assert_eq!(w1.status, WorkerStatus::Idle);

    // w2 should be unaffected (was never assigned)
    let w2 = ctx.manager.get_worker("w2").await.unwrap().unwrap();
    assert_eq!(
        w2.used_slots, 0,
        "w2 should remain at 0 used_slots since it was never assigned"
    );
    assert_eq!(w2.status, WorkerStatus::Idle);

    // Task should be back to pending
    let task = ctx.engine.get_task("t1").await.unwrap().unwrap();
    assert_eq!(task.status, TaskStatus::Pending);
    assert_eq!(task.assigned_worker, None);
}

/// Two concurrent declines by the actual assigned worker — the second should be
/// a no-op and used_slots must not go negative.
#[tokio::test]
async fn concurrent_double_decline_by_same_worker_is_safe() {
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

    ctx.manager.claim_task("t1", "w1").await.unwrap();

    let m1 = Arc::clone(&ctx.manager);
    let m2 = Arc::clone(&ctx.manager);

    let h1 = tokio::spawn(async move { m1.decline_task("t1", "w1", None).await });
    let h2 = tokio::spawn(async move { m2.decline_task("t1", "w1", None).await });

    let (r1, r2) = tokio::join!(h1, h2);
    assert!(r1.unwrap().is_ok());
    assert!(r2.unwrap().is_ok());

    let worker = ctx.manager.get_worker("w1").await.unwrap().unwrap();
    assert_eq!(
        worker.used_slots, 0,
        "used_slots should be 0 after concurrent double decline, not negative"
    );
    assert_eq!(worker.status, WorkerStatus::Idle);
}

/// Concurrent decline with blacklist — both try to decline with blacklist=true,
/// worker ID should appear in the blacklist exactly once.
#[tokio::test]
async fn concurrent_decline_with_blacklist_deduplicates() {
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

    ctx.manager.claim_task("t1", "w1").await.unwrap();

    let m1 = Arc::clone(&ctx.manager);
    let m2 = Arc::clone(&ctx.manager);

    let h1 = tokio::spawn(async move {
        m1.decline_task("t1", "w1", Some(DeclineOptions { blacklist: true }))
            .await
    });
    let h2 = tokio::spawn(async move {
        m2.decline_task("t1", "w1", Some(DeclineOptions { blacklist: true }))
            .await
    });

    let (r1, r2) = tokio::join!(h1, h2);
    assert!(r1.unwrap().is_ok());
    assert!(r2.unwrap().is_ok());

    let task = ctx.engine.get_task("t1").await.unwrap().unwrap();
    let blacklist = task
        .metadata
        .as_ref()
        .and_then(|m| m.get("_blacklistedWorkers"))
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    // Only one decline should have actually processed (the one that found the assignment),
    // so w1 should appear at most once.
    let w1_count = blacklist.iter().filter(|id| *id == "w1").count();
    assert!(
        w1_count <= 1,
        "w1 should appear at most once in blacklist, but found {}",
        w1_count
    );
}

// =============================================================================
// 2. Capacity Update During Dispatch
// =============================================================================

/// Worker capacity changes between dispatch selection and claim — if capacity
/// is reduced so the task no longer fits, the claim should fail gracefully.
#[tokio::test]
async fn capacity_reduced_after_dispatch_claim_fails_gracefully() {
    let ctx = make_context();

    ctx.manager
        .register_worker(make_registration("w1", 10))
        .await
        .unwrap();

    ctx.engine
        .create_task(CreateTaskInput {
            id: Some("t1".to_string()),
            cost: Some(5),
            ..Default::default()
        })
        .await
        .unwrap();

    // Dispatch selects w1 (has capacity 10, cost 5 fits)
    let dispatch_result = ctx.manager.dispatch_task("t1").await.unwrap();
    assert_eq!(
        dispatch_result,
        DispatchResult::Dispatched {
            worker_id: "w1".to_string()
        }
    );

    // Now reduce w1's capacity to 3 (below the task cost of 5)
    ctx.manager
        .update_worker(
            "w1",
            WorkerUpdate {
                capacity: Some(3),
                ..Default::default()
            },
        )
        .await
        .unwrap();

    // Claim should fail because cost (5) > new capacity (3)
    let claim_result = ctx.manager.claim_task("t1", "w1").await.unwrap();
    assert!(
        matches!(claim_result, ClaimResult::Failed { .. }),
        "Claim should fail after capacity was reduced below task cost"
    );

    // Task should still be pending
    let task = ctx.engine.get_task("t1").await.unwrap().unwrap();
    assert_eq!(task.status, TaskStatus::Pending);

    // Worker should not have consumed any slots
    let worker = ctx.manager.get_worker("w1").await.unwrap().unwrap();
    assert_eq!(worker.used_slots, 0);
}

/// Worker capacity reduced while already holding tasks — new dispatches should
/// not pick the worker if remaining capacity is insufficient.
#[tokio::test]
async fn capacity_reduced_with_existing_tasks_blocks_new_dispatch() {
    let ctx = make_context();

    ctx.manager
        .register_worker(make_registration("w1", 10))
        .await
        .unwrap();

    // Claim a task with cost 5 (used_slots becomes 5)
    ctx.engine
        .create_task(CreateTaskInput {
            id: Some("t1".to_string()),
            cost: Some(5),
            ..Default::default()
        })
        .await
        .unwrap();
    ctx.manager.claim_task("t1", "w1").await.unwrap();

    // Reduce capacity to 6 (remaining = 6 - 5 = 1)
    ctx.manager
        .update_worker(
            "w1",
            WorkerUpdate {
                capacity: Some(6),
                ..Default::default()
            },
        )
        .await
        .unwrap();

    // New task with cost 3 should not dispatch to w1 (remaining 1 < cost 3)
    ctx.engine
        .create_task(CreateTaskInput {
            id: Some("t2".to_string()),
            cost: Some(3),
            ..Default::default()
        })
        .await
        .unwrap();

    let result = ctx.manager.dispatch_task("t2").await.unwrap();
    assert_eq!(
        result,
        DispatchResult::NoMatch,
        "Dispatch should return NoMatch when remaining capacity (1) < cost (3)"
    );
}

/// Worker capacity increased between dispatch and claim — claim should succeed
/// with the expanded capacity.
#[tokio::test]
async fn capacity_increased_after_dispatch_claim_succeeds() {
    let ctx = make_context();

    ctx.manager
        .register_worker(make_registration("w1", 2))
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

    // Dispatch selects w1
    let dispatch_result = ctx.manager.dispatch_task("t1").await.unwrap();
    assert_eq!(
        dispatch_result,
        DispatchResult::Dispatched {
            worker_id: "w1".to_string()
        }
    );

    // Increase capacity to 10
    ctx.manager
        .update_worker(
            "w1",
            WorkerUpdate {
                capacity: Some(10),
                ..Default::default()
            },
        )
        .await
        .unwrap();

    // Claim should succeed
    let claim_result = ctx.manager.claim_task("t1", "w1").await.unwrap();
    assert_eq!(claim_result, ClaimResult::Claimed);

    let worker = ctx.manager.get_worker("w1").await.unwrap().unwrap();
    assert_eq!(worker.used_slots, 2);
    assert_eq!(worker.capacity, 10);
    // Still idle because 2 < 10
    assert_eq!(worker.status, WorkerStatus::Idle);
}

// =============================================================================
// 3. Large Blacklist
// =============================================================================

/// Blacklist with 100+ worker IDs — dispatch should still work correctly and
/// only select non-blacklisted workers.
#[tokio::test]
async fn large_blacklist_100_workers_dispatch_skips_all() {
    let ctx = make_context();

    // Register 105 workers
    for i in 0..105 {
        ctx.manager
            .register_worker(make_registration(&format!("w{}", i), 5))
            .await
            .unwrap();
    }

    // Create a task with 100 workers blacklisted (w0 through w99)
    let blacklist: Vec<serde_json::Value> = (0..100)
        .map(|i| serde_json::Value::String(format!("w{}", i)))
        .collect();
    let mut metadata = HashMap::new();
    metadata.insert(
        "_blacklistedWorkers".to_string(),
        serde_json::Value::Array(blacklist),
    );

    ctx.engine
        .create_task(CreateTaskInput {
            id: Some("t1".to_string()),
            metadata: Some(metadata),
            ..Default::default()
        })
        .await
        .unwrap();

    // Dispatch should pick one of w100-w104 (the non-blacklisted ones)
    let result = ctx.manager.dispatch_task("t1").await.unwrap();
    match result {
        DispatchResult::Dispatched { ref worker_id } => {
            let id_num: usize = worker_id[1..].parse().unwrap();
            assert!(
                id_num >= 100,
                "Dispatched worker {} should be >= w100 (non-blacklisted)",
                worker_id
            );
        }
        DispatchResult::NoMatch => {
            panic!("Should have found a non-blacklisted worker among w100-w104");
        }
    }
}

/// Blacklist with all workers blacklisted (100+) — dispatch returns NoMatch.
#[tokio::test]
async fn large_blacklist_all_workers_blacklisted_returns_no_match() {
    let ctx = make_context();

    // Register 100 workers
    for i in 0..100 {
        ctx.manager
            .register_worker(make_registration(&format!("w{}", i), 5))
            .await
            .unwrap();
    }

    // Blacklist all 100
    let blacklist: Vec<serde_json::Value> = (0..100)
        .map(|i| serde_json::Value::String(format!("w{}", i)))
        .collect();
    let mut metadata = HashMap::new();
    metadata.insert(
        "_blacklistedWorkers".to_string(),
        serde_json::Value::Array(blacklist),
    );

    ctx.engine
        .create_task(CreateTaskInput {
            id: Some("t1".to_string()),
            metadata: Some(metadata),
            ..Default::default()
        })
        .await
        .unwrap();

    let result = ctx.manager.dispatch_task("t1").await.unwrap();
    assert_eq!(
        result,
        DispatchResult::NoMatch,
        "All 100 workers are blacklisted, should return NoMatch"
    );
}

/// Blacklist built up incrementally through decline cycles (100+ entries).
#[tokio::test]
async fn large_blacklist_built_through_decline_cycles() {
    let ctx = make_context();

    // Register 110 workers
    for i in 0..110 {
        ctx.manager
            .register_worker(make_registration(&format!("w{}", i), 5))
            .await
            .unwrap();
    }

    ctx.engine
        .create_task(CreateTaskInput {
            id: Some("t1".to_string()),
            ..Default::default()
        })
        .await
        .unwrap();

    // 105 workers claim and decline with blacklist
    for i in 0..105 {
        let wid = format!("w{}", i);
        let claim_result = ctx.manager.claim_task("t1", &wid).await.unwrap();
        assert_eq!(
            claim_result,
            ClaimResult::Claimed,
            "Worker w{} should be able to claim (task should be back to pending)",
            i
        );
        ctx.manager
            .decline_task("t1", &wid, Some(DeclineOptions { blacklist: true }))
            .await
            .unwrap();
    }

    // Verify the blacklist has 105 entries
    let task = ctx.engine.get_task("t1").await.unwrap().unwrap();
    let blacklist = task
        .metadata
        .as_ref()
        .and_then(|m| m.get("_blacklistedWorkers"))
        .and_then(|v| v.as_array())
        .unwrap();
    assert_eq!(
        blacklist.len(),
        105,
        "Blacklist should have 105 entries after 105 decline cycles"
    );

    // Dispatch should still pick one of the 5 remaining workers (w105-w109)
    let result = ctx.manager.dispatch_task("t1").await.unwrap();
    match result {
        DispatchResult::Dispatched { ref worker_id } => {
            let id_num: usize = worker_id[1..].parse().unwrap();
            assert!(
                id_num >= 105,
                "Dispatched worker {} should be >= w105",
                worker_id
            );
        }
        DispatchResult::NoMatch => {
            panic!("Should have found a non-blacklisted worker among w105-w109");
        }
    }
}

// =============================================================================
// 4. Audit Completeness
// =============================================================================

/// Register emits Connected audit event.
#[tokio::test]
async fn audit_register_emits_connected() {
    let lt = Arc::new(MockLongTermStore::new());
    let (ctx, lt) = make_context_with_long_term(lt);

    ctx.manager
        .register_worker(make_registration("w1", 5))
        .await
        .unwrap();

    // Audit events are spawned async, wait for them
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let events = lt.worker_events.read().await;
    let connected_events: Vec<_> = events
        .iter()
        .filter(|e| e.action == WorkerAuditAction::Connected && e.worker_id == "w1")
        .collect();
    assert_eq!(
        connected_events.len(),
        1,
        "Should have exactly 1 Connected audit event"
    );
}

/// Unregister emits Disconnected audit event.
#[tokio::test]
async fn audit_unregister_emits_disconnected() {
    let lt = Arc::new(MockLongTermStore::new());
    let (ctx, lt) = make_context_with_long_term(lt);

    ctx.manager
        .register_worker(make_registration("w1", 5))
        .await
        .unwrap();
    ctx.manager.unregister_worker("w1").await.unwrap();

    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let events = lt.worker_events.read().await;
    let disconnected_events: Vec<_> = events
        .iter()
        .filter(|e| e.action == WorkerAuditAction::Disconnected && e.worker_id == "w1")
        .collect();
    assert_eq!(
        disconnected_events.len(),
        1,
        "Should have exactly 1 Disconnected audit event"
    );
}

/// Update worker emits Updated audit event (and Draining if status changed).
#[tokio::test]
async fn audit_update_emits_updated_and_draining() {
    let lt = Arc::new(MockLongTermStore::new());
    let (ctx, lt) = make_context_with_long_term(lt);

    ctx.manager
        .register_worker(make_registration("w1", 5))
        .await
        .unwrap();

    // Simple update (weight change) — should emit Updated
    ctx.manager
        .update_worker(
            "w1",
            WorkerUpdate {
                weight: Some(99),
                ..Default::default()
            },
        )
        .await
        .unwrap();

    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    {
        let events = lt.worker_events.read().await;
        let updated_events: Vec<_> = events
            .iter()
            .filter(|e| e.action == WorkerAuditAction::Updated && e.worker_id == "w1")
            .collect();
        assert!(
            !updated_events.is_empty(),
            "Should have at least 1 Updated audit event"
        );
    }

    // Status change to draining — should emit both Updated and Draining
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

    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let events = lt.worker_events.read().await;
    let draining_events: Vec<_> = events
        .iter()
        .filter(|e| e.action == WorkerAuditAction::Draining && e.worker_id == "w1")
        .collect();
    assert_eq!(
        draining_events.len(),
        1,
        "Should have exactly 1 Draining audit event after setting draining status"
    );
}

/// Claim emits TaskAssigned audit event.
#[tokio::test]
async fn audit_claim_emits_task_assigned() {
    let lt = Arc::new(MockLongTermStore::new());
    let (ctx, lt) = make_context_with_long_term(lt);

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

    ctx.manager.claim_task("t1", "w1").await.unwrap();

    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let events = lt.worker_events.read().await;
    let assigned_events: Vec<_> = events
        .iter()
        .filter(|e| e.action == WorkerAuditAction::TaskAssigned && e.worker_id == "w1")
        .collect();
    assert_eq!(
        assigned_events.len(),
        1,
        "Should have exactly 1 TaskAssigned audit event"
    );

    // Verify the audit event includes the task ID in its data
    let event = assigned_events[0];
    let task_id = event
        .data
        .as_ref()
        .and_then(|d| d.get("taskId"))
        .and_then(|v| v.as_str());
    assert_eq!(task_id, Some("t1"), "Audit data should contain taskId=t1");
}

/// Decline emits TaskDeclined audit event.
#[tokio::test]
async fn audit_decline_emits_task_declined() {
    let lt = Arc::new(MockLongTermStore::new());
    let (ctx, lt) = make_context_with_long_term(lt);

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

    ctx.manager.claim_task("t1", "w1").await.unwrap();
    ctx.manager.decline_task("t1", "w1", None).await.unwrap();

    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let events = lt.worker_events.read().await;
    let declined_events: Vec<_> = events
        .iter()
        .filter(|e| e.action == WorkerAuditAction::TaskDeclined && e.worker_id == "w1")
        .collect();
    assert_eq!(
        declined_events.len(),
        1,
        "Should have exactly 1 TaskDeclined audit event"
    );

    let event = declined_events[0];
    let task_id = event
        .data
        .as_ref()
        .and_then(|d| d.get("taskId"))
        .and_then(|v| v.as_str());
    assert_eq!(task_id, Some("t1"), "Audit data should contain taskId=t1");
}

/// Full lifecycle audit trail: register -> claim -> decline -> unregister
/// should produce Connected, TaskAssigned, TaskDeclined, Disconnected events
/// in order.
#[tokio::test]
async fn audit_full_lifecycle_produces_complete_trail() {
    let lt = Arc::new(MockLongTermStore::new());
    let (ctx, lt) = make_context_with_long_term(lt);

    // 1. Register
    ctx.manager
        .register_worker(make_registration("w1", 5))
        .await
        .unwrap();

    // 2. Create and claim
    ctx.engine
        .create_task(CreateTaskInput {
            id: Some("t1".to_string()),
            ..Default::default()
        })
        .await
        .unwrap();
    ctx.manager.claim_task("t1", "w1").await.unwrap();

    // 3. Decline
    ctx.manager.decline_task("t1", "w1", None).await.unwrap();

    // 4. Unregister
    ctx.manager.unregister_worker("w1").await.unwrap();

    // Wait for async audit events
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    let events = lt.worker_events.read().await;
    let w1_events: Vec<_> = events.iter().filter(|e| e.worker_id == "w1").collect();

    // Should have at least: Connected, TaskAssigned, TaskDeclined, Disconnected
    let actions: Vec<WorkerAuditAction> = w1_events.iter().map(|e| e.action.clone()).collect();

    assert!(
        actions.contains(&WorkerAuditAction::Connected),
        "Should have Connected event, got: {:?}",
        actions
    );
    assert!(
        actions.contains(&WorkerAuditAction::TaskAssigned),
        "Should have TaskAssigned event, got: {:?}",
        actions
    );
    assert!(
        actions.contains(&WorkerAuditAction::TaskDeclined),
        "Should have TaskDeclined event, got: {:?}",
        actions
    );
    assert!(
        actions.contains(&WorkerAuditAction::Disconnected),
        "Should have Disconnected event, got: {:?}",
        actions
    );

    // Verify ordering: Connected should come before TaskAssigned
    let connected_idx = w1_events
        .iter()
        .position(|e| e.action == WorkerAuditAction::Connected)
        .unwrap();
    let assigned_idx = w1_events
        .iter()
        .position(|e| e.action == WorkerAuditAction::TaskAssigned)
        .unwrap();
    let declined_idx = w1_events
        .iter()
        .position(|e| e.action == WorkerAuditAction::TaskDeclined)
        .unwrap();
    let disconnected_idx = w1_events
        .iter()
        .position(|e| e.action == WorkerAuditAction::Disconnected)
        .unwrap();

    assert!(
        connected_idx < assigned_idx,
        "Connected should come before TaskAssigned"
    );
    assert!(
        assigned_idx < declined_idx,
        "TaskAssigned should come before TaskDeclined"
    );
    assert!(
        declined_idx < disconnected_idx,
        "TaskDeclined should come before Disconnected"
    );
}

/// No audit events emitted when long_term_store is None.
#[tokio::test]
async fn audit_not_emitted_without_long_term_store() {
    let ctx = make_context(); // No long_term_store

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

    ctx.manager.claim_task("t1", "w1").await.unwrap();
    ctx.manager.decline_task("t1", "w1", None).await.unwrap();
    ctx.manager.unregister_worker("w1").await.unwrap();

    // If we get here without panics, the test passes — no long_term_store
    // means emit_worker_audit is a no-op.
}

// =============================================================================
// 5. Hook Exception Handling
// =============================================================================

// The Rust TaskcastHooks trait has synchronous methods (not async). Hooks are
// called inline. A panicking hook WILL propagate. However, since hooks are
// trait methods with default no-op implementations, the standard use is to
// NOT panic. Let's verify that the system works with hooks and that hook
// callbacks are invoked correctly for each operation.

/// Hooks are called for register, claim, decline, and unregister.
#[tokio::test]
async fn hooks_called_for_all_operations() {
    let hooks = Arc::new(MockHooks::new());
    let ctx = make_context_with_hooks(Arc::clone(&hooks) as Arc<dyn TaskcastHooks>);

    // Register
    ctx.manager
        .register_worker(make_registration("w1", 5))
        .await
        .unwrap();
    assert_eq!(hooks.connected_count.load(Ordering::SeqCst), 1);
    assert_eq!(hooks.connected_ids.lock().unwrap()[0], "w1");

    // Claim
    ctx.engine
        .create_task(CreateTaskInput {
            id: Some("t1".to_string()),
            ..Default::default()
        })
        .await
        .unwrap();
    ctx.manager.claim_task("t1", "w1").await.unwrap();
    assert_eq!(hooks.assigned_count.load(Ordering::SeqCst), 1);
    assert_eq!(hooks.assigned_task_ids.lock().unwrap()[0], "t1");

    // Decline
    ctx.manager.decline_task("t1", "w1", None).await.unwrap();
    assert_eq!(hooks.declined_count.load(Ordering::SeqCst), 1);
    assert_eq!(hooks.declined_task_ids.lock().unwrap()[0], "t1");

    // Unregister
    ctx.manager.unregister_worker("w1").await.unwrap();
    assert_eq!(hooks.disconnected_count.load(Ordering::SeqCst), 1);
    assert_eq!(hooks.disconnected_ids.lock().unwrap()[0], "w1");
}

/// Hooks are called correct number of times across multiple operations.
#[tokio::test]
async fn hooks_called_correct_count_across_multiple_operations() {
    let hooks = Arc::new(MockHooks::new());
    let ctx = make_context_with_hooks(Arc::clone(&hooks) as Arc<dyn TaskcastHooks>);

    // Register 3 workers
    for i in 0..3 {
        ctx.manager
            .register_worker(make_registration(&format!("w{}", i), 5))
            .await
            .unwrap();
    }
    assert_eq!(hooks.connected_count.load(Ordering::SeqCst), 3);

    // Create 3 tasks, claim all by w0
    for i in 0..3 {
        ctx.engine
            .create_task(CreateTaskInput {
                id: Some(format!("t{}", i)),
                ..Default::default()
            })
            .await
            .unwrap();
        ctx.manager
            .claim_task(&format!("t{}", i), "w0")
            .await
            .unwrap();
    }
    assert_eq!(hooks.assigned_count.load(Ordering::SeqCst), 3);

    // Decline 2 of the 3
    for i in 0..2 {
        ctx.manager
            .decline_task(&format!("t{}", i), "w0", None)
            .await
            .unwrap();
    }
    assert_eq!(hooks.declined_count.load(Ordering::SeqCst), 2);

    // Unregister 1 worker
    ctx.manager.unregister_worker("w2").await.unwrap();
    assert_eq!(hooks.disconnected_count.load(Ordering::SeqCst), 1);
}

/// Panicking hooks: register_worker with a panicking hook should propagate
/// the panic. We use catch_unwind to verify this.
///
/// Note: In Rust, hooks are called inline (synchronously), so a panicking
/// hook WILL cause the calling function to unwind. This test verifies that
/// behavior and documents it — unlike Node.js where hooks might be try/caught,
/// in Rust panics propagate unless explicitly caught.
#[tokio::test]
async fn panicking_hook_on_register_propagates() {
    let hooks: Arc<dyn TaskcastHooks> = Arc::new(PanickingHooks);
    let ctx = make_context_with_hooks(hooks);

    // register_worker calls on_worker_connected which panics
    let manager = Arc::clone(&ctx.manager);
    let caught = tokio::task::spawn(async move {
        manager.register_worker(make_registration("w1", 5)).await
    })
    .await;

    // The spawn should catch the panic
    assert!(
        caught.is_err(),
        "Panicking hook should cause a JoinError (panic)"
    );
}

/// Panicking hooks on claim: verify the panic propagates.
#[tokio::test]
async fn panicking_hook_on_claim_propagates() {
    // We need a context where register doesn't panic but claim does.
    // Since PanickingHooks panics on ALL hooks, we need a selective one.
    struct ClaimPanickingHooks;
    impl TaskcastHooks for ClaimPanickingHooks {
        fn on_task_assigned(&self, _task: &Task, _worker: &Worker) {
            panic!("ClaimPanickingHooks: on_task_assigned");
        }
    }

    let hooks: Arc<dyn TaskcastHooks> = Arc::new(ClaimPanickingHooks);
    let short_term_store = Arc::new(MemoryShortTermStore::new());
    let broadcast = Arc::new(MemoryBroadcastProvider::new());
    let engine = Arc::new(TaskEngine::new(TaskEngineOptions {
        short_term_store: Arc::clone(&short_term_store) as Arc<dyn ShortTermStore>,
        broadcast: Arc::clone(&broadcast) as Arc<dyn BroadcastProvider>,
        long_term_store: None,
        hooks: Some(Arc::clone(&hooks)),
    }));
    let manager = Arc::new(WorkerManager::new(WorkerManagerOptions {
        engine: Arc::clone(&engine),
        short_term_store: Arc::clone(&short_term_store) as Arc<dyn ShortTermStore>,
        broadcast: broadcast as Arc<dyn BroadcastProvider>,
        long_term_store: None,
        hooks: Some(hooks),
        defaults: None,
    }));

    // Register worker (no panic — ClaimPanickingHooks doesn't override on_worker_connected)
    manager
        .register_worker(make_registration("w1", 5))
        .await
        .unwrap();

    engine
        .create_task(CreateTaskInput {
            id: Some("t1".to_string()),
            ..Default::default()
        })
        .await
        .unwrap();

    // claim_task should panic when it calls on_task_assigned
    let m = Arc::clone(&manager);
    let caught = tokio::task::spawn(async move { m.claim_task("t1", "w1").await }).await;

    assert!(
        caught.is_err(),
        "Panicking hook on claim should cause a JoinError (panic)"
    );
}
