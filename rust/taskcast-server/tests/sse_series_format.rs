//! Integration tests for SSE series format handling.
//!
//! Tests the seriesFormat query parameter, late-join snapshot collapse for
//! accumulate series, and accumulated data swap in the SSE stream.

use std::sync::Arc;

use serde_json::json;
use taskcast_core::{
    BroadcastProvider, Level, MemoryBroadcastProvider, MemoryShortTermStore, ShortTermStore,
    TaskEngine, TaskEngineOptions, TaskStatus,
};
use taskcast_server::{create_app, AuthMode, CorsConfig};

// ─── Test Helpers ────────────────────────────────────────────────────────────

fn make_app() -> (Arc<TaskEngine>, axum::Router) {
    let short_term_store = Arc::new(MemoryShortTermStore::new());
    let broadcast = Arc::new(MemoryBroadcastProvider::new());
    let engine = Arc::new(TaskEngine::new(TaskEngineOptions {
        short_term_store: Arc::clone(&short_term_store) as Arc<dyn ShortTermStore>,
        broadcast: Arc::clone(&broadcast) as Arc<dyn BroadcastProvider>,
        long_term_store: None,
        hooks: None,
    }));
    let (router, _ws_registry) = create_app(
        Arc::clone(&engine),
        AuthMode::None,
        None,
        None,
        CorsConfig::default(),
    );
    (engine, router)
}

async fn serve_app(app: axum::Router) -> std::net::SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    addr
}

async fn create_running_task(engine: &TaskEngine, task_id: &str) {
    engine
        .create_task(taskcast_core::engine::CreateTaskInput {
            id: Some(task_id.to_string()),
            r#type: Some("test".to_string()),
            ..Default::default()
        })
        .await
        .expect("create_task failed");
    engine
        .transition_task(task_id, TaskStatus::Running, None)
        .await
        .expect("transition to running failed");
}

async fn publish_accumulate_event(
    engine: &TaskEngine,
    task_id: &str,
    series_id: &str,
    text: &str,
) {
    engine
        .publish_event(
            task_id,
            taskcast_core::PublishEventInput {
                r#type: "llm.chunk".to_string(),
                level: Level::Info,
                data: json!({"text": text}),
                series_id: Some(series_id.to_string()),
                series_mode: Some(taskcast_core::SeriesMode::Accumulate),
                series_acc_field: Some("text".to_string()),
            },
        )
        .await
        .expect("publish_event failed");
}

/// Parse SSE text into a list of (event_type, data_json) pairs.
fn parse_sse_events(text: &str) -> Vec<(String, serde_json::Value)> {
    let mut results = Vec::new();
    let mut current_event_type = String::new();
    for line in text.lines() {
        if let Some(ev) = line.strip_prefix("event: ") {
            current_event_type = ev.to_string();
        } else if let Some(data) = line.strip_prefix("data: ") {
            if let Ok(val) = serde_json::from_str::<serde_json::Value>(data) {
                results.push((current_event_type.clone(), val));
            }
        }
    }
    results
}

// =============================================================================
// 1. Late-join snapshot collapse: accumulate series collapsed to single event
// =============================================================================

#[tokio::test]
async fn late_join_collapses_accumulate_series_to_snapshot() {
    let (engine, app) = make_app();
    let addr = serve_app(app).await;
    let client = reqwest::Client::new();

    create_running_task(&engine, "sf-collapse-1").await;

    // Publish 3 accumulate events in the same series
    publish_accumulate_event(&engine, "sf-collapse-1", "output", "Hello ").await;
    publish_accumulate_event(&engine, "sf-collapse-1", "output", "world ").await;
    publish_accumulate_event(&engine, "sf-collapse-1", "output", "!").await;

    // Complete the task so SSE stream closes after replay
    engine
        .transition_task("sf-collapse-1", TaskStatus::Completed, None)
        .await
        .unwrap();

    // Connect SSE without since cursor — should get snapshot collapse
    let response = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        client
            .get(format!("http://{addr}/tasks/sf-collapse-1/events"))
            .send(),
    )
    .await
    .expect("SSE connect timed out")
    .unwrap();
    assert_eq!(response.status(), 200);

    let text = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        response.text(),
    )
    .await
    .expect("SSE stream timed out")
    .unwrap();

    let events = parse_sse_events(&text);

    // Filter to only llm.chunk events (from the accumulate series)
    let chunk_events: Vec<_> = events
        .iter()
        .filter(|(t, v)| {
            t == "taskcast.event"
                && v.get("type").and_then(|t| t.as_str()) == Some("llm.chunk")
        })
        .collect();

    // Should be collapsed to exactly 1 snapshot event (not 3 deltas)
    assert_eq!(
        chunk_events.len(),
        1,
        "Should have exactly 1 collapsed snapshot event, got {}. Events:\n{text}",
        chunk_events.len()
    );

    // The snapshot should have seriesSnapshot: true
    let snapshot = &chunk_events[0].1;
    assert_eq!(
        snapshot.get("seriesSnapshot").and_then(|v| v.as_bool()),
        Some(true),
        "Snapshot event should have seriesSnapshot: true. Got:\n{}",
        serde_json::to_string_pretty(snapshot).unwrap()
    );

    // The snapshot should have accumulated text
    let data = &snapshot["data"];
    let text_val = data["text"].as_str().unwrap_or("");
    assert_eq!(
        text_val, "Hello world !",
        "Snapshot data should contain accumulated text. Got: {text_val}"
    );
}

// =============================================================================
// 2. With since cursor, no collapse — deltas are sent individually
// =============================================================================

#[tokio::test]
async fn since_cursor_skips_snapshot_collapse() {
    let (engine, app) = make_app();
    let addr = serve_app(app).await;
    let client = reqwest::Client::new();

    create_running_task(&engine, "sf-since-2").await;

    // Publish 3 accumulate events
    publish_accumulate_event(&engine, "sf-since-2", "output", "A").await;
    publish_accumulate_event(&engine, "sf-since-2", "output", "B").await;
    publish_accumulate_event(&engine, "sf-since-2", "output", "C").await;

    engine
        .transition_task("sf-since-2", TaskStatus::Completed, None)
        .await
        .unwrap();

    // Connect with since.index=0 — should NOT collapse
    let response = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        client
            .get(format!(
                "http://{addr}/tasks/sf-since-2/events?since.index=0"
            ))
            .send(),
    )
    .await
    .expect("SSE connect timed out")
    .unwrap();
    assert_eq!(response.status(), 200);

    let text = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        response.text(),
    )
    .await
    .expect("SSE stream timed out")
    .unwrap();

    let events = parse_sse_events(&text);

    // Filter to only llm.chunk events
    let chunk_events: Vec<_> = events
        .iter()
        .filter(|(t, v)| {
            t == "taskcast.event"
                && v.get("type").and_then(|t| t.as_str()) == Some("llm.chunk")
        })
        .collect();

    // With since cursor, no seriesSnapshot should be present
    for (_, ev) in &chunk_events {
        assert!(
            ev.get("seriesSnapshot").is_none()
                || ev.get("seriesSnapshot") == Some(&json!(null)),
            "With since cursor, events should NOT have seriesSnapshot. Got:\n{}",
            serde_json::to_string_pretty(ev).unwrap()
        );
    }
}

// =============================================================================
// 3. Mixed accumulate + non-accumulate events: only accumulate collapsed
// =============================================================================

#[tokio::test]
async fn mixed_events_only_accumulate_collapsed() {
    let (engine, app) = make_app();
    let addr = serve_app(app).await;
    let client = reqwest::Client::new();

    create_running_task(&engine, "sf-mixed-3").await;

    // Publish a regular (non-series) event
    engine
        .publish_event(
            "sf-mixed-3",
            taskcast_core::PublishEventInput {
                r#type: "progress".to_string(),
                level: Level::Info,
                data: json!({"pct": 10}),
                series_id: None,
                series_mode: None,
                series_acc_field: None,
            },
        )
        .await
        .unwrap();

    // Publish accumulate series events
    publish_accumulate_event(&engine, "sf-mixed-3", "output", "Hello ").await;
    publish_accumulate_event(&engine, "sf-mixed-3", "output", "world").await;

    // Another regular event
    engine
        .publish_event(
            "sf-mixed-3",
            taskcast_core::PublishEventInput {
                r#type: "progress".to_string(),
                level: Level::Info,
                data: json!({"pct": 100}),
                series_id: None,
                series_mode: None,
                series_acc_field: None,
            },
        )
        .await
        .unwrap();

    engine
        .transition_task("sf-mixed-3", TaskStatus::Completed, None)
        .await
        .unwrap();

    let response = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        client
            .get(format!("http://{addr}/tasks/sf-mixed-3/events"))
            .send(),
    )
    .await
    .expect("SSE connect timed out")
    .unwrap();
    assert_eq!(response.status(), 200);

    let text = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        response.text(),
    )
    .await
    .expect("SSE stream timed out")
    .unwrap();

    let events = parse_sse_events(&text);
    let taskcast_events: Vec<_> = events
        .iter()
        .filter(|(t, _)| t == "taskcast.event")
        .collect();

    // Count event types
    let progress_count = taskcast_events
        .iter()
        .filter(|(_, v)| v.get("type").and_then(|t| t.as_str()) == Some("progress"))
        .count();
    let chunk_count = taskcast_events
        .iter()
        .filter(|(_, v)| v.get("type").and_then(|t| t.as_str()) == Some("llm.chunk"))
        .count();

    // Regular progress events should NOT be collapsed
    assert_eq!(
        progress_count, 2,
        "Should have 2 progress events (not collapsed). Got {progress_count}. Full:\n{text}"
    );

    // Accumulate series should be collapsed to 1 snapshot
    assert_eq!(
        chunk_count, 1,
        "Should have 1 collapsed accumulate snapshot. Got {chunk_count}. Full:\n{text}"
    );
}

// =============================================================================
// 4. seriesFormat=delta query param is accepted (default behavior)
// =============================================================================

#[tokio::test]
async fn series_format_delta_param_accepted() {
    let (engine, app) = make_app();
    let addr = serve_app(app).await;
    let client = reqwest::Client::new();

    create_running_task(&engine, "sf-delta-4").await;
    publish_accumulate_event(&engine, "sf-delta-4", "out", "x").await;

    engine
        .transition_task("sf-delta-4", TaskStatus::Completed, None)
        .await
        .unwrap();

    // Connect with explicit seriesFormat=delta
    let response = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        client
            .get(format!(
                "http://{addr}/tasks/sf-delta-4/events?seriesFormat=delta"
            ))
            .send(),
    )
    .await
    .expect("SSE connect timed out")
    .unwrap();
    assert_eq!(response.status(), 200);

    let text = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        response.text(),
    )
    .await
    .expect("SSE stream timed out")
    .unwrap();

    // Should succeed and contain events
    assert!(
        text.contains("event: taskcast.event"),
        "Should have events. Got:\n{text}"
    );
    assert!(
        text.contains("event: taskcast.done"),
        "Should have done event. Got:\n{text}"
    );
}

// =============================================================================
// 5. seriesFormat=accumulated query param swaps data with accumulated
// =============================================================================

#[tokio::test]
async fn series_format_accumulated_swaps_data_on_live_events() {
    let (engine, app) = make_app();
    let addr = serve_app(app).await;
    let client = reqwest::Client::new();

    create_running_task(&engine, "sf-acc-5").await;

    let engine_clone = Arc::clone(&engine);
    tokio::spawn(async move {
        // Wait for SSE to connect
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;

        // Publish accumulate events — the engine attaches _accumulated_data to broadcast
        publish_accumulate_event(&engine_clone, "sf-acc-5", "out", "Hello ").await;
        publish_accumulate_event(&engine_clone, "sf-acc-5", "out", "world").await;

        // Complete
        engine_clone
            .transition_task("sf-acc-5", TaskStatus::Completed, None)
            .await
            .unwrap();
    });

    // Connect with seriesFormat=accumulated
    let response = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        client
            .get(format!(
                "http://{addr}/tasks/sf-acc-5/events?seriesFormat=accumulated"
            ))
            .send(),
    )
    .await
    .expect("SSE connect timed out")
    .unwrap();
    assert_eq!(response.status(), 200);

    let text = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        response.text(),
    )
    .await
    .expect("SSE stream timed out")
    .unwrap();

    let events = parse_sse_events(&text);

    // Find the chunk events received live (these go through the accumulated swap logic)
    let chunk_events: Vec<_> = events
        .iter()
        .filter(|(t, v)| {
            t == "taskcast.event"
                && v.get("type").and_then(|t| t.as_str()) == Some("llm.chunk")
        })
        .collect();

    assert!(
        !chunk_events.is_empty(),
        "Should have chunk events. Got:\n{text}"
    );

    // The last chunk event should contain accumulated data
    let last_chunk = &chunk_events.last().unwrap().1;
    let data_text = last_chunk["data"]["text"].as_str().unwrap_or("");
    assert!(
        data_text.contains("Hello ") && data_text.contains("world"),
        "Last accumulated event should have full accumulated text. Got: {data_text}"
    );
}

// =============================================================================
// 6. Invalid seriesFormat value is ignored (falls back to delta behavior)
// =============================================================================

#[tokio::test]
async fn series_format_invalid_falls_back_to_delta() {
    let (engine, app) = make_app();
    let addr = serve_app(app).await;
    let client = reqwest::Client::new();

    create_running_task(&engine, "sf-invalid-6").await;
    publish_accumulate_event(&engine, "sf-invalid-6", "out", "test").await;

    engine
        .transition_task("sf-invalid-6", TaskStatus::Completed, None)
        .await
        .unwrap();

    // Connect with invalid seriesFormat
    let response = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        client
            .get(format!(
                "http://{addr}/tasks/sf-invalid-6/events?seriesFormat=bogus"
            ))
            .send(),
    )
    .await
    .expect("SSE connect timed out")
    .unwrap();
    // Should succeed — invalid value is silently ignored
    assert_eq!(response.status(), 200);

    let text = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        response.text(),
    )
    .await
    .expect("SSE stream timed out")
    .unwrap();

    assert!(
        text.contains("event: taskcast.event"),
        "Should have events even with invalid seriesFormat. Got:\n{text}"
    );
    assert!(
        text.contains("event: taskcast.done"),
        "Should have done event. Got:\n{text}"
    );
}

// =============================================================================
// 7. Snapshot collapse with multiple series: each collapsed independently
// =============================================================================

#[tokio::test]
async fn snapshot_collapse_multiple_series_independent() {
    let (engine, app) = make_app();
    let addr = serve_app(app).await;
    let client = reqwest::Client::new();

    create_running_task(&engine, "sf-multi-7").await;

    // Series A: 3 events
    publish_accumulate_event(&engine, "sf-multi-7", "seriesA", "A1 ").await;
    publish_accumulate_event(&engine, "sf-multi-7", "seriesA", "A2 ").await;
    publish_accumulate_event(&engine, "sf-multi-7", "seriesA", "A3").await;

    // Series B: 2 events
    publish_accumulate_event(&engine, "sf-multi-7", "seriesB", "B1 ").await;
    publish_accumulate_event(&engine, "sf-multi-7", "seriesB", "B2").await;

    engine
        .transition_task("sf-multi-7", TaskStatus::Completed, None)
        .await
        .unwrap();

    let response = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        client
            .get(format!("http://{addr}/tasks/sf-multi-7/events"))
            .send(),
    )
    .await
    .expect("SSE connect timed out")
    .unwrap();
    assert_eq!(response.status(), 200);

    let text = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        response.text(),
    )
    .await
    .expect("SSE stream timed out")
    .unwrap();

    let events = parse_sse_events(&text);

    // Filter to chunk events
    let chunk_events: Vec<_> = events
        .iter()
        .filter(|(t, v)| {
            t == "taskcast.event"
                && v.get("type").and_then(|t| t.as_str()) == Some("llm.chunk")
        })
        .collect();

    // Should have exactly 2 snapshot events (one per series)
    assert_eq!(
        chunk_events.len(),
        2,
        "Should have 2 collapsed snapshots (seriesA + seriesB). Got {}. Full:\n{text}",
        chunk_events.len()
    );

    // Both should have seriesSnapshot: true
    for (_, ev) in &chunk_events {
        assert_eq!(
            ev.get("seriesSnapshot").and_then(|v| v.as_bool()),
            Some(true),
            "Each collapsed event should have seriesSnapshot: true"
        );
    }

    // Verify accumulated data per series
    let series_a: Vec<_> = chunk_events
        .iter()
        .filter(|(_, v)| v.get("seriesId").and_then(|s| s.as_str()) == Some("seriesA"))
        .collect();
    let series_b: Vec<_> = chunk_events
        .iter()
        .filter(|(_, v)| v.get("seriesId").and_then(|s| s.as_str()) == Some("seriesB"))
        .collect();

    assert_eq!(series_a.len(), 1, "Should have 1 snapshot for seriesA");
    assert_eq!(series_b.len(), 1, "Should have 1 snapshot for seriesB");

    let a_text = series_a[0].1["data"]["text"].as_str().unwrap_or("");
    assert_eq!(a_text, "A1 A2 A3", "SeriesA should have accumulated text");

    let b_text = series_b[0].1["data"]["text"].as_str().unwrap_or("");
    assert_eq!(b_text, "B1 B2", "SeriesB should have accumulated text");
}

// =============================================================================
// 8. wrap=false with series snapshot: raw event format, not envelope
// =============================================================================

#[tokio::test]
async fn wrap_false_with_snapshot_sends_raw_event() {
    let (engine, app) = make_app();
    let addr = serve_app(app).await;
    let client = reqwest::Client::new();

    create_running_task(&engine, "sf-nowrap-8").await;
    publish_accumulate_event(&engine, "sf-nowrap-8", "out", "data").await;

    engine
        .transition_task("sf-nowrap-8", TaskStatus::Completed, None)
        .await
        .unwrap();

    // Connect with wrap=false
    let response = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        client
            .get(format!(
                "http://{addr}/tasks/sf-nowrap-8/events?wrap=false"
            ))
            .send(),
    )
    .await
    .expect("SSE connect timed out")
    .unwrap();
    assert_eq!(response.status(), 200);

    let text = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        response.text(),
    )
    .await
    .expect("SSE stream timed out")
    .unwrap();

    let events = parse_sse_events(&text);

    let chunk_events: Vec<_> = events
        .iter()
        .filter(|(t, v)| {
            t == "taskcast.event"
                && v.get("type").and_then(|t| t.as_str()) == Some("llm.chunk")
        })
        .collect();

    assert_eq!(chunk_events.len(), 1, "Should have 1 snapshot");

    // With wrap=false, the event is sent as raw TaskEvent, not SSEEnvelope.
    // Raw format should NOT have filteredIndex or rawIndex fields.
    let ev = &chunk_events[0].1;
    assert!(
        ev.get("filteredIndex").is_none(),
        "wrap=false should not have filteredIndex"
    );
    assert!(
        ev.get("rawIndex").is_none(),
        "wrap=false should not have rawIndex"
    );
    // But should have seriesSnapshot from the event itself
    assert_eq!(
        ev.get("seriesSnapshot").and_then(|v| v.as_bool()),
        Some(true),
        "wrap=false snapshot should have seriesSnapshot: true"
    );
}
