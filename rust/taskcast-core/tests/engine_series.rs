use std::sync::Arc;

use serde_json::json;
use taskcast_core::{
    CreateTaskInput, Level, MemoryBroadcastProvider, MemoryShortTermStore, PublishEventInput,
    SeriesMode, TaskEngine, TaskEngineOptions, TaskEvent, TaskStatus,
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

// ─── seriesMode: latest ───────────────────────────────────────────────────

#[tokio::test]
async fn latest_keeps_only_last_event_after_multiple_publishes() {
    let engine = make_engine();
    create_running_task(&engine, "t1").await;

    for i in 1..=5 {
        engine
            .publish_event(
                "t1",
                PublishEventInput {
                    r#type: "agent.message_update".to_string(),
                    level: Level::Info,
                    data: json!({ "content": format!("v{i}") }),
                    series_id: Some("msg".to_string()),
                    series_mode: Some(SeriesMode::Latest),
                    series_acc_field: None,
                },
            )
            .await
            .unwrap();
    }

    let events = engine.get_events("t1", None).await.unwrap();
    let series: Vec<_> = filter_user_events(&events)
        .into_iter()
        .filter(|e| e.series_id.as_deref() == Some("msg"))
        .collect();

    assert_eq!(series.len(), 1);
    assert_eq!(series[0].data, json!({ "content": "v5" }));
}

#[tokio::test]
async fn latest_first_event_stored_exactly_once() {
    let engine = make_engine();
    create_running_task(&engine, "t1").await;

    engine
        .publish_event(
            "t1",
            PublishEventInput {
                r#type: "status".to_string(),
                level: Level::Info,
                data: json!({ "text": "only one" }),
                series_id: Some("s1".to_string()),
                series_mode: Some(SeriesMode::Latest),
                series_acc_field: None,
            },
        )
        .await
        .unwrap();

    let events = engine.get_events("t1", None).await.unwrap();
    let series: Vec<_> = filter_user_events(&events)
        .into_iter()
        .filter(|e| e.series_id.as_deref() == Some("s1"))
        .collect();

    assert_eq!(series.len(), 1);
    assert_eq!(series[0].data, json!({ "text": "only one" }));
}

#[tokio::test]
async fn latest_multiple_independent_series_each_deduplicated() {
    let engine = make_engine();
    create_running_task(&engine, "t1").await;

    for i in 1..=3 {
        engine
            .publish_event(
                "t1",
                PublishEventInput {
                    r#type: "update".to_string(),
                    level: Level::Info,
                    data: json!({ "v": i }),
                    series_id: Some("seriesA".to_string()),
                    series_mode: Some(SeriesMode::Latest),
                    series_acc_field: None,
                },
            )
            .await
            .unwrap();
        engine
            .publish_event(
                "t1",
                PublishEventInput {
                    r#type: "update".to_string(),
                    level: Level::Info,
                    data: json!({ "v": i * 10 }),
                    series_id: Some("seriesB".to_string()),
                    series_mode: Some(SeriesMode::Latest),
                    series_acc_field: None,
                },
            )
            .await
            .unwrap();
    }

    let events = engine.get_events("t1", None).await.unwrap();
    let user = filter_user_events(&events);
    let a: Vec<_> = user
        .iter()
        .filter(|e| e.series_id.as_deref() == Some("seriesA"))
        .collect();
    let b: Vec<_> = user
        .iter()
        .filter(|e| e.series_id.as_deref() == Some("seriesB"))
        .collect();

    assert_eq!(a.len(), 1);
    assert_eq!(a[0].data, json!({ "v": 3 }));
    assert_eq!(b.len(), 1);
    assert_eq!(b[0].data, json!({ "v": 30 }));
}

#[tokio::test]
async fn latest_get_series_latest_returns_the_latest_value() {
    let engine = make_engine();
    create_running_task(&engine, "t1").await;

    for i in 1..=3 {
        engine
            .publish_event(
                "t1",
                PublishEventInput {
                    r#type: "update".to_string(),
                    level: Level::Info,
                    data: json!({ "v": i }),
                    series_id: Some("s1".to_string()),
                    series_mode: Some(SeriesMode::Latest),
                    series_acc_field: None,
                },
            )
            .await
            .unwrap();
    }

    let latest = engine.get_series_latest("t1", "s1").await.unwrap();
    assert!(latest.is_some());
    assert_eq!(latest.unwrap().data, json!({ "v": 3 }));
}

#[tokio::test]
async fn latest_indices_are_unique_after_replacements() {
    let engine = make_engine();
    create_running_task(&engine, "t1").await;

    for i in 1..=5 {
        engine
            .publish_event(
                "t1",
                PublishEventInput {
                    r#type: "update".to_string(),
                    level: Level::Info,
                    data: json!({ "v": i }),
                    series_id: Some("s1".to_string()),
                    series_mode: Some(SeriesMode::Latest),
                    series_acc_field: None,
                },
            )
            .await
            .unwrap();
    }

    engine
        .publish_event(
            "t1",
            PublishEventInput {
                r#type: "plain".to_string(),
                level: Level::Info,
                data: json!({ "x": 1 }),
                series_id: None,
                series_mode: None,
                series_acc_field: None,
            },
        )
        .await
        .unwrap();

    let events = engine.get_events("t1", None).await.unwrap();
    let user = filter_user_events(&events);
    let indices: Vec<u64> = user.iter().map(|e| e.index).collect();
    let unique: std::collections::HashSet<u64> = indices.iter().cloned().collect();
    assert_eq!(unique.len(), indices.len());
}

#[tokio::test]
async fn latest_interleaved_with_non_series_preserves_all_non_series() {
    let engine = make_engine();
    create_running_task(&engine, "t1").await;

    // Interleave: plain → latest → plain → latest → plain
    engine
        .publish_event("t1", PublishEventInput {
            r#type: "log".to_string(), level: Level::Info,
            data: json!({ "n": 1 }),
            series_id: None, series_mode: None, series_acc_field: None,
        })
        .await.unwrap();
    engine
        .publish_event("t1", PublishEventInput {
            r#type: "update".to_string(), level: Level::Info,
            data: json!({ "v": 1 }),
            series_id: Some("s1".to_string()),
            series_mode: Some(SeriesMode::Latest), series_acc_field: None,
        })
        .await.unwrap();
    engine
        .publish_event("t1", PublishEventInput {
            r#type: "log".to_string(), level: Level::Info,
            data: json!({ "n": 2 }),
            series_id: None, series_mode: None, series_acc_field: None,
        })
        .await.unwrap();
    engine
        .publish_event("t1", PublishEventInput {
            r#type: "update".to_string(), level: Level::Info,
            data: json!({ "v": 2 }),
            series_id: Some("s1".to_string()),
            series_mode: Some(SeriesMode::Latest), series_acc_field: None,
        })
        .await.unwrap();
    engine
        .publish_event("t1", PublishEventInput {
            r#type: "log".to_string(), level: Level::Info,
            data: json!({ "n": 3 }),
            series_id: None, series_mode: None, series_acc_field: None,
        })
        .await.unwrap();

    let events = engine.get_events("t1", None).await.unwrap();
    let user = filter_user_events(&events);
    let plain: Vec<_> = user.iter().filter(|e| e.r#type == "log").collect();
    let latest: Vec<_> = user.iter().filter(|e| e.series_id.as_deref() == Some("s1")).collect();

    assert_eq!(plain.len(), 3);
    assert_eq!(plain[0].data["n"], 1);
    assert_eq!(plain[1].data["n"], 2);
    assert_eq!(plain[2].data["n"], 3);
    assert_eq!(latest.len(), 1);
    assert_eq!(latest[0].data, json!({ "v": 2 }));
}

// ─── seriesMode: keep-all ─────────────────────────────────────────────────

#[tokio::test]
async fn keep_all_retains_every_event_in_history() {
    let engine = make_engine();
    create_running_task(&engine, "t1").await;

    for i in 1..=5 {
        engine
            .publish_event(
                "t1",
                PublishEventInput {
                    r#type: "log".to_string(),
                    level: Level::Info,
                    data: json!({ "line": i }),
                    series_id: Some("logs".to_string()),
                    series_mode: Some(SeriesMode::KeepAll),
                    series_acc_field: None,
                },
            )
            .await
            .unwrap();
    }

    let events = engine.get_events("t1", None).await.unwrap();
    let series: Vec<_> = filter_user_events(&events)
        .into_iter()
        .filter(|e| e.series_id.as_deref() == Some("logs"))
        .collect();

    assert_eq!(series.len(), 5);
}

// ─── seriesMode: accumulate ───────────────────────────────────────────────

#[tokio::test]
async fn accumulate_stores_all_deltas_in_history() {
    let engine = make_engine();
    create_running_task(&engine, "t1").await;

    engine
        .publish_event(
            "t1",
            PublishEventInput {
                r#type: "stream".to_string(),
                level: Level::Info,
                data: json!({ "delta": "Hello" }),
                series_id: Some("output".to_string()),
                series_mode: Some(SeriesMode::Accumulate),
                series_acc_field: None,
            },
        )
        .await
        .unwrap();
    engine
        .publish_event(
            "t1",
            PublishEventInput {
                r#type: "stream".to_string(),
                level: Level::Info,
                data: json!({ "delta": " world" }),
                series_id: Some("output".to_string()),
                series_mode: Some(SeriesMode::Accumulate),
                series_acc_field: None,
            },
        )
        .await
        .unwrap();

    let events = engine.get_events("t1", None).await.unwrap();
    let series: Vec<_> = filter_user_events(&events)
        .into_iter()
        .filter(|e| e.series_id.as_deref() == Some("output"))
        .collect();

    assert_eq!(series.len(), 2);
    assert_eq!(series[0].data["delta"], "Hello");
    assert_eq!(series[1].data["delta"], " world");
}

#[tokio::test]
async fn accumulate_get_series_latest_returns_accumulated_value() {
    let engine = make_engine();
    create_running_task(&engine, "t1").await;

    engine
        .publish_event(
            "t1",
            PublishEventInput {
                r#type: "stream".to_string(),
                level: Level::Info,
                data: json!({ "delta": "Hello" }),
                series_id: Some("output".to_string()),
                series_mode: Some(SeriesMode::Accumulate),
                series_acc_field: None,
            },
        )
        .await
        .unwrap();
    engine
        .publish_event(
            "t1",
            PublishEventInput {
                r#type: "stream".to_string(),
                level: Level::Info,
                data: json!({ "delta": " world" }),
                series_id: Some("output".to_string()),
                series_mode: Some(SeriesMode::Accumulate),
                series_acc_field: None,
            },
        )
        .await
        .unwrap();

    let latest = engine.get_series_latest("t1", "output").await.unwrap();
    assert!(latest.is_some());
    assert_eq!(latest.unwrap().data["delta"], "Hello world");
}

// ─── mixed series modes ───────────────────────────────────────────────────

#[tokio::test]
async fn mixed_series_modes_coexist_correctly() {
    let engine = make_engine();
    create_running_task(&engine, "t1").await;

    // latest
    for i in 1..=3 {
        engine
            .publish_event(
                "t1",
                PublishEventInput {
                    r#type: "status".to_string(),
                    level: Level::Info,
                    data: json!({ "status": format!("v{i}") }),
                    series_id: Some("status".to_string()),
                    series_mode: Some(SeriesMode::Latest),
                    series_acc_field: None,
                },
            )
            .await
            .unwrap();
    }

    // keep-all
    for i in 1..=3 {
        engine
            .publish_event(
                "t1",
                PublishEventInput {
                    r#type: "log".to_string(),
                    level: Level::Info,
                    data: json!({ "line": i }),
                    series_id: Some("logs".to_string()),
                    series_mode: Some(SeriesMode::KeepAll),
                    series_acc_field: None,
                },
            )
            .await
            .unwrap();
    }

    // accumulate
    engine
        .publish_event(
            "t1",
            PublishEventInput {
                r#type: "stream".to_string(),
                level: Level::Info,
                data: json!({ "delta": "a" }),
                series_id: Some("output".to_string()),
                series_mode: Some(SeriesMode::Accumulate),
                series_acc_field: None,
            },
        )
        .await
        .unwrap();
    engine
        .publish_event(
            "t1",
            PublishEventInput {
                r#type: "stream".to_string(),
                level: Level::Info,
                data: json!({ "delta": "b" }),
                series_id: Some("output".to_string()),
                series_mode: Some(SeriesMode::Accumulate),
                series_acc_field: None,
            },
        )
        .await
        .unwrap();

    // plain (no series)
    engine
        .publish_event(
            "t1",
            PublishEventInput {
                r#type: "plain".to_string(),
                level: Level::Info,
                data: json!({ "misc": true }),
                series_id: None,
                series_mode: None,
                series_acc_field: None,
            },
        )
        .await
        .unwrap();

    let events = engine.get_events("t1", None).await.unwrap();
    let user = filter_user_events(&events);

    let status_count = user
        .iter()
        .filter(|e| e.series_id.as_deref() == Some("status"))
        .count();
    let logs_count = user
        .iter()
        .filter(|e| e.series_id.as_deref() == Some("logs"))
        .count();
    let output_count = user
        .iter()
        .filter(|e| e.series_id.as_deref() == Some("output"))
        .count();
    let plain_count = user.iter().filter(|e| e.series_id.is_none()).count();

    assert_eq!(status_count, 1); // latest: only last
    assert_eq!(logs_count, 3); // keep-all: all retained
    assert_eq!(output_count, 2); // accumulate: all deltas
    assert_eq!(plain_count, 1); // no series
}

#[tokio::test]
async fn interleaved_series_modes_produce_correct_counts_and_unique_indices() {
    let engine = make_engine();
    create_running_task(&engine, "t1").await;

    for i in 1..=3i64 {
        engine
            .publish_event("t1", PublishEventInput {
                r#type: "status".to_string(), level: Level::Info,
                data: json!({ "v": i }),
                series_id: Some("status".to_string()),
                series_mode: Some(SeriesMode::Latest), series_acc_field: None,
            })
            .await.unwrap();
        engine
            .publish_event("t1", PublishEventInput {
                r#type: "log".to_string(), level: Level::Info,
                data: json!({ "line": i }),
                series_id: Some("logs".to_string()),
                series_mode: Some(SeriesMode::KeepAll), series_acc_field: None,
            })
            .await.unwrap();
        engine
            .publish_event("t1", PublishEventInput {
                r#type: "stream".to_string(), level: Level::Info,
                data: json!({ "delta": format!("{}", (b'a' + i as u8 - 1) as char) }),
                series_id: Some("output".to_string()),
                series_mode: Some(SeriesMode::Accumulate), series_acc_field: None,
            })
            .await.unwrap();
        if i <= 2 {
            engine
                .publish_event("t1", PublishEventInput {
                    r#type: "plain".to_string(), level: Level::Info,
                    data: json!({ "n": i }),
                    series_id: None, series_mode: None, series_acc_field: None,
                })
                .await.unwrap();
        }
    }

    let events = engine.get_events("t1", None).await.unwrap();
    let user = filter_user_events(&events);

    let latest_count = user.iter().filter(|e| e.series_id.as_deref() == Some("status")).count();
    let keep_all_count = user.iter().filter(|e| e.series_id.as_deref() == Some("logs")).count();
    let acc_count = user.iter().filter(|e| e.series_id.as_deref() == Some("output")).count();
    let plain_count = user.iter().filter(|e| e.series_id.is_none()).count();

    // latest: 1, keep-all: 3, accumulate: 3, plain: 2 → total 9
    assert_eq!(latest_count, 1);
    assert_eq!(keep_all_count, 3);
    assert_eq!(acc_count, 3);
    assert_eq!(plain_count, 2);
    assert_eq!(user.len(), 9);

    // All indices unique
    let indices: Vec<u64> = user.iter().map(|e| e.index).collect();
    let unique: std::collections::HashSet<u64> = indices.iter().cloned().collect();
    assert_eq!(unique.len(), indices.len());
}
