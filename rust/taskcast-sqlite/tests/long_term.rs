mod helpers;

use helpers::{make_event, make_task, make_worker_event, setup};
use taskcast_core::types::{
    EventQueryOptions, LongTermStore, SinceCursor, TaskStatus, WorkerAuditAction,
};

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

// ─── save_worker_event / get_worker_events ──────────────────────────────────

#[tokio::test]
async fn save_and_retrieve_worker_events() {
    let ctx = setup().await;

    let e0 = make_worker_event("w1", 0, WorkerAuditAction::Connected);
    let e1 = make_worker_event("w1", 1, WorkerAuditAction::TaskAssigned);
    let e2 = make_worker_event("w1", 2, WorkerAuditAction::Disconnected);

    ctx.long.save_worker_event(e0.clone()).await.unwrap();
    ctx.long.save_worker_event(e1.clone()).await.unwrap();
    ctx.long.save_worker_event(e2.clone()).await.unwrap();

    let events = ctx.long.get_worker_events("w1", None).await.unwrap();
    assert_eq!(events, vec![e0, e1, e2]);
}

#[tokio::test]
async fn return_empty_vec_when_no_worker_events() {
    let ctx = setup().await;
    let events = ctx.long.get_worker_events("w1", None).await.unwrap();
    assert!(events.is_empty());
}

#[tokio::test]
async fn worker_events_ordered_by_timestamp() {
    let ctx = setup().await;

    // Insert out of order — seq 2 first, then 0, then 1
    let e0 = make_worker_event("w1", 0, WorkerAuditAction::Connected);
    let e1 = make_worker_event("w1", 1, WorkerAuditAction::Updated);
    let e2 = make_worker_event("w1", 2, WorkerAuditAction::Disconnected);

    ctx.long.save_worker_event(e2.clone()).await.unwrap();
    ctx.long.save_worker_event(e0.clone()).await.unwrap();
    ctx.long.save_worker_event(e1.clone()).await.unwrap();

    let events = ctx.long.get_worker_events("w1", None).await.unwrap();
    assert_eq!(events, vec![e0, e1, e2]);
}

#[tokio::test]
async fn filter_worker_events_by_since_timestamp() {
    let ctx = setup().await;
    for i in 0..5 {
        ctx.long
            .save_worker_event(make_worker_event("w1", i, WorkerAuditAction::Updated))
            .await
            .unwrap();
    }

    // timestamps: 1000, 1100, 1200, 1300, 1400
    // since timestamp 1200 => events with timestamp > 1200
    let opts = EventQueryOptions {
        since: Some(SinceCursor {
            index: None,
            timestamp: Some(1200.0),
            id: None,
        }),
        limit: None,
    };
    let events = ctx.long.get_worker_events("w1", Some(opts)).await.unwrap();
    assert_eq!(events.len(), 2);
    assert_eq!(events[0].timestamp, 1300.0);
    assert_eq!(events[1].timestamp, 1400.0);
}

#[tokio::test]
async fn filter_worker_events_by_since_id() {
    let ctx = setup().await;
    for i in 0..5 {
        ctx.long
            .save_worker_event(make_worker_event("w1", i, WorkerAuditAction::Connected))
            .await
            .unwrap();
    }

    // since id "wevt-w1-2" (timestamp 1200) => events with timestamp > 1200
    let opts = EventQueryOptions {
        since: Some(SinceCursor {
            index: None,
            timestamp: None,
            id: Some("wevt-w1-2".to_string()),
        }),
        limit: None,
    };
    let events = ctx.long.get_worker_events("w1", Some(opts)).await.unwrap();
    assert_eq!(events.len(), 2);
    assert_eq!(events[0].id, "wevt-w1-3");
    assert_eq!(events[1].id, "wevt-w1-4");
}

#[tokio::test]
async fn return_all_worker_events_when_since_id_not_found() {
    let ctx = setup().await;
    for i in 0..3 {
        ctx.long
            .save_worker_event(make_worker_event("w1", i, WorkerAuditAction::Connected))
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
    let events = ctx.long.get_worker_events("w1", Some(opts)).await.unwrap();
    assert_eq!(events.len(), 3);
}

#[tokio::test]
async fn respect_limit_on_worker_events() {
    let ctx = setup().await;
    for i in 0..10 {
        ctx.long
            .save_worker_event(make_worker_event("w1", i, WorkerAuditAction::Updated))
            .await
            .unwrap();
    }

    let opts = EventQueryOptions {
        since: None,
        limit: Some(3),
    };
    let events = ctx.long.get_worker_events("w1", Some(opts)).await.unwrap();
    assert_eq!(events.len(), 3);
    assert_eq!(events[0].timestamp, 1000.0);
    assert_eq!(events[2].timestamp, 1200.0);
}

#[tokio::test]
async fn apply_limit_after_since_filter_on_worker_events() {
    let ctx = setup().await;
    for i in 0..10 {
        ctx.long
            .save_worker_event(make_worker_event("w1", i, WorkerAuditAction::Updated))
            .await
            .unwrap();
    }

    // timestamps: 1000..1900; since timestamp 1500 => 1600,1700,1800,1900; limit 2 => 1600,1700
    let opts = EventQueryOptions {
        since: Some(SinceCursor {
            index: None,
            timestamp: Some(1500.0),
            id: None,
        }),
        limit: Some(2),
    };
    let events = ctx.long.get_worker_events("w1", Some(opts)).await.unwrap();
    assert_eq!(events.len(), 2);
    assert_eq!(events[0].timestamp, 1600.0);
    assert_eq!(events[1].timestamp, 1700.0);
}

#[tokio::test]
async fn save_worker_event_idempotent_on_duplicate() {
    let ctx = setup().await;

    let event = make_worker_event("w1", 0, WorkerAuditAction::Connected);
    ctx.long.save_worker_event(event.clone()).await.unwrap();
    // Saving the same event again should not error
    ctx.long.save_worker_event(event.clone()).await.unwrap();

    let events = ctx.long.get_worker_events("w1", None).await.unwrap();
    assert_eq!(events.len(), 1);
}

#[tokio::test]
async fn save_worker_event_with_data() {
    let ctx = setup().await;

    let mut event = make_worker_event("w1", 0, WorkerAuditAction::TaskAssigned);
    event.data = Some(
        [
            ("task_id".to_string(), serde_json::json!("task-42")),
            ("reason".to_string(), serde_json::json!("capacity")),
        ]
        .into_iter()
        .collect(),
    );

    ctx.long.save_worker_event(event.clone()).await.unwrap();
    let events = ctx.long.get_worker_events("w1", None).await.unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0], event);
}

#[tokio::test]
async fn save_worker_event_without_data() {
    let ctx = setup().await;

    let event = make_worker_event("w1", 0, WorkerAuditAction::Disconnected);
    assert!(event.data.is_none());

    ctx.long.save_worker_event(event.clone()).await.unwrap();
    let events = ctx.long.get_worker_events("w1", None).await.unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].data, None);
}

#[tokio::test]
async fn worker_events_isolated_per_worker() {
    let ctx = setup().await;

    ctx.long
        .save_worker_event(make_worker_event("w1", 0, WorkerAuditAction::Connected))
        .await
        .unwrap();
    ctx.long
        .save_worker_event(make_worker_event("w1", 1, WorkerAuditAction::Updated))
        .await
        .unwrap();
    ctx.long
        .save_worker_event(make_worker_event("w2", 0, WorkerAuditAction::Connected))
        .await
        .unwrap();

    let w1_events = ctx.long.get_worker_events("w1", None).await.unwrap();
    let w2_events = ctx.long.get_worker_events("w2", None).await.unwrap();

    assert_eq!(w1_events.len(), 2);
    assert_eq!(w2_events.len(), 1);
    assert_eq!(w1_events[0].worker_id, "w1");
    assert_eq!(w2_events[0].worker_id, "w2");
}

#[tokio::test]
async fn worker_events_various_actions_round_trip() {
    let ctx = setup().await;

    let actions = vec![
        WorkerAuditAction::Connected,
        WorkerAuditAction::Disconnected,
        WorkerAuditAction::Updated,
        WorkerAuditAction::TaskAssigned,
        WorkerAuditAction::TaskDeclined,
        WorkerAuditAction::TaskReclaimed,
        WorkerAuditAction::Draining,
        WorkerAuditAction::HeartbeatTimeout,
        WorkerAuditAction::PullRequest,
    ];

    for (i, action) in actions.iter().enumerate() {
        let event = make_worker_event("w1", i as u64, action.clone());
        ctx.long.save_worker_event(event).await.unwrap();
    }

    let events = ctx.long.get_worker_events("w1", None).await.unwrap();
    assert_eq!(events.len(), actions.len());
    for (event, expected_action) in events.iter().zip(actions.iter()) {
        assert_eq!(&event.action, expected_action);
    }
}

#[tokio::test]
async fn worker_events_since_with_empty_cursor_returns_all() {
    let ctx = setup().await;
    for i in 0..3 {
        ctx.long
            .save_worker_event(make_worker_event("w1", i, WorkerAuditAction::Connected))
            .await
            .unwrap();
    }

    // since exists but has no usable cursor fields
    let opts = EventQueryOptions {
        since: Some(SinceCursor {
            index: None,
            timestamp: None,
            id: None,
        }),
        limit: None,
    };
    let events = ctx.long.get_worker_events("w1", Some(opts)).await.unwrap();
    assert_eq!(events.len(), 3);
}

#[tokio::test]
async fn worker_events_since_id_takes_priority_over_timestamp() {
    let ctx = setup().await;
    for i in 0..5 {
        ctx.long
            .save_worker_event(make_worker_event("w1", i, WorkerAuditAction::Updated))
            .await
            .unwrap();
    }

    // Provide both id and timestamp. The id (wevt-w1-3, timestamp=1300) should take priority.
    // timestamp=1000 alone would return 4 events; using id should return only 1 event after wevt-w1-3.
    let opts = EventQueryOptions {
        since: Some(SinceCursor {
            index: None,
            timestamp: Some(1000.0),
            id: Some("wevt-w1-3".to_string()),
        }),
        limit: None,
    };
    let events = ctx.long.get_worker_events("w1", Some(opts)).await.unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].id, "wevt-w1-4");
}
