use sqlx::postgres::PgPoolOptions;
use testcontainers::runners::AsyncRunner;
use testcontainers_modules::postgres::Postgres;

use taskcast_core::types::{
    EventQueryOptions, Level, LongTermStore, SinceCursor, Task, TaskEvent, TaskStatus,
    WorkerAuditAction, WorkerAuditEvent,
};
use taskcast_postgres::PostgresLongTermStore;

use std::collections::HashMap;

// ─── Helpers ─────────────────────────────────────────────────────────────────

async fn setup() -> (
    PostgresLongTermStore,
    testcontainers::ContainerAsync<Postgres>,
) {
    let container = Postgres::default().start().await.unwrap();
    let host_port = container.get_host_port_ipv4(5432).await.unwrap();
    let database_url = format!(
        "postgres://postgres:postgres@127.0.0.1:{}/postgres",
        host_port
    );
    let pool = PgPoolOptions::new()
        .max_connections(5)
        .connect(&database_url)
        .await
        .unwrap();
    let store = PostgresLongTermStore::new(pool);
    store.migrate().await.unwrap();
    (store, container)
}

fn make_task(id: &str) -> Task {
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
        tags: None,
        assign_mode: None,
        cost: None,
        assigned_worker: None,
        disconnect_policy: None,
        reason: None,
        resume_at: None,
        blocked_request: None,
        created_at: 1000.0,
        updated_at: 1000.0,
        completed_at: None,
        ttl: None,
    }
}

fn make_event(task_id: &str, index: u64) -> TaskEvent {
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
        series_acc_field: None,
        series_snapshot: None,
        _accumulated_data: None,
    }
}

// ─── save_task / get_task ─────────────────────────────────────────────────

#[tokio::test]
async fn save_and_retrieve_a_task() {
    let (store, _container) = setup().await;
    let task = make_task("task-1");
    store.save_task(task.clone()).await.unwrap();
    let retrieved = store.get_task("task-1").await.unwrap();
    assert_eq!(retrieved, Some(task));
}

#[tokio::test]
async fn return_none_for_missing_task() {
    let (store, _container) = setup().await;
    let result = store.get_task("nonexistent").await.unwrap();
    assert_eq!(result, None);
}

#[tokio::test]
async fn upsert_task_on_conflict() {
    let (store, _container) = setup().await;
    let task = make_task("task-1");
    store.save_task(task.clone()).await.unwrap();

    let mut updated = task.clone();
    updated.status = TaskStatus::Running;
    updated.updated_at = 2000.0;
    store.save_task(updated.clone()).await.unwrap();

    let retrieved = store.get_task("task-1").await.unwrap().unwrap();
    assert_eq!(retrieved.status, TaskStatus::Running);
    assert_eq!(retrieved.updated_at, 2000.0);
}

#[tokio::test]
async fn preserve_optional_fields_on_round_trip() {
    let (store, _container) = setup().await;
    let mut task = make_task("task-1");
    task.r#type = Some("llm".to_string());
    task.result = Some(
        [("answer".to_string(), serde_json::json!(42))]
            .into_iter()
            .collect(),
    );
    task.error = Some(taskcast_core::types::TaskError {
        message: "boom".to_string(),
        code: Some("ERR".to_string()),
        details: None,
    });
    task.metadata = Some(
        [("source".to_string(), serde_json::json!("test"))]
            .into_iter()
            .collect(),
    );
    task.completed_at = Some(3000.0);
    task.ttl = Some(60);

    store.save_task(task.clone()).await.unwrap();
    let retrieved = store.get_task("task-1").await.unwrap().unwrap();
    assert_eq!(retrieved, task);
}

#[tokio::test]
async fn handle_task_with_no_optional_fields() {
    let (store, _container) = setup().await;
    let task = Task {
        id: "minimal".to_string(),
        r#type: None,
        status: TaskStatus::Pending,
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
        created_at: 1000.0,
        updated_at: 1000.0,
        completed_at: None,
        ttl: None,
    };
    store.save_task(task.clone()).await.unwrap();
    let retrieved = store.get_task("minimal").await.unwrap().unwrap();
    assert_eq!(retrieved, task);
}

// ─── save_event / get_events ──────────────────────────────────────────────

#[tokio::test]
async fn save_and_retrieve_events() {
    let (store, _container) = setup().await;
    store.save_task(make_task("task-1")).await.unwrap();

    let e0 = make_event("task-1", 0);
    let e1 = make_event("task-1", 1);
    let e2 = make_event("task-1", 2);

    store.save_event(e0.clone()).await.unwrap();
    store.save_event(e1.clone()).await.unwrap();
    store.save_event(e2.clone()).await.unwrap();

    let events = store.get_events("task-1", None).await.unwrap();
    assert_eq!(events, vec![e0, e1, e2]);
}

#[tokio::test]
async fn return_empty_vec_when_no_events() {
    let (store, _container) = setup().await;
    store.save_task(make_task("task-1")).await.unwrap();
    let events = store.get_events("task-1", None).await.unwrap();
    assert!(events.is_empty());
}

#[tokio::test]
async fn filter_events_by_since_index() {
    let (store, _container) = setup().await;
    store.save_task(make_task("task-1")).await.unwrap();
    for i in 0..5 {
        store.save_event(make_event("task-1", i)).await.unwrap();
    }

    let opts = EventQueryOptions {
        since: Some(SinceCursor {
            index: Some(2),
            timestamp: None,
            id: None,
        }),
        limit: None,
    };
    let events = store.get_events("task-1", Some(opts)).await.unwrap();
    assert_eq!(events.len(), 2);
    assert_eq!(events[0].index, 3);
    assert_eq!(events[1].index, 4);
}

#[tokio::test]
async fn filter_events_by_since_timestamp() {
    let (store, _container) = setup().await;
    store.save_task(make_task("task-1")).await.unwrap();
    for i in 0..5 {
        store.save_event(make_event("task-1", i)).await.unwrap();
    }

    let opts = EventQueryOptions {
        since: Some(SinceCursor {
            index: None,
            timestamp: Some(1200.0),
            id: None,
        }),
        limit: None,
    };
    let events = store.get_events("task-1", Some(opts)).await.unwrap();
    assert_eq!(events.len(), 2);
    assert_eq!(events[0].timestamp, 1300.0);
    assert_eq!(events[1].timestamp, 1400.0);
}

#[tokio::test]
async fn filter_events_by_since_id() {
    let (store, _container) = setup().await;
    store.save_task(make_task("task-1")).await.unwrap();
    for i in 0..5 {
        store.save_event(make_event("task-1", i)).await.unwrap();
    }

    let opts = EventQueryOptions {
        since: Some(SinceCursor {
            index: None,
            timestamp: None,
            id: Some("evt-task-1-2".to_string()),
        }),
        limit: None,
    };
    let events = store.get_events("task-1", Some(opts)).await.unwrap();
    assert_eq!(events.len(), 2);
    assert_eq!(events[0].id, "evt-task-1-3");
    assert_eq!(events[1].id, "evt-task-1-4");
}

#[tokio::test]
async fn return_all_events_when_since_id_not_found() {
    let (store, _container) = setup().await;
    store.save_task(make_task("task-1")).await.unwrap();
    for i in 0..3 {
        store.save_event(make_event("task-1", i)).await.unwrap();
    }

    let opts = EventQueryOptions {
        since: Some(SinceCursor {
            index: None,
            timestamp: None,
            id: Some("nonexistent-id".to_string()),
        }),
        limit: None,
    };
    let events = store.get_events("task-1", Some(opts)).await.unwrap();
    assert_eq!(events.len(), 3);
}

#[tokio::test]
async fn respect_limit_parameter() {
    let (store, _container) = setup().await;
    store.save_task(make_task("task-1")).await.unwrap();
    for i in 0..10 {
        store.save_event(make_event("task-1", i)).await.unwrap();
    }

    let opts = EventQueryOptions {
        since: None,
        limit: Some(3),
    };
    let events = store.get_events("task-1", Some(opts)).await.unwrap();
    assert_eq!(events.len(), 3);
    assert_eq!(events[0].index, 0);
    assert_eq!(events[2].index, 2);
}

#[tokio::test]
async fn apply_limit_after_since_filter() {
    let (store, _container) = setup().await;
    store.save_task(make_task("task-1")).await.unwrap();
    for i in 0..10 {
        store.save_event(make_event("task-1", i)).await.unwrap();
    }

    let opts = EventQueryOptions {
        since: Some(SinceCursor {
            index: Some(5),
            timestamp: None,
            id: None,
        }),
        limit: Some(2),
    };
    let events = store.get_events("task-1", Some(opts)).await.unwrap();
    assert_eq!(events.len(), 2);
    assert_eq!(events[0].index, 6);
    assert_eq!(events[1].index, 7);
}

#[tokio::test]
async fn save_event_on_conflict_do_nothing() {
    let (store, _container) = setup().await;
    store.save_task(make_task("task-1")).await.unwrap();

    let event = make_event("task-1", 0);
    store.save_event(event.clone()).await.unwrap();
    // Saving the same event again should not error
    store.save_event(event.clone()).await.unwrap();

    let events = store.get_events("task-1", None).await.unwrap();
    assert_eq!(events.len(), 1);
}

#[tokio::test]
async fn preserve_series_fields_on_events() {
    let (store, _container) = setup().await;
    store.save_task(make_task("task-1")).await.unwrap();
    let mut event = make_event("task-1", 0);
    event.series_id = Some("my-series".to_string());
    event.series_mode = Some(taskcast_core::types::SeriesMode::Accumulate);

    store.save_event(event.clone()).await.unwrap();
    let events = store.get_events("task-1", None).await.unwrap();
    assert_eq!(events[0], event);
}

// ─── Worker event helpers ─────────────────────────────────────────────────

fn make_worker_event(id: &str, worker_id: &str, index: u64) -> WorkerAuditEvent {
    WorkerAuditEvent {
        id: id.to_string(),
        worker_id: worker_id.to_string(),
        timestamp: 1000.0 + index as f64 * 100.0,
        action: WorkerAuditAction::Connected,
        data: None,
    }
}

// ─── save_worker_event / get_worker_events ────────────────────────────────

#[tokio::test]
async fn save_and_retrieve_worker_events() {
    let (store, _container) = setup().await;

    let e0 = make_worker_event("we-1", "worker-1", 0);
    let e1 = make_worker_event("we-2", "worker-1", 1);

    store.save_worker_event(e0.clone()).await.unwrap();
    store.save_worker_event(e1.clone()).await.unwrap();

    let events = store.get_worker_events("worker-1", None).await.unwrap();
    assert_eq!(events.len(), 2);
    assert_eq!(events[0], e0);
    assert_eq!(events[1], e1);
}

#[tokio::test]
async fn return_empty_when_no_worker_events() {
    let (store, _container) = setup().await;

    let events = store
        .get_worker_events("nonexistent-worker", None)
        .await
        .unwrap();
    assert!(events.is_empty());
}

#[tokio::test]
async fn save_multiple_worker_events_verify_ordering() {
    let (store, _container) = setup().await;

    // Insert events out of timestamp order
    let e2 = make_worker_event("we-3", "worker-1", 2);
    let e0 = make_worker_event("we-1", "worker-1", 0);
    let e1 = make_worker_event("we-2", "worker-1", 1);

    store.save_worker_event(e2.clone()).await.unwrap();
    store.save_worker_event(e0.clone()).await.unwrap();
    store.save_worker_event(e1.clone()).await.unwrap();

    let events = store.get_worker_events("worker-1", None).await.unwrap();
    assert_eq!(events.len(), 3);
    // Should be ordered by timestamp ASC regardless of insertion order
    assert_eq!(events[0].id, "we-1");
    assert_eq!(events[1].id, "we-2");
    assert_eq!(events[2].id, "we-3");
}

#[tokio::test]
async fn save_worker_event_with_data_field() {
    let (store, _container) = setup().await;

    let mut data = HashMap::new();
    data.insert("reason".to_string(), serde_json::json!("timeout"));
    data.insert("duration_ms".to_string(), serde_json::json!(5000));

    let event = WorkerAuditEvent {
        id: "we-data-1".to_string(),
        worker_id: "worker-1".to_string(),
        timestamp: 1000.0,
        action: WorkerAuditAction::HeartbeatTimeout,
        data: Some(data),
    };

    store.save_worker_event(event.clone()).await.unwrap();

    let events = store.get_worker_events("worker-1", None).await.unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0], event);
    let retrieved_data = events[0].data.as_ref().unwrap();
    assert_eq!(retrieved_data["reason"], serde_json::json!("timeout"));
    assert_eq!(retrieved_data["duration_ms"], serde_json::json!(5000));
}

#[tokio::test]
async fn duplicate_worker_event_id_is_ignored() {
    let (store, _container) = setup().await;

    let event = make_worker_event("we-dup", "worker-1", 0);
    store.save_worker_event(event.clone()).await.unwrap();
    // Saving the same event again should not error (ON CONFLICT DO NOTHING)
    store.save_worker_event(event.clone()).await.unwrap();

    let events = store.get_worker_events("worker-1", None).await.unwrap();
    assert_eq!(events.len(), 1);
}

// ─── Worker event filtering ──────────────────────────────────────────────

#[tokio::test]
async fn filter_worker_events_by_since_timestamp() {
    let (store, _container) = setup().await;

    for i in 0..5 {
        store
            .save_worker_event(make_worker_event(
                &format!("we-{}", i),
                "worker-1",
                i,
            ))
            .await
            .unwrap();
    }

    let opts = EventQueryOptions {
        since: Some(SinceCursor {
            index: None,
            timestamp: Some(1200.0),
            id: None,
        }),
        limit: None,
    };
    let events = store
        .get_worker_events("worker-1", Some(opts))
        .await
        .unwrap();
    assert_eq!(events.len(), 2);
    assert_eq!(events[0].timestamp, 1300.0);
    assert_eq!(events[1].timestamp, 1400.0);
}

#[tokio::test]
async fn filter_worker_events_by_since_id() {
    let (store, _container) = setup().await;

    for i in 0..5 {
        store
            .save_worker_event(make_worker_event(
                &format!("we-{}", i),
                "worker-1",
                i,
            ))
            .await
            .unwrap();
    }

    let opts = EventQueryOptions {
        since: Some(SinceCursor {
            index: None,
            timestamp: None,
            id: Some("we-2".to_string()),
        }),
        limit: None,
    };
    let events = store
        .get_worker_events("worker-1", Some(opts))
        .await
        .unwrap();
    assert_eq!(events.len(), 2);
    assert_eq!(events[0].id, "we-3");
    assert_eq!(events[1].id, "we-4");
}

#[tokio::test]
async fn return_all_worker_events_when_since_id_not_found() {
    let (store, _container) = setup().await;

    for i in 0..3 {
        store
            .save_worker_event(make_worker_event(
                &format!("we-{}", i),
                "worker-1",
                i,
            ))
            .await
            .unwrap();
    }

    let opts = EventQueryOptions {
        since: Some(SinceCursor {
            index: None,
            timestamp: None,
            id: Some("nonexistent-id".to_string()),
        }),
        limit: None,
    };
    let events = store
        .get_worker_events("worker-1", Some(opts))
        .await
        .unwrap();
    assert_eq!(events.len(), 3);
}

#[tokio::test]
async fn respect_limit_on_worker_events() {
    let (store, _container) = setup().await;

    for i in 0..10 {
        store
            .save_worker_event(make_worker_event(
                &format!("we-{}", i),
                "worker-1",
                i,
            ))
            .await
            .unwrap();
    }

    let opts = EventQueryOptions {
        since: None,
        limit: Some(3),
    };
    let events = store
        .get_worker_events("worker-1", Some(opts))
        .await
        .unwrap();
    assert_eq!(events.len(), 3);
    assert_eq!(events[0].id, "we-0");
    assert_eq!(events[2].id, "we-2");
}

#[tokio::test]
async fn combine_since_timestamp_and_limit_on_worker_events() {
    let (store, _container) = setup().await;

    for i in 0..10 {
        store
            .save_worker_event(make_worker_event(
                &format!("we-{}", i),
                "worker-1",
                i,
            ))
            .await
            .unwrap();
    }

    let opts = EventQueryOptions {
        since: Some(SinceCursor {
            index: None,
            timestamp: Some(1400.0),
            id: None,
        }),
        limit: Some(2),
    };
    let events = store
        .get_worker_events("worker-1", Some(opts))
        .await
        .unwrap();
    assert_eq!(events.len(), 2);
    // Events after timestamp 1400.0 are indices 5,6,7,8,9 (timestamps 1500,1600,...,1900)
    assert_eq!(events[0].timestamp, 1500.0);
    assert_eq!(events[1].timestamp, 1600.0);
}
