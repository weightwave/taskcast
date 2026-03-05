mod helpers;

use helpers::{make_event, make_task, setup};
use taskcast_core::types::{EventQueryOptions, SeriesMode, ShortTermStore, SinceCursor, TaskStatus};

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
