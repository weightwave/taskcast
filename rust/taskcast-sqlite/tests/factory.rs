use std::sync::Arc;

use taskcast_core::{
    Level, LongTermStore, MemoryBroadcastProvider, PublishEventInput, ShortTermStore, Task,
    TaskArchive, TaskEngine, TaskEngineOptions, TaskEvent, TaskStatus,
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

    adapters.short_term_store.save_task(task.clone()).await.unwrap();
    let retrieved = adapters.short_term_store.get_task("factory-1").await.unwrap();
    assert_eq!(retrieved, Some(task.clone()));

    adapters.long_term_store.save_task(task.clone()).await.unwrap();
    let retrieved = adapters.long_term_store.get_task("factory-1").await.unwrap();
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
    assert_eq!(short.get_events("archive-task", None).await.unwrap().len(), 2);
    assert_eq!(long.get_events("archive-task", None).await.unwrap().len(), 2);
    assert_eq!(next.index, 1);
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
