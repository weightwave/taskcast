//! Tests for SSE event filtering and terminal status handling in the
//! subscription callback.
//!
//! These tests cover:
//! - Line 250: `matches_filter` excluding non-matching events from the live
//!   subscription callback
//! - Lines 266, 268: Terminal status detection in the subscription callback
//!   triggering the done signal and closing the stream

use std::sync::Arc;

use serde_json::json;
use taskcast_core::{
    CreateTaskInput, Level, MemoryBroadcastProvider, MemoryShortTermStore, PublishEventInput,
    TaskEngine, TaskEngineOptions, TaskStatus,
};
use taskcast_server::{create_app, AuthMode, CorsConfig};

// ─── Test Helpers ────────────────────────────────────────────────────────────

fn make_sse_app() -> (Arc<TaskEngine>, axum::Router) {
    let engine = Arc::new(TaskEngine::new(TaskEngineOptions {
        short_term_store: Arc::new(MemoryShortTermStore::new()),
        broadcast: Arc::new(MemoryBroadcastProvider::new()),
        long_term_store: None,
        hooks: None,
    }));
    let (router, _) = create_app(
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

/// Parse SSE text body into a list of (event_name, data_json) pairs.
fn parse_sse_events(body: &str) -> Vec<(String, serde_json::Value)> {
    let mut results = Vec::new();
    let mut current_event = String::new();

    for line in body.lines() {
        if let Some(ev) = line.strip_prefix("event: ") {
            current_event = ev.to_string();
        } else if let Some(data) = line.strip_prefix("data: ") {
            if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(data) {
                results.push((current_event.clone(), parsed));
            }
        }
    }

    results
}

// =============================================================================
// 1. SSE filter excludes non-matching events (covers line 250)
// =============================================================================

#[tokio::test]
async fn sse_filter_excludes_non_matching_events() {
    let (engine, app) = make_sse_app();
    let addr = serve_app(app).await;
    let client = reqwest::Client::new();

    // Create a task and transition to running so we can subscribe live
    engine
        .create_task(CreateTaskInput {
            id: Some("filter-test-1".to_string()),
            r#type: Some("test".to_string()),
            ..Default::default()
        })
        .await
        .unwrap();
    engine
        .transition_task("filter-test-1", TaskStatus::Running, None)
        .await
        .unwrap();

    // Spawn background work: publish events and then close the stream
    let engine_clone = Arc::clone(&engine);
    tokio::spawn(async move {
        // Wait for SSE subscription to establish
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        // Publish an event with a type that does NOT match the filter
        engine_clone
            .publish_event(
                "filter-test-1",
                PublishEventInput {
                    r#type: "unwanted.type".to_string(),
                    level: Level::Info,
                    data: json!({ "msg": "should be filtered out" }),
                    series_id: None,
                    series_mode: None,
                    series_acc_field: None,
                },
            )
            .await
            .unwrap();

        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        // Publish an event with a type that DOES match the filter
        engine_clone
            .publish_event(
                "filter-test-1",
                PublishEventInput {
                    r#type: "wanted.type".to_string(),
                    level: Level::Info,
                    data: json!({ "msg": "should pass filter" }),
                    series_id: None,
                    series_mode: None,
                    series_acc_field: None,
                },
            )
            .await
            .unwrap();

        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        // Transition to completed to close the stream
        engine_clone
            .transition_task("filter-test-1", TaskStatus::Completed, None)
            .await
            .unwrap();
    });

    // Connect SSE with types filter — include "wanted.type" and "taskcast:status"
    // so that the terminal detection still works (lines 255-268), but "unwanted.type"
    // is filtered out by matches_filter (line 250).
    let response = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        client
            .get(format!(
                "http://{addr}/tasks/filter-test-1/events?types=wanted.type,taskcast:status"
            ))
            .header("Accept", "text/event-stream")
            .send(),
    )
    .await
    .expect("SSE connect timed out")
    .unwrap();
    assert_eq!(response.status(), 200);

    let body = tokio::time::timeout(std::time::Duration::from_secs(5), response.text())
        .await
        .expect("SSE stream timed out")
        .unwrap();

    let events = parse_sse_events(&body);

    // The "unwanted.type" event should have been filtered out (line 250).
    // Check that no event contains the unwanted data.
    assert!(
        !body.contains("should be filtered out"),
        "unwanted.type event data should not appear in stream. Events:\n{body}"
    );
    assert!(
        !body.contains("unwanted.type"),
        "unwanted.type should not appear in stream. Events:\n{body}"
    );

    // The "wanted.type" event should be present
    assert!(
        body.contains("should pass filter"),
        "wanted.type event should be present in stream. Got:\n{body}"
    );

    // Count only user events (non-status). The filter allows "wanted.type" and
    // "taskcast:status", so we should see exactly 1 wanted.type event and some
    // taskcast:status events, but zero unwanted.type events.
    let user_events: Vec<_> = events
        .iter()
        .filter(|(name, data)| {
            name == "taskcast.event"
                && data
                    .get("type")
                    .and_then(|v| v.as_str())
                    .map(|t| t == "wanted.type")
                    .unwrap_or(false)
        })
        .collect();

    assert_eq!(
        user_events.len(),
        1,
        "should have exactly 1 wanted.type event, got {}. Events:\n{body}",
        user_events.len()
    );

    // The stream should have closed because taskcast:status with "completed"
    // passed the filter and triggered the done signal (lines 261-266).
    assert!(
        body.contains("taskcast.done"),
        "should have taskcast.done event after terminal status. Got:\n{body}"
    );
}

// =============================================================================
// 2. SSE closes on terminal status (covers lines 266, 268)
// =============================================================================

#[tokio::test]
async fn sse_closes_on_terminal_status() {
    let (engine, app) = make_sse_app();
    let addr = serve_app(app).await;
    let client = reqwest::Client::new();

    // Create a task and transition to running
    engine
        .create_task(CreateTaskInput {
            id: Some("terminal-test-1".to_string()),
            r#type: Some("test".to_string()),
            ..Default::default()
        })
        .await
        .unwrap();
    engine
        .transition_task("terminal-test-1", TaskStatus::Running, None)
        .await
        .unwrap();

    // Spawn background: wait for SSE to connect, then transition to completed
    let engine_clone = Arc::clone(&engine);
    tokio::spawn(async move {
        // Wait for SSE subscription to establish
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        // Transition to completed — this triggers the terminal status handling
        // in the subscription callback (lines 255-268)
        engine_clone
            .transition_task("terminal-test-1", TaskStatus::Completed, None)
            .await
            .unwrap();
    });

    // Connect SSE without any filters (default includes status events)
    let response = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        client
            .get(format!("http://{addr}/tasks/terminal-test-1/events"))
            .header("Accept", "text/event-stream")
            .send(),
    )
    .await
    .expect("SSE connect timed out")
    .unwrap();
    assert_eq!(response.status(), 200);

    // Read the body — stream should close after the terminal event
    let body = tokio::time::timeout(std::time::Duration::from_secs(5), response.text())
        .await
        .expect("SSE stream should close after terminal status")
        .unwrap();

    let events = parse_sse_events(&body);

    // Should have a taskcast:status event with status=completed
    let has_completed_status = events.iter().any(|(name, data)| {
        name == "taskcast.event"
            && data
                .get("type")
                .and_then(|v| v.as_str())
                .map(|s| s == "taskcast:status")
                .unwrap_or(false)
            && data
                .get("data")
                .and_then(|d| d.get("status"))
                .and_then(|s| s.as_str())
                .map(|s| s == "completed")
                .unwrap_or(false)
    });
    assert!(
        has_completed_status,
        "should have a taskcast:status event with status=completed. Events:\n{body}"
    );

    // Should have a taskcast.done event with reason=completed (line 261)
    let has_done = events.iter().any(|(name, data)| {
        name == "taskcast.done"
            && data
                .get("reason")
                .and_then(|v| v.as_str())
                .map(|s| s == "completed")
                .unwrap_or(false)
    });
    assert!(
        has_done,
        "should have a taskcast.done event with reason=completed. Events:\n{body}"
    );

    // The stream should have closed (we successfully read the full body within the timeout),
    // which means the done_tx signal on line 264 fired and the select! on line 276
    // resolved, causing the stream to end.
}
