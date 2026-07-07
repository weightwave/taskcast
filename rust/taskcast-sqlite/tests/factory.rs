use std::sync::Arc;

use taskcast_core::{
    AssignMode, DisconnectPolicy, Level, LongTermStore, MemoryBroadcastProvider, PublishEventInput,
    SeriesMode, ShortTermStore, Task, TaskArchive, TaskArchiveImportOptions, TaskEngine,
    TaskEngineOptions, TaskError, TaskEvent, TaskStatus, build_task_archive_restore_data,
};
use taskcast_sqlite::create_sqlite_adapters;
use tempfile::TempDir;

#[tokio::test]
async fn returns_working_adapters() {
    let dir = TempDir::new().unwrap();
    let db_path = dir.path().join("test.db");
    let adapters = create_sqlite_adapters(db_path.to_str().unwrap())
        .await
        .unwrap();

    // Verify both adapters are usable
    let task = taskcast_core::types::Task {
        id: "factory-1".to_string(),
        r#type: None,
        status: TaskStatus::Pending,
        params: None,
        result: None,
        error: None,
        metadata: None,
        auth_config: None,
        webhooks: None,
        cleanup: None,
        created_at: 1000.0,
        updated_at: 1000.0,
        completed_at: None,
        ttl: None,
        tags: None,
        assign_mode: None,
        cost: None,
        assigned_worker: None,
        disconnect_policy: None,
        reason: None,
        resume_at: None,
        blocked_request: None,
    };

    adapters
        .short_term_store
        .save_task(task.clone())
        .await
        .unwrap();
    let retrieved = adapters
        .short_term_store
        .get_task("factory-1")
        .await
        .unwrap();
    assert_eq!(retrieved, Some(task.clone()));

    adapters
        .long_term_store
        .save_task(task.clone())
        .await
        .unwrap();
    let retrieved = adapters
        .long_term_store
        .get_task("factory-1")
        .await
        .unwrap();
    assert_eq!(retrieved, Some(task));
}

#[tokio::test]
async fn imports_archive_through_paired_sqlite_adapters() {
    let dir = TempDir::new().unwrap();
    let db_path = dir.path().join("test.db");
    let adapters = create_sqlite_adapters(db_path.to_str().unwrap())
        .await
        .unwrap();
    let short = Arc::new(adapters.short_term_store);
    let long = Arc::new(adapters.long_term_store);
    let engine = TaskEngine::new(TaskEngineOptions {
        short_term_store: short.clone(),
        broadcast: Arc::new(MemoryBroadcastProvider::new()),
        long_term_store: Some(long.clone()),
        hooks: None,
    });
    let archive = TaskArchive {
        schema: "taskcast.taskArchive".to_string(),
        version: 1,
        exported_at: 5000.0,
        task: Task {
            id: "archive-task".to_string(),
            r#type: None,
            status: TaskStatus::Running,
            params: None,
            result: None,
            error: None,
            metadata: None,
            auth_config: None,
            webhooks: None,
            cleanup: None,
            created_at: 1000.0,
            updated_at: 2000.0,
            completed_at: None,
            ttl: None,
            tags: None,
            assign_mode: None,
            cost: None,
            assigned_worker: None,
            disconnect_policy: None,
            reason: None,
            resume_at: None,
            blocked_request: None,
        },
        events: vec![TaskEvent {
            id: "archive-event-0".to_string(),
            task_id: "archive-task".to_string(),
            index: 0,
            timestamp: 3000.0,
            r#type: "archive.event".to_string(),
            level: Level::Info,
            data: serde_json::json!({"value": 1}),
            series_id: None,
            series_mode: None,
            series_acc_field: None,
            series_snapshot: None,
            _accumulated_data: None,
        }],
    };

    let result = engine.import_task_archive(archive, None).await.unwrap();
    let next = engine
        .publish_event(
            "archive-task",
            PublishEventInput {
                r#type: "archive.next".to_string(),
                level: Level::Info,
                data: serde_json::json!({"value": 2}),
                series_id: None,
                series_mode: None,
                series_acc_field: None,
            },
        )
        .await
        .unwrap();

    assert_eq!(result.task_id, "archive-task");
    assert_eq!(result.event_count, 1);
    assert!(!result.overwritten);
    assert_eq!(
        short.get_events("archive-task", None).await.unwrap().len(),
        2
    );
    assert_eq!(
        long.get_events("archive-task", None).await.unwrap().len(),
        2
    );
    assert_eq!(next.index, 1);
}

fn rich_archive(task_id: &str, event_id: &str) -> TaskArchive {
    TaskArchive {
        schema: "taskcast.taskArchive".to_string(),
        version: 1,
        exported_at: 5000.0,
        task: Task {
            id: task_id.to_string(),
            r#type: Some("archive.rich".to_string()),
            status: TaskStatus::Completed,
            params: Some(
                [("prompt".to_string(), serde_json::json!("hello"))]
                    .into_iter()
                    .collect(),
            ),
            result: Some(
                [("ok".to_string(), serde_json::json!(true))]
                    .into_iter()
                    .collect(),
            ),
            error: Some(TaskError {
                code: Some("E_ARCHIVE".to_string()),
                message: "kept for history".to_string(),
                details: Some(
                    [("retryable".to_string(), serde_json::json!(false))]
                        .into_iter()
                        .collect(),
                ),
            }),
            metadata: Some(
                [("source".to_string(), serde_json::json!("test"))]
                    .into_iter()
                    .collect(),
            ),
            auth_config: None,
            webhooks: None,
            cleanup: None,
            created_at: 1000.0,
            updated_at: 2000.0,
            completed_at: Some(2500.0),
            ttl: Some(3600),
            tags: Some(vec!["archive".to_string(), "portable".to_string()]),
            assign_mode: Some(AssignMode::WsOffer),
            cost: Some(7),
            assigned_worker: Some("worker-1".to_string()),
            disconnect_policy: Some(DisconnectPolicy::Reassign),
            reason: None,
            resume_at: None,
            blocked_request: None,
        },
        events: vec![TaskEvent {
            id: event_id.to_string(),
            task_id: task_id.to_string(),
            index: 0,
            timestamp: 3000.0,
            r#type: "archive.delta".to_string(),
            level: Level::Info,
            data: serde_json::json!({"delta": "hello"}),
            series_id: Some("output".to_string()),
            series_mode: Some(SeriesMode::Accumulate),
            series_acc_field: Some("delta".to_string()),
            series_snapshot: None,
            _accumulated_data: None,
        }],
    }
}

#[tokio::test]
async fn short_term_restore_handles_rich_archive_conflicts_and_overwrite() {
    let dir = TempDir::new().unwrap();
    let db_path = dir.path().join("short.db");
    let adapters = create_sqlite_adapters(db_path.to_str().unwrap())
        .await
        .unwrap();
    let data = build_task_archive_restore_data(&rich_archive("rich-short", "rich-event")).unwrap();

    let overwritten = adapters
        .short_term_store
        .restore_task_archive(data.clone(), None)
        .await
        .unwrap();
    let restored = adapters
        .short_term_store
        .get_task("rich-short")
        .await
        .unwrap()
        .unwrap();
    let events = adapters
        .short_term_store
        .get_events("rich-short", None)
        .await
        .unwrap();
    let latest = adapters
        .short_term_store
        .get_series_latest("rich-short", "output")
        .await
        .unwrap()
        .unwrap();

    assert!(!overwritten);
    assert_eq!(restored.r#type.as_deref(), Some("archive.rich"));
    assert_eq!(restored.status, TaskStatus::Completed);
    assert_eq!(restored.completed_at, Some(2500.0));
    assert_eq!(restored.ttl, Some(3600));
    assert_eq!(
        restored.tags.as_deref(),
        Some(&["archive".to_string(), "portable".to_string()][..])
    );
    assert_eq!(restored.assign_mode, Some(AssignMode::WsOffer));
    assert_eq!(restored.cost, Some(7));
    assert_eq!(restored.assigned_worker.as_deref(), Some("worker-1"));
    assert_eq!(restored.disconnect_policy, Some(DisconnectPolicy::Reassign));
    assert_eq!(events[0].series_id.as_deref(), Some("output"));
    assert_eq!(events[0].series_mode, Some(SeriesMode::Accumulate));
    assert_eq!(events[0].series_acc_field.as_deref(), Some("delta"));
    assert_eq!(latest.id, "rich-event");

    let conflict = adapters
        .short_term_store
        .restore_task_archive(data.clone(), None)
        .await
        .unwrap_err()
        .to_string();
    assert!(conflict.contains("Task already exists"));

    let overwritten = adapters
        .short_term_store
        .restore_task_archive(data, Some(TaskArchiveImportOptions { overwrite: true }))
        .await
        .unwrap();
    assert!(overwritten);

    let owner =
        build_task_archive_restore_data(&rich_archive("event-owner", "shared-event")).unwrap();
    adapters
        .short_term_store
        .restore_task_archive(owner, None)
        .await
        .unwrap();
    let cross_task_conflict =
        build_task_archive_restore_data(&rich_archive("event-conflict", "shared-event")).unwrap();
    let err = adapters
        .short_term_store
        .validate_task_archive_restore(&cross_task_conflict, None)
        .await
        .unwrap_err()
        .to_string();
    assert!(err.contains("Archive event id conflicts with another task"));
}

#[tokio::test]
async fn long_term_restore_handles_rich_archive_conflicts_and_overwrite() {
    let dir = TempDir::new().unwrap();
    let db_path = dir.path().join("long.db");
    let adapters = create_sqlite_adapters(db_path.to_str().unwrap())
        .await
        .unwrap();
    let data = build_task_archive_restore_data(&rich_archive("rich-long", "rich-event")).unwrap();

    let overwritten = adapters
        .long_term_store
        .restore_task_archive(data.clone(), None)
        .await
        .unwrap();
    let restored = adapters
        .long_term_store
        .get_task("rich-long")
        .await
        .unwrap()
        .unwrap();
    let events = adapters
        .long_term_store
        .get_events("rich-long", None)
        .await
        .unwrap();

    assert!(!overwritten);
    assert_eq!(restored.r#type.as_deref(), Some("archive.rich"));
    assert_eq!(restored.completed_at, Some(2500.0));
    assert_eq!(restored.ttl, Some(3600));
    assert_eq!(
        restored.tags.as_deref(),
        Some(&["archive".to_string(), "portable".to_string()][..])
    );
    assert_eq!(restored.assign_mode, Some(AssignMode::WsOffer));
    assert_eq!(restored.cost, Some(7));
    assert_eq!(restored.assigned_worker.as_deref(), Some("worker-1"));
    assert_eq!(restored.disconnect_policy, Some(DisconnectPolicy::Reassign));
    assert_eq!(events[0].series_id.as_deref(), Some("output"));
    assert_eq!(events[0].series_mode, Some(SeriesMode::Accumulate));
    assert_eq!(events[0].series_acc_field.as_deref(), Some("delta"));

    let conflict = adapters
        .long_term_store
        .restore_task_archive(data.clone(), None)
        .await
        .unwrap_err()
        .to_string();
    assert!(conflict.contains("Task already exists"));

    let overwritten = adapters
        .long_term_store
        .restore_task_archive(data, Some(TaskArchiveImportOptions { overwrite: true }))
        .await
        .unwrap();
    assert!(overwritten);

    let owner =
        build_task_archive_restore_data(&rich_archive("event-owner", "shared-event")).unwrap();
    adapters
        .long_term_store
        .restore_task_archive(owner, None)
        .await
        .unwrap();
    let cross_task_conflict =
        build_task_archive_restore_data(&rich_archive("event-conflict", "shared-event")).unwrap();
    let err = adapters
        .long_term_store
        .validate_task_archive_restore(&cross_task_conflict, None)
        .await
        .unwrap_err()
        .to_string();
    assert!(err.contains("Archive event id conflicts with another task"));
}

#[tokio::test]
async fn creates_database_file() {
    let dir = TempDir::new().unwrap();
    let db_path = dir.path().join("new.db");
    assert!(!db_path.exists());

    let _adapters = create_sqlite_adapters(db_path.to_str().unwrap())
        .await
        .unwrap();

    assert!(db_path.exists());
}
