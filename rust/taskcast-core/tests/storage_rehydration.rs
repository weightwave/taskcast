use std::sync::Arc;

use taskcast_core::{
    CreateTaskInput, HotWriteToken, Level, LongTermStore, MemoryBroadcastProvider,
    MemoryLongTermStore, MemoryShortTermStore, PublishEventInput, ReleasePreconditions, SeriesMode,
    ShortTermStore, StorageCoordinator, StorageState, Task, TaskEngine, TaskEngineOptions,
    TaskEvent, TaskStatus,
};

fn make_task() -> Task {
    Task {
        id: "task-1".to_string(),
        status: TaskStatus::Running,
        created_at: 1_000.0,
        updated_at: 1_000.0,
        r#type: None,
        params: None,
        result: None,
        error: None,
        metadata: None,
        ttl: None,
        webhooks: None,
        cleanup: None,
        auth_config: None,
        completed_at: None,
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

async fn seed_and_release(
    event_count: u64,
) -> (
    Arc<MemoryShortTermStore>,
    Arc<MemoryLongTermStore>,
    StorageCoordinator,
) {
    let hot = Arc::new(MemoryShortTermStore::new());
    let durable = Arc::new(MemoryLongTermStore::new());
    hot.save_task(make_task()).await.unwrap();
    durable.save_task(make_task()).await.unwrap();
    let token = HotWriteToken {
        task_id: "task-1".to_string(),
        storage_epoch: 1,
    };
    for index in 0..event_count {
        let committed = hot
            .commit_event_fenced("task-1", make_event(index), &token)
            .await
            .unwrap();
        durable.save_event(committed.event).await.unwrap();
    }
    let coordinator =
        StorageCoordinator::new(hot.clone(), durable.clone()).with_storage_lock_ttl_ms(5_000);
    coordinator
        .release_task_storage(
            "task-1",
            ReleasePreconditions {
                expected_last_event_index: event_count as i64 - 1,
                inactive_since: if event_count == 0 {
                    1_000.0
                } else {
                    2_000.0 + event_count as f64
                },
            },
        )
        .await
        .unwrap();
    (hot, durable, coordinator)
}

#[tokio::test]
async fn reads_stay_cold_and_a_write_restores_only_the_bounded_replay_window() {
    let (hot, durable, _) = seed_and_release(1_005).await;
    let engine = TaskEngine::new(TaskEngineOptions {
        short_term_store: hot.clone(),
        long_term_store: Some(durable.clone()),
        broadcast: Arc::new(MemoryBroadcastProvider::new()),
        hooks: None,
    });

    assert!(engine.get_task("task-1").await.unwrap().is_some());
    let cold_presence = hot.get_task_storage_presence("task-1").await.unwrap();
    assert!(!cold_presence.task);
    assert_eq!(cold_presence.event_count, 0);
    let cold = durable
        .get_task_storage_metadata("task-1")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(cold.storage_state, StorageState::Cold);
    assert_eq!(cold.storage_epoch, 1);

    let published = engine
        .publish_event(
            "task-1",
            PublishEventInput {
                r#type: "late.event".to_string(),
                level: Level::Info,
                data: serde_json::json!({ "late": true }),
                series_id: None,
                series_mode: None,
                series_acc_field: None,
            },
        )
        .await
        .unwrap();
    assert_eq!(published.index, 1_005);
    let hot_presence = hot.get_task_storage_presence("task-1").await.unwrap();
    assert!(hot_presence.task);
    assert_eq!(hot_presence.event_count, 1_001);
    let metadata = durable
        .get_task_storage_metadata("task-1")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(metadata.storage_state, StorageState::Hot);
    assert_eq!(metadata.storage_epoch, 2);

    let stale = hot
        .commit_event_fenced(
            "task-1",
            make_event(1_006),
            &HotWriteToken {
                task_id: "task-1".to_string(),
                storage_epoch: 1,
            },
        )
        .await;
    assert!(stale.is_err());
}

#[tokio::test]
async fn rehydration_continues_latest_and_accumulated_series() {
    let hot = Arc::new(MemoryShortTermStore::new());
    let durable = Arc::new(MemoryLongTermStore::new());
    hot.save_task(make_task()).await.unwrap();
    durable.save_task(make_task()).await.unwrap();
    let token = HotWriteToken {
        task_id: "task-1".to_string(),
        storage_epoch: 1,
    };
    let mut first = make_event(0);
    first.id = "delta-a".to_string();
    first.series_id = Some("output".to_string());
    first.series_mode = Some(SeriesMode::Accumulate);
    first.series_acc_field = Some("delta".to_string());
    first.data = serde_json::json!({ "delta": "A" });
    let first = hot
        .commit_event_fenced("task-1", first, &token)
        .await
        .unwrap();
    durable
        .accumulate_series("task-1", "output", first.event, "delta")
        .await
        .unwrap();
    let mut second = make_event(1);
    second.id = "delta-b".to_string();
    second.series_id = Some("output".to_string());
    second.series_mode = Some(SeriesMode::Accumulate);
    second.series_acc_field = Some("delta".to_string());
    second.data = serde_json::json!({ "delta": "B" });
    let second = hot
        .commit_event_fenced("task-1", second, &token)
        .await
        .unwrap();
    durable
        .accumulate_series("task-1", "output", second.event, "delta")
        .await
        .unwrap();
    let mut latest = make_event(2);
    latest.id = "latest".to_string();
    latest.r#type = "progress".to_string();
    latest.series_id = Some("progress".to_string());
    latest.series_mode = Some(SeriesMode::Latest);
    latest.data = serde_json::json!({ "percent": 50 });
    let latest = hot
        .commit_event_fenced("task-1", latest, &token)
        .await
        .unwrap();
    durable
        .replace_last_series_event("task-1", "progress", latest.event)
        .await
        .unwrap();
    let coordinator = StorageCoordinator::new(hot.clone(), durable.clone());
    coordinator
        .release_task_storage(
            "task-1",
            ReleasePreconditions {
                expected_last_event_index: 2,
                inactive_since: 3_000.0,
            },
        )
        .await
        .unwrap();
    let engine = TaskEngine::new(TaskEngineOptions {
        short_term_store: hot.clone(),
        long_term_store: Some(durable),
        broadcast: Arc::new(MemoryBroadcastProvider::new()),
        hooks: None,
    });

    let published = engine
        .publish_event(
            "task-1",
            PublishEventInput {
                r#type: "llm.delta".to_string(),
                level: Level::Info,
                data: serde_json::json!({ "delta": "C" }),
                series_id: Some("output".to_string()),
                series_mode: Some(SeriesMode::Accumulate),
                series_acc_field: Some("delta".to_string()),
            },
        )
        .await
        .unwrap();
    assert_eq!(published.index, 3);
    assert_eq!(
        hot.get_series_latest("task-1", "output")
            .await
            .unwrap()
            .unwrap()
            .data,
        serde_json::json!({ "delta": "ABC" })
    );
    assert_eq!(
        hot.get_series_latest("task-1", "progress")
            .await
            .unwrap()
            .unwrap()
            .data,
        serde_json::json!({ "percent": 50 })
    );
}

#[tokio::test]
async fn concurrent_rehydrators_install_one_new_epoch() {
    let (_hot, durable, coordinator) = seed_and_release(0).await;
    let (first, second) = tokio::join!(
        coordinator.ensure_task_hot_for_write("task-1"),
        coordinator.ensure_task_hot_for_write("task-1"),
    );
    let successful = [first.as_ref().ok(), second.as_ref().ok()]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();
    assert!(!successful.is_empty());
    assert!(successful.iter().all(|token| token.storage_epoch == 2));
    assert_eq!(
        coordinator
            .ensure_task_hot_for_write("task-1")
            .await
            .unwrap()
            .storage_epoch,
        2
    );
    assert_eq!(
        durable
            .get_task_storage_metadata("task-1")
            .await
            .unwrap()
            .unwrap()
            .storage_epoch,
        2
    );
}

#[tokio::test]
async fn a_restored_epoch_is_adopted_after_crash_before_durable_cas() {
    let (hot, durable, coordinator) = seed_and_release(1).await;
    let lease = hot
        .acquire_storage_lock("task-1", "crashed-lock", "crashed-rehydrate", 5_000)
        .await
        .unwrap()
        .unwrap();
    let metadata = durable
        .get_task_storage_metadata("task-1")
        .await
        .unwrap()
        .unwrap();
    let snapshot = taskcast_core::RehydrateSnapshot {
        task: durable.get_task("task-1").await.unwrap().unwrap(),
        archive_watermark: metadata.archive_watermark,
        max_event_index: durable.get_last_event_index("task-1").await.unwrap(),
        replay_events: durable.get_recent_events("task-1", 1_000).await.unwrap(),
        series_latest: durable.get_durable_series_state("task-1").await.unwrap(),
        storage_epoch: metadata.storage_epoch,
    };
    hot.restore_hot_task_fenced(snapshot, &lease, 2)
        .await
        .unwrap();
    hot.release_storage_lock(&lease).await.unwrap();

    let token = coordinator
        .ensure_task_hot_for_write("task-1")
        .await
        .unwrap();
    assert_eq!(token.storage_epoch, 2);
    let metadata = durable
        .get_task_storage_metadata("task-1")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(metadata.storage_state, StorageState::Hot);
    assert_eq!(metadata.storage_epoch, 2);
}

#[tokio::test]
async fn engine_can_still_create_a_fresh_task_after_rehydration_support_is_enabled() {
    let engine = TaskEngine::new(TaskEngineOptions {
        short_term_store: Arc::new(MemoryShortTermStore::new()),
        long_term_store: Some(Arc::new(MemoryLongTermStore::new())),
        broadcast: Arc::new(MemoryBroadcastProvider::new()),
        hooks: None,
    });
    let task = engine
        .create_task(CreateTaskInput {
            id: Some("fresh".to_string()),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(task.id, "fresh");
}
