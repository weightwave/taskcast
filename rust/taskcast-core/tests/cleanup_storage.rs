use serde_json::json;
use taskcast_core::*;

fn event(task_id: &str, index: u64) -> TaskEvent {
    serde_json::from_value(json!({"id": format!("{task_id}-{index}"), "taskId": task_id, "index": index, "timestamp": 1000, "type": "test", "level": "info", "data": {"delta": "x"}})).unwrap()
}

async fn enroll(
    store: &MemoryLongTermStore,
    id: &str,
    target: &str,
    count: u64,
) -> (Task, DurableWriteContext) {
    let ctx = DurableWriteContext {
        creation_token: format!("{id}-generation"),
    };
    let mut task: Task = serde_json::from_value(json!({"id": id, "status": "running", "createdAt": 0, "updatedAt": 0, "cleanupPolicyVersion": 1, "cleanupResolvedAt": 0, "cleanup": {"rules": [{"target": target, "trigger": {"afterMs": 0}}]}})).unwrap();
    assert!(store
        .claim_task_creation(task.clone(), &ctx.creation_token, 30000)
        .await
        .unwrap());
    store
        .complete_task_creation(id, &ctx.creation_token)
        .await
        .unwrap();
    for index in 0..count {
        store
            .save_event_with_context(event(id, index), Some(&ctx))
            .await
            .unwrap();
    }
    task.status = TaskStatus::Completed;
    task.completed_at = Some(1000.0);
    task.result = Some(serde_json::from_value(json!({"value": "keep"})).unwrap());
    store
        .save_task_with_context(task.clone(), Some(&ctx))
        .await
        .unwrap();
    // Storage fixture models a finalized cold archive; coordinator tests exercise the actual release.
    let metadata = store.get_task_storage_metadata(id).await.unwrap().unwrap();
    assert!(store
        .compare_and_set_task_storage_metadata(TaskStorageMetadataCas {
            task_id: id.into(),
            expected_storage_state: StorageState::Hot,
            expected_storage_epoch: 1,
            expected_release_generation: None,
            next: TaskStorageMetadata {
                storage_state: StorageState::Cold,
                archive_watermark: count as i64 - 1,
                cold_at: Some(1000.0),
                ..metadata
            },
        })
        .await
        .unwrap());
    (task, ctx)
}

#[tokio::test]
async fn bounded_cleanup_preserves_results_fences_late_writes_and_resumes() {
    let store = MemoryLongTermStore::new();
    let legacy: Task = serde_json::from_value(json!({"id": "legacy", "status": "completed", "createdAt": 0, "updatedAt": 0, "completedAt": 1000, "cleanup": {"rules": [{"target": "all", "trigger": {}}]}})).unwrap();
    store.save_task(legacy).await.unwrap();
    let (task, ctx) = enroll(&store, "large", "events", 1501).await;
    assert!(store.save_task(task.clone()).await.is_err());
    let claims = store.claim_cleanup_tasks(10, 30000).await.unwrap();
    assert_eq!(claims.len(), 1);
    let claim = &claims[0];
    assert_eq!(claim.task_id, "large");
    assert!(!store.begin_task_cleanup(claim, 2, 1500).await.unwrap());
    assert!(store.begin_task_cleanup(claim, 1, 1500).await.unwrap());
    let first = store.delete_task_cleanup_batch(claim, 1000).await.unwrap();
    assert_eq!(first.deleted_events, 1000);
    assert!(!first.complete);
    store.defer_cleanup_claim(claim, 0).await.unwrap();
    let retry = store.claim_cleanup_tasks(1, 30000).await.unwrap().remove(0);
    let last = store.delete_task_cleanup_batch(&retry, 1000).await.unwrap();
    assert_eq!(last.deleted_events, 501);
    assert!(last.complete);
    let retained = store.get_task("large").await.unwrap().unwrap();
    assert!(retained.history_expired_at.is_some());
    assert_eq!(retained.result.unwrap()["value"], "keep");
    assert_eq!(store.get_last_event_index("large").await.unwrap(), 1500);
    assert!(store
        .save_event_with_context(event("large", 0), Some(&ctx))
        .await
        .is_err());
    assert!(store
        .save_task_with_context(task, Some(&ctx))
        .await
        .is_err());
    let mut late = event("large", 0);
    late.series_id = Some("s".into());
    late.series_mode = Some(SeriesMode::Latest);
    assert!(store
        .replace_last_series_event_with_context("large", "s", late.clone(), Some(&ctx))
        .await
        .is_err());
    late.series_mode = Some(SeriesMode::Accumulate);
    assert!(store
        .accumulate_series_with_context("large", "s", late, "delta", Some(&ctx))
        .await
        .is_err());
    assert!(store
        .claim_cleanup_tasks(1, 30000)
        .await
        .unwrap()
        .is_empty());
    assert!(store.get_task("legacy").await.unwrap().is_some());

    let (old, old_ctx) = enroll(&store, "reuse", "all", 1).await;
    let claim = store.claim_cleanup_tasks(1, 30000).await.unwrap().remove(0);
    assert!(store.renew_cleanup_claim(&claim, 30000).await.unwrap());
    let mut stolen = claim.clone();
    stolen.claim_token = "wrong".into();
    assert!(!store.renew_cleanup_claim(&stolen, 30000).await.unwrap());
    assert!(!store.begin_task_cleanup(&stolen, 1, 0).await.unwrap());
    assert!(store.begin_task_cleanup(&claim, 1, 0).await.unwrap());
    assert!(
        store
            .delete_task_cleanup_batch(&claim, 1000)
            .await
            .unwrap()
            .complete
    );
    assert!(store.get_task("reuse").await.unwrap().is_none());
    assert!(store
        .save_task_with_context(old.clone(), Some(&old_ctx))
        .await
        .is_err());
    assert!(store.save_task(old.clone()).await.is_err());
    let mut new = old;
    new.status = TaskStatus::Running;
    new.completed_at = None;
    assert!(store
        .claim_task_creation(new, "new-generation", 30000)
        .await
        .unwrap());
    store
        .complete_task_creation("reuse", "new-generation")
        .await
        .unwrap();
    assert!(store
        .save_event_with_context(event("reuse", 0), Some(&old_ctx))
        .await
        .is_err());
    store
        .save_event_with_context(
            event("reuse", 1),
            Some(&DurableWriteContext {
                creation_token: "new-generation".into(),
            }),
        )
        .await
        .unwrap();
    assert_eq!(store.get_events("reuse", None).await.unwrap().len(), 1);
    assert!(store.claim_cleanup_tasks(0, 1).await.is_err());
}
