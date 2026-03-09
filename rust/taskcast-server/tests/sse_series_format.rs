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

// =============================================================================
// 9. seriesFormat=accumulated with wrap=false on live events
// =============================================================================

#[tokio::test]
async fn series_format_accumulated_wrap_false_live_events() {
    let (engine, app) = make_app();
    let addr = serve_app(app).await;
    let client = reqwest::Client::new();

    create_running_task(&engine, "sf-acc-wrap-9").await;

    let engine_clone = Arc::clone(&engine);
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;

        publish_accumulate_event(&engine_clone, "sf-acc-wrap-9", "out", "foo").await;
        publish_accumulate_event(&engine_clone, "sf-acc-wrap-9", "out", "bar").await;

        engine_clone
            .transition_task("sf-acc-wrap-9", TaskStatus::Completed, None)
            .await
            .unwrap();
    });

    let response = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        client
            .get(format!(
                "http://{addr}/tasks/sf-acc-wrap-9/events?seriesFormat=accumulated&wrap=false"
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

    // With accumulated format, each live event should have accumulated text
    let last_chunk = &chunk_events.last().unwrap().1;
    let data_text = last_chunk["data"]["text"].as_str().unwrap_or("");
    assert_eq!(
        data_text, "foobar",
        "Accumulated format should deliver full accumulated text. Got: {data_text}"
    );

    // wrap=false: no envelope fields
    assert!(
        last_chunk.get("filteredIndex").is_none(),
        "wrap=false should not have filteredIndex"
    );

    // _accumulated_data should NOT appear in output (it's transient)
    assert!(
        last_chunk.get("_accumulated_data").is_none(),
        "_accumulated_data should be stripped from SSE output"
    );

    // First event should also have accumulated text (just "foo" for first event)
    if chunk_events.len() >= 2 {
        let first_chunk = &chunk_events[0].1;
        let first_text = first_chunk["data"]["text"].as_str().unwrap_or("");
        assert_eq!(
            first_text, "foo",
            "First accumulated event should have 'foo'. Got: {first_text}"
        );
    }
}

// =============================================================================
// 10. Verify _accumulated_data is never leaked to SSE output
// =============================================================================

#[tokio::test]
async fn accumulated_data_field_stripped_from_output() {
    let (engine, app) = make_app();
    let addr = serve_app(app).await;
    let client = reqwest::Client::new();

    create_running_task(&engine, "sf-strip-10").await;

    let engine_clone = Arc::clone(&engine);
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;

        publish_accumulate_event(&engine_clone, "sf-strip-10", "out", "data").await;

        engine_clone
            .transition_task("sf-strip-10", TaskStatus::Completed, None)
            .await
            .unwrap();
    });

    // Use default format (delta) — _accumulated_data should still be stripped
    let response = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        client
            .get(format!(
                "http://{addr}/tasks/sf-strip-10/events"
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

    // _accumulated_data should never appear in the raw SSE text
    assert!(
        !text.contains("_accumulated_data"),
        "_accumulated_data should never appear in SSE output. Got:\n{text}"
    );
    assert!(
        !text.contains("_accumulatedData"),
        "_accumulatedData should never appear in SSE output. Got:\n{text}"
    );
}

// =============================================================================
// 11. seriesFormat=delta from start delivers original deltas
// =============================================================================

#[tokio::test]
async fn delta_from_start_delivers_original_deltas() {
    let (engine, app) = make_app();
    let addr = serve_app(app).await;
    let client = reqwest::Client::new();

    create_running_task(&engine, "sf-delta-11").await;

    let engine_clone = Arc::clone(&engine);
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;

        publish_accumulate_event(&engine_clone, "sf-delta-11", "out", "Hello").await;
        publish_accumulate_event(&engine_clone, "sf-delta-11", "out", " world").await;
        publish_accumulate_event(&engine_clone, "sf-delta-11", "out", "!").await;

        engine_clone
            .transition_task("sf-delta-11", TaskStatus::Completed, None)
            .await
            .unwrap();
    });

    let response = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        client
            .get(format!(
                "http://{addr}/tasks/sf-delta-11/events?seriesFormat=delta"
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

    let chunk_events: Vec<_> = events
        .iter()
        .filter(|(t, v)| {
            t == "taskcast.event"
                && v.get("type").and_then(|t| t.as_str()) == Some("llm.chunk")
        })
        .collect();

    assert_eq!(
        chunk_events.len(),
        3,
        "Should have 3 delta chunk events. Got {}. Full:\n{text}",
        chunk_events.len()
    );

    // Each event should have its original delta data (not accumulated)
    let texts: Vec<&str> = chunk_events
        .iter()
        .map(|(_, v)| v["data"]["text"].as_str().unwrap_or(""))
        .collect();
    assert_eq!(
        texts,
        vec!["Hello", " world", "!"],
        "Delta events should have original delta data. Got: {texts:?}"
    );
}

// =============================================================================
// 12. seriesFormat=accumulated from start delivers running totals
// =============================================================================

#[tokio::test]
async fn accumulated_from_start_delivers_running_totals() {
    let (engine, app) = make_app();
    let addr = serve_app(app).await;
    let client = reqwest::Client::new();

    create_running_task(&engine, "sf-acc-12").await;

    let engine_clone = Arc::clone(&engine);
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;

        publish_accumulate_event(&engine_clone, "sf-acc-12", "out", "Hello").await;
        publish_accumulate_event(&engine_clone, "sf-acc-12", "out", " world").await;
        publish_accumulate_event(&engine_clone, "sf-acc-12", "out", "!").await;

        engine_clone
            .transition_task("sf-acc-12", TaskStatus::Completed, None)
            .await
            .unwrap();
    });

    let response = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        client
            .get(format!(
                "http://{addr}/tasks/sf-acc-12/events?seriesFormat=accumulated"
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

    let chunk_events: Vec<_> = events
        .iter()
        .filter(|(t, v)| {
            t == "taskcast.event"
                && v.get("type").and_then(|t| t.as_str()) == Some("llm.chunk")
        })
        .collect();

    assert_eq!(
        chunk_events.len(),
        3,
        "Should have 3 accumulated chunk events. Got {}. Full:\n{text}",
        chunk_events.len()
    );

    // Each event should have running accumulated totals
    let texts: Vec<&str> = chunk_events
        .iter()
        .map(|(_, v)| v["data"]["text"].as_str().unwrap_or(""))
        .collect();
    assert_eq!(
        texts,
        vec!["Hello", "Hello world", "Hello world!"],
        "Accumulated events should have running totals. Got: {texts:?}"
    );
}

// =============================================================================
// 13. Late-join delta: snapshot then live deltas
// =============================================================================

#[tokio::test]
async fn late_join_delta_snapshot_then_live_deltas() {
    let (engine, app) = make_app();
    let addr = serve_app(app).await;
    let client = reqwest::Client::new();

    create_running_task(&engine, "sf-late-delta-13").await;

    // Publish 5 accumulate events BEFORE connecting
    publish_accumulate_event(&engine, "sf-late-delta-13", "out", "A").await;
    publish_accumulate_event(&engine, "sf-late-delta-13", "out", "B").await;
    publish_accumulate_event(&engine, "sf-late-delta-13", "out", "C").await;
    publish_accumulate_event(&engine, "sf-late-delta-13", "out", "D").await;
    publish_accumulate_event(&engine, "sf-late-delta-13", "out", "E").await;

    let engine_clone = Arc::clone(&engine);
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;

        publish_accumulate_event(&engine_clone, "sf-late-delta-13", "out", "F").await;
        publish_accumulate_event(&engine_clone, "sf-late-delta-13", "out", "G").await;

        engine_clone
            .transition_task("sf-late-delta-13", TaskStatus::Completed, None)
            .await
            .unwrap();
    });

    // Connect with seriesFormat=delta (no since cursor)
    let response = tokio::time::timeout(
        std::time::Duration::from_secs(15),
        client
            .get(format!(
                "http://{addr}/tasks/sf-late-delta-13/events?seriesFormat=delta"
            ))
            .send(),
    )
    .await
    .expect("SSE connect timed out")
    .unwrap();
    assert_eq!(response.status(), 200);

    let text = tokio::time::timeout(
        std::time::Duration::from_secs(15),
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

    // First event should be a snapshot with accumulated text of all 5 pre-publish events
    assert!(
        !chunk_events.is_empty(),
        "Should have chunk events. Got:\n{text}"
    );

    let first = &chunk_events[0].1;
    assert_eq!(
        first.get("seriesSnapshot").and_then(|v| v.as_bool()),
        Some(true),
        "First event should be a snapshot (seriesSnapshot: true). Got:\n{}",
        serde_json::to_string_pretty(first).unwrap()
    );

    let snapshot_text = first["data"]["text"].as_str().unwrap_or("");
    assert_eq!(
        snapshot_text, "ABCDE",
        "Snapshot should contain accumulated text of all 5 pre-publish deltas. Got: {snapshot_text}"
    );

    // Remaining events should be live deltas (F and G)
    let live_events: Vec<_> = chunk_events[1..].to_vec();
    assert_eq!(
        live_events.len(),
        2,
        "Should have 2 live delta events after snapshot. Got {}. Full:\n{text}",
        live_events.len()
    );

    let live_texts: Vec<&str> = live_events
        .iter()
        .map(|(_, v)| v["data"]["text"].as_str().unwrap_or(""))
        .collect();
    assert_eq!(
        live_texts,
        vec!["F", "G"],
        "Live events should be original deltas. Got: {live_texts:?}"
    );
}

// =============================================================================
// 14. Late-join accumulated: snapshot then accumulated live
// =============================================================================

#[tokio::test]
async fn late_join_accumulated_snapshot_then_accumulated_live() {
    let (engine, app) = make_app();
    let addr = serve_app(app).await;
    let client = reqwest::Client::new();

    create_running_task(&engine, "sf-late-acc-14").await;

    // Publish 3 accumulate events BEFORE connecting
    publish_accumulate_event(&engine, "sf-late-acc-14", "out", "A").await;
    publish_accumulate_event(&engine, "sf-late-acc-14", "out", "B").await;
    publish_accumulate_event(&engine, "sf-late-acc-14", "out", "C").await;

    let engine_clone = Arc::clone(&engine);
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;

        publish_accumulate_event(&engine_clone, "sf-late-acc-14", "out", "D").await;

        engine_clone
            .transition_task("sf-late-acc-14", TaskStatus::Completed, None)
            .await
            .unwrap();
    });

    // Connect with seriesFormat=accumulated
    let response = tokio::time::timeout(
        std::time::Duration::from_secs(15),
        client
            .get(format!(
                "http://{addr}/tasks/sf-late-acc-14/events?seriesFormat=accumulated"
            ))
            .send(),
    )
    .await
    .expect("SSE connect timed out")
    .unwrap();
    assert_eq!(response.status(), 200);

    let text = tokio::time::timeout(
        std::time::Duration::from_secs(15),
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

    assert!(
        !chunk_events.is_empty(),
        "Should have chunk events. Got:\n{text}"
    );

    // First event is snapshot with "ABC"
    let first = &chunk_events[0].1;
    let snapshot_text = first["data"]["text"].as_str().unwrap_or("");
    assert_eq!(
        snapshot_text, "ABC",
        "Snapshot should contain accumulated text 'ABC'. Got: {snapshot_text}"
    );

    // Live event should have accumulated "ABCD"
    let last = &chunk_events.last().unwrap().1;
    let last_text = last["data"]["text"].as_str().unwrap_or("");
    assert_eq!(
        last_text, "ABCD",
        "Live accumulated event should have 'ABCD'. Got: {last_text}"
    );
}

// =============================================================================
// 15. Terminal task replay: snapshot and done
// =============================================================================

#[tokio::test]
async fn terminal_task_replay_snapshot_and_done() {
    let (engine, app) = make_app();
    let addr = serve_app(app).await;
    let client = reqwest::Client::new();

    create_running_task(&engine, "sf-terminal-15").await;

    // Publish 3 accumulate events
    publish_accumulate_event(&engine, "sf-terminal-15", "out", "X").await;
    publish_accumulate_event(&engine, "sf-terminal-15", "out", "Y").await;
    publish_accumulate_event(&engine, "sf-terminal-15", "out", "Z").await;

    // Complete the task BEFORE connecting
    engine
        .transition_task("sf-terminal-15", TaskStatus::Completed, None)
        .await
        .unwrap();

    // THEN connect SSE
    let response = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        client
            .get(format!("http://{addr}/tasks/sf-terminal-15/events"))
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

    // Should have 1 snapshot event with accumulated data
    let chunk_events: Vec<_> = events
        .iter()
        .filter(|(t, v)| {
            t == "taskcast.event"
                && v.get("type").and_then(|t| t.as_str()) == Some("llm.chunk")
        })
        .collect();

    assert_eq!(
        chunk_events.len(),
        1,
        "Should have exactly 1 snapshot event. Got {}. Full:\n{text}",
        chunk_events.len()
    );

    let snapshot = &chunk_events[0].1;
    let snapshot_text = snapshot["data"]["text"].as_str().unwrap_or("");
    assert_eq!(
        snapshot_text, "XYZ",
        "Snapshot should have accumulated data 'XYZ'. Got: {snapshot_text}"
    );

    // Should have a taskcast.done event with reason "completed"
    let done_events: Vec<_> = events
        .iter()
        .filter(|(t, _)| t == "taskcast.done")
        .collect();

    assert_eq!(
        done_events.len(),
        1,
        "Should have exactly 1 done event. Got {}. Full:\n{text}",
        done_events.len()
    );

    let reason = done_events[0].1.get("reason").and_then(|r| r.as_str());
    assert_eq!(
        reason,
        Some("completed"),
        "Done event should have reason 'completed'. Got: {reason:?}"
    );
}

// =============================================================================
// 16. Reconnect with since cursor: no collapse, individual deltas
// =============================================================================

#[tokio::test]
async fn reconnect_since_cursor_no_collapse() {
    let (engine, app) = make_app();
    let addr = serve_app(app).await;
    let client = reqwest::Client::new();

    create_running_task(&engine, "sf-reconnect-16").await;

    // Publish 4 accumulate events
    publish_accumulate_event(&engine, "sf-reconnect-16", "out", "a").await;
    publish_accumulate_event(&engine, "sf-reconnect-16", "out", "b").await;
    publish_accumulate_event(&engine, "sf-reconnect-16", "out", "c").await;
    publish_accumulate_event(&engine, "sf-reconnect-16", "out", "d").await;

    engine
        .transition_task("sf-reconnect-16", TaskStatus::Completed, None)
        .await
        .unwrap();

    // Connect with since.index=1 — should get events after index 1 as individual deltas
    let response = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        client
            .get(format!(
                "http://{addr}/tasks/sf-reconnect-16/events?since.index=1"
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

    let chunk_events: Vec<_> = events
        .iter()
        .filter(|(t, v)| {
            t == "taskcast.event"
                && v.get("type").and_then(|t| t.as_str()) == Some("llm.chunk")
        })
        .collect();

    // Events after index 1 should be individual deltas (not collapsed to snapshot)
    for (_, ev) in &chunk_events {
        assert!(
            ev.get("seriesSnapshot").is_none()
                || ev.get("seriesSnapshot") == Some(&json!(null))
                || ev.get("seriesSnapshot") == Some(&json!(false)),
            "With since cursor, events should NOT have seriesSnapshot: true. Got:\n{}",
            serde_json::to_string_pretty(ev).unwrap()
        );
    }

    // Should have received individual delta events (not a single snapshot)
    assert!(
        chunk_events.len() > 1,
        "Should have multiple individual delta events (not collapsed). Got {}. Full:\n{text}",
        chunk_events.len()
    );
}

// =============================================================================
// 17. Multiple series with non-series events preserved
// =============================================================================

#[tokio::test]
async fn multiple_series_with_non_series_events_preserved() {
    let (engine, app) = make_app();
    let addr = serve_app(app).await;
    let client = reqwest::Client::new();

    create_running_task(&engine, "sf-multi-ns-17").await;

    // Publish 2 events for series "sA"
    publish_accumulate_event(&engine, "sf-multi-ns-17", "sA", "A1").await;
    publish_accumulate_event(&engine, "sf-multi-ns-17", "sA", "A2").await;

    // Publish 1 non-series event (type="progress")
    engine
        .publish_event(
            "sf-multi-ns-17",
            taskcast_core::PublishEventInput {
                r#type: "progress".to_string(),
                level: Level::Info,
                data: json!({"pct": 50}),
                series_id: None,
                series_mode: None,
                series_acc_field: None,
            },
        )
        .await
        .unwrap();

    // Publish 2 events for series "sB"
    publish_accumulate_event(&engine, "sf-multi-ns-17", "sB", "B1").await;
    publish_accumulate_event(&engine, "sf-multi-ns-17", "sB", "B2").await;

    engine
        .transition_task("sf-multi-ns-17", TaskStatus::Completed, None)
        .await
        .unwrap();

    // Connect SSE (late-join)
    let response = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        client
            .get(format!("http://{addr}/tasks/sf-multi-ns-17/events"))
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
    let taskcast_events: Vec<_> = events
        .iter()
        .filter(|(t, _)| t == "taskcast.event")
        .collect();

    // Snapshot for sA with accumulated data
    let sa_events: Vec<_> = taskcast_events
        .iter()
        .filter(|(_, v)| v.get("seriesId").and_then(|s| s.as_str()) == Some("sA"))
        .collect();
    assert_eq!(
        sa_events.len(),
        1,
        "Should have 1 snapshot for series sA. Got {}. Full:\n{text}",
        sa_events.len()
    );
    let sa_text = sa_events[0].1["data"]["text"].as_str().unwrap_or("");
    assert_eq!(
        sa_text, "A1A2",
        "Series sA snapshot should have accumulated 'A1A2'. Got: {sa_text}"
    );

    // Progress event preserved
    let progress_events: Vec<_> = taskcast_events
        .iter()
        .filter(|(_, v)| v.get("type").and_then(|t| t.as_str()) == Some("progress"))
        .collect();
    assert_eq!(
        progress_events.len(),
        1,
        "Should have 1 progress event preserved. Got {}. Full:\n{text}",
        progress_events.len()
    );
    let pct = progress_events[0].1["data"]["pct"].as_i64();
    assert_eq!(
        pct,
        Some(50),
        "Progress event should have original data (pct: 50). Got: {pct:?}"
    );

    // Snapshot for sB with accumulated data
    let sb_events: Vec<_> = taskcast_events
        .iter()
        .filter(|(_, v)| v.get("seriesId").and_then(|s| s.as_str()) == Some("sB"))
        .collect();
    assert_eq!(
        sb_events.len(),
        1,
        "Should have 1 snapshot for series sB. Got {}. Full:\n{text}",
        sb_events.len()
    );
    let sb_text = sb_events[0].1["data"]["text"].as_str().unwrap_or("");
    assert_eq!(
        sb_text, "B1B2",
        "Series sB snapshot should have accumulated 'B1B2'. Got: {sb_text}"
    );
}

// =============================================================================
// 18. Rapid publishing: 50 events all received in order
// =============================================================================

#[tokio::test]
async fn rapid_publishing_50_events_all_received_in_order() {
    let (engine, app) = make_app();
    let addr = serve_app(app).await;
    let client = reqwest::Client::new();

    create_running_task(&engine, "sf-rapid-18").await;

    let engine_clone = Arc::clone(&engine);
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;

        for i in 0..50 {
            publish_accumulate_event(
                &engine_clone,
                "sf-rapid-18",
                "out",
                &format!("chunk-{i}"),
            )
            .await;
        }

        engine_clone
            .transition_task("sf-rapid-18", TaskStatus::Completed, None)
            .await
            .unwrap();
    });

    let response = tokio::time::timeout(
        std::time::Duration::from_secs(15),
        client
            .get(format!(
                "http://{addr}/tasks/sf-rapid-18/events?seriesFormat=delta"
            ))
            .send(),
    )
    .await
    .expect("SSE connect timed out")
    .unwrap();
    assert_eq!(response.status(), 200);

    let text = tokio::time::timeout(
        std::time::Duration::from_secs(15),
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

    assert_eq!(
        chunk_events.len(),
        50,
        "Should have all 50 chunk events. Got {}. Full:\n{text}",
        chunk_events.len()
    );

    // Verify all 50 chunks received in order with correct delta values
    for (i, (_, ev)) in chunk_events.iter().enumerate() {
        let expected = format!("chunk-{i}");
        let actual = ev["data"]["text"].as_str().unwrap_or("");
        assert_eq!(
            actual, expected,
            "Event {i} should have delta '{expected}'. Got: '{actual}'"
        );
    }
}

// =============================================================================
// 19. Mid-stream join: snapshot + complete, no gaps
// =============================================================================

#[tokio::test]
async fn mid_stream_join_snapshot_complete_no_gaps() {
    let (engine, app) = make_app();
    let addr = serve_app(app).await;
    let client = reqwest::Client::new();

    create_running_task(&engine, "sf-midstream-19").await;

    // Publish 10 accumulate events BEFORE connecting
    for i in 0..10 {
        publish_accumulate_event(
            &engine,
            "sf-midstream-19",
            "out",
            &format!("p{i}"),
        )
        .await;
    }

    let engine_clone = Arc::clone(&engine);
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;

        for i in 0..5 {
            publish_accumulate_event(
                &engine_clone,
                "sf-midstream-19",
                "out",
                &format!("l{i}"),
            )
            .await;
        }

        engine_clone
            .transition_task("sf-midstream-19", TaskStatus::Completed, None)
            .await
            .unwrap();
    });

    // Connect with seriesFormat=delta
    let response = tokio::time::timeout(
        std::time::Duration::from_secs(15),
        client
            .get(format!(
                "http://{addr}/tasks/sf-midstream-19/events?seriesFormat=delta"
            ))
            .send(),
    )
    .await
    .expect("SSE connect timed out")
    .unwrap();
    assert_eq!(response.status(), 200);

    let text = tokio::time::timeout(
        std::time::Duration::from_secs(15),
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

    assert!(
        !chunk_events.is_empty(),
        "Should have chunk events. Got:\n{text}"
    );

    // First event should be a snapshot with accumulated text of all 10 pre-publish deltas
    let first = &chunk_events[0].1;
    assert_eq!(
        first.get("seriesSnapshot").and_then(|v| v.as_bool()),
        Some(true),
        "First event should be a snapshot. Got:\n{}",
        serde_json::to_string_pretty(first).unwrap()
    );

    let expected_snapshot = "p0p1p2p3p4p5p6p7p8p9";
    let snapshot_text = first["data"]["text"].as_str().unwrap_or("");
    assert_eq!(
        snapshot_text, expected_snapshot,
        "Snapshot should be concatenation of all 10 pre-publish deltas. Got: {snapshot_text}"
    );

    // Remaining events should be the 5 live deltas
    let live_events: Vec<_> = chunk_events[1..].to_vec();
    assert_eq!(
        live_events.len(),
        5,
        "Should have 5 live delta events after snapshot. Got {}. Full:\n{text}",
        live_events.len()
    );

    for (i, (_, ev)) in live_events.iter().enumerate() {
        let expected = format!("l{i}");
        let actual = ev["data"]["text"].as_str().unwrap_or("");
        assert_eq!(
            actual, expected,
            "Live event {i} should have delta '{expected}'. Got: '{actual}'"
        );
    }
}

// =============================================================================
// 20. Mixed subscribers: concurrent delta and accumulated
// =============================================================================

#[tokio::test]
async fn mixed_subscribers_concurrent_delta_and_accumulated() {
    let (engine, app) = make_app();
    let addr = serve_app(app).await;
    let client = reqwest::Client::new();

    create_running_task(&engine, "sf-mixed-sub-20").await;

    let engine_clone = Arc::clone(&engine);
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;

        publish_accumulate_event(&engine_clone, "sf-mixed-sub-20", "out", "A").await;
        publish_accumulate_event(&engine_clone, "sf-mixed-sub-20", "out", "B").await;
        publish_accumulate_event(&engine_clone, "sf-mixed-sub-20", "out", "C").await;
        publish_accumulate_event(&engine_clone, "sf-mixed-sub-20", "out", "D").await;
        publish_accumulate_event(&engine_clone, "sf-mixed-sub-20", "out", "E").await;

        engine_clone
            .transition_task("sf-mixed-sub-20", TaskStatus::Completed, None)
            .await
            .unwrap();
    });

    // Open TWO concurrent SSE connections
    let delta_fut = async {
        let resp = tokio::time::timeout(
            std::time::Duration::from_secs(15),
            client
                .get(format!(
                    "http://{addr}/tasks/sf-mixed-sub-20/events?seriesFormat=delta"
                ))
                .send(),
        )
        .await
        .expect("Delta SSE connect timed out")
        .unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(15), resp.text())
            .await
            .expect("Delta SSE stream timed out")
            .unwrap()
    };

    let accumulated_fut = async {
        let resp = tokio::time::timeout(
            std::time::Duration::from_secs(15),
            client
                .get(format!(
                    "http://{addr}/tasks/sf-mixed-sub-20/events?seriesFormat=accumulated"
                ))
                .send(),
        )
        .await
        .expect("Accumulated SSE connect timed out")
        .unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(15), resp.text())
            .await
            .expect("Accumulated SSE stream timed out")
            .unwrap()
    };

    let (delta_text, accumulated_text) = tokio::join!(delta_fut, accumulated_fut);

    // Verify delta subscriber gets original deltas
    let delta_events = parse_sse_events(&delta_text);
    let delta_chunks: Vec<_> = delta_events
        .iter()
        .filter(|(t, v)| {
            t == "taskcast.event"
                && v.get("type").and_then(|t| t.as_str()) == Some("llm.chunk")
        })
        .collect();

    assert_eq!(
        delta_chunks.len(),
        5,
        "Delta subscriber should have 5 chunk events. Got {}. Full:\n{delta_text}",
        delta_chunks.len()
    );

    let delta_texts: Vec<&str> = delta_chunks
        .iter()
        .map(|(_, v)| v["data"]["text"].as_str().unwrap_or(""))
        .collect();
    assert_eq!(
        delta_texts,
        vec!["A", "B", "C", "D", "E"],
        "Delta subscriber should get original deltas. Got: {delta_texts:?}"
    );

    // Verify accumulated subscriber gets running totals
    let acc_events = parse_sse_events(&accumulated_text);
    let acc_chunks: Vec<_> = acc_events
        .iter()
        .filter(|(t, v)| {
            t == "taskcast.event"
                && v.get("type").and_then(|t| t.as_str()) == Some("llm.chunk")
        })
        .collect();

    assert_eq!(
        acc_chunks.len(),
        5,
        "Accumulated subscriber should have 5 chunk events. Got {}. Full:\n{accumulated_text}",
        acc_chunks.len()
    );

    let acc_texts: Vec<&str> = acc_chunks
        .iter()
        .map(|(_, v)| v["data"]["text"].as_str().unwrap_or(""))
        .collect();
    assert_eq!(
        acc_texts,
        vec!["A", "AB", "ABC", "ABCD", "ABCDE"],
        "Accumulated subscriber should get running totals. Got: {acc_texts:?}"
    );
}

// =============================================================================
// 21. Non-series events unaffected by seriesFormat
// =============================================================================

#[tokio::test]
async fn non_series_events_unaffected_by_series_format() {
    let (engine, app) = make_app();
    let addr = serve_app(app).await;
    let client = reqwest::Client::new();

    create_running_task(&engine, "sf-nonseries-21").await;

    let engine_clone = Arc::clone(&engine);
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;

        // Publish an accumulate event
        publish_accumulate_event(&engine_clone, "sf-nonseries-21", "out", "Hello").await;

        // Publish a non-series event
        engine_clone
            .publish_event(
                "sf-nonseries-21",
                taskcast_core::PublishEventInput {
                    r#type: "progress".to_string(),
                    level: Level::Info,
                    data: json!({"pct": 25}),
                    series_id: None,
                    series_mode: None,
                    series_acc_field: None,
                },
            )
            .await
            .unwrap();

        // Another accumulate event
        publish_accumulate_event(&engine_clone, "sf-nonseries-21", "out", " world").await;

        // Another non-series event
        engine_clone
            .publish_event(
                "sf-nonseries-21",
                taskcast_core::PublishEventInput {
                    r#type: "status".to_string(),
                    level: Level::Info,
                    data: json!({"msg": "processing"}),
                    series_id: None,
                    series_mode: None,
                    series_acc_field: None,
                },
            )
            .await
            .unwrap();

        engine_clone
            .transition_task("sf-nonseries-21", TaskStatus::Completed, None)
            .await
            .unwrap();
    });

    // Connect with seriesFormat=accumulated
    let response = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        client
            .get(format!(
                "http://{addr}/tasks/sf-nonseries-21/events?seriesFormat=accumulated"
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
    let taskcast_events: Vec<_> = events
        .iter()
        .filter(|(t, _)| t == "taskcast.event")
        .collect();

    // Accumulate events should have accumulated data
    let acc_events: Vec<_> = taskcast_events
        .iter()
        .filter(|(_, v)| v.get("type").and_then(|t| t.as_str()) == Some("llm.chunk"))
        .collect();

    assert_eq!(
        acc_events.len(),
        2,
        "Should have 2 accumulated chunk events. Got {}. Full:\n{text}",
        acc_events.len()
    );

    // First accumulated event: "Hello"
    let first_acc_text = acc_events[0].1["data"]["text"].as_str().unwrap_or("");
    assert_eq!(
        first_acc_text, "Hello",
        "First accumulated event should be 'Hello'. Got: {first_acc_text}"
    );

    // Second accumulated event: "Hello world"
    let second_acc_text = acc_events[1].1["data"]["text"].as_str().unwrap_or("");
    assert_eq!(
        second_acc_text, "Hello world",
        "Second accumulated event should be 'Hello world'. Got: {second_acc_text}"
    );

    // Non-series events should have unchanged original data
    let progress_events: Vec<_> = taskcast_events
        .iter()
        .filter(|(_, v)| v.get("type").and_then(|t| t.as_str()) == Some("progress"))
        .collect();
    assert_eq!(
        progress_events.len(),
        1,
        "Should have 1 progress event. Got {}",
        progress_events.len()
    );
    assert_eq!(
        progress_events[0].1["data"]["pct"].as_i64(),
        Some(25),
        "Progress event data should be unchanged"
    );

    let status_events: Vec<_> = taskcast_events
        .iter()
        .filter(|(_, v)| v.get("type").and_then(|t| t.as_str()) == Some("status"))
        .collect();
    assert_eq!(
        status_events.len(),
        1,
        "Should have 1 status event. Got {}",
        status_events.len()
    );
    assert_eq!(
        status_events[0].1["data"]["msg"].as_str(),
        Some("processing"),
        "Status event data should be unchanged"
    );
}
