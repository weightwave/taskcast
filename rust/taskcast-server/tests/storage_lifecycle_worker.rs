use std::sync::Arc;

use taskcast_core::{
    BroadcastProvider, Level, LongTermStore, MemoryBroadcastProvider, MemoryLongTermStore,
    MemoryShortTermStore, ResolvedStorageLifecycleConfig, ShortTermStore, StorageReleaseRequest,
    StorageState, StorageWriterRegistration, Task, TaskEngine, TaskEngineOptions, TaskEvent,
    TaskStatus, TaskStorageMetadataCas,
};
use taskcast_server::{StorageLifecycleWorker, StorageLifecycleWorkerOptions};

fn task(id: &str, status: TaskStatus) -> Task {
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
        created_at: 1_000.0,
        updated_at: 1_000.0,
        completed_at: None,
        ttl: None,
    }
}

#[tokio::test]
async fn sweeps_durable_ttl_and_retries_persisted_release_requests() {
    let hot = Arc::new(MemoryShortTermStore::new());
    let durable = Arc::new(MemoryLongTermStore::new());
    let short: Arc<dyn ShortTermStore> = hot.clone();
    let long: Arc<dyn LongTermStore> = durable.clone();
    let broadcast: Arc<dyn BroadcastProvider> = Arc::new(MemoryBroadcastProvider::new());
    let engine = Arc::new(TaskEngine::new(TaskEngineOptions {
        short_term_store: short.clone(),
        long_term_store: Some(long),
        broadcast,
        hooks: None,
    }));
    engine
        .register_storage_writer(
            StorageWriterRegistration {
                instance_id: "writer-v2".to_string(),
                storage_protocol_version: 2,
                build: "test".to_string(),
                expires_at: 0.0,
            },
            30_000,
        )
        .await
        .unwrap();

    let mut overdue = task("overdue", TaskStatus::Running);
    overdue.ttl = Some(60);
    hot.save_task(overdue.clone()).await.unwrap();
    durable.save_task(overdue).await.unwrap();
    let metadata = durable
        .get_task_storage_metadata("overdue")
        .await
        .unwrap()
        .unwrap();
    assert!(durable
        .compare_and_set_task_storage_metadata(TaskStorageMetadataCas {
            task_id: "overdue".to_string(),
            expected_storage_state: metadata.storage_state.clone(),
            expected_storage_epoch: metadata.storage_epoch,
            expected_release_generation: metadata.active_release_generation.clone(),
            next: taskcast_core::TaskStorageMetadata {
                execution_deadline_at: Some(0.0),
                ..metadata
            },
        })
        .await
        .unwrap());

    let mut release = task("explicit-release", TaskStatus::Completed);
    release.completed_at = Some(1_000.0);
    hot.save_task(release.clone()).await.unwrap();
    durable.save_task(release).await.unwrap();
    durable
        .persist_storage_release_request(StorageReleaseRequest {
            task_id: "explicit-release".to_string(),
            requested_at: 2_000.0,
            expected_last_event_index: -1,
            inactive_since: 0.0,
        })
        .await
        .unwrap();

    let worker = StorageLifecycleWorker::new(StorageLifecycleWorkerOptions {
        engine: Arc::clone(&engine),
        short_term_store: short,
        config: ResolvedStorageLifecycleConfig {
            hot_retention_enabled: false,
            hot_retention_terminal_seconds: 60,
            hot_retention_idle_seconds: 1,
            rehydrate_replay_events: 1_000,
            storage_lock_ttl_seconds: 30,
            ttl_sweep_interval_seconds: 5,
            ttl_sweep_batch_size: 10,
        },
    });
    let result = worker.tick().await.unwrap();

    assert_eq!(result.ttl.timed_out, 1);
    assert_eq!(
        durable.get_task("overdue").await.unwrap().unwrap().status,
        TaskStatus::Timeout
    );
    assert!(hot.get_task("explicit-release").await.unwrap().is_none());
    assert_eq!(
        durable
            .get_task_storage_metadata("explicit-release")
            .await
            .unwrap()
            .unwrap()
            .storage_state,
        StorageState::Cold
    );
    assert!(durable
        .list_storage_release_requests(10)
        .await
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn automatically_releases_only_old_terminal_tasks_when_enabled() {
    let hot = Arc::new(MemoryShortTermStore::new());
    let durable = Arc::new(MemoryLongTermStore::new());
    let short: Arc<dyn ShortTermStore> = hot.clone();
    let long: Arc<dyn LongTermStore> = durable.clone();
    let engine = Arc::new(TaskEngine::new(TaskEngineOptions {
        short_term_store: short.clone(),
        long_term_store: Some(long),
        broadcast: Arc::new(MemoryBroadcastProvider::new()),
        hooks: None,
    }));
    engine
        .register_storage_writer(
            StorageWriterRegistration {
                instance_id: "writer-v2".to_string(),
                storage_protocol_version: 2,
                build: "test".to_string(),
                expires_at: 0.0,
            },
            30_000,
        )
        .await
        .unwrap();

    let old = chrono::Utc::now().timestamp_millis() as f64 - 61_000.0;
    let mut terminal = task("old-terminal", TaskStatus::Completed);
    terminal.updated_at = old;
    terminal.completed_at = Some(old);
    let mut pending = task("old-pending", TaskStatus::Pending);
    pending.updated_at = old;
    hot.save_task(terminal.clone()).await.unwrap();
    hot.save_task(pending.clone()).await.unwrap();
    durable.save_task(terminal).await.unwrap();
    durable.save_task(pending).await.unwrap();

    let worker = StorageLifecycleWorker::new(StorageLifecycleWorkerOptions {
        engine,
        short_term_store: short,
        config: ResolvedStorageLifecycleConfig {
            hot_retention_enabled: true,
            hot_retention_terminal_seconds: 60,
            hot_retention_idle_seconds: 1,
            rehydrate_replay_events: 1_000,
            storage_lock_ttl_seconds: 30,
            ttl_sweep_interval_seconds: 5,
            ttl_sweep_batch_size: 10,
        },
    });
    let result = worker.tick().await.unwrap();

    assert_eq!(result.retention.eligible, 1);
    assert_eq!(result.retention.released, 1);
    assert!(hot.get_task("old-terminal").await.unwrap().is_none());
    assert!(hot.get_task("old-pending").await.unwrap().is_some());
}

#[tokio::test]
async fn release_backoff_does_not_suppress_durable_ttl_sweeps() {
    let hot = Arc::new(MemoryShortTermStore::new());
    let durable = Arc::new(MemoryLongTermStore::new());
    let short: Arc<dyn ShortTermStore> = hot.clone();
    let long: Arc<dyn LongTermStore> = durable.clone();
    let engine = Arc::new(TaskEngine::new(TaskEngineOptions {
        short_term_store: short.clone(),
        long_term_store: Some(long),
        broadcast: Arc::new(MemoryBroadcastProvider::new()),
        hooks: None,
    }));
    let worker = StorageLifecycleWorker::new(StorageLifecycleWorkerOptions {
        engine: Arc::clone(&engine),
        short_term_store: short,
        config: ResolvedStorageLifecycleConfig {
            hot_retention_enabled: false,
            hot_retention_terminal_seconds: 60,
            hot_retention_idle_seconds: 1,
            rehydrate_replay_events: 1_000,
            storage_lock_ttl_seconds: 30,
            ttl_sweep_interval_seconds: 5,
            ttl_sweep_batch_size: 10,
        },
    });

    let first = worker.tick().await.unwrap();
    assert_eq!(first.release_requests.failed, 1);

    let mut overdue = task("ttl-during-release-backoff", TaskStatus::Running);
    overdue.ttl = Some(60);
    hot.save_task(overdue.clone()).await.unwrap();
    durable.save_task(overdue).await.unwrap();
    let metadata = durable
        .get_task_storage_metadata("ttl-during-release-backoff")
        .await
        .unwrap()
        .unwrap();
    assert!(durable
        .compare_and_set_task_storage_metadata(TaskStorageMetadataCas {
            task_id: "ttl-during-release-backoff".to_string(),
            expected_storage_state: metadata.storage_state.clone(),
            expected_storage_epoch: metadata.storage_epoch,
            expected_release_generation: metadata.active_release_generation.clone(),
            next: taskcast_core::TaskStorageMetadata {
                execution_deadline_at: Some(0.0),
                ..metadata
            },
        })
        .await
        .unwrap());

    let second = worker.tick().await.unwrap();
    assert_eq!(second.ttl.timed_out, 1);
    assert_eq!(
        durable
            .get_task("ttl-during-release-backoff")
            .await
            .unwrap()
            .unwrap()
            .status,
        TaskStatus::Timeout
    );
}

#[tokio::test]
async fn samples_old_and_large_hot_tasks_without_payloads() {
    let hot = Arc::new(MemoryShortTermStore::new());
    let durable = Arc::new(MemoryLongTermStore::new());
    let short: Arc<dyn ShortTermStore> = hot.clone();
    let long: Arc<dyn LongTermStore> = durable.clone();
    let engine = Arc::new(TaskEngine::new(TaskEngineOptions {
        short_term_store: short.clone(),
        long_term_store: Some(long),
        broadcast: Arc::new(MemoryBroadcastProvider::new()),
        hooks: None,
    }));
    let observed = task("large-old-hot", TaskStatus::Pending);
    hot.save_task(observed.clone()).await.unwrap();
    durable.save_task(observed.clone()).await.unwrap();
    for index in 0..2 {
        hot.append_event(
            &observed.id,
            TaskEvent {
                id: format!("event-{index}"),
                task_id: observed.id.clone(),
                index,
                timestamp: 2_000.0 + index as f64,
                r#type: "llm.delta".to_string(),
                level: Level::Info,
                data: serde_json::json!({ "secretPayload": "must-not-be-logged" }),
                series_id: None,
                series_mode: None,
                series_acc_field: None,
                series_snapshot: None,
                _accumulated_data: None,
            },
        )
        .await
        .unwrap();
    }
    let worker = StorageLifecycleWorker::new(StorageLifecycleWorkerOptions {
        engine,
        short_term_store: short,
        config: ResolvedStorageLifecycleConfig {
            hot_retention_enabled: false,
            hot_retention_terminal_seconds: 60,
            hot_retention_idle_seconds: 1,
            rehydrate_replay_events: 1,
            storage_lock_ttl_seconds: 30,
            ttl_sweep_interval_seconds: 5,
            ttl_sweep_batch_size: 10,
        },
    });

    let result = worker.tick().await.unwrap();

    assert_eq!(result.hot_storage.scanned, 1);
    assert_eq!(result.hot_storage.old, 1);
    assert_eq!(result.hot_storage.large, 1);
    assert_eq!(result.hot_storage.failed, 0);
}
