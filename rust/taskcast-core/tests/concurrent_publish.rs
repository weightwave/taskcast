//! Regression test for concurrent event publishing ordering.
//!
//! When multiple events are published to the same task concurrently, they
//! must be stored in the same order as their atomically-assigned indices.
//! Without per-task serialization in `emit`, async scheduling can cause
//! `append_event` calls to interleave, producing storage order that differs
//! from index order.

use std::sync::Arc;

use serde_json::json;
use taskcast_core::{
    CreateTaskInput, Level, MemoryBroadcastProvider, MemoryShortTermStore, PublishEventInput,
    TaskEngine, TaskEngineOptions, TaskEvent, TaskStatus,
};

fn make_engine() -> TaskEngine {
    TaskEngine::new(TaskEngineOptions {
        short_term_store: Arc::new(MemoryShortTermStore::new()),
        broadcast: Arc::new(MemoryBroadcastProvider::new()),
        long_term_store: None,
        hooks: None,
    })
}

async fn create_running_task(engine: &TaskEngine, task_id: &str) {
    engine
        .create_task(CreateTaskInput {
            id: Some(task_id.to_string()),
            ..Default::default()
        })
        .await
        .unwrap();
    engine
        .transition_task(task_id, TaskStatus::Running, None)
        .await
        .unwrap();
}

fn filter_user_events(events: &[TaskEvent]) -> Vec<&TaskEvent> {
    events
        .iter()
        .filter(|e| !e.r#type.starts_with("taskcast:"))
        .collect()
}

/// Concurrent publishes to the same task must produce events whose storage
/// order matches their index order. This is the regression test for the
/// race condition where `turn_end` could be stored before `message_end`
/// when both are published nearly simultaneously.
#[tokio::test]
async fn concurrent_publishes_preserve_index_order() {
    let engine = Arc::new(make_engine());
    create_running_task(&engine, "t1").await;

    let n = 20;
    let mut handles = Vec::new();

    for i in 0..n {
        let engine = Arc::clone(&engine);
        handles.push(tokio::spawn(async move {
            engine
                .publish_event(
                    "t1",
                    PublishEventInput {
                        r#type: format!("event.{i}"),
                        level: Level::Info,
                        data: json!({ "order": i }),
                        series_id: None,
                        series_mode: None,
                        series_acc_field: None,
                    },
                )
                .await
                .unwrap()
        }));
    }

    for h in handles {
        h.await.unwrap();
    }

    let events = engine.get_events("t1", None).await.unwrap();
    let user_events = filter_user_events(&events);
    assert_eq!(user_events.len(), n);

    // Verify events are stored in ascending index order
    for window in user_events.windows(2) {
        assert!(
            window[0].index < window[1].index,
            "Events out of order: index {} should come before index {}, \
             but got types [{}, {}]",
            window[0].index,
            window[1].index,
            window[0].r#type,
            window[1].r#type,
        );
    }
}

/// Same test but with interleaved latest-series and non-series events.
/// Simulates the real-world scenario: message_update (latest) events
/// interleaved with message_end / turn_end (non-series) events.
#[tokio::test]
async fn concurrent_latest_and_plain_events_preserve_relative_order() {
    let engine = Arc::new(make_engine());
    create_running_task(&engine, "t1").await;

    // Simulate: several message_updates (latest), then message_end, then turn_end
    // All published concurrently to stress the ordering.
    let engine1 = Arc::clone(&engine);
    let engine2 = Arc::clone(&engine);
    let engine3 = Arc::clone(&engine);

    let h1 = tokio::spawn(async move {
        for i in 0..5 {
            engine1
                .publish_event(
                    "t1",
                    PublishEventInput {
                        r#type: "message_update".to_string(),
                        level: Level::Info,
                        data: json!({ "content": format!("v{i}") }),
                        series_id: Some("msg_content".to_string()),
                        series_mode: Some(taskcast_core::SeriesMode::Latest),
                        series_acc_field: None,
                    },
                )
                .await
                .unwrap();
        }
    });

    let h2 = tokio::spawn(async move {
        engine2
            .publish_event(
                "t1",
                PublishEventInput {
                    r#type: "message_end".to_string(),
                    level: Level::Info,
                    data: json!({ "done": true }),
                    series_id: None,
                    series_mode: None,
                    series_acc_field: None,
                },
            )
            .await
            .unwrap()
    });

    let h3 = tokio::spawn(async move {
        engine3
            .publish_event(
                "t1",
                PublishEventInput {
                    r#type: "turn_end".to_string(),
                    level: Level::Info,
                    data: json!({ "turn": 0 }),
                    series_id: None,
                    series_mode: None,
                    series_acc_field: None,
                },
            )
            .await
            .unwrap()
    });

    h1.await.unwrap();
    h2.await.unwrap();
    h3.await.unwrap();

    let events = engine.get_events("t1", None).await.unwrap();
    let user_events = filter_user_events(&events);

    // With the per-task lock, all events are stored in index order.
    // Verify monotonically increasing indices.
    for window in user_events.windows(2) {
        assert!(
            window[0].index < window[1].index,
            "Events out of order: index {} (type={}) should come before index {} (type={})",
            window[0].index,
            window[0].r#type,
            window[1].index,
            window[1].r#type,
        );
    }
}

/// Publishing to DIFFERENT tasks concurrently should not interfere.
/// The per-task lock must not serialize across different tasks.
#[tokio::test]
async fn concurrent_publishes_to_different_tasks_are_independent() {
    let engine = Arc::new(make_engine());
    create_running_task(&engine, "t1").await;
    create_running_task(&engine, "t2").await;

    let mut handles = Vec::new();
    for i in 0..10 {
        let engine = Arc::clone(&engine);
        let task_id = if i % 2 == 0 { "t1" } else { "t2" };
        handles.push(tokio::spawn(async move {
            engine
                .publish_event(
                    task_id,
                    PublishEventInput {
                        r#type: format!("event.{i}"),
                        level: Level::Info,
                        data: json!({ "i": i }),
                        series_id: None,
                        series_mode: None,
                        series_acc_field: None,
                    },
                )
                .await
                .unwrap()
        }));
    }

    for h in handles {
        h.await.unwrap();
    }

    // Both tasks should have their events in index order
    for tid in &["t1", "t2"] {
        let events = engine.get_events(tid, None).await.unwrap();
        let user_events = filter_user_events(&events);
        for window in user_events.windows(2) {
            assert!(
                window[0].index < window[1].index,
                "Task {tid}: events out of order: index {} before index {}",
                window[0].index,
                window[1].index,
            );
        }
    }
}
