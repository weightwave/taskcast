use taskcast_core::types::{LongTermStore, ShortTermStore, TaskStatus};
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
async fn creates_database_file() {
    let dir = TempDir::new().unwrap();
    let db_path = dir.path().join("new.db");
    assert!(!db_path.exists());

    let _adapters = create_sqlite_adapters(db_path.to_str().unwrap())
        .await
        .unwrap();

    assert!(db_path.exists());
}
