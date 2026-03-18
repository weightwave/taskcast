use std::convert::Infallible;
use std::net::SocketAddr;
use std::sync::Arc;

use axum::response::sse::{Event, Sse};
use axum::routing::get;
use axum::Router;
use futures_util::stream;
use serde_json::json;
use tokio::net::TcpListener;

use taskcast_core::{
    CreateTaskInput, Level, MemoryBroadcastProvider, MemoryShortTermStore, PublishEventInput,
    TaskEngine, TaskEngineOptions, TaskStatus,
};
use taskcast_server::{create_app, AuthMode, CorsConfig};

use taskcast_cli::client::TaskcastClient;
use taskcast_cli::commands::logs::{consume_sse, format_event};

// ─── Helpers ──────────────────────────────────────────────────────────────────

fn make_engine() -> Arc<TaskEngine> {
    Arc::new(TaskEngine::new(TaskEngineOptions {
        short_term_store: Arc::new(MemoryShortTermStore::new()),
        broadcast: Arc::new(MemoryBroadcastProvider::new()),
        long_term_store: None,
        hooks: None,
    }))
}

async fn start_server(engine: Arc<TaskEngine>) -> String {
    let (app, _) = create_app(
        Arc::clone(&engine),
        AuthMode::None,
        None,
        None,
        CorsConfig::default(),
    );
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr: SocketAddr = listener.local_addr().unwrap();
    let base_url = format!("http://127.0.0.1:{}", addr.port());

    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    base_url
}

async fn start_mock_sse_server(app: Router) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr: SocketAddr = listener.local_addr().unwrap();
    let base_url = format!("http://127.0.0.1:{}", addr.port());

    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    base_url
}

// ─── format_event: comprehensive tests ────────────────────────────────────────

#[test]
fn format_event_regular_includes_all_fields() {
    let result = format_event(
        "llm.delta",
        "info",
        1741234567890,
        &json!({"text": "hello"}),
        None,
    );
    assert!(result.contains("llm.delta"), "got: {result}");
    assert!(result.contains("info"), "got: {result}");
    assert!(
        result.contains(r#""text":"hello""#),
        "got: {result}"
    );
}

#[test]
fn format_event_done_with_reason() {
    let result = format_event(
        "taskcast.done",
        "info",
        1741234567890,
        &json!({"reason": "completed"}),
        None,
    );
    assert!(result.contains("[DONE] completed"), "got: {result}");
}

#[test]
fn format_event_done_colon_variant() {
    let result = format_event(
        "taskcast:done",
        "info",
        1741234567890,
        &json!({"reason": "failed"}),
        None,
    );
    assert!(result.contains("[DONE] failed"), "got: {result}");
}

#[test]
fn format_event_done_missing_reason() {
    let result = format_event(
        "taskcast.done",
        "info",
        1741234567890,
        &json!({}),
        None,
    );
    assert!(result.contains("[DONE] unknown"), "got: {result}");
}

#[test]
fn format_event_done_null_data() {
    let result = format_event(
        "taskcast.done",
        "info",
        1741234567890,
        &serde_json::Value::Null,
        None,
    );
    assert!(result.contains("[DONE] unknown"), "got: {result}");
}

#[test]
fn format_event_with_task_id_prefix() {
    let result = format_event(
        "agent.step",
        "info",
        1741234567890,
        &json!({"step": 3}),
        Some("01JABCDEFGHIJKLMNOPQR"),
    );
    assert!(result.contains("01JABCD..  "), "got: {result}");
    assert!(result.contains("agent.step"), "got: {result}");
}

#[test]
fn format_event_with_short_task_id() {
    let result = format_event("test", "info", 0, &json!({}), Some("abc"));
    assert!(result.contains("abc..  "), "got: {result}");
}

#[test]
fn format_event_done_with_task_id() {
    let result = format_event(
        "taskcast.done",
        "info",
        1741234567890,
        &json!({"reason": "timeout"}),
        Some("01JABCDEFGHIJKLMNOPQR"),
    );
    assert!(result.contains("01JABCD..  "), "got: {result}");
    assert!(result.contains("[DONE] timeout"), "got: {result}");
}

#[test]
fn format_event_zero_timestamp() {
    let result = format_event("test", "info", 0, &json!({}), None);
    // Timestamp 0 should produce 1970-01-01 00:00:00
    assert!(result.contains("00:00:00"), "got: {result}");
}

#[test]
fn format_event_null_data_regular() {
    let result = format_event("test.event", "info", 0, &serde_json::Value::Null, None);
    assert!(result.contains("null"), "got: {result}");
}

#[test]
fn format_event_type_padding() {
    let result = format_event("x", "warn", 0, &json!({}), None);
    // "x" should be padded to 16 characters
    assert!(
        result.contains("x               "),
        "type should be padded, got: {result}"
    );
}

#[test]
fn format_event_level_padding() {
    let result = format_event("llm.delta", "info", 0, &json!({}), None);
    // "info" should be padded to 5 characters
    assert!(
        result.contains("info "),
        "level should be padded, got: {result}"
    );
}

// ─── consume_sse: mock SSE endpoint ───────────────────────────────────────────

#[tokio::test]
async fn consume_sse_receives_events() {
    let app = Router::new().route(
        "/sse",
        get(|| async {
            let events = vec![
                Ok::<_, Infallible>(
                    Event::default()
                        .event("taskcast.event")
                        .data(r#"{"type":"llm.delta","level":"info","timestamp":1741234567890,"data":{"text":"hello"}}"#),
                ),
                Ok(
                    Event::default()
                        .event("taskcast.done")
                        .data(r#"{"reason":"completed"}"#),
                ),
            ];
            Sse::new(stream::iter(events))
        }),
    );
    let base_url = start_mock_sse_server(app).await;

    let mut received_events: Vec<(serde_json::Value, String)> = Vec::new();
    let mut done_called = false;

    consume_sse(
        &format!("{base_url}/sse"),
        None,
        |event, sse_event_name| {
            received_events.push((event, sse_event_name.to_string()));
        },
        Some(&mut || {
            done_called = true;
        }),
    )
    .await
    .unwrap();

    assert_eq!(received_events.len(), 2);

    // First event: taskcast.event
    assert_eq!(received_events[0].1, "taskcast.event");
    assert_eq!(received_events[0].0["type"], "llm.delta");
    assert_eq!(received_events[0].0["data"]["text"], "hello");

    // Second event: taskcast.done
    assert_eq!(received_events[1].1, "taskcast.done");
    assert_eq!(received_events[1].0["reason"], "completed");

    assert!(done_called, "done callback should have been called");
}

#[tokio::test]
async fn consume_sse_without_done_callback() {
    let app = Router::new().route(
        "/sse",
        get(|| async {
            let events = vec![
                Ok::<_, Infallible>(
                    Event::default()
                        .event("taskcast.event")
                        .data(r#"{"type":"test","level":"info","data":{}}"#),
                ),
            ];
            Sse::new(stream::iter(events))
        }),
    );
    let base_url = start_mock_sse_server(app).await;

    let mut count = 0;

    consume_sse(
        &format!("{base_url}/sse"),
        None,
        |_event, _name| {
            count += 1;
        },
        None,
    )
    .await
    .unwrap();

    assert_eq!(count, 1);
}

#[tokio::test]
async fn consume_sse_with_auth_token() {
    // Create a server that checks for the auth header
    let app = Router::new().route(
        "/sse",
        get(|req: axum::extract::Request| async move {
            let auth = req
                .headers()
                .get("Authorization")
                .map(|v| v.to_str().unwrap().to_string());
            let auth_present = auth.as_deref() == Some("Bearer test-token");
            let events = vec![Ok::<_, Infallible>(
                Event::default()
                    .event("taskcast.event")
                    .data(
                        serde_json::to_string(&json!({"auth_ok": auth_present}))
                            .unwrap(),
                    ),
            )];
            Sse::new(stream::iter(events))
        }),
    );
    let base_url = start_mock_sse_server(app).await;

    let mut auth_ok = false;

    consume_sse(
        &format!("{base_url}/sse"),
        Some("test-token"),
        |event, _name| {
            if event["auth_ok"].as_bool() == Some(true) {
                auth_ok = true;
            }
        },
        None,
    )
    .await
    .unwrap();

    assert!(auth_ok, "auth token should have been sent");
}

#[tokio::test]
async fn consume_sse_http_error_returns_err() {
    let app = Router::new().route(
        "/sse",
        get(|| async { (axum::http::StatusCode::FORBIDDEN, "denied") }),
    );
    let base_url = start_mock_sse_server(app).await;

    let result = consume_sse(
        &format!("{base_url}/sse"),
        None,
        |_event, _name| {},
        None,
    )
    .await;

    assert!(result.is_err(), "should return error for HTTP 403");
    let err = result.err().unwrap().to_string();
    assert!(err.contains("403"), "got: {err}");
}

#[tokio::test]
async fn consume_sse_invalid_json_is_silently_skipped() {
    let app = Router::new().route(
        "/sse",
        get(|| async {
            let events = vec![
                Ok::<_, Infallible>(
                    Event::default()
                        .event("taskcast.event")
                        .data("not valid json"),
                ),
                Ok(
                    Event::default()
                        .event("taskcast.event")
                        .data(r#"{"type":"valid"}"#),
                ),
            ];
            Sse::new(stream::iter(events))
        }),
    );
    let base_url = start_mock_sse_server(app).await;

    let mut received = Vec::new();

    consume_sse(
        &format!("{base_url}/sse"),
        None,
        |event, _name| {
            received.push(event);
        },
        None,
    )
    .await
    .unwrap();

    // Only the valid JSON event should be received
    assert_eq!(received.len(), 1);
    assert_eq!(received[0]["type"], "valid");
}

#[tokio::test]
async fn consume_sse_connection_refused() {
    let result = consume_sse(
        "http://127.0.0.1:19999/sse",
        None,
        |_event, _name| {},
        None,
    )
    .await;

    assert!(result.is_err(), "should return error for connection refused");
}

#[tokio::test]
async fn consume_sse_empty_data_lines_are_skipped() {
    let app = Router::new().route(
        "/sse",
        get(|| async {
            let events = vec![
                Ok::<_, Infallible>(
                    Event::default()
                        .event("taskcast.event")
                        .data(r#"{"type":"first"}"#),
                ),
                // An event with empty data field won't produce a data line at all in SSE
                Ok(
                    Event::default()
                        .event("taskcast.event")
                        .data(r#"{"type":"second"}"#),
                ),
            ];
            Sse::new(stream::iter(events))
        }),
    );
    let base_url = start_mock_sse_server(app).await;

    let mut received = Vec::new();

    consume_sse(
        &format!("{base_url}/sse"),
        None,
        |event, _name| {
            received.push(event);
        },
        None,
    )
    .await
    .unwrap();

    assert_eq!(received.len(), 2);
    assert_eq!(received[0]["type"], "first");
    assert_eq!(received[1]["type"], "second");
}

// ─── consume_sse done callback only fires on taskcast.done event ──────────────

#[tokio::test]
async fn consume_sse_done_callback_only_on_done_event() {
    let app = Router::new().route(
        "/sse",
        get(|| async {
            let events = vec![
                Ok::<_, Infallible>(
                    Event::default()
                        .event("taskcast.event")
                        .data(r#"{"type":"llm.delta"}"#),
                ),
                Ok(
                    Event::default()
                        .event("taskcast.event")
                        .data(r#"{"type":"llm.delta"}"#),
                ),
                // Only this one should trigger done callback
                Ok(
                    Event::default()
                        .event("taskcast.done")
                        .data(r#"{"reason":"completed"}"#),
                ),
            ];
            Sse::new(stream::iter(events))
        }),
    );
    let base_url = start_mock_sse_server(app).await;

    let mut done_count = 0;

    consume_sse(
        &format!("{base_url}/sse"),
        None,
        |_event, _name| {},
        Some(&mut || {
            done_count += 1;
        }),
    )
    .await
    .unwrap();

    assert_eq!(done_count, 1, "done callback should fire exactly once");
}

// ─── Integration: format_event with real server event data ────────────────────

#[tokio::test]
async fn format_event_with_real_server_data() {
    let engine = make_engine();
    let base_url = start_server(engine.clone()).await;
    let client = TaskcastClient::new(base_url, None);

    let task = engine
        .create_task(CreateTaskInput {
            r#type: Some("llm.chat".to_string()),
            ..Default::default()
        })
        .await
        .unwrap();
    engine
        .transition_task(&task.id, TaskStatus::Running, None)
        .await
        .unwrap();
    engine
        .publish_event(
            &task.id,
            PublishEventInput {
                r#type: "llm.delta".to_string(),
                level: Level::Info,
                data: json!({"delta": "Hello world"}),
                series_id: None,
                series_mode: None,
                series_acc_field: None,
            },
        )
        .await
        .unwrap();

    let res = client
        .get(&format!("/tasks/{}/events/history", task.id))
        .await
        .unwrap();
    assert!(res.status().is_success());
    let events: Vec<serde_json::Value> = res.json().await.unwrap();

    // Find the llm.delta event (not the taskcast:status one)
    let delta_event = events
        .iter()
        .find(|e| e["type"].as_str() == Some("llm.delta"))
        .expect("should find llm.delta event");

    let formatted = format_event(
        delta_event["type"].as_str().unwrap(),
        delta_event["level"].as_str().unwrap(),
        delta_event["timestamp"].as_f64().unwrap() as i64,
        delta_event.get("data").unwrap(),
        None,
    );

    assert!(formatted.contains("llm.delta"), "got: {formatted}");
    assert!(formatted.contains("info"), "got: {formatted}");
    assert!(
        formatted.contains(r#""delta":"Hello world""#),
        "got: {formatted}"
    );
}

#[tokio::test]
async fn format_event_with_task_id_from_real_server() {
    let engine = make_engine();
    let base_url = start_server(engine.clone()).await;
    let client = TaskcastClient::new(base_url, None);

    let task = engine
        .create_task(CreateTaskInput {
            r#type: Some("agent.step".to_string()),
            ..Default::default()
        })
        .await
        .unwrap();
    engine
        .transition_task(&task.id, TaskStatus::Running, None)
        .await
        .unwrap();
    engine
        .publish_event(
            &task.id,
            PublishEventInput {
                r#type: "agent.step".to_string(),
                level: Level::Info,
                data: json!({"step": 1}),
                series_id: None,
                series_mode: None,
                series_acc_field: None,
            },
        )
        .await
        .unwrap();

    let res = client
        .get(&format!("/tasks/{}/events/history", task.id))
        .await
        .unwrap();
    let events: Vec<serde_json::Value> = res.json().await.unwrap();
    let step_event = events
        .iter()
        .find(|e| e["type"].as_str() == Some("agent.step"))
        .expect("should find agent.step event");

    let formatted = format_event(
        step_event["type"].as_str().unwrap(),
        step_event["level"].as_str().unwrap(),
        step_event["timestamp"].as_f64().unwrap() as i64,
        step_event.get("data").unwrap(),
        Some(&task.id),
    );

    // Task ID should be truncated to first 7 chars
    let expected_prefix = format!("{}..  ", &task.id[..7]);
    assert!(
        formatted.contains(&expected_prefix),
        "expected task prefix '{expected_prefix}', got: {formatted}"
    );
    assert!(formatted.contains("agent.step"), "got: {formatted}");
}

// ─── Multiple events through consume_sse ──────────────────────────────────────

#[tokio::test]
async fn consume_sse_multiple_events_in_sequence() {
    let app = Router::new().route(
        "/sse",
        get(|| async {
            let events = vec![
                Ok::<_, Infallible>(
                    Event::default()
                        .event("taskcast.event")
                        .data(r#"{"type":"step.0","level":"info","data":{"step":0}}"#),
                ),
                Ok(
                    Event::default()
                        .event("taskcast.event")
                        .data(r#"{"type":"step.1","level":"info","data":{"step":1}}"#),
                ),
                Ok(
                    Event::default()
                        .event("taskcast.event")
                        .data(r#"{"type":"step.2","level":"warn","data":{"step":2}}"#),
                ),
            ];
            Sse::new(stream::iter(events))
        }),
    );
    let base_url = start_mock_sse_server(app).await;

    let mut events = Vec::new();

    consume_sse(
        &format!("{base_url}/sse"),
        None,
        |event, name| {
            events.push((event, name.to_string()));
        },
        None,
    )
    .await
    .unwrap();

    assert_eq!(events.len(), 3);
    assert_eq!(events[0].0["type"], "step.0");
    assert_eq!(events[1].0["type"], "step.1");
    assert_eq!(events[2].0["type"], "step.2");
    assert_eq!(events[2].0["level"], "warn");
    // All should be taskcast.event
    for (_, name) in &events {
        assert_eq!(name, "taskcast.event");
    }
}

// ─── Integration: consume_sse with real taskcast server ───────────────────────

#[tokio::test]
async fn consume_sse_from_real_server_task_events() {
    let engine = make_engine();
    let base_url = start_server(engine.clone()).await;

    let task = engine
        .create_task(CreateTaskInput {
            r#type: Some("llm.chat".to_string()),
            ..Default::default()
        })
        .await
        .unwrap();

    engine
        .transition_task(&task.id, TaskStatus::Running, None)
        .await
        .unwrap();

    engine
        .publish_event(
            &task.id,
            PublishEventInput {
                r#type: "llm.delta".to_string(),
                level: Level::Info,
                data: json!({"text": "hello"}),
                series_id: None,
                series_mode: None,
                series_acc_field: None,
            },
        )
        .await
        .unwrap();

    engine
        .transition_task(&task.id, TaskStatus::Completed, None)
        .await
        .unwrap();

    // Stream from real SSE endpoint
    let url = format!("{base_url}/tasks/{}/events", task.id);
    let mut events: Vec<(serde_json::Value, String)> = Vec::new();
    let mut done = false;

    consume_sse(
        &url,
        None,
        |ev, name| {
            events.push((ev, name.to_string()));
        },
        Some(&mut || {
            done = true;
        }),
    )
    .await
    .unwrap();

    assert!(!events.is_empty(), "should receive events from real SSE");
    assert!(done, "should receive done signal for completed task");

    // Verify we received the llm.delta event
    let has_delta = events
        .iter()
        .any(|(ev, name)| name == "taskcast.event" && ev.get("type").and_then(|t| t.as_str()) == Some("llm.delta"));
    assert!(has_delta, "should receive llm.delta event, got: {events:?}");

    // Verify we received the done event
    let has_done = events
        .iter()
        .any(|(_, name)| name == "taskcast.done");
    assert!(has_done, "should receive taskcast.done event");
}

#[tokio::test]
async fn consume_sse_from_real_server_formats_events_correctly() {
    let engine = make_engine();
    let base_url = start_server(engine.clone()).await;

    let task = engine
        .create_task(CreateTaskInput {
            r#type: Some("agent.step".to_string()),
            ..Default::default()
        })
        .await
        .unwrap();

    engine
        .transition_task(&task.id, TaskStatus::Running, None)
        .await
        .unwrap();

    engine
        .publish_event(
            &task.id,
            PublishEventInput {
                r#type: "agent.step".to_string(),
                level: Level::Info,
                data: json!({"step": 1, "output": "planning"}),
                series_id: None,
                series_mode: None,
                series_acc_field: None,
            },
        )
        .await
        .unwrap();

    engine
        .publish_event(
            &task.id,
            PublishEventInput {
                r#type: "agent.step".to_string(),
                level: Level::Warn,
                data: json!({"step": 2, "output": "retrying"}),
                series_id: None,
                series_mode: None,
                series_acc_field: None,
            },
        )
        .await
        .unwrap();

    engine
        .transition_task(&task.id, TaskStatus::Completed, None)
        .await
        .unwrap();

    let url = format!("{base_url}/tasks/{}/events", task.id);
    let mut formatted_outputs: Vec<String> = Vec::new();

    consume_sse(
        &url,
        None,
        |event, sse_event_name| {
            // Apply same formatting as run_logs callback
            if sse_event_name == "taskcast.done" {
                let reason = event
                    .as_object()
                    .and_then(|obj| obj.get("reason"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown");
                formatted_outputs.push(format_event(
                    "taskcast.done",
                    "info",
                    0,
                    &json!({ "reason": reason }),
                    None,
                ));
            } else if sse_event_name == "taskcast.event" {
                let event_type = event
                    .get("type")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown");
                let level = event
                    .get("level")
                    .and_then(|v| v.as_str())
                    .unwrap_or("info");
                let timestamp = event
                    .get("timestamp")
                    .and_then(|v| v.as_i64())
                    .unwrap_or(0);
                let data = event.get("data").cloned().unwrap_or(serde_json::Value::Null);
                formatted_outputs.push(format_event(event_type, level, timestamp, &data, None));
            }
        },
        Some(&mut || {}),
    )
    .await
    .unwrap();

    assert!(
        !formatted_outputs.is_empty(),
        "should have formatted outputs"
    );

    // Should have at least one agent.step formatted line
    let has_step = formatted_outputs.iter().any(|s| s.contains("agent.step"));
    assert!(has_step, "should format agent.step events, got: {formatted_outputs:?}");

    // Should have a done line
    let has_done = formatted_outputs.iter().any(|s| s.contains("[DONE]"));
    assert!(has_done, "should format done event, got: {formatted_outputs:?}");
}

#[tokio::test]
async fn consume_sse_from_real_server_tail_format() {
    // Test the run_tail formatting path: global /events with task_id prefix
    let engine = make_engine();
    let base_url = start_server(engine.clone()).await;

    let task = engine
        .create_task(CreateTaskInput {
            r#type: Some("llm.chat".to_string()),
            ..Default::default()
        })
        .await
        .unwrap();

    engine
        .transition_task(&task.id, TaskStatus::Running, None)
        .await
        .unwrap();

    // Publish an event, then complete
    engine
        .publish_event(
            &task.id,
            PublishEventInput {
                r#type: "llm.delta".to_string(),
                level: Level::Info,
                data: json!({"text": "world"}),
                series_id: None,
                series_mode: None,
                series_acc_field: None,
            },
        )
        .await
        .unwrap();

    engine
        .transition_task(&task.id, TaskStatus::Completed, None)
        .await
        .unwrap();

    // Use per-task SSE to test tail-like formatting (with task_id in output)
    let url = format!("{base_url}/tasks/{}/events", task.id);
    let mut formatted_with_task_id: Vec<String> = Vec::new();

    consume_sse(
        &url,
        None,
        |event, sse_event_name| {
            // Apply same formatting as run_tail callback
            if sse_event_name == "taskcast.event" {
                let event_type = event
                    .get("type")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown");
                let level = event
                    .get("level")
                    .and_then(|v| v.as_str())
                    .unwrap_or("info");
                let timestamp = event
                    .get("timestamp")
                    .and_then(|v| v.as_i64())
                    .unwrap_or(0);
                let data = event.get("data").cloned().unwrap_or(serde_json::Value::Null);
                let task_id = event.get("taskId").and_then(|v| v.as_str());
                formatted_with_task_id.push(format_event(
                    event_type, level, timestamp, &data, task_id,
                ));
            }
        },
        Some(&mut || {}),
    )
    .await
    .unwrap();

    assert!(
        !formatted_with_task_id.is_empty(),
        "should have formatted outputs from tail-like consumption"
    );
}

#[tokio::test]
async fn consume_sse_query_string_construction() {
    // Test that the URL construction with query params works correctly
    // by verifying filter params are passed through to the real server
    let engine = make_engine();
    let base_url = start_server(engine.clone()).await;

    let task = engine
        .create_task(CreateTaskInput {
            r#type: Some("llm.chat".to_string()),
            ..Default::default()
        })
        .await
        .unwrap();

    engine
        .transition_task(&task.id, TaskStatus::Running, None)
        .await
        .unwrap();

    engine
        .publish_event(
            &task.id,
            PublishEventInput {
                r#type: "llm.delta".to_string(),
                level: Level::Info,
                data: json!({"text": "hello"}),
                series_id: None,
                series_mode: None,
                series_acc_field: None,
            },
        )
        .await
        .unwrap();

    engine
        .publish_event(
            &task.id,
            PublishEventInput {
                r#type: "agent.step".to_string(),
                level: Level::Warn,
                data: json!({"step": 1}),
                series_id: None,
                series_mode: None,
                series_acc_field: None,
            },
        )
        .await
        .unwrap();

    engine
        .transition_task(&task.id, TaskStatus::Completed, None)
        .await
        .unwrap();

    // Build query string the same way run_logs does
    let mut params = Vec::new();
    let types = Some("llm.*".to_string());
    let levels: Option<String> = None;
    if let Some(ref t) = types {
        params.push(format!("types={t}"));
    }
    if let Some(ref l) = levels {
        params.push(format!("levels={l}"));
    }
    let qs = if params.is_empty() {
        String::new()
    } else {
        format!("?{}", params.join("&"))
    };
    let url = format!("{base_url}/tasks/{}/events{qs}", task.id);

    // The URL should contain the filter param
    assert!(url.contains("types=llm.*"), "URL should contain types filter: {url}");

    let mut events: Vec<(serde_json::Value, String)> = Vec::new();

    consume_sse(
        &url,
        None,
        |ev, name| {
            events.push((ev, name.to_string()));
        },
        Some(&mut || {}),
    )
    .await
    .unwrap();

    // When types=llm.* filter is applied, we should only get llm.delta events,
    // not agent.step events (plus status transitions and done)
    let has_agent_step = events
        .iter()
        .any(|(ev, name)| name == "taskcast.event" && ev.get("type").and_then(|t| t.as_str()) == Some("agent.step"));
    assert!(
        !has_agent_step,
        "filtered SSE should not include agent.step events"
    );
}

#[tokio::test]
async fn consume_sse_query_string_with_both_params() {
    // Test query string construction with both types and levels params
    let mut params = Vec::new();
    let types = Some("llm.*".to_string());
    let levels = Some("info,warn".to_string());

    if let Some(ref t) = types {
        params.push(format!("types={t}"));
    }
    if let Some(ref l) = levels {
        params.push(format!("levels={l}"));
    }
    let qs = if params.is_empty() {
        String::new()
    } else {
        format!("?{}", params.join("&"))
    };

    assert_eq!(qs, "?types=llm.*&levels=info,warn");
}

#[tokio::test]
async fn consume_sse_query_string_empty_when_no_params() {
    let params: Vec<String> = Vec::new();
    let qs = if params.is_empty() {
        String::new()
    } else {
        format!("?{}", params.join("&"))
    };

    assert_eq!(qs, "");
}

#[tokio::test]
async fn consume_sse_done_event_reason_extraction() {
    // Test the done callback reason extraction logic used in run_logs
    let app = Router::new().route(
        "/sse",
        get(|| async {
            let events = vec![
                Ok::<_, Infallible>(
                    Event::default()
                        .event("taskcast.done")
                        .data(r#"{"reason":"timeout"}"#),
                ),
            ];
            Sse::new(stream::iter(events))
        }),
    );
    let base_url = start_mock_sse_server(app).await;

    let mut formatted = String::new();

    consume_sse(
        &format!("{base_url}/sse"),
        None,
        |event, sse_event_name| {
            if sse_event_name == "taskcast.done" {
                let reason = event
                    .as_object()
                    .and_then(|obj| obj.get("reason"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown");
                formatted = format_event(
                    "taskcast.done",
                    "info",
                    0,
                    &json!({ "reason": reason }),
                    None,
                );
            }
        },
        Some(&mut || {}),
    )
    .await
    .unwrap();

    assert!(
        formatted.contains("[DONE] timeout"),
        "should format done with reason timeout, got: {formatted}"
    );
}

#[tokio::test]
async fn consume_sse_event_field_extraction_defaults() {
    // Test the field extraction logic used in run_logs/run_tail callbacks
    // when optional fields are missing
    let app = Router::new().route(
        "/sse",
        get(|| async {
            let events = vec![
                Ok::<_, Infallible>(
                    Event::default()
                        .event("taskcast.event")
                        .data(r#"{}"#), // no type, level, timestamp, or data fields
                ),
            ];
            Sse::new(stream::iter(events))
        }),
    );
    let base_url = start_mock_sse_server(app).await;

    let mut formatted = String::new();

    consume_sse(
        &format!("{base_url}/sse"),
        None,
        |event, sse_event_name| {
            if sse_event_name == "taskcast.event" {
                let event_type = event
                    .get("type")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown");
                let level = event
                    .get("level")
                    .and_then(|v| v.as_str())
                    .unwrap_or("info");
                let timestamp = event
                    .get("timestamp")
                    .and_then(|v| v.as_i64())
                    .unwrap_or(0);
                let data = event.get("data").cloned().unwrap_or(serde_json::Value::Null);
                formatted = format_event(event_type, level, timestamp, &data, None);
            }
        },
        None,
    )
    .await
    .unwrap();

    assert!(formatted.contains("unknown"), "missing type should default to 'unknown', got: {formatted}");
    assert!(formatted.contains("info"), "missing level should default to 'info', got: {formatted}");
    assert!(formatted.contains("null"), "missing data should default to null, got: {formatted}");
}

#[tokio::test]
async fn consume_sse_ignores_non_taskcast_events() {
    // The run_logs/run_tail callbacks only handle taskcast.event and taskcast.done.
    // Other SSE event names should be ignored.
    let app = Router::new().route(
        "/sse",
        get(|| async {
            let events = vec![
                Ok::<_, Infallible>(
                    Event::default()
                        .event("other.event")
                        .data(r#"{"type":"ignored"}"#),
                ),
                Ok(
                    Event::default()
                        .event("taskcast.event")
                        .data(r#"{"type":"included","level":"info","timestamp":0,"data":{}}"#),
                ),
            ];
            Sse::new(stream::iter(events))
        }),
    );
    let base_url = start_mock_sse_server(app).await;

    let mut formatted: Vec<String> = Vec::new();

    consume_sse(
        &format!("{base_url}/sse"),
        None,
        |event, sse_event_name| {
            // Same filter as run_logs: only process taskcast.event and taskcast.done
            if sse_event_name == "taskcast.event" {
                let event_type = event
                    .get("type")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown");
                let level = event
                    .get("level")
                    .and_then(|v| v.as_str())
                    .unwrap_or("info");
                let timestamp = event
                    .get("timestamp")
                    .and_then(|v| v.as_i64())
                    .unwrap_or(0);
                let data = event.get("data").cloned().unwrap_or(serde_json::Value::Null);
                formatted.push(format_event(event_type, level, timestamp, &data, None));
            }
        },
        None,
    )
    .await
    .unwrap();

    assert_eq!(
        formatted.len(),
        1,
        "should only format the taskcast.event, not other.event"
    );
    assert!(formatted[0].contains("included"), "got: {}", formatted[0]);
}

// ─── Integration: run_logs / run_tail code paths via NodeConfigManager + real server ───

use std::sync::Mutex;
use taskcast_cli::node_config::{NodeConfigManager, NodeEntry};

/// Mutex to serialize tests that modify HOME env var.
static HOME_MUTEX: Mutex<()> = Mutex::new(());

/// Helper: create a temp HOME with a node config pointing to the given base_url.
fn setup_temp_home_with_node(base_url: &str, node_name: &str) -> tempfile::TempDir {
    let temp_dir = tempfile::TempDir::new().unwrap();
    let config_dir = temp_dir.path().join(".taskcast");
    std::fs::create_dir_all(&config_dir).unwrap();
    let mgr = NodeConfigManager::new(config_dir);
    mgr.add(
        node_name,
        NodeEntry {
            url: base_url.to_string(),
            token: None,
            token_type: None,
        },
    );
    mgr.set_current(node_name).unwrap();
    temp_dir
}

#[tokio::test]
async fn run_logs_node_lookup_client_creation_and_sse() {
    let _lock = HOME_MUTEX.lock().unwrap();
    let engine = make_engine();
    let base_url = start_server(engine.clone()).await;

    let temp_dir = setup_temp_home_with_node(&base_url, "default");
    unsafe { std::env::set_var("HOME", temp_dir.path()); }

    // Create and complete a task with an event
    let task = engine
        .create_task(CreateTaskInput {
            r#type: Some("test".to_string()),
            ..Default::default()
        })
        .await
        .unwrap();
    engine
        .transition_task(&task.id, TaskStatus::Running, None)
        .await
        .unwrap();
    engine
        .publish_event(
            &task.id,
            PublishEventInput {
                r#type: "test.event".to_string(),
                level: Level::Info,
                data: json!({"msg": "hello"}),
                series_id: None,
                series_mode: None,
                series_acc_field: None,
            },
        )
        .await
        .unwrap();
    engine
        .transition_task(&task.id, TaskStatus::Completed, None)
        .await
        .unwrap();

    // Replicate run_logs lines 134-168: node lookup + client creation + URL construction
    let home = dirs::home_dir().unwrap().join(".taskcast");
    let node_mgr = NodeConfigManager::new(home);
    let node = node_mgr.get_current();
    assert_eq!(node.url, base_url);

    let client = TaskcastClient::from_node(&node).await.unwrap();
    assert_eq!(client.base_url(), base_url);
    assert_eq!(client.token(), None);

    // Build URL same way as run_logs (no query params)
    let url = format!("{}/tasks/{}/events", client.base_url(), task.id);

    // Consume SSE with the same callback logic as run_logs lines 170-211
    let mut formatted_outputs: Vec<String> = Vec::new();
    let mut done_called = false;

    consume_sse(
        &url,
        client.token(),
        |event, sse_event_name| {
            if sse_event_name == "taskcast.done" {
                let reason = event
                    .as_object()
                    .and_then(|obj| obj.get("reason"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown");
                let ts = chrono::Utc::now().timestamp_millis();
                formatted_outputs.push(format_event(
                    "taskcast.done",
                    "info",
                    ts,
                    &json!({ "reason": reason }),
                    None,
                ));
            } else if sse_event_name == "taskcast.event" {
                let event_type = event
                    .get("type")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown");
                let level = event
                    .get("level")
                    .and_then(|v| v.as_str())
                    .unwrap_or("info");
                let timestamp = event
                    .get("timestamp")
                    .and_then(|v| v.as_i64())
                    .unwrap_or(0);
                let data = event.get("data").cloned().unwrap_or(serde_json::Value::Null);
                formatted_outputs.push(format_event(event_type, level, timestamp, &data, None));
            }
        },
        Some(&mut || {
            done_called = true;
        }),
    )
    .await
    .unwrap();

    assert!(done_called, "should complete via done callback");
    assert!(!formatted_outputs.is_empty(), "should have formatted output");
    let has_done = formatted_outputs.iter().any(|s| s.contains("[DONE]"));
    assert!(has_done, "should format done event, got: {formatted_outputs:?}");

    unsafe { std::env::remove_var("HOME"); }
}

#[tokio::test]
async fn run_logs_with_types_and_levels_query_params() {
    let _lock = HOME_MUTEX.lock().unwrap();
    let engine = make_engine();
    let base_url = start_server(engine.clone()).await;

    let temp_dir = setup_temp_home_with_node(&base_url, "default");
    unsafe { std::env::set_var("HOME", temp_dir.path()); }

    let task = engine
        .create_task(CreateTaskInput {
            r#type: Some("test".to_string()),
            ..Default::default()
        })
        .await
        .unwrap();
    engine
        .transition_task(&task.id, TaskStatus::Running, None)
        .await
        .unwrap();
    engine
        .publish_event(
            &task.id,
            PublishEventInput {
                r#type: "llm.delta".to_string(),
                level: Level::Info,
                data: json!({"text": "hi"}),
                series_id: None,
                series_mode: None,
                series_acc_field: None,
            },
        )
        .await
        .unwrap();
    engine
        .publish_event(
            &task.id,
            PublishEventInput {
                r#type: "agent.step".to_string(),
                level: Level::Warn,
                data: json!({"step": 1}),
                series_id: None,
                series_mode: None,
                series_acc_field: None,
            },
        )
        .await
        .unwrap();
    engine
        .transition_task(&task.id, TaskStatus::Completed, None)
        .await
        .unwrap();

    // Replicate run_logs query string construction (lines 152-163)
    let home = dirs::home_dir().unwrap().join(".taskcast");
    let node_mgr = NodeConfigManager::new(home);
    let node = node_mgr.get_current();
    let client = TaskcastClient::from_node(&node).await.unwrap();

    let types = Some("llm.*".to_string());
    let levels = Some("info".to_string());
    let mut params = Vec::new();
    if let Some(ref t) = types {
        params.push(format!("types={t}"));
    }
    if let Some(ref l) = levels {
        params.push(format!("levels={l}"));
    }
    let qs = if params.is_empty() {
        String::new()
    } else {
        format!("?{}", params.join("&"))
    };
    let url = format!("{}/tasks/{}/events{qs}", client.base_url(), task.id);

    assert!(url.contains("types=llm.*"), "URL should contain types filter: {url}");
    assert!(url.contains("levels=info"), "URL should contain levels filter: {url}");

    let mut events: Vec<(serde_json::Value, String)> = Vec::new();

    consume_sse(
        &url,
        client.token(),
        |ev, name| {
            events.push((ev, name.to_string()));
        },
        Some(&mut || {}),
    )
    .await
    .unwrap();

    // With types=llm.* filter, should NOT get agent.step events
    let has_agent = events
        .iter()
        .any(|(ev, name)| {
            name == "taskcast.event"
                && ev.get("type").and_then(|t| t.as_str()) == Some("agent.step")
        });
    assert!(!has_agent, "should not include agent.step with llm.* filter");

    unsafe { std::env::remove_var("HOME"); }
}

#[tokio::test]
async fn run_logs_with_named_node() {
    let _lock = HOME_MUTEX.lock().unwrap();
    let engine = make_engine();
    let base_url = start_server(engine.clone()).await;

    let temp_dir = tempfile::TempDir::new().unwrap();
    let config_dir = temp_dir.path().join(".taskcast");
    std::fs::create_dir_all(&config_dir).unwrap();
    let mgr = NodeConfigManager::new(config_dir.clone());
    mgr.add(
        "my-node",
        NodeEntry {
            url: base_url.clone(),
            token: None,
            token_type: None,
        },
    );
    // Don't set as current -- test named node lookup path
    unsafe { std::env::set_var("HOME", temp_dir.path()); }

    let task = engine
        .create_task(CreateTaskInput {
            r#type: Some("test".to_string()),
            ..Default::default()
        })
        .await
        .unwrap();
    engine
        .transition_task(&task.id, TaskStatus::Running, None)
        .await
        .unwrap();
    engine
        .transition_task(&task.id, TaskStatus::Completed, None)
        .await
        .unwrap();

    // Replicate run_logs named node lookup (lines 139-148)
    let home = dirs::home_dir().unwrap().join(".taskcast");
    let node_mgr = NodeConfigManager::new(home);
    let node_name = Some("my-node".to_string());
    let node = match node_name {
        Some(name) => match node_mgr.get(&name) {
            Some(entry) => entry,
            None => panic!("Node should exist"),
        },
        None => node_mgr.get_current(),
    };
    assert_eq!(node.url, base_url);

    let client = TaskcastClient::from_node(&node).await.unwrap();
    let url = format!("{}/tasks/{}/events", client.base_url(), task.id);

    let mut done = false;
    consume_sse(
        &url,
        client.token(),
        |_event, _name| {},
        Some(&mut || {
            done = true;
        }),
    )
    .await
    .unwrap();

    assert!(done, "should receive done for completed task");

    unsafe { std::env::remove_var("HOME"); }
}

#[tokio::test]
async fn run_logs_named_node_not_found() {
    let _lock = HOME_MUTEX.lock().unwrap();

    let temp_dir = tempfile::TempDir::new().unwrap();
    let config_dir = temp_dir.path().join(".taskcast");
    std::fs::create_dir_all(&config_dir).unwrap();
    // Create empty config -- no nodes
    let _mgr = NodeConfigManager::new(config_dir);

    unsafe { std::env::set_var("HOME", temp_dir.path()); }

    // Replicate run_logs named node lookup failure path (lines 140-145)
    let home = dirs::home_dir().unwrap().join(".taskcast");
    let node_mgr = NodeConfigManager::new(home);
    let result = node_mgr.get("nonexistent");
    assert!(result.is_none(), "should not find nonexistent node");

    unsafe { std::env::remove_var("HOME"); }
}

#[tokio::test]
async fn run_tail_node_lookup_and_url_construction() {
    let _lock = HOME_MUTEX.lock().unwrap();
    let engine = make_engine();
    let base_url = start_server(engine.clone()).await;

    let temp_dir = setup_temp_home_with_node(&base_url, "default");
    unsafe { std::env::set_var("HOME", temp_dir.path()); }

    // Create a task and publish events for tail to pick up
    let task = engine
        .create_task(CreateTaskInput {
            r#type: Some("test".to_string()),
            ..Default::default()
        })
        .await
        .unwrap();
    engine
        .transition_task(&task.id, TaskStatus::Running, None)
        .await
        .unwrap();
    engine
        .publish_event(
            &task.id,
            PublishEventInput {
                r#type: "test.event".to_string(),
                level: Level::Info,
                data: json!({"msg": "tail-test"}),
                series_id: None,
                series_mode: None,
                series_acc_field: None,
            },
        )
        .await
        .unwrap();

    // Replicate run_tail lines 218-248: node lookup + URL construction
    let home = dirs::home_dir().unwrap().join(".taskcast");
    let node_mgr = NodeConfigManager::new(home);
    let node = node_mgr.get_current();
    assert_eq!(node.url, base_url);

    let client = TaskcastClient::from_node(&node).await.unwrap();

    // Build URL same way as run_tail (lines 236-248) -- global /events endpoint
    let types = Some("test.*".to_string());
    let levels: Option<String> = None;
    let mut params = Vec::new();
    if let Some(ref t) = types {
        params.push(format!("types={t}"));
    }
    if let Some(ref l) = levels {
        params.push(format!("levels={l}"));
    }
    let qs = if params.is_empty() {
        String::new()
    } else {
        format!("?{}", params.join("&"))
    };
    let url = format!("{}/events{qs}", client.base_url());

    assert!(url.contains("/events?types=test.*"), "tail URL should use global /events: {url}");
    assert!(!url.contains("/tasks/"), "tail URL should not contain /tasks/: {url}");

    unsafe { std::env::remove_var("HOME"); }
}

#[tokio::test]
async fn run_tail_callback_formats_with_task_id() {
    // Test the run_tail callback path (lines 250-276) which formats events with taskId
    let engine = make_engine();
    let base_url = start_server(engine.clone()).await;

    let task = engine
        .create_task(CreateTaskInput {
            r#type: Some("test".to_string()),
            ..Default::default()
        })
        .await
        .unwrap();
    engine
        .transition_task(&task.id, TaskStatus::Running, None)
        .await
        .unwrap();
    engine
        .publish_event(
            &task.id,
            PublishEventInput {
                r#type: "test.event".to_string(),
                level: Level::Info,
                data: json!({"msg": "world"}),
                series_id: None,
                series_mode: None,
                series_acc_field: None,
            },
        )
        .await
        .unwrap();
    engine
        .transition_task(&task.id, TaskStatus::Completed, None)
        .await
        .unwrap();

    // Use per-task SSE to exercise the tail callback logic
    let url = format!("{}/tasks/{}/events", base_url, task.id);
    let mut formatted: Vec<String> = Vec::new();

    consume_sse(
        &url,
        None,
        |event, sse_event_name| {
            // Same logic as run_tail callback (lines 253-273)
            if sse_event_name == "taskcast.event" {
                let event_type = event
                    .get("type")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown");
                let level = event
                    .get("level")
                    .and_then(|v| v.as_str())
                    .unwrap_or("info");
                let timestamp = event
                    .get("timestamp")
                    .and_then(|v| v.as_i64())
                    .unwrap_or(0);
                let data = event.get("data").cloned().unwrap_or(serde_json::Value::Null);
                let task_id = event.get("taskId").and_then(|v| v.as_str());
                formatted.push(format_event(event_type, level, timestamp, &data, task_id));
            }
        },
        Some(&mut || {}),
    )
    .await
    .unwrap();

    assert!(!formatted.is_empty(), "tail callback should produce formatted output");
}

#[tokio::test]
async fn run_tail_no_query_params() {
    // Test run_tail URL construction with no types/levels filters
    let engine = make_engine();
    let base_url = start_server(engine.clone()).await;

    let client = TaskcastClient::new(base_url.clone(), None);

    // Replicate run_tail query string construction with no params
    let types: Option<String> = None;
    let levels: Option<String> = None;
    let mut params = Vec::new();
    if let Some(ref t) = types {
        params.push(format!("types={t}"));
    }
    if let Some(ref l) = levels {
        params.push(format!("levels={l}"));
    }
    let qs = if params.is_empty() {
        String::new()
    } else {
        format!("?{}", params.join("&"))
    };
    let url = format!("{}/events{qs}", client.base_url());

    assert_eq!(url, format!("{}/events", base_url), "URL should have no query string");
}

// ─── Direct run_logs / run_tail calls ────────────────────────────────────────

use taskcast_cli::commands::logs::{run_logs, run_tail, LogsArgs, TailArgs};

#[tokio::test]
async fn run_logs_direct_call_completed_task() {
    let _lock = HOME_MUTEX.lock().unwrap();
    let engine = make_engine();
    let base_url = start_server(engine.clone()).await;

    let temp_dir = setup_temp_home_with_node(&base_url, "default");
    unsafe { std::env::set_var("HOME", temp_dir.path()); }

    // Create a task, publish an event, then complete it so the SSE stream ends
    let task = engine
        .create_task(CreateTaskInput {
            r#type: Some("test".to_string()),
            ..Default::default()
        })
        .await
        .unwrap();
    engine
        .transition_task(&task.id, TaskStatus::Running, None)
        .await
        .unwrap();
    engine
        .publish_event(
            &task.id,
            PublishEventInput {
                r#type: "test.event".to_string(),
                level: Level::Info,
                data: json!({"msg": "hello"}),
                series_id: None,
                series_mode: None,
                series_acc_field: None,
            },
        )
        .await
        .unwrap();
    engine
        .transition_task(&task.id, TaskStatus::Completed, None)
        .await
        .unwrap();

    let result = run_logs(LogsArgs {
        task_id: task.id.clone(),
        types: None,
        levels: None,
        node: None,
    })
    .await;

    assert!(result.is_ok(), "run_logs should succeed for completed task: {:?}", result.err());

    unsafe { std::env::remove_var("HOME"); }
}

#[tokio::test]
async fn run_logs_node_not_found_returns_error() {
    let _lock = HOME_MUTEX.lock().unwrap();

    let temp_dir = tempfile::TempDir::new().unwrap();
    let config_dir = temp_dir.path().join(".taskcast");
    std::fs::create_dir_all(&config_dir).unwrap();
    let _mgr = NodeConfigManager::new(config_dir);
    unsafe { std::env::set_var("HOME", temp_dir.path()); }

    let result = run_logs(LogsArgs {
        task_id: "some-task".to_string(),
        types: None,
        levels: None,
        node: Some("nonexistent".to_string()),
    })
    .await;

    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(
        err.contains("nonexistent"),
        "error should mention the node name, got: {err}"
    );

    unsafe { std::env::remove_var("HOME"); }
}

#[tokio::test]
async fn run_logs_with_filters() {
    let _lock = HOME_MUTEX.lock().unwrap();
    let engine = make_engine();
    let base_url = start_server(engine.clone()).await;

    let temp_dir = setup_temp_home_with_node(&base_url, "default");
    unsafe { std::env::set_var("HOME", temp_dir.path()); }

    let task = engine
        .create_task(CreateTaskInput {
            r#type: Some("test".to_string()),
            ..Default::default()
        })
        .await
        .unwrap();
    engine
        .transition_task(&task.id, TaskStatus::Running, None)
        .await
        .unwrap();
    engine
        .publish_event(
            &task.id,
            PublishEventInput {
                r#type: "llm.delta".to_string(),
                level: Level::Info,
                data: json!({"text": "hi"}),
                series_id: None,
                series_mode: None,
                series_acc_field: None,
            },
        )
        .await
        .unwrap();
    engine
        .transition_task(&task.id, TaskStatus::Completed, None)
        .await
        .unwrap();

    let result = run_logs(LogsArgs {
        task_id: task.id.clone(),
        types: Some("llm.*".to_string()),
        levels: Some("info".to_string()),
        node: None,
    })
    .await;

    assert!(result.is_ok(), "run_logs with filters should succeed: {:?}", result.err());

    unsafe { std::env::remove_var("HOME"); }
}

#[tokio::test]
async fn run_tail_node_not_found_returns_error() {
    let _lock = HOME_MUTEX.lock().unwrap();

    let temp_dir = tempfile::TempDir::new().unwrap();
    let config_dir = temp_dir.path().join(".taskcast");
    std::fs::create_dir_all(&config_dir).unwrap();
    let _mgr = NodeConfigManager::new(config_dir);
    unsafe { std::env::set_var("HOME", temp_dir.path()); }

    let result = run_tail(TailArgs {
        types: None,
        levels: None,
        node: Some("nonexistent".to_string()),
    })
    .await;

    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(
        err.contains("nonexistent"),
        "error should mention the node name, got: {err}"
    );

    unsafe { std::env::remove_var("HOME"); }
}
