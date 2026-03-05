mod helpers;

use helpers::{make_event, make_task, setup};
use taskcast_core::types::{EventQueryOptions, LongTermStore, SinceCursor, TaskStatus};

// ─── save_task / get_task ─────────────────────────────────────────────────

#[tokio::test]
async fn save_and_retrieve_a_task() {
    let ctx = setup().await;
    let task = make_task("task-1");
    ctx.long.save_task(task.clone()).await.unwrap();
    let retrieved = ctx.long.get_task("task-1").await.unwrap();
    assert_eq!(retrieved, Some(task));
}

#[tokio::test]
async fn return_none_for_missing_task() {
    let ctx = setup().await;
    let result = ctx.long.get_task("nonexistent").await.unwrap();
    assert_eq!(result, None);
}

#[tokio::test]
async fn upsert_task_on_conflict() {
    let ctx = setup().await;
    let task = make_task("task-1");
    ctx.long.save_task(task.clone()).await.unwrap();

    let mut updated = task.clone();
    updated.status = TaskStatus::Running;
    updated.updated_at = 2000.0;
    ctx.long.save_task(updated.clone()).await.unwrap();

    let retrieved = ctx.long.get_task("task-1").await.unwrap().unwrap();
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

    ctx.long.save_task(task.clone()).await.unwrap();
    let retrieved = ctx.long.get_task("task-1").await.unwrap().unwrap();
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
    ctx.long.save_task(task.clone()).await.unwrap();
    let retrieved = ctx.long.get_task("minimal").await.unwrap().unwrap();
    assert_eq!(retrieved, task);
}

// ─── save_event / get_events ──────────────────────────────────────────────

#[tokio::test]
async fn save_and_retrieve_events() {
    let ctx = setup().await;
    ctx.long.save_task(make_task("task-1")).await.unwrap();

    let e0 = make_event("task-1", 0);
    let e1 = make_event("task-1", 1);
    let e2 = make_event("task-1", 2);

    ctx.long.save_event(e0.clone()).await.unwrap();
    ctx.long.save_event(e1.clone()).await.unwrap();
    ctx.long.save_event(e2.clone()).await.unwrap();

    let events = ctx.long.get_events("task-1", None).await.unwrap();
    assert_eq!(events, vec![e0, e1, e2]);
}

#[tokio::test]
async fn return_empty_vec_when_no_events() {
    let ctx = setup().await;
    ctx.long.save_task(make_task("task-1")).await.unwrap();
    let events = ctx.long.get_events("task-1", None).await.unwrap();
    assert!(events.is_empty());
}

#[tokio::test]
async fn filter_events_by_since_index() {
    let ctx = setup().await;
    ctx.long.save_task(make_task("task-1")).await.unwrap();
    for i in 0..5 {
        ctx.long.save_event(make_event("task-1", i)).await.unwrap();
    }

    let opts = EventQueryOptions {
        since: Some(SinceCursor {
            index: Some(2),
            timestamp: None,
            id: None,
        }),
        limit: None,
    };
    let events = ctx.long.get_events("task-1", Some(opts)).await.unwrap();
    assert_eq!(events.len(), 2);
    assert_eq!(events[0].index, 3);
    assert_eq!(events[1].index, 4);
}

#[tokio::test]
async fn filter_events_by_since_timestamp() {
    let ctx = setup().await;
    ctx.long.save_task(make_task("task-1")).await.unwrap();
    for i in 0..5 {
        ctx.long.save_event(make_event("task-1", i)).await.unwrap();
    }

    let opts = EventQueryOptions {
        since: Some(SinceCursor {
            index: None,
            timestamp: Some(1200.0),
            id: None,
        }),
        limit: None,
    };
    let events = ctx.long.get_events("task-1", Some(opts)).await.unwrap();
    assert_eq!(events.len(), 2);
    assert_eq!(events[0].timestamp, 1300.0);
    assert_eq!(events[1].timestamp, 1400.0);
}

#[tokio::test]
async fn filter_events_by_since_id() {
    let ctx = setup().await;
    ctx.long.save_task(make_task("task-1")).await.unwrap();
    for i in 0..5 {
        ctx.long.save_event(make_event("task-1", i)).await.unwrap();
    }

    let opts = EventQueryOptions {
        since: Some(SinceCursor {
            index: None,
            timestamp: None,
            id: Some("evt-task-1-2".to_string()),
        }),
        limit: None,
    };
    let events = ctx.long.get_events("task-1", Some(opts)).await.unwrap();
    assert_eq!(events.len(), 2);
    assert_eq!(events[0].id, "evt-task-1-3");
    assert_eq!(events[1].id, "evt-task-1-4");
}

#[tokio::test]
async fn return_all_events_when_since_id_not_found() {
    let ctx = setup().await;
    ctx.long.save_task(make_task("task-1")).await.unwrap();
    for i in 0..3 {
        ctx.long.save_event(make_event("task-1", i)).await.unwrap();
    }

    let opts = EventQueryOptions {
        since: Some(SinceCursor {
            index: None,
            timestamp: None,
            id: Some("nonexistent-id".to_string()),
        }),
        limit: None,
    };
    let events = ctx.long.get_events("task-1", Some(opts)).await.unwrap();
    assert_eq!(events.len(), 3);
}

#[tokio::test]
async fn respect_limit_parameter() {
    let ctx = setup().await;
    ctx.long.save_task(make_task("task-1")).await.unwrap();
    for i in 0..10 {
        ctx.long.save_event(make_event("task-1", i)).await.unwrap();
    }

    let opts = EventQueryOptions {
        since: None,
        limit: Some(3),
    };
    let events = ctx.long.get_events("task-1", Some(opts)).await.unwrap();
    assert_eq!(events.len(), 3);
    assert_eq!(events[0].index, 0);
    assert_eq!(events[2].index, 2);
}

#[tokio::test]
async fn apply_limit_after_since_filter() {
    let ctx = setup().await;
    ctx.long.save_task(make_task("task-1")).await.unwrap();
    for i in 0..10 {
        ctx.long.save_event(make_event("task-1", i)).await.unwrap();
    }

    let opts = EventQueryOptions {
        since: Some(SinceCursor {
            index: Some(5),
            timestamp: None,
            id: None,
        }),
        limit: Some(2),
    };
    let events = ctx.long.get_events("task-1", Some(opts)).await.unwrap();
    assert_eq!(events.len(), 2);
    assert_eq!(events[0].index, 6);
    assert_eq!(events[1].index, 7);
}

#[tokio::test]
async fn save_event_on_conflict_do_nothing() {
    let ctx = setup().await;
    ctx.long.save_task(make_task("task-1")).await.unwrap();

    let event = make_event("task-1", 0);
    ctx.long.save_event(event.clone()).await.unwrap();
    // Saving the same event again should not error
    ctx.long.save_event(event.clone()).await.unwrap();

    let events = ctx.long.get_events("task-1", None).await.unwrap();
    assert_eq!(events.len(), 1);
}

#[tokio::test]
async fn preserve_series_fields_on_events() {
    let ctx = setup().await;
    ctx.long.save_task(make_task("task-1")).await.unwrap();
    let mut event = make_event("task-1", 0);
    event.series_id = Some("my-series".to_string());
    event.series_mode = Some(taskcast_core::types::SeriesMode::Accumulate);

    ctx.long.save_event(event.clone()).await.unwrap();
    let events = ctx.long.get_events("task-1", None).await.unwrap();
    assert_eq!(events[0], event);
}
