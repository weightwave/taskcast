use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use serde_json::json;
use taskcast_core::{
    build_task_archive_restore_data, validate_task_archive, BroadcastProvider, CreateTaskInput,
    EngineError, EventQueryOptions, Level, LongTermStore, MemoryBroadcastProvider,
    MemoryShortTermStore, PublishEventInput, SeriesMode, ShortTermStore, Task, TaskArchive,
    TaskArchiveImportOptions, TaskEngine, TaskEngineOptions, TaskEvent, TaskStatus,
    WorkerAuditEvent,
};

fn make_task(id: &str) -> Task {
    Task {
        id: id.to_string(),
        r#type: Some("demo".to_string()),
        status: TaskStatus::Running,
        params: None,
        result: None,
        error: None,
        metadata: None,
        created_at: 1000.0,
        updated_at: 2000.0,
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

fn make_event(id: &str, task_id: &str, index: u64, data: serde_json::Value) -> TaskEvent {
    TaskEvent {
        id: id.to_string(),
        task_id: task_id.to_string(),
        index,
        timestamp: 3000.0 + index as f64,
        r#type: "demo.event".to_string(),
        level: Level::Info,
        data,
        series_id: None,
        series_mode: None,
        series_acc_field: None,
        series_snapshot: None,
        _accumulated_data: None,
    }
}

fn make_archive(events: Vec<TaskEvent>) -> TaskArchive {
    TaskArchive {
        schema: "taskcast.taskArchive".to_string(),
        version: 1,
        exported_at: 5000.0,
        task: make_task("task-1"),
        events,
    }
}

fn make_engine(
    short_term_store: Arc<MemoryShortTermStore>,
    broadcast: Arc<MemoryBroadcastProvider>,
    long_term_store: Option<Arc<dyn LongTermStore>>,
) -> TaskEngine {
    TaskEngine::new(TaskEngineOptions {
        short_term_store,
        broadcast,
        long_term_store,
        hooks: None,
    })
}

#[test]
fn validate_rejects_duplicate_indexes() {
    let archive = make_archive(vec![
        make_event("event-1", "task-1", 0, json!(null)),
        make_event("event-2", "task-1", 0, json!(null)),
    ]);

    let err = validate_task_archive(&archive).unwrap_err();

    assert!(err.to_string().contains("duplicate event index"));
}

#[test]
fn validate_rejects_non_contiguous_indexes_task_id_mismatch_and_transient_fields() {
    let non_contiguous = make_archive(vec![
        make_event("event-1", "task-1", 0, json!(null)),
        make_event("event-2", "task-1", 2, json!(null)),
    ]);
    assert!(validate_task_archive(&non_contiguous)
        .unwrap_err()
        .to_string()
        .contains("contiguous"));

    let mismatch = make_archive(vec![make_event("event-1", "other-task", 0, json!(null))]);
    assert!(validate_task_archive(&mismatch)
        .unwrap_err()
        .to_string()
        .contains("task_id"));

    let mut snapshot = make_event("event-1", "task-1", 0, json!(null));
    snapshot.series_snapshot = Some(true);
    assert!(validate_task_archive(&make_archive(vec![snapshot]))
        .unwrap_err()
        .to_string()
        .contains("series_snapshot"));

    let mut accumulated = make_event("event-1", "task-1", 0, json!(null));
    accumulated._accumulated_data = Some(json!({ "delta": "hello world" }));
    assert!(validate_task_archive(&make_archive(vec![accumulated]))
        .unwrap_err()
        .to_string()
        .contains("_accumulated_data"));
}

#[test]
fn restore_data_sets_next_index_and_rebuilds_accumulate_and_latest_series() {
    let mut latest_old = make_event("status-old", "task-1", 0, json!({ "status": "starting" }));
    latest_old.r#type = "task.status".to_string();
    latest_old.series_id = Some("status".to_string());
    latest_old.series_mode = Some(SeriesMode::Latest);

    let mut latest_new = make_event("status-new", "task-1", 1, json!({ "status": "ready" }));
    latest_new.r#type = "task.status".to_string();
    latest_new.series_id = Some("status".to_string());
    latest_new.series_mode = Some(SeriesMode::Latest);

    let mut output_one = make_event("output-1", "task-1", 2, json!({ "delta": "hello " }));
    output_one.r#type = "task.output".to_string();
    output_one.series_id = Some("output".to_string());
    output_one.series_mode = Some(SeriesMode::Accumulate);

    let mut output_two = make_event("output-2", "task-1", 3, json!({ "delta": "world" }));
    output_two.r#type = "task.output".to_string();
    output_two.series_id = Some("output".to_string());
    output_two.series_mode = Some(SeriesMode::Accumulate);

    let restore = build_task_archive_restore_data(&make_archive(vec![
        latest_old, latest_new, output_one, output_two,
    ]))
    .unwrap();
    let latest_by_series: HashMap<_, _> = restore
        .series_latest
        .iter()
        .map(|entry| (entry.series_id.as_str(), &entry.event))
        .collect();

    assert_eq!(restore.next_index, 4);
    assert_eq!(latest_by_series["status"].id, "status-new");
    assert_eq!(
        latest_by_series["status"].data,
        json!({ "status": "ready" })
    );
    assert_eq!(latest_by_series["output"].id, "output-2");
    assert_eq!(
        latest_by_series["output"].data,
        json!({ "delta": "hello world" })
    );
}

#[tokio::test]
async fn engine_import_preserves_history_is_silent_and_next_index_continues() {
    let archive = make_archive(vec![
        make_event("event-1", "task-1", 0, json!({ "value": 1 })),
        make_event("event-2", "task-1", 1, json!({ "value": 2 })),
    ]);
    let short = Arc::new(MemoryShortTermStore::new());
    let broadcast = Arc::new(MemoryBroadcastProvider::new());
    let observed = Arc::new(Mutex::new(Vec::new()));
    let observed_for_handler = Arc::clone(&observed);
    let _unsubscribe = broadcast
        .subscribe_sync(
            "task-1",
            Box::new(move |event| {
                observed_for_handler.lock().unwrap().push(event);
            }),
        )
        .unwrap();
    let engine = make_engine(Arc::clone(&short), Arc::clone(&broadcast), None);

    let result = engine.import_task_archive(archive, None).await.unwrap();
    let restored = engine.get_events("task-1", None).await.unwrap();
    let next = engine
        .publish_event(
            "task-1",
            PublishEventInput {
                r#type: "demo.next".to_string(),
                level: Level::Info,
                data: json!(null),
                series_id: None,
                series_mode: None,
                series_acc_field: None,
            },
        )
        .await
        .unwrap();
    let observed = observed.lock().unwrap();

    assert_eq!(result.task_id, "task-1");
    assert_eq!(result.event_count, 2);
    assert!(!result.overwritten);
    assert_eq!(
        restored
            .iter()
            .map(|event| event.id.as_str())
            .collect::<Vec<_>>(),
        vec!["event-1", "event-2"]
    );
    assert_eq!(next.index, 2);
    assert_eq!(observed.len(), 1);
    assert_eq!(observed[0].r#type, "demo.next");
}

#[tokio::test]
async fn engine_import_rejects_conflict_without_overwrite_and_allows_overwrite() {
    let short = Arc::new(MemoryShortTermStore::new());
    let engine = make_engine(
        Arc::clone(&short),
        Arc::new(MemoryBroadcastProvider::new()),
        None,
    );
    engine
        .create_task(CreateTaskInput {
            id: Some("task-1".to_string()),
            ..Default::default()
        })
        .await
        .unwrap();
    engine
        .publish_event(
            "task-1",
            PublishEventInput {
                r#type: "old.event".to_string(),
                level: Level::Info,
                data: json!(null),
                series_id: None,
                series_mode: None,
                series_acc_field: None,
            },
        )
        .await
        .unwrap();

    let conflict = engine.import_task_archive(make_archive(vec![]), None).await;
    assert!(matches!(conflict, Err(EngineError::TaskConflict(task_id)) if task_id == "task-1"));

    let result = engine
        .import_task_archive(
            make_archive(vec![make_event("imported-event", "task-1", 0, json!(null))]),
            Some(TaskArchiveImportOptions { overwrite: true }),
        )
        .await
        .unwrap();
    let events = engine.get_events("task-1", None).await.unwrap();

    assert!(result.overwritten);
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].id, "imported-event");
}

#[tokio::test]
async fn export_clears_transient_fields_and_returns_sorted_raw_events() {
    let short = Arc::new(MemoryShortTermStore::new());
    short.save_task(make_task("task-1")).await.unwrap();

    let mut second = make_event("event-2", "task-1", 1, json!({ "value": 2 }));
    second.series_snapshot = Some(true);
    second._accumulated_data = Some(json!({ "value": "transient" }));
    short.append_event("task-1", second).await.unwrap();
    short
        .append_event(
            "task-1",
            make_event("event-1", "task-1", 0, json!({ "value": 1 })),
        )
        .await
        .unwrap();
    let engine = make_engine(
        Arc::clone(&short),
        Arc::new(MemoryBroadcastProvider::new()),
        None,
    );

    let archive = engine.export_task_archive("task-1").await.unwrap();

    assert_eq!(
        archive
            .events
            .iter()
            .map(|event| event.id.as_str())
            .collect::<Vec<_>>(),
        vec!["event-1", "event-2"]
    );
    assert!(archive
        .events
        .iter()
        .all(|event| event.series_snapshot.is_none()));
    assert!(archive
        .events
        .iter()
        .all(|event| event._accumulated_data.is_none()));
}

#[tokio::test]
async fn engine_import_fails_closed_when_long_term_store_cannot_restore_archives() {
    let short = Arc::new(MemoryShortTermStore::new());
    let long = Arc::new(MockLongTermStore::default());
    let engine = make_engine(
        Arc::clone(&short),
        Arc::new(MemoryBroadcastProvider::new()),
        Some(long),
    );

    let result = engine.import_task_archive(make_archive(vec![]), None).await;

    assert!(result
        .unwrap_err()
        .to_string()
        .contains("longTermStore does not support restore_task_archive"));
    assert!(short.get_task("task-1").await.unwrap().is_none());
    assert!(short.get_events("task-1", None).await.unwrap().is_empty());
}

#[derive(Default)]
struct MockLongTermStore {
    tasks: Mutex<HashMap<String, Task>>,
    events: Mutex<Vec<TaskEvent>>,
}

#[async_trait]
impl LongTermStore for MockLongTermStore {
    async fn save_task(&self, task: Task) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        self.tasks.lock().unwrap().insert(task.id.clone(), task);
        Ok(())
    }

    async fn get_task(
        &self,
        task_id: &str,
    ) -> Result<Option<Task>, Box<dyn std::error::Error + Send + Sync>> {
        Ok(self.tasks.lock().unwrap().get(task_id).cloned())
    }

    async fn save_event(
        &self,
        event: TaskEvent,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        self.events.lock().unwrap().push(event);
        Ok(())
    }

    async fn get_events(
        &self,
        task_id: &str,
        _opts: Option<EventQueryOptions>,
    ) -> Result<Vec<TaskEvent>, Box<dyn std::error::Error + Send + Sync>> {
        Ok(self
            .events
            .lock()
            .unwrap()
            .iter()
            .filter(|event| event.task_id == task_id)
            .cloned()
            .collect())
    }

    async fn save_worker_event(
        &self,
        _event: WorkerAuditEvent,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        Ok(())
    }

    async fn get_worker_events(
        &self,
        _worker_id: &str,
        _opts: Option<EventQueryOptions>,
    ) -> Result<Vec<WorkerAuditEvent>, Box<dyn std::error::Error + Send + Sync>> {
        Ok(Vec::new())
    }
}
