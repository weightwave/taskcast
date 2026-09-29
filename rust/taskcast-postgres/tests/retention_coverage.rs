use serde_json::json;
use sqlx::postgres::PgPoolOptions;
use taskcast_core::{
    AssignMode, DisconnectPolicy, LongTermStore, Task, TaskArchiveImportOptions,
    TaskArchiveRestoreData, TaskEvent,
};
use taskcast_postgres::PostgresLongTermStore;
use testcontainers::runners::AsyncRunner;
use testcontainers_modules::postgres::Postgres;

#[tokio::test]
async fn archive_restore_validation_rejects_conflicts_and_preserves_task_options() {
    let container = Postgres::default().start().await.unwrap();
    let port = container.get_host_port_ipv4(5432).await.unwrap();
    let pool = PgPoolOptions::new()
        .max_connections(5)
        .connect(&format!(
            "postgres://postgres:postgres@127.0.0.1:{port}/postgres"
        ))
        .await
        .unwrap();
    let store = PostgresLongTermStore::new(pool);
    store.migrate().await.unwrap();

    let task: Task = serde_json::from_value(json!({
        "id": "restored", "status": "completed", "createdAt": 1000,
        "updatedAt": 2000, "completedAt": 2000,
        "assignMode": "pull", "disconnectPolicy": "fail"
    }))
    .unwrap();
    let event: TaskEvent = serde_json::from_value(json!({
        "id": "restored-event", "taskId": "restored", "index": 0,
        "timestamp": 1500, "type": "output", "level": "info", "data": {"text": "kept"}
    }))
    .unwrap();
    let data = TaskArchiveRestoreData {
        task,
        events: vec![event],
        next_index: 1,
        series_latest: vec![],
        storage_epoch: None,
        expected_creation_token: None,
    };
    store
        .validate_task_archive_restore(&data, None)
        .await
        .unwrap();
    assert!(!store
        .restore_task_archive(data.clone(), None)
        .await
        .unwrap());
    let restored = store.get_task("restored").await.unwrap().unwrap();
    assert_eq!(restored.assign_mode, Some(AssignMode::Pull));
    assert_eq!(restored.disconnect_policy, Some(DisconnectPolicy::Fail));
    assert!(store
        .validate_task_archive_restore(&data, None)
        .await
        .is_err());
    let overwrite = Some(TaskArchiveImportOptions { overwrite: true });
    store
        .validate_task_archive_restore(&data, overwrite)
        .await
        .unwrap();

    let mut stale_generation = data.clone();
    stale_generation.expected_creation_token = Some("stale-generation".into());
    assert!(store
        .validate_task_archive_restore(&stale_generation, overwrite)
        .await
        .is_err());

    let other: Task = serde_json::from_value(json!({
        "id": "other", "status": "pending", "createdAt": 1000, "updatedAt": 1000
    }))
    .unwrap();
    store.save_task(other).await.unwrap();
    let collision: TaskEvent = serde_json::from_value(json!({
        "id": "collision", "taskId": "other", "index": 0,
        "timestamp": 1000, "type": "output", "level": "info", "data": null
    }))
    .unwrap();
    store.save_event(collision.clone()).await.unwrap();
    let mut conflicting = data;
    conflicting.events[0].id = collision.id;
    assert!(store
        .validate_task_archive_restore(&conflicting, overwrite)
        .await
        .is_err());
    assert_eq!(store.get_events("restored", None).await.unwrap().len(), 1);
    store.pool().close().await;
}

#[tokio::test]
async fn atomic_task_creation_persists_resolved_cleanup_time() {
    let container = Postgres::default().start().await.unwrap();
    let port = container.get_host_port_ipv4(5432).await.unwrap();
    let pool = PgPoolOptions::new()
        .max_connections(5)
        .connect(&format!(
            "postgres://postgres:postgres@127.0.0.1:{port}/postgres"
        ))
        .await
        .unwrap();
    let store = PostgresLongTermStore::new(pool);
    store.migrate().await.unwrap();

    let task: Task = serde_json::from_value(json!({
        "id": "cleanup-snapshot", "status": "pending", "createdAt": 1000,
        "updatedAt": 1000, "cleanupPolicyVersion": 1, "cleanupResolvedAt": 1234,
        "cleanup": {"rules": [{"target": "events", "trigger": {"afterMs": 0}}]}
    }))
    .unwrap();
    assert!(store.create_task_if_absent(task.clone()).await.unwrap());
    assert!(!store.create_task_if_absent(task).await.unwrap());
    let saved = store.get_task("cleanup-snapshot").await.unwrap().unwrap();
    assert_eq!(saved.cleanup_resolved_at, Some(1234.0));
    store.pool().close().await;
}

#[tokio::test]
async fn legacy_terminal_task_cannot_be_reopened_by_a_storage_update() {
    let container = Postgres::default().start().await.unwrap();
    let port = container.get_host_port_ipv4(5432).await.unwrap();
    let pool = PgPoolOptions::new()
        .max_connections(5)
        .connect(&format!(
            "postgres://postgres:postgres@127.0.0.1:{port}/postgres"
        ))
        .await
        .unwrap();
    let store = PostgresLongTermStore::new(pool);
    store.migrate().await.unwrap();

    let terminal: Task = serde_json::from_value(json!({
        "id": "legacy-terminal", "status": "completed", "createdAt": 1000,
        "updatedAt": 2000, "completedAt": 2000
    }))
    .unwrap();
    store.save_task(terminal.clone()).await.unwrap();
    let mut reopened = terminal;
    reopened.status = taskcast_core::TaskStatus::Pending;
    reopened.completed_at = None;
    assert!(store.save_task(reopened).await.is_err());
    assert_eq!(
        store
            .get_task("legacy-terminal")
            .await
            .unwrap()
            .unwrap()
            .status,
        taskcast_core::TaskStatus::Completed
    );
    store.pool().close().await;
}
