use taskcast_core::types::{Level, Task, TaskEvent, TaskStatus};
use taskcast_sqlite::{SqliteLongTermStore, SqliteShortTermStore};
use tempfile::TempDir;

pub struct TestContext {
    pub short: SqliteShortTermStore,
    pub long: SqliteLongTermStore,
    pub _dir: TempDir, // prevent cleanup until context is dropped
}

pub async fn setup() -> TestContext {
    let dir = TempDir::new().unwrap();
    let db_path = dir.path().join("test.db");
    let adapters = taskcast_sqlite::create_sqlite_adapters(db_path.to_str().unwrap())
        .await
        .unwrap();

    TestContext {
        short: adapters.short_term_store,
        long: adapters.long_term_store,
        _dir: dir,
    }
}

pub fn make_task(id: &str) -> Task {
    Task {
        id: id.to_string(),
        r#type: None,
        status: TaskStatus::Pending,
        params: Some(
            [("prompt".to_string(), serde_json::json!("hello"))]
                .into_iter()
                .collect(),
        ),
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
    }
}

pub fn make_event(task_id: &str, index: u64) -> TaskEvent {
    TaskEvent {
        id: format!("evt-{}-{}", task_id, index),
        task_id: task_id.to_string(),
        index,
        timestamp: 1000.0 + index as f64 * 100.0,
        r#type: "llm.delta".to_string(),
        level: Level::Info,
        data: serde_json::json!({"text": format!("msg-{}", index)}),
        series_id: None,
        series_mode: None,
    }
}
