mod helpers;

use helpers::{make_event, make_task, setup};
use taskcast_core::types::{
    AssignMode, ConnectionMode, DisconnectPolicy, EventQueryOptions, SeriesMode, ShortTermStore,
    SinceCursor, TagMatcher, TaskFilter, TaskStatus, Worker, WorkerAssignment,
    WorkerAssignmentStatus, WorkerFilter, WorkerMatchRule, WorkerStatus,
};

// ─── save_task / get_task ─────────────────────────────────────────────────

#[tokio::test]
async fn save_and_retrieve_a_task() {
    let ctx = setup().await;
    let task = make_task("task-1");
    ctx.short.save_task(task.clone()).await.unwrap();
    let retrieved = ctx.short.get_task("task-1").await.unwrap();
    assert_eq!(retrieved, Some(task));
}

#[tokio::test]
async fn return_none_for_missing_task() {
    let ctx = setup().await;
    let result = ctx.short.get_task("nonexistent").await.unwrap();
    assert_eq!(result, None);
}

#[tokio::test]
async fn upsert_task_on_conflict() {
    let ctx = setup().await;
    let task = make_task("task-1");
    ctx.short.save_task(task.clone()).await.unwrap();

    let mut updated = task.clone();
    updated.status = TaskStatus::Running;
    updated.updated_at = 2000.0;
    ctx.short.save_task(updated.clone()).await.unwrap();

    let retrieved = ctx.short.get_task("task-1").await.unwrap().unwrap();
    assert_eq!(retrieved.status, TaskStatus::Running);
    assert_eq!(retrieved.updated_at, 2000.0);
}

#[tokio::test]
async fn preserve_optional_fields_on_round_trip() {
    let ctx = setup().await;
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

    ctx.short.save_task(task.clone()).await.unwrap();
    let retrieved = ctx.short.get_task("task-1").await.unwrap().unwrap();
    assert_eq!(retrieved, task);
}

#[tokio::test]
async fn handle_task_with_no_optional_fields() {
    let ctx = setup().await;
    let task = taskcast_core::types::Task {
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
        created_at: 1000.0,
        updated_at: 1000.0,
        completed_at: None,
        ttl: None,
        tags: None,
        assign_mode: None,
        cost: None,
        assigned_worker: None,
        disconnect_policy: None,
    };
    ctx.short.save_task(task.clone()).await.unwrap();
    let retrieved = ctx.short.get_task("minimal").await.unwrap().unwrap();
    assert_eq!(retrieved, task);
    assert!(retrieved.params.is_none());
    assert!(retrieved.r#type.is_none());
    assert!(retrieved.result.is_none());
    assert!(retrieved.error.is_none());
    assert!(retrieved.metadata.is_none());
    assert!(retrieved.completed_at.is_none());
    assert!(retrieved.ttl.is_none());
}

// ─── next_index ──────────────────────────────────────────────────────────

#[tokio::test]
async fn generate_monotonic_indices_starting_from_0() {
    let ctx = setup().await;
    ctx.short.save_task(make_task("task-1")).await.unwrap();

    let i0 = ctx.short.next_index("task-1").await.unwrap();
    let i1 = ctx.short.next_index("task-1").await.unwrap();
    let i2 = ctx.short.next_index("task-1").await.unwrap();

    assert_eq!(i0, 0);
    assert_eq!(i1, 1);
    assert_eq!(i2, 2);
}

#[tokio::test]
async fn maintain_separate_counters_per_task() {
    let ctx = setup().await;
    ctx.short.save_task(make_task("task-a")).await.unwrap();
    ctx.short.save_task(make_task("task-b")).await.unwrap();

    let a0 = ctx.short.next_index("task-a").await.unwrap();
    let b0 = ctx.short.next_index("task-b").await.unwrap();
    let a1 = ctx.short.next_index("task-a").await.unwrap();

    assert_eq!(a0, 0);
    assert_eq!(b0, 0);
    assert_eq!(a1, 1);
}

// ─── append_event / get_events ────────────────────────────────────────────

#[tokio::test]
async fn append_and_retrieve_events_in_order() {
    let ctx = setup().await;
    ctx.short.save_task(make_task("task-1")).await.unwrap();

    let e0 = make_event("task-1", 0);
    let e1 = make_event("task-1", 1);
    let e2 = make_event("task-1", 2);

    ctx.short.append_event("task-1", e0.clone()).await.unwrap();
    ctx.short.append_event("task-1", e1.clone()).await.unwrap();
    ctx.short.append_event("task-1", e2.clone()).await.unwrap();

    let events = ctx.short.get_events("task-1", None).await.unwrap();
    assert_eq!(events, vec![e0, e1, e2]);
}

#[tokio::test]
async fn return_empty_vec_when_no_events_exist() {
    let ctx = setup().await;
    ctx.short.save_task(make_task("task-1")).await.unwrap();
    let events = ctx.short.get_events("task-1", None).await.unwrap();
    assert!(events.is_empty());
}

#[tokio::test]
async fn filter_events_by_since_index() {
    let ctx = setup().await;
    ctx.short.save_task(make_task("task-1")).await.unwrap();
    for i in 0..5 {
        ctx.short
            .append_event("task-1", make_event("task-1", i))
            .await
            .unwrap();
    }

    let opts = EventQueryOptions {
        since: Some(SinceCursor {
            index: Some(2),
            timestamp: None,
            id: None,
        }),
        limit: None,
    };
    let events = ctx.short.get_events("task-1", Some(opts)).await.unwrap();
    assert_eq!(events.len(), 2);
    assert_eq!(events[0].index, 3);
    assert_eq!(events[1].index, 4);
}

#[tokio::test]
async fn filter_events_by_since_timestamp() {
    let ctx = setup().await;
    ctx.short.save_task(make_task("task-1")).await.unwrap();
    for i in 0..5 {
        ctx.short
            .append_event("task-1", make_event("task-1", i))
            .await
            .unwrap();
    }

    // Timestamps: 1000, 1100, 1200, 1300, 1400
    let opts = EventQueryOptions {
        since: Some(SinceCursor {
            index: None,
            timestamp: Some(1200.0),
            id: None,
        }),
        limit: None,
    };
    let events = ctx.short.get_events("task-1", Some(opts)).await.unwrap();
    assert_eq!(events.len(), 2);
    assert_eq!(events[0].timestamp, 1300.0);
    assert_eq!(events[1].timestamp, 1400.0);
}

#[tokio::test]
async fn filter_events_by_since_id() {
    let ctx = setup().await;
    ctx.short.save_task(make_task("task-1")).await.unwrap();
    for i in 0..5 {
        ctx.short
            .append_event("task-1", make_event("task-1", i))
            .await
            .unwrap();
    }

    let opts = EventQueryOptions {
        since: Some(SinceCursor {
            index: None,
            timestamp: None,
            id: Some("evt-task-1-2".to_string()),
        }),
        limit: None,
    };
    let events = ctx.short.get_events("task-1", Some(opts)).await.unwrap();
    assert_eq!(events.len(), 2);
    assert_eq!(events[0].id, "evt-task-1-3");
    assert_eq!(events[1].id, "evt-task-1-4");
}

#[tokio::test]
async fn return_all_events_when_since_id_not_found() {
    let ctx = setup().await;
    ctx.short.save_task(make_task("task-1")).await.unwrap();
    for i in 0..3 {
        ctx.short
            .append_event("task-1", make_event("task-1", i))
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
    let events = ctx.short.get_events("task-1", Some(opts)).await.unwrap();
    assert_eq!(events.len(), 3);
}

#[tokio::test]
async fn respect_limit_parameter() {
    let ctx = setup().await;
    ctx.short.save_task(make_task("task-1")).await.unwrap();
    for i in 0..10 {
        ctx.short
            .append_event("task-1", make_event("task-1", i))
            .await
            .unwrap();
    }

    let opts = EventQueryOptions {
        since: None,
        limit: Some(3),
    };
    let events = ctx.short.get_events("task-1", Some(opts)).await.unwrap();
    assert_eq!(events.len(), 3);
    assert_eq!(events[0].index, 0);
    assert_eq!(events[2].index, 2);
}

#[tokio::test]
async fn apply_limit_after_since_filter() {
    let ctx = setup().await;
    ctx.short.save_task(make_task("task-1")).await.unwrap();
    for i in 0..10 {
        ctx.short
            .append_event("task-1", make_event("task-1", i))
            .await
            .unwrap();
    }

    let opts = EventQueryOptions {
        since: Some(SinceCursor {
            index: Some(5),
            timestamp: None,
            id: None,
        }),
        limit: Some(2),
    };
    let events = ctx.short.get_events("task-1", Some(opts)).await.unwrap();
    assert_eq!(events.len(), 2);
    assert_eq!(events[0].index, 6);
    assert_eq!(events[1].index, 7);
}

// ─── series ─────────────────────────────────────────────────────────────

#[tokio::test]
async fn manage_series_latest_set_and_get() {
    let ctx = setup().await;
    ctx.short.save_task(make_task("task-1")).await.unwrap();
    let event = make_event("task-1", 0);
    ctx.short
        .set_series_latest("task-1", "series-a", event.clone())
        .await
        .unwrap();

    let latest = ctx
        .short
        .get_series_latest("task-1", "series-a")
        .await
        .unwrap();
    assert_eq!(latest, Some(event));
}

#[tokio::test]
async fn return_none_for_missing_series() {
    let ctx = setup().await;
    let latest = ctx
        .short
        .get_series_latest("task-1", "nonexistent")
        .await
        .unwrap();
    assert_eq!(latest, None);
}

#[tokio::test]
async fn update_series_latest_on_conflict() {
    let ctx = setup().await;
    ctx.short.save_task(make_task("task-1")).await.unwrap();
    let e0 = make_event("task-1", 0);
    let e1 = make_event("task-1", 1);

    ctx.short
        .set_series_latest("task-1", "series-a", e0)
        .await
        .unwrap();
    ctx.short
        .set_series_latest("task-1", "series-a", e1.clone())
        .await
        .unwrap();

    let latest = ctx
        .short
        .get_series_latest("task-1", "series-a")
        .await
        .unwrap();
    assert_eq!(latest, Some(e1));
}

// ─── replace_last_series_event ─────────────────────────────────────────

#[tokio::test]
async fn replace_last_series_event_in_event_list() {
    let ctx = setup().await;
    ctx.short.save_task(make_task("task-1")).await.unwrap();
    let e0 = make_event("task-1", 0);

    ctx.short.append_event("task-1", e0.clone()).await.unwrap();
    ctx.short
        .set_series_latest("task-1", "series-a", e0.clone())
        .await
        .unwrap();

    let mut replacement = make_event("task-1", 1);
    replacement.data = serde_json::json!({"text": "replaced"});
    ctx.short
        .replace_last_series_event("task-1", "series-a", replacement.clone())
        .await
        .unwrap();

    let events = ctx.short.get_events("task-1", None).await.unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].id, e0.id); // original id preserved
    assert_eq!(events[0].index, 0); // original idx preserved
    assert_eq!(events[0].data["text"], "replaced");

    // series latest should be updated
    let latest = ctx
        .short
        .get_series_latest("task-1", "series-a")
        .await
        .unwrap();
    assert_eq!(latest, Some(replacement));
}

#[tokio::test]
async fn append_when_no_previous_series_event() {
    let ctx = setup().await;
    ctx.short.save_task(make_task("task-1")).await.unwrap();

    let e0 = make_event("task-1", 0);
    ctx.short
        .replace_last_series_event("task-1", "series-a", e0.clone())
        .await
        .unwrap();

    let events = ctx.short.get_events("task-1", None).await.unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0], e0);

    let latest = ctx
        .short
        .get_series_latest("task-1", "series-a")
        .await
        .unwrap();
    assert_eq!(latest, Some(e0));
}

#[tokio::test]
async fn only_replace_correct_series_event_not_others() {
    let ctx = setup().await;
    ctx.short.save_task(make_task("task-1")).await.unwrap();

    let e0 = make_event("task-1", 0); // not part of series
    let mut e1 = make_event("task-1", 1);
    e1.series_id = Some("series-a".to_string());
    e1.series_mode = Some(SeriesMode::Latest);
    let e2 = make_event("task-1", 2); // not part of series

    ctx.short.append_event("task-1", e0.clone()).await.unwrap();
    ctx.short.append_event("task-1", e1.clone()).await.unwrap();
    ctx.short.append_event("task-1", e2.clone()).await.unwrap();
    ctx.short
        .set_series_latest("task-1", "series-a", e1.clone())
        .await
        .unwrap();

    let mut replacement = make_event("task-1", 3);
    replacement.series_id = Some("series-a".to_string());
    replacement.series_mode = Some(SeriesMode::Latest);
    ctx.short
        .replace_last_series_event("task-1", "series-a", replacement.clone())
        .await
        .unwrap();

    let events = ctx.short.get_events("task-1", None).await.unwrap();
    assert_eq!(events.len(), 3);
    assert_eq!(events[0], e0);
    assert_eq!(events[1].id, e1.id); // original id preserved
    assert_eq!(events[1].index, 1); // original idx preserved
    assert_eq!(events[1].r#type, replacement.r#type); // content replaced
    assert_eq!(events[2], e2);
}

// ─── set_ttl ─────────────────────────────────────────────────────────────

#[tokio::test]
async fn set_ttl_does_not_error() {
    let ctx = setup().await;
    ctx.short.save_task(make_task("task-1")).await.unwrap();
    let result = ctx.short.set_ttl("task-1", 60).await;
    assert!(result.is_ok());
}

// ─── event with series fields ───────────────────────────────────────────

#[tokio::test]
async fn preserve_series_id_and_series_mode_on_events() {
    let ctx = setup().await;
    ctx.short.save_task(make_task("task-1")).await.unwrap();
    let mut event = make_event("task-1", 0);
    event.series_id = Some("my-series".to_string());
    event.series_mode = Some(SeriesMode::Accumulate);

    ctx.short
        .append_event("task-1", event.clone())
        .await
        .unwrap();
    let events = ctx.short.get_events("task-1", None).await.unwrap();
    assert_eq!(events[0], event);
    assert_eq!(events[0].series_id, Some("my-series".to_string()));
    assert_eq!(events[0].series_mode, Some(SeriesMode::Accumulate));
}

// ─── edge cases ──────────────────────────────────────────────────────────

#[tokio::test]
async fn since_with_no_cursor_fields_returns_all() {
    let ctx = setup().await;
    ctx.short.save_task(make_task("task-1")).await.unwrap();
    for i in 0..3 {
        ctx.short
            .append_event("task-1", make_event("task-1", i))
            .await
            .unwrap();
    }

    let opts = EventQueryOptions {
        since: Some(SinceCursor {
            index: None,
            timestamp: None,
            id: None,
        }),
        limit: None,
    };
    let events = ctx.short.get_events("task-1", Some(opts)).await.unwrap();
    assert_eq!(events.len(), 3);
}

#[tokio::test]
async fn event_data_with_null_value() {
    let ctx = setup().await;
    ctx.short.save_task(make_task("task-1")).await.unwrap();
    let mut event = make_event("task-1", 0);
    event.data = serde_json::Value::Null;

    ctx.short
        .append_event("task-1", event.clone())
        .await
        .unwrap();
    let events = ctx.short.get_events("task-1", None).await.unwrap();
    assert_eq!(events[0].data, serde_json::Value::Null);
}

// ─── helpers ─────────────────────────────────────────────────────────────

fn make_worker(id: &str) -> Worker {
    Worker {
        id: id.to_string(),
        status: WorkerStatus::Idle,
        match_rule: WorkerMatchRule::default(),
        capacity: 5,
        used_slots: 0,
        weight: 1,
        connection_mode: ConnectionMode::Pull,
        connected_at: 1000.0,
        last_heartbeat_at: 1000.0,
        metadata: None,
    }
}

fn make_assignment(task_id: &str, worker_id: &str) -> WorkerAssignment {
    WorkerAssignment {
        task_id: task_id.to_string(),
        worker_id: worker_id.to_string(),
        cost: 1,
        assigned_at: 2000.0,
        status: WorkerAssignmentStatus::Assigned,
    }
}

// ─── list_tasks ──────────────────────────────────────────────────────────

#[tokio::test]
async fn list_tasks_returns_empty_when_no_tasks() {
    let ctx = setup().await;
    let tasks = ctx
        .short
        .list_tasks(TaskFilter::default())
        .await
        .unwrap();
    assert!(tasks.is_empty());
}

#[tokio::test]
async fn list_tasks_returns_all_saved_tasks() {
    let ctx = setup().await;
    ctx.short.save_task(make_task("task-1")).await.unwrap();
    ctx.short.save_task(make_task("task-2")).await.unwrap();
    ctx.short.save_task(make_task("task-3")).await.unwrap();

    let tasks = ctx
        .short
        .list_tasks(TaskFilter::default())
        .await
        .unwrap();
    assert_eq!(tasks.len(), 3);
}

#[tokio::test]
async fn list_tasks_filters_by_status() {
    let ctx = setup().await;
    ctx.short.save_task(make_task("task-1")).await.unwrap(); // pending

    let mut running_task = make_task("task-2");
    running_task.status = TaskStatus::Running;
    ctx.short.save_task(running_task).await.unwrap();

    let mut completed_task = make_task("task-3");
    completed_task.status = TaskStatus::Completed;
    ctx.short.save_task(completed_task).await.unwrap();

    let filter = TaskFilter {
        status: Some(vec![TaskStatus::Pending]),
        ..Default::default()
    };
    let tasks = ctx.short.list_tasks(filter).await.unwrap();
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0].id, "task-1");

    let filter = TaskFilter {
        status: Some(vec![TaskStatus::Running, TaskStatus::Completed]),
        ..Default::default()
    };
    let tasks = ctx.short.list_tasks(filter).await.unwrap();
    assert_eq!(tasks.len(), 2);
}

#[tokio::test]
async fn list_tasks_filters_by_type() {
    let ctx = setup().await;
    let mut t1 = make_task("task-1");
    t1.r#type = Some("llm".to_string());
    ctx.short.save_task(t1).await.unwrap();

    let mut t2 = make_task("task-2");
    t2.r#type = Some("image".to_string());
    ctx.short.save_task(t2).await.unwrap();

    ctx.short.save_task(make_task("task-3")).await.unwrap(); // no type

    let filter = TaskFilter {
        types: Some(vec!["llm".to_string()]),
        ..Default::default()
    };
    let tasks = ctx.short.list_tasks(filter).await.unwrap();
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0].id, "task-1");
}

#[tokio::test]
async fn list_tasks_filters_by_assign_mode() {
    let ctx = setup().await;
    let mut t1 = make_task("task-1");
    t1.assign_mode = Some(AssignMode::Pull);
    ctx.short.save_task(t1).await.unwrap();

    let mut t2 = make_task("task-2");
    t2.assign_mode = Some(AssignMode::External);
    ctx.short.save_task(t2).await.unwrap();

    ctx.short.save_task(make_task("task-3")).await.unwrap(); // no assign_mode

    let filter = TaskFilter {
        assign_mode: Some(vec![AssignMode::Pull]),
        ..Default::default()
    };
    let tasks = ctx.short.list_tasks(filter).await.unwrap();
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0].id, "task-1");
}

#[tokio::test]
async fn list_tasks_excludes_task_ids() {
    let ctx = setup().await;
    ctx.short.save_task(make_task("task-1")).await.unwrap();
    ctx.short.save_task(make_task("task-2")).await.unwrap();
    ctx.short.save_task(make_task("task-3")).await.unwrap();

    let filter = TaskFilter {
        exclude_task_ids: Some(vec!["task-1".to_string(), "task-3".to_string()]),
        ..Default::default()
    };
    let tasks = ctx.short.list_tasks(filter).await.unwrap();
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0].id, "task-2");
}

#[tokio::test]
async fn list_tasks_respects_limit() {
    let ctx = setup().await;
    for i in 0..10 {
        ctx.short
            .save_task(make_task(&format!("task-{}", i)))
            .await
            .unwrap();
    }

    let filter = TaskFilter {
        limit: Some(3),
        ..Default::default()
    };
    let tasks = ctx.short.list_tasks(filter).await.unwrap();
    assert_eq!(tasks.len(), 3);
}

#[tokio::test]
async fn list_tasks_combines_multiple_filters() {
    let ctx = setup().await;

    let mut t1 = make_task("task-1");
    t1.status = TaskStatus::Running;
    t1.r#type = Some("llm".to_string());
    ctx.short.save_task(t1).await.unwrap();

    let mut t2 = make_task("task-2");
    t2.status = TaskStatus::Running;
    t2.r#type = Some("image".to_string());
    ctx.short.save_task(t2).await.unwrap();

    let mut t3 = make_task("task-3");
    t3.status = TaskStatus::Pending;
    t3.r#type = Some("llm".to_string());
    ctx.short.save_task(t3).await.unwrap();

    let filter = TaskFilter {
        status: Some(vec![TaskStatus::Running]),
        types: Some(vec!["llm".to_string()]),
        ..Default::default()
    };
    let tasks = ctx.short.list_tasks(filter).await.unwrap();
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0].id, "task-1");
}

// ─── save_worker / get_worker ────────────────────────────────────────────

#[tokio::test]
async fn save_and_retrieve_a_worker() {
    let ctx = setup().await;
    let worker = make_worker("worker-1");
    ctx.short.save_worker(worker.clone()).await.unwrap();
    let retrieved = ctx.short.get_worker("worker-1").await.unwrap();
    assert_eq!(retrieved, Some(worker));
}

#[tokio::test]
async fn return_none_for_missing_worker() {
    let ctx = setup().await;
    let result = ctx.short.get_worker("nonexistent").await.unwrap();
    assert_eq!(result, None);
}

#[tokio::test]
async fn upsert_worker_on_conflict() {
    let ctx = setup().await;
    let worker = make_worker("worker-1");
    ctx.short.save_worker(worker.clone()).await.unwrap();

    let mut updated = worker.clone();
    updated.status = WorkerStatus::Busy;
    updated.used_slots = 3;
    updated.last_heartbeat_at = 2000.0;
    ctx.short.save_worker(updated.clone()).await.unwrap();

    let retrieved = ctx.short.get_worker("worker-1").await.unwrap().unwrap();
    assert_eq!(retrieved.status, WorkerStatus::Busy);
    assert_eq!(retrieved.used_slots, 3);
    assert_eq!(retrieved.last_heartbeat_at, 2000.0);
}

#[tokio::test]
async fn worker_preserves_all_fields_on_round_trip() {
    let ctx = setup().await;
    let worker = Worker {
        id: "worker-full".to_string(),
        status: WorkerStatus::Busy,
        match_rule: WorkerMatchRule {
            task_types: Some(vec!["llm".to_string(), "image".to_string()]),
            tags: None,
        },
        capacity: 10,
        used_slots: 3,
        weight: 5,
        connection_mode: ConnectionMode::Websocket,
        connected_at: 1500.0,
        last_heartbeat_at: 1600.0,
        metadata: Some(
            [("region".to_string(), serde_json::json!("us-east"))]
                .into_iter()
                .collect(),
        ),
    };
    ctx.short.save_worker(worker.clone()).await.unwrap();
    let retrieved = ctx.short.get_worker("worker-full").await.unwrap().unwrap();
    assert_eq!(retrieved, worker);
}

#[tokio::test]
async fn worker_with_no_metadata_round_trips() {
    let ctx = setup().await;
    let worker = make_worker("worker-minimal");
    ctx.short.save_worker(worker.clone()).await.unwrap();
    let retrieved = ctx.short.get_worker("worker-minimal").await.unwrap().unwrap();
    assert!(retrieved.metadata.is_none());
    assert_eq!(retrieved, worker);
}

// ─── list_workers ────────────────────────────────────────────────────────

#[tokio::test]
async fn list_workers_returns_empty_when_none() {
    let ctx = setup().await;
    let workers = ctx.short.list_workers(None).await.unwrap();
    assert!(workers.is_empty());
}

#[tokio::test]
async fn list_workers_returns_all_workers() {
    let ctx = setup().await;
    ctx.short.save_worker(make_worker("w-1")).await.unwrap();
    ctx.short.save_worker(make_worker("w-2")).await.unwrap();
    ctx.short.save_worker(make_worker("w-3")).await.unwrap();

    let workers = ctx.short.list_workers(None).await.unwrap();
    assert_eq!(workers.len(), 3);
}

#[tokio::test]
async fn list_workers_filters_by_status() {
    let ctx = setup().await;
    ctx.short.save_worker(make_worker("w-idle")).await.unwrap(); // idle

    let mut busy = make_worker("w-busy");
    busy.status = WorkerStatus::Busy;
    ctx.short.save_worker(busy).await.unwrap();

    let mut draining = make_worker("w-drain");
    draining.status = WorkerStatus::Draining;
    ctx.short.save_worker(draining).await.unwrap();

    let filter = WorkerFilter {
        status: Some(vec![WorkerStatus::Idle]),
        connection_mode: None,
    };
    let workers = ctx.short.list_workers(Some(filter)).await.unwrap();
    assert_eq!(workers.len(), 1);
    assert_eq!(workers[0].id, "w-idle");

    let filter = WorkerFilter {
        status: Some(vec![WorkerStatus::Busy, WorkerStatus::Draining]),
        connection_mode: None,
    };
    let workers = ctx.short.list_workers(Some(filter)).await.unwrap();
    assert_eq!(workers.len(), 2);
}

#[tokio::test]
async fn list_workers_filters_by_connection_mode() {
    let ctx = setup().await;
    ctx.short.save_worker(make_worker("w-pull")).await.unwrap(); // ConnectionMode::Pull

    let mut ws_worker = make_worker("w-ws");
    ws_worker.connection_mode = ConnectionMode::Websocket;
    ctx.short.save_worker(ws_worker).await.unwrap();

    let filter = WorkerFilter {
        status: None,
        connection_mode: Some(vec![ConnectionMode::Websocket]),
    };
    let workers = ctx.short.list_workers(Some(filter)).await.unwrap();
    assert_eq!(workers.len(), 1);
    assert_eq!(workers[0].id, "w-ws");
}

#[tokio::test]
async fn list_workers_combines_filters() {
    let ctx = setup().await;

    // idle + pull
    ctx.short.save_worker(make_worker("w-1")).await.unwrap();

    // busy + websocket
    let mut w2 = make_worker("w-2");
    w2.status = WorkerStatus::Busy;
    w2.connection_mode = ConnectionMode::Websocket;
    ctx.short.save_worker(w2).await.unwrap();

    // idle + websocket
    let mut w3 = make_worker("w-3");
    w3.connection_mode = ConnectionMode::Websocket;
    ctx.short.save_worker(w3).await.unwrap();

    let filter = WorkerFilter {
        status: Some(vec![WorkerStatus::Idle]),
        connection_mode: Some(vec![ConnectionMode::Websocket]),
    };
    let workers = ctx.short.list_workers(Some(filter)).await.unwrap();
    assert_eq!(workers.len(), 1);
    assert_eq!(workers[0].id, "w-3");
}

// ─── delete_worker ───────────────────────────────────────────────────────

#[tokio::test]
async fn delete_worker_removes_it() {
    let ctx = setup().await;
    ctx.short.save_worker(make_worker("w-1")).await.unwrap();
    ctx.short.save_worker(make_worker("w-2")).await.unwrap();

    ctx.short.delete_worker("w-1").await.unwrap();

    let result = ctx.short.get_worker("w-1").await.unwrap();
    assert_eq!(result, None);

    // Other worker still exists
    let result = ctx.short.get_worker("w-2").await.unwrap();
    assert!(result.is_some());
}

#[tokio::test]
async fn delete_nonexistent_worker_does_not_error() {
    let ctx = setup().await;
    let result = ctx.short.delete_worker("nonexistent").await;
    assert!(result.is_ok());
}

// ─── claim_task ──────────────────────────────────────────────────────────

#[tokio::test]
async fn claim_task_succeeds_for_pending_task_with_capacity() {
    let ctx = setup().await;
    ctx.short.save_task(make_task("task-1")).await.unwrap(); // pending
    ctx.short.save_worker(make_worker("w-1")).await.unwrap(); // capacity=5, used=0

    let claimed = ctx.short.claim_task("task-1", "w-1", 1).await.unwrap();
    assert!(claimed);

    // Verify task is now assigned
    let task = ctx.short.get_task("task-1").await.unwrap().unwrap();
    assert_eq!(task.status, TaskStatus::Assigned);
    assert_eq!(task.assigned_worker, Some("w-1".to_string()));
    assert_eq!(task.cost, Some(1));

    // Verify worker used_slots incremented
    let worker = ctx.short.get_worker("w-1").await.unwrap().unwrap();
    assert_eq!(worker.used_slots, 1);
}

#[tokio::test]
async fn claim_task_fails_when_worker_at_capacity() {
    let ctx = setup().await;
    ctx.short.save_task(make_task("task-1")).await.unwrap();

    let mut worker = make_worker("w-1");
    worker.capacity = 2;
    worker.used_slots = 2;
    ctx.short.save_worker(worker).await.unwrap();

    let claimed = ctx.short.claim_task("task-1", "w-1", 1).await.unwrap();
    assert!(!claimed);

    // Task should remain pending
    let task = ctx.short.get_task("task-1").await.unwrap().unwrap();
    assert_eq!(task.status, TaskStatus::Pending);
}

#[tokio::test]
async fn claim_task_fails_when_cost_exceeds_remaining_capacity() {
    let ctx = setup().await;
    ctx.short.save_task(make_task("task-1")).await.unwrap();

    let mut worker = make_worker("w-1");
    worker.capacity = 5;
    worker.used_slots = 3;
    ctx.short.save_worker(worker).await.unwrap();

    // cost=3, but only 2 slots remaining
    let claimed = ctx.short.claim_task("task-1", "w-1", 3).await.unwrap();
    assert!(!claimed);
}

#[tokio::test]
async fn claim_task_fails_for_nonexistent_worker() {
    let ctx = setup().await;
    ctx.short.save_task(make_task("task-1")).await.unwrap();

    let claimed = ctx.short.claim_task("task-1", "no-worker", 1).await.unwrap();
    assert!(!claimed);
}

#[tokio::test]
async fn claim_task_fails_for_nonexistent_task() {
    let ctx = setup().await;
    ctx.short.save_worker(make_worker("w-1")).await.unwrap();

    let claimed = ctx.short.claim_task("no-task", "w-1", 1).await.unwrap();
    assert!(!claimed);
}

#[tokio::test]
async fn claim_task_fails_for_running_task() {
    let ctx = setup().await;
    let mut task = make_task("task-1");
    task.status = TaskStatus::Running;
    ctx.short.save_task(task).await.unwrap();
    ctx.short.save_worker(make_worker("w-1")).await.unwrap();

    let claimed = ctx.short.claim_task("task-1", "w-1", 1).await.unwrap();
    assert!(!claimed);
}

#[tokio::test]
async fn claim_task_fails_for_completed_task() {
    let ctx = setup().await;
    let mut task = make_task("task-1");
    task.status = TaskStatus::Completed;
    ctx.short.save_task(task).await.unwrap();
    ctx.short.save_worker(make_worker("w-1")).await.unwrap();

    let claimed = ctx.short.claim_task("task-1", "w-1", 1).await.unwrap();
    assert!(!claimed);
}

#[tokio::test]
async fn claim_task_succeeds_for_assigned_task() {
    let ctx = setup().await;
    let mut task = make_task("task-1");
    task.status = TaskStatus::Assigned;
    ctx.short.save_task(task).await.unwrap();
    ctx.short.save_worker(make_worker("w-1")).await.unwrap();

    let claimed = ctx.short.claim_task("task-1", "w-1", 1).await.unwrap();
    assert!(claimed);
}

#[tokio::test]
async fn claim_task_increments_used_slots_by_cost() {
    let ctx = setup().await;
    ctx.short.save_task(make_task("task-1")).await.unwrap();

    let mut worker = make_worker("w-1");
    worker.capacity = 10;
    worker.used_slots = 2;
    ctx.short.save_worker(worker).await.unwrap();

    let claimed = ctx.short.claim_task("task-1", "w-1", 3).await.unwrap();
    assert!(claimed);

    let worker = ctx.short.get_worker("w-1").await.unwrap().unwrap();
    assert_eq!(worker.used_slots, 5); // 2 + 3
}

// ─── add_assignment / get_task_assignment / get_worker_assignments ───────

#[tokio::test]
async fn add_and_retrieve_assignment_by_task() {
    let ctx = setup().await;
    let assignment = make_assignment("task-1", "w-1");
    ctx.short.add_assignment(assignment.clone()).await.unwrap();

    let retrieved = ctx.short.get_task_assignment("task-1").await.unwrap();
    assert_eq!(retrieved, Some(assignment));
}

#[tokio::test]
async fn get_task_assignment_returns_none_when_missing() {
    let ctx = setup().await;
    let result = ctx.short.get_task_assignment("nonexistent").await.unwrap();
    assert_eq!(result, None);
}

#[tokio::test]
async fn get_worker_assignments_returns_all_for_worker() {
    let ctx = setup().await;
    let a1 = make_assignment("task-1", "w-1");
    let a2 = WorkerAssignment {
        task_id: "task-2".to_string(),
        worker_id: "w-1".to_string(),
        cost: 2,
        assigned_at: 3000.0,
        status: WorkerAssignmentStatus::Running,
    };
    let a3 = make_assignment("task-3", "w-2"); // different worker

    ctx.short.add_assignment(a1.clone()).await.unwrap();
    ctx.short.add_assignment(a2.clone()).await.unwrap();
    ctx.short.add_assignment(a3.clone()).await.unwrap();

    let assignments = ctx.short.get_worker_assignments("w-1").await.unwrap();
    assert_eq!(assignments.len(), 2);
    let task_ids: Vec<&str> = assignments.iter().map(|a| a.task_id.as_str()).collect();
    assert!(task_ids.contains(&"task-1"));
    assert!(task_ids.contains(&"task-2"));
}

#[tokio::test]
async fn get_worker_assignments_returns_empty_when_none() {
    let ctx = setup().await;
    let assignments = ctx.short.get_worker_assignments("w-1").await.unwrap();
    assert!(assignments.is_empty());
}

#[tokio::test]
async fn add_assignment_upserts_on_same_task_id() {
    let ctx = setup().await;
    let a1 = make_assignment("task-1", "w-1");
    ctx.short.add_assignment(a1).await.unwrap();

    // Reassign task-1 to w-2
    let a2 = WorkerAssignment {
        task_id: "task-1".to_string(),
        worker_id: "w-2".to_string(),
        cost: 3,
        assigned_at: 4000.0,
        status: WorkerAssignmentStatus::Offered,
    };
    ctx.short.add_assignment(a2.clone()).await.unwrap();

    let retrieved = ctx.short.get_task_assignment("task-1").await.unwrap().unwrap();
    assert_eq!(retrieved.worker_id, "w-2");
    assert_eq!(retrieved.cost, 3);
    assert_eq!(retrieved.status, WorkerAssignmentStatus::Offered);
}

// ─── remove_assignment ───────────────────────────────────────────────────

#[tokio::test]
async fn remove_assignment_deletes_by_task_id() {
    let ctx = setup().await;
    ctx.short
        .add_assignment(make_assignment("task-1", "w-1"))
        .await
        .unwrap();
    ctx.short
        .add_assignment(make_assignment("task-2", "w-1"))
        .await
        .unwrap();

    ctx.short.remove_assignment("task-1").await.unwrap();

    let removed = ctx.short.get_task_assignment("task-1").await.unwrap();
    assert_eq!(removed, None);

    // Other assignment still exists
    let still_there = ctx.short.get_task_assignment("task-2").await.unwrap();
    assert!(still_there.is_some());
}

#[tokio::test]
async fn remove_nonexistent_assignment_does_not_error() {
    let ctx = setup().await;
    let result = ctx.short.remove_assignment("nonexistent").await;
    assert!(result.is_ok());
}

#[tokio::test]
async fn remove_assignment_reflected_in_worker_assignments() {
    let ctx = setup().await;
    ctx.short
        .add_assignment(make_assignment("task-1", "w-1"))
        .await
        .unwrap();
    ctx.short
        .add_assignment(make_assignment("task-2", "w-1"))
        .await
        .unwrap();

    ctx.short.remove_assignment("task-1").await.unwrap();

    let assignments = ctx.short.get_worker_assignments("w-1").await.unwrap();
    assert_eq!(assignments.len(), 1);
    assert_eq!(assignments[0].task_id, "task-2");
}

// ─── assignment round-trip preserves all fields ──────────────────────────

#[tokio::test]
async fn assignment_preserves_all_fields_on_round_trip() {
    let ctx = setup().await;
    let assignment = WorkerAssignment {
        task_id: "task-99".to_string(),
        worker_id: "w-42".to_string(),
        cost: 7,
        assigned_at: 9999.0,
        status: WorkerAssignmentStatus::Offered,
    };
    ctx.short.add_assignment(assignment.clone()).await.unwrap();

    let retrieved = ctx
        .short
        .get_task_assignment("task-99")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(retrieved, assignment);
}

#[tokio::test]
async fn assignment_running_status_round_trips() {
    let ctx = setup().await;
    let assignment = WorkerAssignment {
        task_id: "task-1".to_string(),
        worker_id: "w-1".to_string(),
        cost: 1,
        assigned_at: 1000.0,
        status: WorkerAssignmentStatus::Running,
    };
    ctx.short.add_assignment(assignment.clone()).await.unwrap();

    let retrieved = ctx
        .short
        .get_task_assignment("task-1")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(retrieved.status, WorkerAssignmentStatus::Running);
}

// ─── disconnect_policy round-trip (covers disconnect_policy_to_string) ───

#[tokio::test]
async fn task_with_disconnect_policy_reassign_round_trips() {
    let ctx = setup().await;
    let mut task = make_task("dp-reassign");
    task.disconnect_policy = Some(DisconnectPolicy::Reassign);
    ctx.short.save_task(task.clone()).await.unwrap();

    let retrieved = ctx.short.get_task("dp-reassign").await.unwrap().unwrap();
    assert_eq!(retrieved.disconnect_policy, Some(DisconnectPolicy::Reassign));
}

#[tokio::test]
async fn task_with_disconnect_policy_mark_round_trips() {
    let ctx = setup().await;
    let mut task = make_task("dp-mark");
    task.disconnect_policy = Some(DisconnectPolicy::Mark);
    ctx.short.save_task(task.clone()).await.unwrap();

    let retrieved = ctx.short.get_task("dp-mark").await.unwrap().unwrap();
    assert_eq!(retrieved.disconnect_policy, Some(DisconnectPolicy::Mark));
}

#[tokio::test]
async fn task_with_disconnect_policy_fail_round_trips() {
    let ctx = setup().await;
    let mut task = make_task("dp-fail");
    task.disconnect_policy = Some(DisconnectPolicy::Fail);
    ctx.short.save_task(task.clone()).await.unwrap();

    let retrieved = ctx.short.get_task("dp-fail").await.unwrap().unwrap();
    assert_eq!(retrieved.disconnect_policy, Some(DisconnectPolicy::Fail));
}

// ─── task cost round-trip ────────────────────────────────────────────────

#[tokio::test]
async fn task_with_cost_round_trips() {
    let ctx = setup().await;
    let mut task = make_task("task-cost");
    task.cost = Some(42);
    ctx.short.save_task(task.clone()).await.unwrap();

    let retrieved = ctx.short.get_task("task-cost").await.unwrap().unwrap();
    assert_eq!(retrieved.cost, Some(42));
}

#[tokio::test]
async fn task_with_zero_cost_round_trips() {
    let ctx = setup().await;
    let mut task = make_task("task-cost-zero");
    task.cost = Some(0);
    ctx.short.save_task(task.clone()).await.unwrap();

    let retrieved = ctx.short.get_task("task-cost-zero").await.unwrap().unwrap();
    assert_eq!(retrieved.cost, Some(0));
}

// ─── list_tasks tag matching ─────────────────────────────────────────────

#[tokio::test]
async fn list_tasks_filters_by_tags_all() {
    let ctx = setup().await;

    let mut t1 = make_task("task-t1");
    t1.tags = Some(vec!["gpu".to_string(), "large".to_string()]);
    ctx.short.save_task(t1).await.unwrap();

    let mut t2 = make_task("task-t2");
    t2.tags = Some(vec!["cpu".to_string(), "small".to_string()]);
    ctx.short.save_task(t2).await.unwrap();

    let mut t3 = make_task("task-t3");
    t3.tags = Some(vec!["gpu".to_string(), "small".to_string()]);
    ctx.short.save_task(t3).await.unwrap();

    // no tags
    ctx.short.save_task(make_task("task-t4")).await.unwrap();

    // Filter: all tags must include "gpu"
    let filter = TaskFilter {
        tags: Some(TagMatcher {
            all: Some(vec!["gpu".to_string()]),
            any: None,
            none: None,
        }),
        ..Default::default()
    };
    let tasks = ctx.short.list_tasks(filter).await.unwrap();
    assert_eq!(tasks.len(), 2);
    let ids: Vec<&str> = tasks.iter().map(|t| t.id.as_str()).collect();
    assert!(ids.contains(&"task-t1"));
    assert!(ids.contains(&"task-t3"));
}

#[tokio::test]
async fn list_tasks_filters_by_tags_any() {
    let ctx = setup().await;

    let mut t1 = make_task("task-a1");
    t1.tags = Some(vec!["gpu".to_string(), "large".to_string()]);
    ctx.short.save_task(t1).await.unwrap();

    let mut t2 = make_task("task-a2");
    t2.tags = Some(vec!["cpu".to_string()]);
    ctx.short.save_task(t2).await.unwrap();

    let mut t3 = make_task("task-a3");
    t3.tags = Some(vec!["tpu".to_string()]);
    ctx.short.save_task(t3).await.unwrap();

    // Filter: any of ["gpu", "cpu"]
    let filter = TaskFilter {
        tags: Some(TagMatcher {
            all: None,
            any: Some(vec!["gpu".to_string(), "cpu".to_string()]),
            none: None,
        }),
        ..Default::default()
    };
    let tasks = ctx.short.list_tasks(filter).await.unwrap();
    assert_eq!(tasks.len(), 2);
    let ids: Vec<&str> = tasks.iter().map(|t| t.id.as_str()).collect();
    assert!(ids.contains(&"task-a1"));
    assert!(ids.contains(&"task-a2"));
}

#[tokio::test]
async fn list_tasks_filters_by_tags_none() {
    let ctx = setup().await;

    let mut t1 = make_task("task-n1");
    t1.tags = Some(vec!["gpu".to_string()]);
    ctx.short.save_task(t1).await.unwrap();

    let mut t2 = make_task("task-n2");
    t2.tags = Some(vec!["cpu".to_string()]);
    ctx.short.save_task(t2).await.unwrap();

    ctx.short.save_task(make_task("task-n3")).await.unwrap(); // no tags

    // Filter: none of ["gpu"] — excludes task-n1
    let filter = TaskFilter {
        tags: Some(TagMatcher {
            all: None,
            any: None,
            none: Some(vec!["gpu".to_string()]),
        }),
        ..Default::default()
    };
    let tasks = ctx.short.list_tasks(filter).await.unwrap();
    assert_eq!(tasks.len(), 2);
    let ids: Vec<&str> = tasks.iter().map(|t| t.id.as_str()).collect();
    assert!(ids.contains(&"task-n2"));
    assert!(ids.contains(&"task-n3"));
}
