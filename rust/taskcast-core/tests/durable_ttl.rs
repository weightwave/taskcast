use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use taskcast_core::{
    BroadcastProvider, ConnectionMode, LongTermStore, MemoryBroadcastProvider, MemoryLongTermStore,
    MemoryShortTermStore, ReleasePreconditions, ShortTermStore, StorageCoordinator, Task,
    TaskEngine, TaskEngineOptions, TaskEvent, TaskStatus, TaskStorageMetadataCas, TaskcastHooks,
    Worker, WorkerAssignment, WorkerAssignmentStatus, WorkerMatchRule, WorkerStatus,
};

struct TimeoutHooks {
    timeouts: Arc<AtomicUsize>,
}

impl TaskcastHooks for TimeoutHooks {
    fn on_task_timeout(&self, _task: &Task) {
        self.timeouts.fetch_add(1, Ordering::SeqCst);
    }
}

fn now_ms() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as f64
}

fn make_task(id: &str, status: TaskStatus) -> Task {
    let now = now_ms();
    Task {
        id: id.to_string(),
        r#type: None,
        status,
        params: None,
        result: None,
        error: None,
        metadata: None,
        auth_config: None,
        webhooks: None,
        cleanup: None,
        tags: None,
        assign_mode: None,
        cost: None,
        assigned_worker: None,
        disconnect_policy: None,
        reason: None,
        resume_at: None,
        blocked_request: None,
        created_at: now,
        updated_at: now,
        completed_at: None,
        ttl: Some(60),
    }
}

async fn seed_task(hot: &MemoryShortTermStore, durable: &MemoryLongTermStore, task: Task) {
    hot.save_task(task.clone()).await.unwrap();
    durable.save_task(task).await.unwrap();
}

async fn mark_overdue(durable: &MemoryLongTermStore, task_id: &str) {
    let metadata = durable
        .get_task_storage_metadata(task_id)
        .await
        .unwrap()
        .unwrap();
    let mut next = metadata.clone();
    next.execution_deadline_at = Some(now_ms() - 1.0);
    assert!(durable
        .compare_and_set_task_storage_metadata(TaskStorageMetadataCas {
            task_id: task_id.to_string(),
            expected_storage_state: metadata.storage_state,
            expected_storage_epoch: metadata.storage_epoch,
            expected_release_generation: metadata.active_release_generation,
            next,
        })
        .await
        .unwrap());
}

fn make_engine(
    hot: Arc<MemoryShortTermStore>,
    durable: Arc<MemoryLongTermStore>,
    broadcast: Arc<MemoryBroadcastProvider>,
) -> TaskEngine {
    let short: Arc<dyn ShortTermStore> = hot;
    let long: Arc<dyn LongTermStore> = durable;
    let events: Arc<dyn BroadcastProvider> = broadcast;
    TaskEngine::new(TaskEngineOptions {
        short_term_store: short,
        long_term_store: Some(long),
        broadcast: events,
        hooks: None,
    })
}

#[tokio::test]
async fn durable_timeout_fires_timeout_hook_and_transition_listener() {
    let hot = Arc::new(MemoryShortTermStore::new());
    let durable = Arc::new(MemoryLongTermStore::new());
    seed_task(&hot, &durable, make_task("ttl-hooks", TaskStatus::Running)).await;
    mark_overdue(&durable, "ttl-hooks").await;
    let timeout_count = Arc::new(AtomicUsize::new(0));
    let listener_count = Arc::new(AtomicUsize::new(0));
    let short: Arc<dyn ShortTermStore> = hot;
    let long: Arc<dyn LongTermStore> = durable;
    let events: Arc<dyn BroadcastProvider> = Arc::new(MemoryBroadcastProvider::new());
    let engine = TaskEngine::new(TaskEngineOptions {
        short_term_store: short,
        long_term_store: Some(long),
        broadcast: events,
        hooks: Some(Arc::new(TimeoutHooks {
            timeouts: Arc::clone(&timeout_count),
        })),
    });
    let observed_listener_count = Arc::clone(&listener_count);
    engine.add_transition_listener(Box::new(move |_task, from, to| {
        assert_eq!(from, &TaskStatus::Running);
        assert_eq!(to, &TaskStatus::Timeout);
        observed_listener_count.fetch_add(1, Ordering::SeqCst);
    }));

    assert_eq!(
        engine.sweep_durable_ttl(1, None).await.unwrap().timed_out,
        1
    );
    assert_eq!(timeout_count.load(Ordering::SeqCst), 1);
    assert_eq!(listener_count.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn times_out_every_non_terminal_state_durably() {
    for (suffix, status) in [
        ("pending", TaskStatus::Pending),
        ("assigned", TaskStatus::Assigned),
        ("running", TaskStatus::Running),
        ("paused", TaskStatus::Paused),
        ("blocked", TaskStatus::Blocked),
    ] {
        let hot = Arc::new(MemoryShortTermStore::new());
        let durable = Arc::new(MemoryLongTermStore::new());
        let task = make_task(&format!("task-{suffix}"), status);
        seed_task(&hot, &durable, task.clone()).await;
        mark_overdue(&durable, &task.id).await;
        let engine = make_engine(
            Arc::clone(&hot),
            Arc::clone(&durable),
            Arc::new(MemoryBroadcastProvider::new()),
        );

        let result = engine.sweep_durable_ttl(10, None).await.unwrap();
        assert_eq!(result.timed_out, 1);
        assert_eq!(result.failed, 0);
        assert_eq!(
            hot.get_task(&task.id).await.unwrap().unwrap().status,
            TaskStatus::Timeout
        );
        assert_eq!(
            durable.get_task(&task.id).await.unwrap().unwrap().status,
            TaskStatus::Timeout
        );
        assert_eq!(durable.get_events(&task.id, None).await.unwrap().len(), 1);
    }
}

#[tokio::test]
async fn two_replicas_and_restart_share_one_durable_claim() {
    let hot = Arc::new(MemoryShortTermStore::new());
    let durable = Arc::new(MemoryLongTermStore::new());
    seed_task(
        &hot,
        &durable,
        make_task("two-replicas", TaskStatus::Running),
    )
    .await;
    mark_overdue(&durable, "two-replicas").await;
    let first = make_engine(
        Arc::clone(&hot),
        Arc::clone(&durable),
        Arc::new(MemoryBroadcastProvider::new()),
    );
    let second = make_engine(
        Arc::clone(&hot),
        Arc::clone(&durable),
        Arc::new(MemoryBroadcastProvider::new()),
    );

    let (first, second) = tokio::join!(
        first.sweep_durable_ttl(10, None),
        second.sweep_durable_ttl(10, None)
    );
    assert_eq!(first.unwrap().timed_out + second.unwrap().timed_out, 1);
    assert_eq!(
        durable
            .get_events("two-replicas", None)
            .await
            .unwrap()
            .len(),
        1
    );
}

#[tokio::test]
async fn restart_rehydrates_and_times_out_a_cold_task() {
    let hot = Arc::new(MemoryShortTermStore::new());
    let durable = Arc::new(MemoryLongTermStore::new());
    let task = make_task("cold-overdue", TaskStatus::Running);
    seed_task(&hot, &durable, task.clone()).await;
    let short: Arc<dyn ShortTermStore> = hot.clone();
    let long: Arc<dyn LongTermStore> = durable.clone();
    let storage = StorageCoordinator::new(short, long);
    storage
        .release_task_storage(
            &task.id,
            ReleasePreconditions {
                expected_last_event_index: -1,
                inactive_since: now_ms(),
            },
        )
        .await
        .unwrap();
    mark_overdue(&durable, &task.id).await;

    let restarted = make_engine(
        Arc::clone(&hot),
        Arc::clone(&durable),
        Arc::new(MemoryBroadcastProvider::new()),
    );
    assert_eq!(
        restarted
            .sweep_durable_ttl(10, None)
            .await
            .unwrap()
            .timed_out,
        1
    );
    assert_eq!(
        hot.get_task(&task.id).await.unwrap().unwrap().status,
        TaskStatus::Timeout
    );
    let metadata = durable
        .get_task_storage_metadata(&task.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(metadata.storage_epoch, 3);
}

#[tokio::test]
async fn repairs_postgres_commit_before_hot_projection_and_settles_capacity_once() {
    let hot = Arc::new(MemoryShortTermStore::new());
    let durable = Arc::new(MemoryLongTermStore::new());
    let mut task = make_task("assigned-timeout", TaskStatus::Assigned);
    task.assigned_worker = Some("worker-1".to_string());
    task.cost = Some(3);
    seed_task(&hot, &durable, task.clone()).await;
    hot.save_worker(Worker {
        id: "worker-1".to_string(),
        status: WorkerStatus::Busy,
        match_rule: WorkerMatchRule::default(),
        capacity: 5,
        used_slots: 3,
        weight: 50,
        connection_mode: ConnectionMode::Pull,
        connected_at: 1_000.0,
        last_heartbeat_at: 1_000.0,
        metadata: None,
    })
    .await
    .unwrap();
    let assignment = WorkerAssignment {
        task_id: task.id.clone(),
        worker_id: "worker-1".to_string(),
        cost: 3,
        assigned_at: 2_000.0,
        status: WorkerAssignmentStatus::Running,
    };
    hot.add_assignment(assignment.clone()).await.unwrap();
    durable
        .save_durable_assignment(assignment.clone())
        .await
        .unwrap();
    mark_overdue(&durable, &task.id).await;
    let claim = durable.claim_overdue_tasks(1, 20).await.unwrap().remove(0);
    let mut timeout = task.clone();
    timeout.status = TaskStatus::Timeout;
    timeout.updated_at = 3_000.0;
    timeout.completed_at = Some(3_000.0);
    timeout.assigned_worker = None;
    let event = TaskEvent {
        id: "timeout-event".to_string(),
        task_id: task.id.clone(),
        index: 0,
        timestamp: 3_000.0,
        r#type: "taskcast:status".to_string(),
        level: taskcast_core::Level::Info,
        data: serde_json::json!({"status": "timeout"}),
        series_id: None,
        series_mode: None,
        series_acc_field: None,
        series_snapshot: None,
        _accumulated_data: None,
    };
    durable
        .terminalize_ttl_claim(claim, timeout, event, Some(assignment))
        .await
        .unwrap()
        .unwrap();
    tokio::time::sleep(Duration::from_millis(25)).await;

    let restarted = make_engine(
        Arc::clone(&hot),
        Arc::clone(&durable),
        Arc::new(MemoryBroadcastProvider::new()),
    );
    let repaired = restarted
        .sweep_terminal_projections(10, Some(1_000))
        .await
        .unwrap();
    assert_eq!(repaired.projected, 1);
    assert_eq!(
        hot.get_task(&task.id).await.unwrap().unwrap().status,
        TaskStatus::Timeout
    );
    assert!(hot.get_task_assignment(&task.id).await.unwrap().is_none());
    assert_eq!(
        hot.get_worker("worker-1")
            .await
            .unwrap()
            .unwrap()
            .used_slots,
        0
    );
    assert_eq!(
        restarted
            .sweep_terminal_projections(10, Some(1_000))
            .await
            .unwrap()
            .claimed,
        0
    );
    assert_eq!(
        hot.get_worker("worker-1")
            .await
            .unwrap()
            .unwrap()
            .used_slots,
        0
    );
}
