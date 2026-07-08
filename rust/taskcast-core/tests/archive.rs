use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use serde_json::json;
use taskcast_core::{
    BroadcastProvider, CreateTaskInput, EngineError, EventQueryOptions, Level, LongTermStore,
    MemoryBroadcastProvider, MemoryShortTermStore, PublishEventInput, SeriesMode, ShortTermStore,
    Task, TaskArchive, TaskArchiveImportOptions, TaskEngine, TaskEngineOptions, TaskEvent,
    TaskStatus, WorkerAuditEvent, build_task_archive_restore_data, validate_task_archive,
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

fn validate_archive_json(value: serde_json::Value) -> Result<(), String> {
    let archive: TaskArchive = serde_json::from_value(value).map_err(|err| err.to_string())?;
    validate_task_archive(&archive)
        .map(|_| ())
        .map_err(|err| err.to_string())
}

fn archive_json_with_event_field(
    field_name: &str,
    field_value: serde_json::Value,
) -> serde_json::Value {
    let mut event = json!({
        "id": format!("event-{field_name}"),
        "taskId": "task-1",
        "index": 0,
        "timestamp": 3000.0,
        "type": "demo.event",
        "level": "info",
        "data": null
    });
    event
        .as_object_mut()
        .unwrap()
        .insert(field_name.to_string(), field_value);

    json!({
        "schema": "taskcast.taskArchive",
        "version": 1,
        "exportedAt": 5000.0,
        "task": {
            "id": "task-1",
            "type": "demo",
            "status": "running",
            "createdAt": 1000.0,
            "updatedAt": 2000.0
        },
        "events": [event]
    })
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
fn validate_rejects_archive_metadata_optional_timestamps_and_empty_fields() {
    let mut unsupported_version = make_archive(vec![]);
    unsupported_version.version = 2;
    assert!(
        validate_task_archive(&unsupported_version)
            .unwrap_err()
            .to_string()
            .contains("Unsupported archive version")
    );

    let mut bad_exported_at = make_archive(vec![]);
    bad_exported_at.exported_at = f64::NAN;
    assert!(
        validate_task_archive(&bad_exported_at)
            .unwrap_err()
            .to_string()
            .contains("exported_at")
    );

    let mut duplicate_ids = make_archive(vec![
        make_event("same-id", "task-1", 0, json!(null)),
        make_event("same-id", "task-1", 1, json!(null)),
    ]);
    assert!(
        validate_task_archive(&duplicate_ids)
            .unwrap_err()
            .to_string()
            .contains("duplicate event id")
    );

    duplicate_ids.task.completed_at = Some(f64::INFINITY);
    assert!(
        validate_task_archive(&duplicate_ids)
            .unwrap_err()
            .to_string()
            .contains("completed_at")
    );

    let mut bad_resume_at = make_archive(vec![]);
    bad_resume_at.task.resume_at = Some(f64::NEG_INFINITY);
    assert!(
        validate_task_archive(&bad_resume_at)
            .unwrap_err()
            .to_string()
            .contains("resume_at")
    );

    let mut empty_task_id = make_archive(vec![]);
    empty_task_id.task.id.clear();
    assert!(
        validate_task_archive(&empty_task_id)
            .unwrap_err()
            .to_string()
            .contains("task.id")
    );

    let mut bad_event = make_event("event-1", "task-1", 0, json!(null));
    bad_event.timestamp = f64::NAN;
    assert!(
        validate_task_archive(&make_archive(vec![bad_event]))
            .unwrap_err()
            .to_string()
            .contains("event.timestamp")
    );

    let mut empty_series_acc_field = make_event("event-1", "task-1", 0, json!(null));
    empty_series_acc_field.series_id = Some("series".to_string());
    empty_series_acc_field.series_mode = Some(SeriesMode::Accumulate);
    empty_series_acc_field.series_acc_field = Some(String::new());
    assert!(
        validate_task_archive(&make_archive(vec![empty_series_acc_field]))
            .unwrap_err()
            .to_string()
            .contains("series_acc_field")
    );
}

#[test]
fn validate_rejects_non_contiguous_indexes_task_id_mismatch_and_transient_fields() {
    let non_contiguous = make_archive(vec![
        make_event("event-1", "task-1", 0, json!(null)),
        make_event("event-2", "task-1", 2, json!(null)),
    ]);
    assert!(
        validate_task_archive(&non_contiguous)
            .unwrap_err()
            .to_string()
            .contains("contiguous")
    );

    let mismatch = make_archive(vec![make_event("event-1", "other-task", 0, json!(null))]);
    assert!(
        validate_task_archive(&mismatch)
            .unwrap_err()
            .to_string()
            .contains("task_id")
    );

    let mut snapshot = make_event("event-1", "task-1", 0, json!(null));
    snapshot.series_snapshot = Some(true);
    assert!(
        validate_task_archive(&make_archive(vec![snapshot]))
            .unwrap_err()
            .to_string()
            .contains("series_snapshot")
    );

    let mut accumulated = make_event("event-1", "task-1", 0, json!(null));
    accumulated._accumulated_data = Some(json!({ "delta": "hello world" }));
    assert!(
        validate_task_archive(&make_archive(vec![accumulated]))
            .unwrap_err()
            .to_string()
            .contains("_accumulated_data")
    );
}

#[test]
fn validate_rejects_deserialized_accumulated_data_fields() {
    for field_name in ["_accumulatedData", "_accumulated_data"] {
        let err = validate_archive_json(archive_json_with_event_field(
            field_name,
            json!({ "delta": "transient" }),
        ))
        .unwrap_err();
        assert!(
            err.contains(field_name) || err.contains("_accumulated_data"),
            "expected {field_name} to be rejected, got {err}"
        );
    }
}

#[test]
fn validate_rejects_deserialized_null_presentation_fields_by_presence() {
    for field_name in ["seriesSnapshot", "_accumulatedData", "_accumulated_data"] {
        let err = validate_archive_json(archive_json_with_event_field(field_name, json!(null)))
            .unwrap_err();
        assert!(
            err.contains(field_name) || err.contains("_accumulated_data"),
            "expected {field_name}: null to be rejected, got {err}"
        );
    }
}

#[test]
fn task_event_does_not_deserialize_accumulated_data_outside_archive() {
    for field_name in ["_accumulatedData", "_accumulated_data"] {
        let mut value = json!({
            "id": "event-1",
            "taskId": "task-1",
            "index": 0,
            "timestamp": 3000.0,
            "type": "demo.event",
            "level": "info",
            "data": null
        });
        value
            .as_object_mut()
            .unwrap()
            .insert(field_name.to_string(), json!({ "delta": "transient" }));

        let event: TaskEvent = serde_json::from_value(value).unwrap();

        assert!(
            event._accumulated_data.is_none(),
            "shared TaskEvent should not deserialize {field_name}"
        );
    }
}

#[test]
fn archive_forbidden_field_error_includes_event_position_and_id() {
    let mut bad_event = json!({
        "id": "bad-event",
        "taskId": "task-1",
        "index": 1,
        "timestamp": 3001.0,
        "type": "demo.event",
        "level": "info",
        "data": null
    });
    bad_event
        .as_object_mut()
        .unwrap()
        .insert("seriesSnapshot".to_string(), json!(null));
    let archive = json!({
        "schema": "taskcast.taskArchive",
        "version": 1,
        "exportedAt": 5000.0,
        "task": {
            "id": "task-1",
            "type": "demo",
            "status": "running",
            "createdAt": 1000.0,
            "updatedAt": 2000.0
        },
        "events": [
            {
                "id": "good-event",
                "taskId": "task-1",
                "index": 0,
                "timestamp": 3000.0,
                "type": "demo.event",
                "level": "info",
                "data": null
            },
            bad_event
        ]
    });

    let err = serde_json::from_value::<TaskArchive>(archive)
        .unwrap_err()
        .to_string();

    assert!(
        err.contains("event[1]"),
        "expected event position in error, got {err}"
    );
    assert!(
        err.contains("bad-event"),
        "expected event id in error, got {err}"
    );
    assert!(err.contains("seriesSnapshot"));
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

#[test]
fn restore_data_skips_keep_all_and_falls_back_when_accumulate_fields_are_not_strings() {
    let mut keep_all = make_event("keep-all", "task-1", 0, json!({ "delta": "ignored" }));
    keep_all.series_id = Some("keep".to_string());
    keep_all.series_mode = Some(SeriesMode::KeepAll);

    let mut non_string_previous =
        make_event("non-string-previous", "task-1", 1, json!({ "delta": 1 }));
    non_string_previous.series_id = Some("numbers".to_string());
    non_string_previous.series_mode = Some(SeriesMode::Accumulate);

    let mut non_string_current =
        make_event("non-string-current", "task-1", 2, json!({ "delta": "two" }));
    non_string_current.series_id = Some("numbers".to_string());
    non_string_current.series_mode = Some(SeriesMode::Accumulate);

    let mut string_previous =
        make_event("string-previous", "task-1", 3, json!({ "delta": "hello" }));
    string_previous.series_id = Some("mixed".to_string());
    string_previous.series_mode = Some(SeriesMode::Accumulate);

    let mut object_current = make_event("object-current", "task-1", 4, json!({ "delta": 2 }));
    object_current.series_id = Some("mixed".to_string());
    object_current.series_mode = Some(SeriesMode::Accumulate);

    let restore = build_task_archive_restore_data(&make_archive(vec![
        keep_all,
        non_string_previous,
        non_string_current,
        string_previous,
        object_current,
    ]))
    .unwrap();
    let latest_by_series: HashMap<_, _> = restore
        .series_latest
        .iter()
        .map(|entry| (entry.series_id.as_str(), &entry.event))
        .collect();

    assert!(!latest_by_series.contains_key("keep"));
    assert_eq!(latest_by_series["numbers"].id, "non-string-current");
    assert_eq!(latest_by_series["numbers"].data, json!({ "delta": "two" }));
    assert_eq!(latest_by_series["mixed"].id, "object-current");
    assert_eq!(latest_by_series["mixed"].data, json!({ "delta": 2 }));
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
async fn engine_import_rejects_short_term_store_without_archive_restore_support() {
    let engine = TaskEngine::new(TaskEngineOptions {
        short_term_store: Arc::new(UnsupportedShortTermStore),
        broadcast: Arc::new(MemoryBroadcastProvider::new()),
        long_term_store: None,
        hooks: None,
    });

    let result = engine.import_task_archive(make_archive(vec![]), None).await;

    assert!(
        result
            .unwrap_err()
            .to_string()
            .contains("shortTermStore does not support restore_task_archive")
    );
}

#[tokio::test]
async fn default_archive_restore_trait_methods_report_unsupported() {
    let data = build_task_archive_restore_data(&make_archive(vec![])).unwrap();
    let short = UnsupportedShortTermStore;
    let long = MockLongTermStore::default();

    assert!(!short.supports_task_archive_restore());
    assert!(
        short
            .validate_task_archive_restore(&data, None)
            .await
            .unwrap_err()
            .to_string()
            .contains("not supported")
    );
    assert!(
        short
            .restore_task_archive(data.clone(), None)
            .await
            .unwrap_err()
            .to_string()
            .contains("not supported")
    );

    assert!(!long.shares_task_archive_restore_storage());
    assert!(
        long.validate_task_archive_restore(&data, None)
            .await
            .unwrap_err()
            .to_string()
            .contains("not supported")
    );
    assert!(
        long.restore_task_archive(data, None)
            .await
            .unwrap_err()
            .to_string()
            .contains("not supported")
    );
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
async fn export_prefers_long_term_history_but_uses_short_term_raw_accumulate_deltas() {
    let short = Arc::new(MemoryShortTermStore::new());
    let long = Arc::new(MockLongTermStore::default());
    short.save_task(make_task("task-1")).await.unwrap();

    let mut raw_delta = make_event("delta-0", "task-1", 0, json!({ "delta": "hello" }));
    raw_delta.series_id = Some("output".to_string());
    raw_delta.series_mode = Some(SeriesMode::Accumulate);
    short
        .append_event("task-1", raw_delta.clone())
        .await
        .unwrap();
    short
        .append_event(
            "task-1",
            make_event("short-only", "task-1", 2, json!({ "value": "tail" })),
        )
        .await
        .unwrap();

    let mut accumulated = make_event("delta-0", "task-1", 0, json!({ "delta": "hello world" }));
    accumulated.series_id = Some("output".to_string());
    accumulated.series_mode = Some(SeriesMode::Accumulate);
    long.save_event(accumulated).await.unwrap();
    long.save_event(make_event(
        "long-1",
        "task-1",
        1,
        json!({ "value": "durable" }),
    ))
    .await
    .unwrap();

    let engine = make_engine(
        Arc::clone(&short),
        Arc::new(MemoryBroadcastProvider::new()),
        Some(long),
    );

    let archive = engine.export_task_archive("task-1").await.unwrap();

    assert_eq!(
        archive
            .events
            .iter()
            .map(|event| event.id.as_str())
            .collect::<Vec<_>>(),
        vec!["delta-0", "long-1", "short-only"]
    );
    assert_eq!(archive.events[0].data, json!({ "delta": "hello" }));
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
    assert!(
        archive
            .events
            .iter()
            .all(|event| event.series_snapshot.is_none())
    );
    assert!(
        archive
            .events
            .iter()
            .all(|event| event._accumulated_data.is_none())
    );
}

struct UnsupportedShortTermStore;

#[async_trait]
impl ShortTermStore for UnsupportedShortTermStore {
    async fn save_task(&self, _task: Task) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        Ok(())
    }

    async fn get_task(
        &self,
        _task_id: &str,
    ) -> Result<Option<Task>, Box<dyn std::error::Error + Send + Sync>> {
        Ok(None)
    }

    async fn append_event(
        &self,
        _task_id: &str,
        _event: TaskEvent,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        Ok(())
    }

    async fn get_events(
        &self,
        _task_id: &str,
        _opts: Option<EventQueryOptions>,
    ) -> Result<Vec<TaskEvent>, Box<dyn std::error::Error + Send + Sync>> {
        Ok(Vec::new())
    }

    async fn set_ttl(
        &self,
        _task_id: &str,
        _ttl_seconds: u64,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        Ok(())
    }

    async fn get_series_latest(
        &self,
        _task_id: &str,
        _series_id: &str,
    ) -> Result<Option<TaskEvent>, Box<dyn std::error::Error + Send + Sync>> {
        Ok(None)
    }

    async fn set_series_latest(
        &self,
        _task_id: &str,
        _series_id: &str,
        _event: TaskEvent,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        Ok(())
    }

    async fn replace_last_series_event(
        &self,
        _task_id: &str,
        _series_id: &str,
        _event: TaskEvent,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        Ok(())
    }

    async fn accumulate_series(
        &self,
        _task_id: &str,
        _series_id: &str,
        event: TaskEvent,
        _field: &str,
    ) -> Result<TaskEvent, Box<dyn std::error::Error + Send + Sync>> {
        Ok(event)
    }

    async fn next_index(
        &self,
        _task_id: &str,
    ) -> Result<u64, Box<dyn std::error::Error + Send + Sync>> {
        Ok(0)
    }

    async fn list_tasks(
        &self,
        _filter: taskcast_core::TaskFilter,
    ) -> Result<Vec<Task>, Box<dyn std::error::Error + Send + Sync>> {
        Ok(Vec::new())
    }

    async fn save_worker(
        &self,
        _worker: taskcast_core::Worker,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        Ok(())
    }

    async fn get_worker(
        &self,
        _worker_id: &str,
    ) -> Result<Option<taskcast_core::Worker>, Box<dyn std::error::Error + Send + Sync>> {
        Ok(None)
    }

    async fn list_workers(
        &self,
        _filter: Option<taskcast_core::WorkerFilter>,
    ) -> Result<Vec<taskcast_core::Worker>, Box<dyn std::error::Error + Send + Sync>> {
        Ok(Vec::new())
    }

    async fn delete_worker(
        &self,
        _worker_id: &str,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        Ok(())
    }

    async fn claim_task(
        &self,
        _task_id: &str,
        _worker_id: &str,
        _cost: u32,
    ) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
        Ok(false)
    }

    async fn add_assignment(
        &self,
        _assignment: taskcast_core::WorkerAssignment,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        Ok(())
    }

    async fn get_task_assignment(
        &self,
        _task_id: &str,
    ) -> Result<Option<taskcast_core::WorkerAssignment>, Box<dyn std::error::Error + Send + Sync>>
    {
        Ok(None)
    }

    async fn get_worker_assignments(
        &self,
        _worker_id: &str,
    ) -> Result<Vec<taskcast_core::WorkerAssignment>, Box<dyn std::error::Error + Send + Sync>>
    {
        Ok(Vec::new())
    }

    async fn remove_assignment(
        &self,
        _task_id: &str,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        Ok(())
    }
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

    assert!(
        result
            .unwrap_err()
            .to_string()
            .contains("longTermStore does not support restore_task_archive")
    );
    assert!(short.get_task("task-1").await.unwrap().is_none());
    assert!(short.get_events("task-1", None).await.unwrap().is_empty());
}

#[tokio::test]
async fn engine_import_passes_original_options_to_final_restore() {
    let long = Arc::new(RecordingLongTermStore::default());
    let engine = make_engine(
        Arc::new(MemoryShortTermStore::new()),
        Arc::new(MemoryBroadcastProvider::new()),
        Some(long.clone()),
    );

    engine
        .import_task_archive(
            make_archive(vec![]),
            Some(TaskArchiveImportOptions { overwrite: false }),
        )
        .await
        .unwrap();

    let restore_options = long.restore_options.lock().unwrap();
    assert_eq!(
        restore_options.as_slice(),
        &[Some(TaskArchiveImportOptions { overwrite: false })]
    );
}

#[tokio::test]
async fn engine_import_does_not_mutate_short_term_when_long_term_final_restore_fails() {
    let short = Arc::new(MemoryShortTermStore::new());
    let long = Arc::new(RecordingLongTermStore {
        fail_restore: true,
        ..Default::default()
    });
    let engine = make_engine(
        Arc::clone(&short),
        Arc::new(MemoryBroadcastProvider::new()),
        Some(long.clone()),
    );

    let result = engine.import_task_archive(make_archive(vec![]), None).await;

    assert!(
        result
            .unwrap_err()
            .to_string()
            .contains("long restore failed")
    );
    assert_eq!(long.restore_options.lock().unwrap().len(), 1);
    assert!(short.get_task("task-1").await.unwrap().is_none());
    assert!(short.get_events("task-1", None).await.unwrap().is_empty());
}

#[tokio::test]
async fn engine_import_skips_long_term_final_restore_when_archive_storage_is_shared() {
    let short = Arc::new(MemoryShortTermStore::new());
    let long = Arc::new(RecordingLongTermStore {
        fail_restore: true,
        shares_restore_storage: true,
        ..Default::default()
    });
    let engine = make_engine(
        Arc::clone(&short),
        Arc::new(MemoryBroadcastProvider::new()),
        Some(long.clone()),
    );

    let result = engine
        .import_task_archive(make_archive(vec![]), None)
        .await
        .unwrap();

    assert_eq!(result.task_id, "task-1");
    assert!(long.restore_options.lock().unwrap().is_empty());
    assert!(short.get_task("task-1").await.unwrap().is_some());
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

#[derive(Default)]
struct RecordingLongTermStore {
    restore_options: Mutex<Vec<Option<TaskArchiveImportOptions>>>,
    fail_restore: bool,
    shares_restore_storage: bool,
}

#[async_trait]
impl LongTermStore for RecordingLongTermStore {
    async fn save_task(&self, _task: Task) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        Ok(())
    }

    async fn get_task(
        &self,
        _task_id: &str,
    ) -> Result<Option<Task>, Box<dyn std::error::Error + Send + Sync>> {
        Ok(None)
    }

    async fn save_event(
        &self,
        _event: TaskEvent,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        Ok(())
    }

    async fn get_events(
        &self,
        _task_id: &str,
        _opts: Option<EventQueryOptions>,
    ) -> Result<Vec<TaskEvent>, Box<dyn std::error::Error + Send + Sync>> {
        Ok(Vec::new())
    }

    fn supports_task_archive_restore(&self) -> bool {
        true
    }

    fn shares_task_archive_restore_storage(&self) -> bool {
        self.shares_restore_storage
    }

    async fn validate_task_archive_restore(
        &self,
        _data: &taskcast_core::TaskArchiveRestoreData,
        _options: Option<TaskArchiveImportOptions>,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        Ok(())
    }

    async fn restore_task_archive(
        &self,
        _data: taskcast_core::TaskArchiveRestoreData,
        options: Option<TaskArchiveImportOptions>,
    ) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
        self.restore_options.lock().unwrap().push(options);
        if self.fail_restore {
            return Err(
                std::io::Error::new(std::io::ErrorKind::Other, "long restore failed").into(),
            );
        }
        Ok(false)
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
