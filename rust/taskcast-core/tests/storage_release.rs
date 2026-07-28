use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use taskcast_core::{
    compute_archive_batch_digest, compute_archive_source_digest,
    compute_archive_source_page_digest, compute_series_state_digest, ArchiveBatch,
    ArchiveBatchReceipt, ArchiveGeneration, ArchiveGenerationStatus, ArchiveSourceManifest,
    ConnectionMode, CreateTaskInput, EngineError, HotWriteToken, Level, LongTermStore,
    MemoryBroadcastProvider, MemoryLongTermStore, MemoryShortTermStore, PublishEventInput,
    ReleasePreconditions, SeriesMode, ShortTermStore, StorageCoordinator, StorageIntegrityError,
    StorageState, Task, TaskEngine, TaskEngineOptions, TaskEvent, TaskStatus,
    TaskStorageMetadataCas, Worker, WorkerAuditEvent, WorkerMatchRule, WorkerStatus,
};

fn make_task() -> Task {
    Task {
        id: "task-1".to_string(),
        r#type: None,
        status: TaskStatus::Running,
        params: None,
        result: None,
        error: None,
        metadata: None,
        created_at: 1_000.0,
        updated_at: 1_000.0,
        completed_at: None,
        ttl: None,
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
    }
}

fn make_event(index: u64) -> TaskEvent {
    TaskEvent {
        id: format!("event-{index}"),
        task_id: "task-1".to_string(),
        index,
        timestamp: 2_000.0 + index as f64,
        r#type: "llm.delta".to_string(),
        level: Level::Info,
        data: serde_json::json!({ "delta": index.to_string() }),
        series_id: None,
        series_mode: None,
        series_acc_field: None,
        series_snapshot: None,
        _accumulated_data: None,
    }
}

fn make_worker(id: &str) -> Worker {
    Worker {
        id: id.to_string(),
        status: WorkerStatus::Idle,
        match_rule: WorkerMatchRule::default(),
        capacity: 2,
        used_slots: 0,
        weight: 1,
        connection_mode: ConnectionMode::Pull,
        connected_at: 1_000.0,
        last_heartbeat_at: 1_000.0,
        metadata: None,
    }
}

struct LostCompleteResponseStore {
    inner: MemoryLongTermStore,
    attempts: AtomicU64,
}

#[async_trait::async_trait]
impl LongTermStore for LostCompleteResponseStore {
    fn supports_task_creation_claims(&self) -> bool {
        true
    }

    async fn save_task(&self, task: Task) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        self.inner.save_task(task).await
    }

    async fn claim_task_creation(
        &self,
        task: Task,
        creation_token: &str,
        claim_ttl_ms: u64,
    ) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
        self.inner
            .claim_task_creation(task, creation_token, claim_ttl_ms)
            .await
    }

    async fn complete_task_creation(
        &self,
        task_id: &str,
        creation_token: &str,
    ) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
        let completed = self
            .inner
            .complete_task_creation(task_id, creation_token)
            .await?;
        if self.attempts.fetch_add(1, Ordering::SeqCst) == 0 {
            return Err(Box::new(std::io::Error::new(
                std::io::ErrorKind::ConnectionReset,
                "connection lost after commit",
            )));
        }
        Ok(completed)
    }

    async fn abort_task_creation(
        &self,
        task_id: &str,
        creation_token: &str,
    ) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
        self.inner
            .abort_task_creation(task_id, creation_token)
            .await
    }

    async fn get_task(
        &self,
        task_id: &str,
    ) -> Result<Option<Task>, Box<dyn std::error::Error + Send + Sync>> {
        self.inner.get_task(task_id).await
    }

    async fn save_event(
        &self,
        event: TaskEvent,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        self.inner.save_event(event).await
    }

    async fn get_events(
        &self,
        task_id: &str,
        opts: Option<taskcast_core::EventQueryOptions>,
    ) -> Result<Vec<TaskEvent>, Box<dyn std::error::Error + Send + Sync>> {
        self.inner.get_events(task_id, opts).await
    }

    async fn save_worker_event(
        &self,
        event: WorkerAuditEvent,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        self.inner.save_worker_event(event).await
    }

    async fn get_worker_events(
        &self,
        worker_id: &str,
        opts: Option<taskcast_core::EventQueryOptions>,
    ) -> Result<Vec<WorkerAuditEvent>, Box<dyn std::error::Error + Send + Sync>> {
        self.inner.get_worker_events(worker_id, opts).await
    }
}

#[tokio::test]
async fn release_deletes_hot_storage_only_after_the_durable_watermark() {
    let hot = Arc::new(MemoryShortTermStore::new());
    let durable = Arc::new(MemoryLongTermStore::new());
    hot.save_task(make_task()).await.unwrap();
    durable.save_task(make_task()).await.unwrap();
    let token = HotWriteToken {
        task_id: "task-1".to_string(),
        storage_epoch: 1,
    };
    for index in 0..3 {
        let committed = hot
            .commit_event_fenced("task-1", make_event(index), &token)
            .await
            .unwrap();
        durable.save_event(committed.event).await.unwrap();
    }

    let next_id = Arc::new(AtomicU64::new(0));
    let coordinator = StorageCoordinator::new(hot.clone(), durable.clone())
        .with_archive_batch_size(2)
        .with_storage_lock_ttl_ms(5_000)
        .with_id_generator({
            let next_id = Arc::clone(&next_id);
            move || format!("generation-{}", next_id.fetch_add(1, Ordering::SeqCst) + 1)
        });
    let released = coordinator
        .release_task_storage(
            "task-1",
            ReleasePreconditions {
                expected_last_event_index: 2,
                inactive_since: 3_000.0,
            },
        )
        .await
        .unwrap();

    assert_eq!(released.storage_state, StorageState::Cold);
    assert_eq!(released.archive_watermark, 2);
    assert!(released.released);
    let presence = hot.get_task_storage_presence("task-1").await.unwrap();
    assert!(!presence.task);
    assert_eq!(presence.event_count, 0);
    assert!(!presence.write_fence);
    assert_eq!(durable.get_archive_watermark("task-1").await.unwrap(), 2);
    assert_eq!(
        durable
            .get_events("task-1", None)
            .await
            .unwrap()
            .iter()
            .map(|event| event.id.as_str())
            .collect::<Vec<_>>(),
        vec!["event-0", "event-1", "event-2"]
    );
}

#[tokio::test]
async fn engine_routes_mutations_and_release_through_the_storage_coordinator() {
    let hot = Arc::new(MemoryShortTermStore::new());
    let durable = Arc::new(MemoryLongTermStore::new());
    let engine = TaskEngine::new(TaskEngineOptions {
        short_term_store: hot,
        long_term_store: Some(durable),
        broadcast: Arc::new(MemoryBroadcastProvider::new()),
        hooks: None,
    });
    engine
        .create_task(CreateTaskInput {
            id: Some("task-1".to_string()),
            ..Default::default()
        })
        .await
        .unwrap();
    engine
        .transition_task("task-1", TaskStatus::Running, None)
        .await
        .unwrap();
    let event = engine
        .publish_event(
            "task-1",
            PublishEventInput {
                r#type: "llm.delta".to_string(),
                level: Level::Info,
                data: serde_json::json!({ "delta": "hello" }),
                series_id: None,
                series_mode: None,
                series_acc_field: None,
            },
        )
        .await
        .unwrap();

    let released = engine
        .release_task_storage(
            "task-1",
            ReleasePreconditions {
                expected_last_event_index: event.index as i64,
                inactive_since: event.timestamp,
            },
        )
        .await
        .unwrap();
    assert_eq!(released.storage_state, StorageState::Cold);
    assert_eq!(released.archive_watermark, event.index as i64);
}

#[tokio::test]
async fn engine_recovers_an_interrupted_release_before_explicit_retry() {
    let hot = Arc::new(MemoryShortTermStore::new());
    let durable = Arc::new(MemoryLongTermStore::new());
    let engine = TaskEngine::new(TaskEngineOptions {
        short_term_store: hot.clone(),
        long_term_store: Some(durable.clone()),
        broadcast: Arc::new(MemoryBroadcastProvider::new()),
        hooks: None,
    });
    engine
        .create_task(CreateTaskInput {
            id: Some("task-1".to_string()),
            ..Default::default()
        })
        .await
        .unwrap();
    engine
        .transition_task("task-1", TaskStatus::Running, None)
        .await
        .unwrap();
    let event = engine
        .publish_event(
            "task-1",
            PublishEventInput {
                r#type: "llm.delta".to_string(),
                level: Level::Info,
                data: serde_json::json!({ "delta": "canary" }),
                series_id: None,
                series_mode: None,
                series_acc_field: None,
            },
        )
        .await
        .unwrap();
    let preconditions = ReleasePreconditions {
        expected_last_event_index: event.index as i64,
        inactive_since: event.timestamp,
    };

    engine
        .release_task_storage("task-1", preconditions.clone())
        .await
        .unwrap();
    let metadata = durable
        .get_task_storage_metadata("task-1")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(metadata.storage_state, StorageState::Cold);
    assert!(durable
        .compare_and_set_task_storage_metadata(TaskStorageMetadataCas {
            task_id: "task-1".to_string(),
            expected_storage_state: StorageState::Cold,
            expected_storage_epoch: metadata.storage_epoch,
            expected_release_generation: None,
            next: taskcast_core::TaskStorageMetadata {
                storage_state: StorageState::Releasing,
                active_release_generation: Some("interrupted-generation".to_string()),
                cold_at: None,
                ..metadata
            },
        })
        .await
        .unwrap());

    let released = engine
        .release_task_storage("task-1", preconditions)
        .await
        .unwrap();

    assert_eq!(released.storage_state, StorageState::Cold);
    assert!(released.released);
    assert!(durable
        .list_storage_release_requests(10)
        .await
        .unwrap()
        .is_empty());
    let presence = hot.get_task_storage_presence("task-1").await.unwrap();
    assert!(!presence.task);
    assert_eq!(presence.event_count, 0);
    assert!(!presence.write_fence);
}

#[tokio::test]
async fn engine_allows_only_one_concurrent_terminal_transition() {
    let hot = Arc::new(MemoryShortTermStore::new());
    let durable = Arc::new(MemoryLongTermStore::new());
    let first = TaskEngine::new(TaskEngineOptions {
        short_term_store: hot.clone(),
        long_term_store: Some(durable.clone()),
        broadcast: Arc::new(MemoryBroadcastProvider::new()),
        hooks: None,
    });
    let second = TaskEngine::new(TaskEngineOptions {
        short_term_store: hot.clone(),
        long_term_store: Some(durable),
        broadcast: Arc::new(MemoryBroadcastProvider::new()),
        hooks: None,
    });
    first
        .create_task(CreateTaskInput {
            id: Some("task-1".to_string()),
            ..Default::default()
        })
        .await
        .unwrap();
    first
        .transition_task("task-1", TaskStatus::Running, None)
        .await
        .unwrap();

    let completed = first.transition_task("task-1", TaskStatus::Completed, None);
    let cancelled = second.transition_task("task-1", TaskStatus::Cancelled, None);
    let (completed, cancelled) = tokio::join!(completed, cancelled);

    assert_eq!(
        [completed.is_ok(), cancelled.is_ok()]
            .into_iter()
            .filter(|succeeded| *succeeded)
            .count(),
        1
    );
    let events = hot.get_events("task-1", None).await.unwrap();
    assert_eq!(
        events
            .iter()
            .filter(|event| event.r#type == "taskcast:status")
            .count(),
        2
    );
    assert!(matches!(
        hot.get_task("task-1").await.unwrap().unwrap().status,
        TaskStatus::Completed | TaskStatus::Cancelled
    ));
}

#[tokio::test]
async fn memory_durable_creation_claim_can_be_aborted_and_retried() {
    let durable = MemoryLongTermStore::new();
    let mut task = make_task();
    task.status = TaskStatus::Pending;
    assert!(durable
        .claim_task_creation(task.clone(), "token-1", 30_000)
        .await
        .unwrap());
    assert!(!durable
        .abort_task_creation("task-1", "wrong-token")
        .await
        .unwrap());
    assert!(durable
        .abort_task_creation("task-1", "token-1")
        .await
        .unwrap());
    assert!(durable.get_task("task-1").await.unwrap().is_none());
    assert!(durable
        .claim_task_creation(task, "token-2", 30_000)
        .await
        .unwrap());
}

#[tokio::test]
async fn stale_task_mutation_is_rejected_after_same_status_worker_reclaim() {
    let hot = MemoryShortTermStore::new();
    let mut task = make_task();
    task.status = TaskStatus::Pending;
    hot.save_task(task).await.unwrap();
    hot.save_worker(make_worker("worker-a")).await.unwrap();
    hot.save_worker(make_worker("worker-b")).await.unwrap();
    assert!(hot.claim_task("task-1", "worker-a", 1).await.unwrap());
    let snapshot = hot
        .get_task_mutation_snapshot("task-1")
        .await
        .unwrap()
        .unwrap();
    assert!(hot.claim_task("task-1", "worker-b", 1).await.unwrap());
    let mut running = snapshot.task;
    running.status = TaskStatus::Running;
    let token = HotWriteToken {
        task_id: "task-1".to_string(),
        storage_epoch: 1,
    };
    let mut status_event = make_event(0);
    status_event.r#type = "taskcast:status".to_string();

    assert!(hot
        .commit_task_events_fenced(running, &snapshot.revision, vec![status_event], &token)
        .await
        .unwrap()
        .is_none());
    let current = hot.get_task("task-1").await.unwrap().unwrap();
    assert_eq!(current.status, TaskStatus::Assigned);
    assert_eq!(current.assigned_worker.as_deref(), Some("worker-b"));
    assert!(hot.get_events("task-1", None).await.unwrap().is_empty());
}

#[tokio::test]
async fn expired_pristine_creation_claim_can_be_taken_over_and_completed_idempotently() {
    let durable = MemoryLongTermStore::new();
    let mut task = make_task();
    task.status = TaskStatus::Pending;
    assert!(durable
        .claim_task_creation(task.clone(), "token-1", 100)
        .await
        .unwrap());
    assert!(!durable
        .claim_task_creation(task.clone(), "token-2", 100)
        .await
        .unwrap());
    tokio::time::sleep(std::time::Duration::from_millis(110)).await;
    assert!(durable
        .claim_task_creation(task, "token-2", 30_000)
        .await
        .unwrap());
    assert!(durable
        .complete_task_creation("task-1", "token-2")
        .await
        .unwrap());
    assert!(durable
        .complete_task_creation("task-1", "token-2")
        .await
        .unwrap());
    assert!(!durable
        .abort_task_creation("task-1", "token-2")
        .await
        .unwrap());
}

#[tokio::test]
async fn engine_recovers_an_expired_claim_left_before_the_hot_save() {
    let hot = Arc::new(MemoryShortTermStore::new());
    let durable = Arc::new(MemoryLongTermStore::new());
    let mut crashed_task = make_task();
    crashed_task.id = "task-crashed-create".to_string();
    crashed_task.status = TaskStatus::Pending;
    assert!(durable
        .claim_task_creation(crashed_task, "crashed-token", 100)
        .await
        .unwrap());
    tokio::time::sleep(std::time::Duration::from_millis(110)).await;
    let engine = TaskEngine::new(TaskEngineOptions {
        short_term_store: hot.clone(),
        long_term_store: Some(durable),
        broadcast: Arc::new(MemoryBroadcastProvider::new()),
        hooks: None,
    });

    let created = engine
        .create_task(CreateTaskInput {
            id: Some("task-crashed-create".to_string()),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(created.id, "task-crashed-create");
    assert!(hot.get_task("task-crashed-create").await.unwrap().is_some());
}

#[tokio::test]
async fn engine_retries_an_idempotent_creation_complete_after_response_loss() {
    let durable = Arc::new(LostCompleteResponseStore {
        inner: MemoryLongTermStore::new(),
        attempts: AtomicU64::new(0),
    });
    let engine = TaskEngine::new(TaskEngineOptions {
        short_term_store: Arc::new(MemoryShortTermStore::new()),
        long_term_store: Some(durable.clone()),
        broadcast: Arc::new(MemoryBroadcastProvider::new()),
        hooks: None,
    });

    let created = engine
        .create_task(CreateTaskInput {
            id: Some("task-idempotent".to_string()),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(created.id, "task-idempotent");
    assert_eq!(durable.attempts.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn stale_accumulate_write_is_ignored_after_archive_watermark_advances() {
    let durable = MemoryLongTermStore::new();
    durable.save_task(make_task()).await.unwrap();
    let mut event = make_event(0);
    event.series_id = Some("output".to_string());
    event.series_mode = Some(SeriesMode::Accumulate);
    event.series_acc_field = Some("delta".to_string());
    event.data = serde_json::json!({ "delta": "a" });
    let first = durable
        .accumulate_series("task-1", "output", event.clone(), "delta")
        .await
        .unwrap();
    assert_eq!(first.data, serde_json::json!({ "delta": "a" }));
    let metadata = durable
        .get_task_storage_metadata("task-1")
        .await
        .unwrap()
        .unwrap();
    let mut next = metadata.clone();
    next.archive_watermark = 0;
    assert!(durable
        .compare_and_set_task_storage_metadata(TaskStorageMetadataCas {
            task_id: "task-1".to_string(),
            expected_storage_state: metadata.storage_state,
            expected_storage_epoch: metadata.storage_epoch,
            expected_release_generation: metadata.active_release_generation,
            next,
        })
        .await
        .unwrap());

    let stale = durable
        .accumulate_series("task-1", "output", event, "delta")
        .await
        .unwrap();
    assert_eq!(stale.data, serde_json::json!({ "delta": "a" }));
}

#[tokio::test]
async fn recovery_invalidates_a_stale_executor_and_reopens_retained_hot_storage() {
    let hot = Arc::new(MemoryShortTermStore::new());
    let durable = Arc::new(MemoryLongTermStore::new());
    hot.save_task(make_task()).await.unwrap();
    durable.save_task(make_task()).await.unwrap();
    let token = HotWriteToken {
        task_id: "task-1".to_string(),
        storage_epoch: 1,
    };
    for index in 0..3 {
        let committed = hot
            .commit_event_fenced("task-1", make_event(index), &token)
            .await
            .unwrap();
        durable.save_event(committed.event).await.unwrap();
    }
    let stale = hot
        .acquire_storage_lock("task-1", "stale-lock", "stale-generation", 5_000)
        .await
        .unwrap()
        .unwrap();
    hot.close_write_fence(&stale, 1).await.unwrap();
    let metadata = durable
        .get_task_storage_metadata("task-1")
        .await
        .unwrap()
        .unwrap();
    durable
        .compare_and_set_task_storage_metadata(TaskStorageMetadataCas {
            task_id: "task-1".to_string(),
            expected_storage_state: StorageState::Hot,
            expected_storage_epoch: 1,
            expected_release_generation: None,
            next: taskcast_core::TaskStorageMetadata {
                storage_state: StorageState::Releasing,
                active_release_generation: Some("stale-generation".to_string()),
                ..metadata
            },
        })
        .await
        .unwrap();
    hot.release_storage_lock(&stale).await.unwrap();

    let next_id = Arc::new(AtomicU64::new(0));
    let coordinator = StorageCoordinator::new(hot.clone(), durable.clone()).with_id_generator({
        let next_id = Arc::clone(&next_id);
        move || format!("recovery-{}", next_id.fetch_add(1, Ordering::SeqCst) + 1)
    });
    let recovered = coordinator.recover_task_storage("task-1").await.unwrap();
    assert_eq!(recovered.storage_state, StorageState::Hot);
    assert!(!recovered.released);
    let fence = hot.get_write_fence("task-1").await.unwrap().unwrap();
    assert!(fence.accepting_writes);
    assert_eq!(fence.storage_epoch, 2);
    let metadata = durable
        .get_task_storage_metadata("task-1")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(metadata.storage_state, StorageState::Hot);
    assert_eq!(metadata.storage_epoch, 2);
    assert!(hot.delete_task_storage_fenced(&stale, 1).await.is_err());
}

#[tokio::test]
async fn recovery_repairs_releasing_metadata_after_redis_reopened_a_newer_epoch() {
    let hot = Arc::new(MemoryShortTermStore::new());
    let durable = Arc::new(MemoryLongTermStore::new());
    hot.save_task(make_task()).await.unwrap();
    durable.save_task(make_task()).await.unwrap();
    let stale = hot
        .acquire_storage_lock("task-1", "stale-lock", "stale-generation", 5_000)
        .await
        .unwrap()
        .unwrap();
    hot.close_write_fence(&stale, 1).await.unwrap();
    let metadata = durable
        .get_task_storage_metadata("task-1")
        .await
        .unwrap()
        .unwrap();
    durable
        .compare_and_set_task_storage_metadata(TaskStorageMetadataCas {
            task_id: "task-1".to_string(),
            expected_storage_state: StorageState::Hot,
            expected_storage_epoch: 1,
            expected_release_generation: None,
            next: taskcast_core::TaskStorageMetadata {
                storage_state: StorageState::Releasing,
                active_release_generation: Some("stale-generation".to_string()),
                ..metadata
            },
        })
        .await
        .unwrap();
    hot.reopen_write_fence(&stale, 1).await.unwrap();
    hot.release_storage_lock(&stale).await.unwrap();

    let coordinator = StorageCoordinator::new(hot, durable.clone());
    let recovered = coordinator.recover_task_storage("task-1").await.unwrap();
    assert_eq!(recovered.storage_state, StorageState::Hot);
    let metadata = durable
        .get_task_storage_metadata("task-1")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(metadata.storage_state, StorageState::Hot);
    assert_eq!(metadata.storage_epoch, 2);
    assert!(metadata.active_release_generation.is_none());
}

#[tokio::test]
async fn engine_does_not_recreate_a_cold_explicit_task_id() {
    let hot = Arc::new(MemoryShortTermStore::new());
    let durable = Arc::new(MemoryLongTermStore::new());
    let engine = TaskEngine::new(TaskEngineOptions {
        short_term_store: hot.clone(),
        long_term_store: Some(durable),
        broadcast: Arc::new(MemoryBroadcastProvider::new()),
        hooks: None,
    });
    engine
        .create_task(CreateTaskInput {
            id: Some("task-1".to_string()),
            ..Default::default()
        })
        .await
        .unwrap();
    engine
        .transition_task("task-1", TaskStatus::Running, None)
        .await
        .unwrap();
    let status = hot.get_events("task-1", None).await.unwrap().remove(0);
    engine
        .release_task_storage(
            "task-1",
            ReleasePreconditions {
                expected_last_event_index: status.index as i64,
                inactive_since: status.timestamp,
            },
        )
        .await
        .unwrap();

    let error = engine
        .create_task(CreateTaskInput {
            id: Some("task-1".to_string()),
            ..Default::default()
        })
        .await
        .unwrap_err();
    assert!(matches!(error, EngineError::TaskConflict(ref id) if id == "task-1"));
}

#[tokio::test]
async fn memory_long_term_store_compacts_latest_and_accumulated_series() {
    let durable = MemoryLongTermStore::new();
    durable.save_task(make_task()).await.unwrap();
    let mut first = make_event(0);
    first.series_id = Some("output".to_string());
    first.series_mode = Some(SeriesMode::Accumulate);
    first.series_acc_field = Some("delta".to_string());
    first.data = serde_json::json!({ "delta": "hello" });
    let mut second = make_event(1);
    second.series_id = Some("output".to_string());
    second.series_mode = Some(SeriesMode::Accumulate);
    second.series_acc_field = Some("delta".to_string());
    second.data = serde_json::json!({ "delta": " world" });
    durable
        .accumulate_series("task-1", "output", first, "delta")
        .await
        .unwrap();
    let accumulated = durable
        .accumulate_series("task-1", "output", second, "delta")
        .await
        .unwrap();

    let mut old_latest = make_event(2);
    old_latest.series_id = Some("status".to_string());
    old_latest.series_mode = Some(SeriesMode::Latest);
    let mut new_latest = make_event(3);
    new_latest.series_id = Some("status".to_string());
    new_latest.series_mode = Some(SeriesMode::Latest);
    durable
        .replace_last_series_event("task-1", "status", old_latest)
        .await
        .unwrap();
    durable
        .replace_last_series_event("task-1", "status", new_latest)
        .await
        .unwrap();

    assert!(durable.supports_series_compaction());
    assert_eq!(
        accumulated.data,
        serde_json::json!({ "delta": "hello world" })
    );
    let events = durable.get_events("task-1", None).await.unwrap();
    assert_eq!(
        events.iter().map(|event| event.index).collect::<Vec<_>>(),
        vec![1, 3]
    );
}

#[tokio::test]
async fn memory_archive_finalization_replaces_source_series_with_final_states() {
    let durable = MemoryLongTermStore::new();
    durable.save_task(make_task()).await.unwrap();
    let mut source = vec![];
    for index in 0..4 {
        let mut event = make_event(index);
        if index < 2 {
            event.series_id = Some("output".to_string());
            event.series_mode = Some(SeriesMode::Accumulate);
            event.series_acc_field = Some("delta".to_string());
            event.data = serde_json::json!({ "delta": if index == 0 { "a" } else { "b" } });
        } else {
            event.series_id = Some("status".to_string());
            event.series_mode = Some(SeriesMode::Latest);
        }
        source.push(event);
    }
    let mut accumulated = source[1].clone();
    accumulated.data = serde_json::json!({ "delta": "ab" });
    let final_states = vec![
        taskcast_core::DurableSeriesState {
            task_id: "task-1".to_string(),
            series_id: "output".to_string(),
            mode: SeriesMode::Accumulate,
            event: accumulated,
            through_index: 1,
        },
        taskcast_core::DurableSeriesState {
            task_id: "task-1".to_string(),
            series_id: "status".to_string(),
            mode: SeriesMode::Latest,
            event: source[3].clone(),
            through_index: 3,
        },
    ];
    let metadata = durable
        .get_task_storage_metadata("task-1")
        .await
        .unwrap()
        .unwrap();
    durable
        .compare_and_set_task_storage_metadata(TaskStorageMetadataCas {
            task_id: "task-1".to_string(),
            expected_storage_state: StorageState::Hot,
            expected_storage_epoch: 1,
            expected_release_generation: None,
            next: taskcast_core::TaskStorageMetadata {
                storage_state: StorageState::Releasing,
                active_release_generation: Some("generation-1".to_string()),
                ..metadata
            },
        })
        .await
        .unwrap();
    let page_digest = compute_archive_source_page_digest(&source).unwrap();
    durable
        .begin_archive(ArchiveGeneration {
            task_id: "task-1".to_string(),
            generation: "generation-1".to_string(),
            storage_epoch: 1,
            target_watermark: 3,
            manifest: ArchiveSourceManifest {
                prior_watermark: -1,
                target_watermark: 3,
                source_entry_count: 4,
                source_digest: compute_archive_source_digest(&[page_digest]),
                series_state_digest: compute_series_state_digest(&final_states).unwrap(),
                expected_batch_ordinals: vec![0],
            },
            status: ArchiveGenerationStatus::Open,
            created_at: 1.0,
            updated_at: 1.0,
        })
        .await
        .unwrap();
    let batch_digest = compute_archive_batch_digest(None, &source, &[]).unwrap();
    durable
        .archive_batch(
            "task-1",
            "generation-1",
            ArchiveBatch {
                receipt: ArchiveBatchReceipt {
                    task_id: "task-1".to_string(),
                    generation: "generation-1".to_string(),
                    ordinal: 0,
                    previous_batch_digest: None,
                    batch_digest,
                    entry_count: 4,
                    first_index: Some(0),
                    last_index: Some(3),
                },
                events: source,
                series_latest: vec![],
            },
        )
        .await
        .unwrap();
    durable
        .finalize_archive("task-1", "generation-1", make_task(), final_states)
        .await
        .unwrap();

    let events = durable.get_events("task-1", None).await.unwrap();
    assert_eq!(
        events.iter().map(|event| event.index).collect::<Vec<_>>(),
        vec![1, 3]
    );
    assert_eq!(events[0].data, serde_json::json!({ "delta": "ab" }));
}

#[tokio::test]
async fn memory_archive_batch_rejects_a_superseded_release_generation() {
    let durable = MemoryLongTermStore::new();
    durable.save_task(make_task()).await.unwrap();
    let metadata = durable
        .get_task_storage_metadata("task-1")
        .await
        .unwrap()
        .unwrap();
    let releasing = taskcast_core::TaskStorageMetadata {
        storage_state: StorageState::Releasing,
        active_release_generation: Some("generation-1".to_string()),
        ..metadata
    };
    durable
        .compare_and_set_task_storage_metadata(TaskStorageMetadataCas {
            task_id: "task-1".to_string(),
            expected_storage_state: StorageState::Hot,
            expected_storage_epoch: 1,
            expected_release_generation: None,
            next: releasing.clone(),
        })
        .await
        .unwrap();
    let event = make_event(0);
    let page_digest = compute_archive_source_page_digest(std::slice::from_ref(&event)).unwrap();
    let manifest = ArchiveSourceManifest {
        prior_watermark: -1,
        target_watermark: 0,
        source_entry_count: 1,
        source_digest: compute_archive_source_digest(&[page_digest]),
        series_state_digest: compute_series_state_digest(&[]).unwrap(),
        expected_batch_ordinals: vec![0],
    };
    durable
        .begin_archive(ArchiveGeneration {
            task_id: "task-1".to_string(),
            generation: "generation-1".to_string(),
            storage_epoch: 1,
            target_watermark: 0,
            manifest,
            status: ArchiveGenerationStatus::Open,
            created_at: 1.0,
            updated_at: 1.0,
        })
        .await
        .unwrap();
    durable
        .compare_and_set_task_storage_metadata(TaskStorageMetadataCas {
            task_id: "task-1".to_string(),
            expected_storage_state: StorageState::Releasing,
            expected_storage_epoch: 1,
            expected_release_generation: Some("generation-1".to_string()),
            next: taskcast_core::TaskStorageMetadata {
                active_release_generation: Some("generation-2".to_string()),
                ..releasing
            },
        })
        .await
        .unwrap();
    let batch_digest =
        compute_archive_batch_digest(None, std::slice::from_ref(&event), &[]).unwrap();
    let error = durable
        .archive_batch(
            "task-1",
            "generation-1",
            ArchiveBatch {
                receipt: ArchiveBatchReceipt {
                    task_id: "task-1".to_string(),
                    generation: "generation-1".to_string(),
                    ordinal: 0,
                    previous_batch_digest: None,
                    batch_digest,
                    entry_count: 1,
                    first_index: Some(0),
                    last_index: Some(0),
                },
                events: vec![event],
                series_latest: vec![],
            },
        )
        .await
        .unwrap_err();
    assert!(error.downcast_ref::<StorageIntegrityError>().is_some());
}
