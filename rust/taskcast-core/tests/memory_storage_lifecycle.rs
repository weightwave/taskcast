use taskcast_core::{
    DurableSeriesState, HotWriteToken, Level, MemoryShortTermStore, RehydrateSnapshot, SeriesMode,
    ShortTermStore, Task, TaskEvent, TaskStatus,
};

fn task() -> Task {
    Task {
        id: "task-1".into(),
        r#type: Some("agent.session".into()),
        status: TaskStatus::Pending,
        params: None,
        result: None,
        error: None,
        metadata: None,
        created_at: 100.0,
        updated_at: 100.0,
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

fn event(index: u64) -> TaskEvent {
    TaskEvent {
        id: format!("event-{index}"),
        task_id: "task-1".into(),
        index,
        timestamp: 1_000.0 + index as f64,
        r#type: "agent.message".into(),
        level: Level::Info,
        data: serde_json::json!({ "index": index }),
        series_id: None,
        series_mode: None,
        series_acc_field: None,
        series_snapshot: None,
        _accumulated_data: None,
    }
}

#[tokio::test]
async fn local_storage_lease_serializes_owners() {
    let store = MemoryShortTermStore::new();
    store.save_task(task()).await.unwrap();

    let lease = store
        .acquire_storage_lock("task-1", "lock-1", "generation-1", 60_000)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(lease.storage_epoch, 1);
    assert!(store
        .acquire_storage_lock("task-1", "lock-2", "generation-2", 60_000)
        .await
        .unwrap()
        .is_none());

    let mut wrong = lease.clone();
    wrong.lock_token = "lock-2".into();
    assert!(!store.release_storage_lock(&wrong).await.unwrap());
    assert!(store.renew_storage_lock(&lease, 60_000).await.unwrap());
    assert!(store.release_storage_lock(&lease).await.unwrap());
}

#[tokio::test]
async fn fenced_commit_rejects_old_epoch_without_consuming_index() {
    let store = MemoryShortTermStore::new();
    store.save_task(task()).await.unwrap();
    let lease = store
        .acquire_storage_lock("task-1", "lock-1", "generation-1", 60_000)
        .await
        .unwrap()
        .unwrap();
    let old_token = HotWriteToken {
        task_id: "task-1".into(),
        storage_epoch: 1,
    };

    let closed = store.close_write_fence(&lease, 1).await.unwrap();
    assert_eq!(closed.high_watermark, -1);
    assert!(store
        .commit_event_fenced("task-1", event(999), &old_token)
        .await
        .is_err());

    let token = store.reopen_write_fence(&lease, 1).await.unwrap();
    let committed = store
        .commit_event_fenced("task-1", event(999), &token)
        .await
        .unwrap();
    assert_eq!(committed.event.index, 0);
}

#[tokio::test]
async fn sparse_archive_pages_delete_and_restore_preserve_next_index() {
    let store = MemoryShortTermStore::new();
    store.save_task(task()).await.unwrap();
    store.append_event("task-1", event(2)).await.unwrap();
    store.append_event("task-1", event(7)).await.unwrap();
    store.append_event("task-1", event(11)).await.unwrap();

    let first = store
        .read_archive_source_page("task-1", 11, None, 2)
        .await
        .unwrap();
    let second = store
        .read_archive_source_page("task-1", 11, first.next_cursor.as_deref(), 2)
        .await
        .unwrap();
    assert_eq!(
        first
            .events
            .iter()
            .map(|entry| entry.index)
            .collect::<Vec<_>>(),
        vec![2, 7]
    );
    assert!(!first.done);
    assert_eq!(second.events[0].index, 11);
    assert!(second.done);

    let lease = store
        .acquire_storage_lock("task-1", "lock-1", "generation-1", 60_000)
        .await
        .unwrap()
        .unwrap();
    store.close_write_fence(&lease, 1).await.unwrap();
    store
        .delete_task_storage_fenced(&lease, 1)
        .await
        .unwrap();
    let presence = store
        .get_task_storage_presence("task-1")
        .await
        .unwrap();
    assert!(!presence.task);
    assert_eq!(presence.event_count, 0);

    let mut latest = event(7);
    latest.series_id = Some("series-1".into());
    latest.series_mode = Some(SeriesMode::Latest);
    let snapshot = RehydrateSnapshot {
        task: task(),
        archive_watermark: 7,
        max_event_index: 7,
        replay_events: vec![latest.clone()],
        series_latest: vec![DurableSeriesState {
            task_id: "task-1".into(),
            series_id: "series-1".into(),
            mode: SeriesMode::Latest,
            event: latest,
            through_index: 7,
        }],
        storage_epoch: 1,
    };
    let token = store
        .restore_hot_task_fenced(snapshot, &lease, 2)
        .await
        .unwrap();
    let committed = store
        .commit_event_fenced("task-1", event(999), &token)
        .await
        .unwrap();
    assert_eq!(committed.event.index, 8);
}
