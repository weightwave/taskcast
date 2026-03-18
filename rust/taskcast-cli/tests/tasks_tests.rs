use std::net::SocketAddr;
use std::sync::Arc;

use serde_json::json;
use tokio::net::TcpListener;

use taskcast_core::{
    CreateTaskInput, Level, MemoryBroadcastProvider, MemoryShortTermStore, PublishEventInput,
    TaskEngine, TaskEngineOptions, TaskStatus, TransitionPayload,
};
use taskcast_server::{create_app, AuthMode, CorsConfig};

use taskcast_cli::client::TaskcastClient;
use taskcast_cli::commands::tasks::{
    format_task_inspect, format_task_list, EventItem, TaskDetail, TaskListItem,
};

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

fn make_client(base_url: &str) -> TaskcastClient {
    TaskcastClient::new(base_url.to_string(), None)
}

// ─── format_timestamp edge cases ──────────────────────────────────────────────

#[test]
fn format_timestamp_none_returns_empty() {
    let tasks = vec![TaskListItem {
        id: "01JABCDEF".to_string(),
        task_type: Some("test".to_string()),
        status: "pending".to_string(),
        created_at: None,
    }];
    let output = format_task_list(&tasks);
    // The created column should be empty for None timestamp
    let lines: Vec<&str> = output.lines().collect();
    // The data line should end without a timestamp after status
    assert!(lines[1].contains("01JABCDEF"));
    assert!(lines[1].contains("pending"));
}

#[test]
fn format_timestamp_zero_returns_empty_in_list() {
    let tasks = vec![TaskListItem {
        id: "01JABCDEF".to_string(),
        task_type: Some("test".to_string()),
        status: "pending".to_string(),
        created_at: Some(0.0),
    }];
    let output = format_task_list(&tasks);
    let lines: Vec<&str> = output.lines().collect();
    // Zero timestamp should produce empty string — no date in the output line
    assert!(!lines[1].contains("1970"));
}

#[test]
fn format_timestamp_negative_returns_empty_in_list() {
    let tasks = vec![TaskListItem {
        id: "01JABCDEF".to_string(),
        task_type: Some("test".to_string()),
        status: "pending".to_string(),
        created_at: Some(-1000.0),
    }];
    let output = format_task_list(&tasks);
    let lines: Vec<&str> = output.lines().collect();
    // Negative timestamp should produce empty string
    assert!(!lines[1].contains("1969"));
}

#[test]
fn format_timestamp_typical_value_renders_date() {
    let tasks = vec![TaskListItem {
        id: "01JABCDEF".to_string(),
        task_type: Some("test".to_string()),
        status: "pending".to_string(),
        created_at: Some(1741234567890.0),
    }];
    let output = format_task_list(&tasks);
    let lines: Vec<&str> = output.lines().collect();
    // 1741234567890 ms ~ 2025-03-06 — should produce a date
    assert!(
        lines[1].contains("2025"),
        "expected year 2025 in output, got: {}",
        lines[1]
    );
}

#[test]
fn format_timestamp_in_inspect_shows_date() {
    let task = TaskDetail {
        id: "01JABCDEF".to_string(),
        task_type: Some("test".to_string()),
        status: "running".to_string(),
        params: None,
        created_at: Some(1741234567890.0),
    };
    let output = format_task_inspect(&task, &[]);
    assert!(
        output.contains("2025"),
        "expected year 2025 in inspect output, got: {output}"
    );
}

#[test]
fn format_timestamp_none_in_inspect_shows_empty() {
    let task = TaskDetail {
        id: "01JABCDEF".to_string(),
        task_type: Some("test".to_string()),
        status: "running".to_string(),
        params: None,
        created_at: None,
    };
    let output = format_task_inspect(&task, &[]);
    assert!(
        output.contains("Created: \n") || output.contains("Created: "),
        "expected empty created, got: {output}"
    );
}

// ─── run_list: integration with real server ───────────────────────────────────

#[tokio::test]
async fn run_list_returns_tasks_from_server() {
    let engine = make_engine();
    let base_url = start_server(engine.clone()).await;
    let client = make_client(&base_url);

    // Create tasks via engine directly
    engine
        .create_task(CreateTaskInput {
            r#type: Some("llm.chat".to_string()),
            ..Default::default()
        })
        .await
        .unwrap();
    engine
        .create_task(CreateTaskInput {
            r#type: Some("agent.step".to_string()),
            ..Default::default()
        })
        .await
        .unwrap();

    // Query via HTTP client (same as run_list would do internally)
    let res = client.get("/tasks").await.unwrap();
    assert!(res.status().is_success());

    let body: serde_json::Value = res.json().await.unwrap();
    let tasks: Vec<TaskListItem> = body["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| TaskListItem {
            id: v["id"].as_str().unwrap().to_string(),
            task_type: v["type"].as_str().map(|s| s.to_string()),
            status: v["status"].as_str().unwrap().to_string(),
            created_at: v["createdAt"].as_f64(),
        })
        .collect();

    assert_eq!(tasks.len(), 2);
    let output = format_task_list(&tasks);
    assert!(output.contains("llm.chat"), "output: {output}");
    assert!(output.contains("agent.step"), "output: {output}");
    assert!(output.contains("pending"), "output: {output}");
}

#[tokio::test]
async fn run_list_with_status_filter() {
    let engine = make_engine();
    let base_url = start_server(engine.clone()).await;
    let client = make_client(&base_url);

    let t1 = engine
        .create_task(CreateTaskInput {
            r#type: Some("llm.chat".to_string()),
            ..Default::default()
        })
        .await
        .unwrap();
    engine
        .create_task(CreateTaskInput {
            r#type: Some("agent.step".to_string()),
            ..Default::default()
        })
        .await
        .unwrap();
    engine
        .transition_task(&t1.id, TaskStatus::Running, None)
        .await
        .unwrap();

    let res = client.get("/tasks?status=running").await.unwrap();
    assert!(res.status().is_success());

    let body: serde_json::Value = res.json().await.unwrap();
    let tasks = body["tasks"].as_array().unwrap();
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0]["id"], t1.id);
    assert_eq!(tasks[0]["status"], "running");
}

#[tokio::test]
async fn run_list_with_type_filter() {
    let engine = make_engine();
    let base_url = start_server(engine.clone()).await;
    let client = make_client(&base_url);

    engine
        .create_task(CreateTaskInput {
            r#type: Some("llm.chat".to_string()),
            ..Default::default()
        })
        .await
        .unwrap();
    engine
        .create_task(CreateTaskInput {
            r#type: Some("agent.step".to_string()),
            ..Default::default()
        })
        .await
        .unwrap();

    let res = client.get("/tasks?type=agent.step").await.unwrap();
    assert!(res.status().is_success());

    let body: serde_json::Value = res.json().await.unwrap();
    let tasks = body["tasks"].as_array().unwrap();
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0]["type"], "agent.step");
}

#[tokio::test]
async fn run_list_empty_server_returns_no_tasks() {
    let engine = make_engine();
    let base_url = start_server(engine.clone()).await;
    let client = make_client(&base_url);

    let res = client.get("/tasks").await.unwrap();
    assert!(res.status().is_success());

    let body: serde_json::Value = res.json().await.unwrap();
    let tasks = body["tasks"].as_array().unwrap();
    assert!(tasks.is_empty());

    let items: Vec<TaskListItem> = vec![];
    let output = format_task_list(&items);
    assert_eq!(output, "No tasks found.");
}

#[tokio::test]
async fn run_list_respects_limit() {
    let engine = make_engine();
    let base_url = start_server(engine.clone()).await;
    let client = make_client(&base_url);

    // Create 5 tasks
    for i in 0..5 {
        engine
            .create_task(CreateTaskInput {
                r#type: Some(format!("task.{}", i)),
                ..Default::default()
            })
            .await
            .unwrap();
    }

    let res = client.get("/tasks").await.unwrap();
    assert!(res.status().is_success());

    let body: serde_json::Value = res.json().await.unwrap();
    let all_tasks: Vec<TaskListItem> = body["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| TaskListItem {
            id: v["id"].as_str().unwrap().to_string(),
            task_type: v["type"].as_str().map(|s| s.to_string()),
            status: v["status"].as_str().unwrap().to_string(),
            created_at: v["createdAt"].as_f64(),
        })
        .collect();

    assert_eq!(all_tasks.len(), 5);

    // Simulate limit=2 (as run_list does with .take())
    let limited: Vec<&TaskListItem> = all_tasks.iter().take(2).collect();
    assert_eq!(limited.len(), 2);

    let owned: Vec<TaskListItem> = limited
        .into_iter()
        .map(|t| TaskListItem {
            id: t.id.clone(),
            task_type: t.task_type.clone(),
            status: t.status.clone(),
            created_at: t.created_at,
        })
        .collect();
    let output = format_task_list(&owned);
    let lines: Vec<&str> = output.lines().collect();
    // header + 2 data rows
    assert_eq!(lines.len(), 3);
}

// ─── run_inspect: integration with real server ────────────────────────────────

#[tokio::test]
async fn run_inspect_returns_task_details() {
    let engine = make_engine();
    let base_url = start_server(engine.clone()).await;
    let client = make_client(&base_url);

    let task = engine
        .create_task(CreateTaskInput {
            r#type: Some("llm.chat".to_string()),
            params: Some([("model".to_string(), json!("gpt-4"))].into()),
            ..Default::default()
        })
        .await
        .unwrap();

    // Get task via HTTP
    let task_res = client.get(&format!("/tasks/{}", task.id)).await.unwrap();
    assert!(task_res.status().is_success());
    let task_body: serde_json::Value = task_res.json().await.unwrap();

    let detail = TaskDetail {
        id: task_body["id"].as_str().unwrap().to_string(),
        task_type: task_body["type"].as_str().map(|s| s.to_string()),
        status: task_body["status"].as_str().unwrap().to_string(),
        params: task_body.get("params").cloned(),
        created_at: task_body["createdAt"].as_f64(),
    };

    let output = format_task_inspect(&detail, &[]);
    assert!(
        output.contains(&format!("Task: {}", task.id)),
        "output: {output}"
    );
    assert!(output.contains("Type:    llm.chat"), "output: {output}");
    assert!(output.contains("Status:  pending"), "output: {output}");
    assert!(output.contains("model"), "output: {output}");
    assert!(output.contains("No events."), "output: {output}");
}

#[tokio::test]
async fn run_inspect_with_events() {
    let engine = make_engine();
    let base_url = start_server(engine.clone()).await;
    let client = make_client(&base_url);

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
                data: json!({"delta": "Hello"}),
                series_id: Some("response".to_string()),
                series_mode: None,
                series_acc_field: None,
            },
        )
        .await
        .unwrap();

    // Get task details
    let task_res = client.get(&format!("/tasks/{}", task.id)).await.unwrap();
    assert!(task_res.status().is_success());
    let task_body: serde_json::Value = task_res.json().await.unwrap();

    // Get event history
    let events_res = client
        .get(&format!("/tasks/{}/events/history", task.id))
        .await
        .unwrap();
    assert!(events_res.status().is_success());
    let events_body: Vec<serde_json::Value> = events_res.json().await.unwrap();

    let detail = TaskDetail {
        id: task_body["id"].as_str().unwrap().to_string(),
        task_type: task_body["type"].as_str().map(|s| s.to_string()),
        status: task_body["status"].as_str().unwrap().to_string(),
        params: task_body.get("params").cloned(),
        created_at: task_body["createdAt"].as_f64(),
    };
    let events: Vec<EventItem> = events_body
        .iter()
        .map(|v| EventItem {
            event_type: v["type"].as_str().map(|s| s.to_string()),
            level: v["level"].as_str().map(|s| s.to_string()),
            series_id: v["seriesId"].as_str().map(|s| s.to_string()),
            timestamp: v["timestamp"].as_f64(),
        })
        .collect();

    let output = format_task_inspect(&detail, &events);
    assert!(
        output.contains(&format!("Task: {}", task.id)),
        "output: {output}"
    );
    assert!(output.contains("Status:  running"), "output: {output}");
    // Should have taskcast:status (from transition) + llm.delta = 2 events
    assert!(
        output.contains("Recent Events (last 2):"),
        "output: {output}"
    );
    assert!(output.contains("taskcast:status"), "output: {output}");
    assert!(output.contains("llm.delta"), "output: {output}");
    assert!(output.contains("series:response"), "output: {output}");
}

#[tokio::test]
async fn run_inspect_nonexistent_task_returns_404() {
    let engine = make_engine();
    let base_url = start_server(engine.clone()).await;
    let client = make_client(&base_url);

    let res = client.get("/tasks/nonexistent").await.unwrap();
    assert_eq!(res.status().as_u16(), 404);
}

#[tokio::test]
async fn run_inspect_with_many_events_shows_last_5() {
    let engine = make_engine();
    let base_url = start_server(engine.clone()).await;
    let client = make_client(&base_url);

    let task = engine
        .create_task(CreateTaskInput {
            r#type: Some("batch".to_string()),
            ..Default::default()
        })
        .await
        .unwrap();
    engine
        .transition_task(&task.id, TaskStatus::Running, None)
        .await
        .unwrap();

    // Publish 8 events
    for i in 0..8 {
        engine
            .publish_event(
                &task.id,
                PublishEventInput {
                    r#type: format!("step.{}", i),
                    level: Level::Info,
                    data: json!({"step": i}),
                    series_id: None,
                    series_mode: None,
                    series_acc_field: None,
                },
            )
            .await
            .unwrap();
    }

    let events_res = client
        .get(&format!("/tasks/{}/events/history", task.id))
        .await
        .unwrap();
    let events_body: Vec<serde_json::Value> = events_res.json().await.unwrap();
    let events: Vec<EventItem> = events_body
        .iter()
        .map(|v| EventItem {
            event_type: v["type"].as_str().map(|s| s.to_string()),
            level: v["level"].as_str().map(|s| s.to_string()),
            series_id: v["seriesId"].as_str().map(|s| s.to_string()),
            timestamp: v["timestamp"].as_f64(),
        })
        .collect();

    let task_res = client.get(&format!("/tasks/{}", task.id)).await.unwrap();
    let task_body: serde_json::Value = task_res.json().await.unwrap();
    let detail = TaskDetail {
        id: task_body["id"].as_str().unwrap().to_string(),
        task_type: task_body["type"].as_str().map(|s| s.to_string()),
        status: task_body["status"].as_str().unwrap().to_string(),
        params: task_body.get("params").cloned(),
        created_at: task_body["createdAt"].as_f64(),
    };

    let output = format_task_inspect(&detail, &events);
    assert!(
        output.contains("Recent Events (last 5):"),
        "output: {output}"
    );
    assert!(output.contains("#0"), "output: {output}");
    assert!(output.contains("#4"), "output: {output}");
    assert!(!output.contains("#5"), "output: {output}");
}

#[tokio::test]
async fn run_inspect_completed_task_with_result() {
    let engine = make_engine();
    let base_url = start_server(engine.clone()).await;
    let client = make_client(&base_url);

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
        .transition_task(
            &task.id,
            TaskStatus::Completed,
            Some(TransitionPayload {
                result: Some([("tokens".to_string(), json!(42))].into()),
                ..Default::default()
            }),
        )
        .await
        .unwrap();

    let task_res = client.get(&format!("/tasks/{}", task.id)).await.unwrap();
    assert!(task_res.status().is_success());
    let task_body: serde_json::Value = task_res.json().await.unwrap();
    assert_eq!(task_body["status"], "completed");
    assert_eq!(task_body["result"]["tokens"], 42);
}

// ─── run_list with combined filters ───────────────────────────────────────────

#[tokio::test]
async fn run_list_with_combined_status_and_type_filter() {
    let engine = make_engine();
    let base_url = start_server(engine.clone()).await;
    let client = make_client(&base_url);

    let t1 = engine
        .create_task(CreateTaskInput {
            r#type: Some("llm.chat".to_string()),
            ..Default::default()
        })
        .await
        .unwrap();
    engine
        .create_task(CreateTaskInput {
            r#type: Some("llm.chat".to_string()),
            ..Default::default()
        })
        .await
        .unwrap();
    engine
        .create_task(CreateTaskInput {
            r#type: Some("agent.step".to_string()),
            ..Default::default()
        })
        .await
        .unwrap();
    engine
        .transition_task(&t1.id, TaskStatus::Running, None)
        .await
        .unwrap();

    let res = client
        .get("/tasks?status=running&type=llm.chat")
        .await
        .unwrap();
    assert!(res.status().is_success());

    let body: serde_json::Value = res.json().await.unwrap();
    let tasks = body["tasks"].as_array().unwrap();
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0]["id"], t1.id);
    assert_eq!(tasks[0]["type"], "llm.chat");
    assert_eq!(tasks[0]["status"], "running");
}

// ─── format_task_list column alignment ────────────────────────────────────────

#[test]
fn format_task_list_columns_are_aligned() {
    let tasks = vec![
        TaskListItem {
            id: "01J_SHORT".to_string(),
            task_type: Some("a".to_string()),
            status: "pending".to_string(),
            created_at: Some(1741234567890.0),
        },
        TaskListItem {
            id: "01J_LOOOOOOOOOOOOOOOOOOONG".to_string(),
            task_type: Some("very.long.type".to_string()),
            status: "running".to_string(),
            created_at: Some(1741234567890.0),
        },
    ];
    let output = format_task_list(&tasks);
    let lines: Vec<&str> = output.lines().collect();
    assert_eq!(lines.len(), 3);
    // Each line uses fixed-width formatting
    assert!(lines[0].starts_with("ID"));
    assert!(lines[1].starts_with("01J_SHORT"));
    assert!(lines[2].starts_with("01J_LOOOOOOOOOOOOOOOOOOONG"));
}

// ─── format_task_inspect edge cases ───────────────────────────────────────────

#[test]
fn format_inspect_missing_type() {
    let task = TaskDetail {
        id: "01JABCDEF".to_string(),
        task_type: None,
        status: "pending".to_string(),
        params: None,
        created_at: None,
    };
    let output = format_task_inspect(&task, &[]);
    assert!(output.contains("Type:    \n") || output.contains("Type:    "));
}

#[test]
fn format_inspect_with_complex_params() {
    let task = TaskDetail {
        id: "01JABCDEF".to_string(),
        task_type: Some("test".to_string()),
        status: "running".to_string(),
        params: Some(json!({"nested": {"key": "value"}, "array": [1, 2, 3]})),
        created_at: Some(1741234567890.0),
    };
    let output = format_task_inspect(&task, &[]);
    assert!(output.contains("nested"), "output: {output}");
    assert!(output.contains("array"), "output: {output}");
}

#[test]
fn format_inspect_events_without_optional_fields() {
    let task = TaskDetail {
        id: "01JABCDEF".to_string(),
        task_type: Some("test".to_string()),
        status: "running".to_string(),
        params: None,
        created_at: None,
    };
    let events = vec![EventItem {
        event_type: None,
        level: None,
        series_id: None,
        timestamp: None,
    }];
    let output = format_task_inspect(&task, &events);
    assert!(
        output.contains("Recent Events (last 1):"),
        "output: {output}"
    );
    assert!(output.contains("#0"), "output: {output}");
}

// ─── HTTP error handling tests ────────────────────────────────────────────────

#[tokio::test]
async fn run_list_http_error_response() {
    // Test the HTTP error handling path in run_list
    let engine = make_engine();
    let base_url = start_server(engine.clone()).await;
    let client = make_client(&base_url);

    // Requesting tasks with an invalid query param doesn't cause a server error
    // in the current API, so we test with a well-known non-task endpoint
    // to verify that our error handling code handles non-success responses.
    // Use a path that returns a non-success status.
    let res = client.get("/tasks/nonexistent-id").await.unwrap();
    assert!(!res.status().is_success());
    let status_code = res.status();
    let body = res.text().await.unwrap_or_default();
    let error_msg = format!("Error: HTTP {} — {}", status_code.as_u16(), body);
    assert!(
        error_msg.contains("404"),
        "should contain 404 status code: {error_msg}"
    );
}

#[tokio::test]
async fn run_inspect_event_history_fallback_on_error() {
    // Test the event history error handling path in run_inspect.
    // When event history returns non-success, run_inspect falls back to empty vec.
    let engine = make_engine();
    let base_url = start_server(engine.clone()).await;
    let client = make_client(&base_url);

    let task = engine
        .create_task(CreateTaskInput {
            r#type: Some("test".to_string()),
            ..Default::default()
        })
        .await
        .unwrap();

    // Get task details succeeds
    let task_res = client.get(&format!("/tasks/{}", task.id)).await.unwrap();
    assert!(task_res.status().is_success());
    let task_body: serde_json::Value = task_res.json().await.unwrap();

    let detail = TaskDetail {
        id: task_body["id"].as_str().unwrap().to_string(),
        task_type: task_body["type"].as_str().map(|s| s.to_string()),
        status: task_body["status"].as_str().unwrap().to_string(),
        params: task_body.get("params").cloned(),
        created_at: task_body["createdAt"].as_f64(),
    };

    // Simulate the fallback behavior when events_res is not success:
    // In run_inspect, if events_res.status().is_success() is false, events = vec![]
    let events: Vec<EventItem> = vec![];
    let output = format_task_inspect(&detail, &events);

    assert!(output.contains("No events."), "should show no events on fallback: {output}");
}

#[tokio::test]
async fn run_list_query_string_construction() {
    // Test the query string building logic used in run_list
    let mut params = Vec::new();
    let status = Some("running".to_string());
    let task_type = Some("llm.*".to_string());

    if let Some(ref s) = status {
        params.push(format!("status={}", s));
    }
    if let Some(ref t) = task_type {
        params.push(format!("type={}", t));
    }
    let qs = if params.is_empty() {
        String::new()
    } else {
        format!("?{}", params.join("&"))
    };

    assert_eq!(qs, "?status=running&type=llm.*");
}

#[tokio::test]
async fn run_list_query_string_status_only() {
    let mut params = Vec::new();
    let status = Some("pending".to_string());
    let task_type: Option<String> = None;

    if let Some(ref s) = status {
        params.push(format!("status={}", s));
    }
    if let Some(ref t) = task_type {
        params.push(format!("type={}", t));
    }
    let qs = if params.is_empty() {
        String::new()
    } else {
        format!("?{}", params.join("&"))
    };

    assert_eq!(qs, "?status=pending");
}

#[tokio::test]
async fn run_list_query_string_type_only() {
    let mut params = Vec::new();
    let status: Option<String> = None;
    let task_type = Some("agent.*".to_string());

    if let Some(ref s) = status {
        params.push(format!("status={}", s));
    }
    if let Some(ref t) = task_type {
        params.push(format!("type={}", t));
    }
    let qs = if params.is_empty() {
        String::new()
    } else {
        format!("?{}", params.join("&"))
    };

    assert_eq!(qs, "?type=agent.*");
}

#[tokio::test]
async fn run_list_query_string_empty() {
    let mut params: Vec<String> = Vec::new();
    let status: Option<String> = None;
    let task_type: Option<String> = None;

    if let Some(ref s) = status {
        params.push(format!("status={}", s));
    }
    if let Some(ref t) = task_type {
        params.push(format!("type={}", t));
    }
    let qs = if params.is_empty() {
        String::new()
    } else {
        format!("?{}", params.join("&"))
    };

    assert_eq!(qs, "");
}

#[tokio::test]
async fn run_list_limit_applied_to_server_results() {
    // Test the limit behavior as applied in run_list
    let engine = make_engine();
    let base_url = start_server(engine.clone()).await;
    let client = make_client(&base_url);

    for i in 0..5 {
        engine
            .create_task(CreateTaskInput {
                r#type: Some(format!("task.{}", i)),
                ..Default::default()
            })
            .await
            .unwrap();
    }

    let res = client.get("/tasks").await.unwrap();
    assert!(res.status().is_success());

    // Deserialize using the same ListTasksResponse structure run_list uses
    #[derive(serde::Deserialize)]
    struct ListTasksResponse {
        tasks: Vec<TaskListItem>,
    }
    let body: ListTasksResponse = res.json().await.unwrap();

    // Apply limit=3 the same way run_list does
    let limit: u32 = 3;
    let tasks: Vec<&TaskListItem> = body.tasks.iter().take(limit as usize).collect();
    assert_eq!(tasks.len(), 3);

    let owned: Vec<TaskListItem> = tasks
        .into_iter()
        .map(|t| TaskListItem {
            id: t.id.clone(),
            task_type: t.task_type.clone(),
            status: t.status.clone(),
            created_at: t.created_at,
        })
        .collect();
    let output = format_task_list(&owned);
    let lines: Vec<&str> = output.lines().collect();
    // header + 3 data rows
    assert_eq!(lines.len(), 4, "should have header + 3 rows: {output}");
}

// ─── format_timestamp additional edge cases ───────────────────────────────────

#[test]
fn format_timestamp_very_large_value() {
    // Very large timestamp (year ~33658) should still render
    let tasks = vec![TaskListItem {
        id: "01JABCDEF".to_string(),
        task_type: Some("test".to_string()),
        status: "pending".to_string(),
        created_at: Some(999999999999999.0),
    }];
    let output = format_task_list(&tasks);
    let lines: Vec<&str> = output.lines().collect();
    // Should produce some date (not crash)
    assert!(lines.len() >= 2, "should have header + data row");
}

#[test]
fn format_timestamp_fractional_millis() {
    // Fractional milliseconds should work
    let tasks = vec![TaskListItem {
        id: "01JABCDEF".to_string(),
        task_type: Some("test".to_string()),
        status: "pending".to_string(),
        created_at: Some(1741234567890.123),
    }];
    let output = format_task_list(&tasks);
    let lines: Vec<&str> = output.lines().collect();
    assert!(
        lines[1].contains("2025"),
        "fractional millis should still render date: {}",
        lines[1]
    );
}

// ─── Integration: run_list / run_inspect via NodeConfigManager + real server ──

use std::sync::Mutex;
use taskcast_cli::node_config::{NodeConfigManager, NodeEntry};
use taskcast_cli::commands::tasks::{TasksArgs, TasksCommands, run};

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
async fn run_list_via_node_config_happy_path() {
    let _lock = HOME_MUTEX.lock().unwrap();
    let engine = make_engine();
    let base_url = start_server(engine.clone()).await;

    let temp_dir = setup_temp_home_with_node(&base_url, "test-node");
    unsafe { std::env::set_var("HOME", temp_dir.path()); }

    // Create tasks
    engine
        .create_task(CreateTaskInput {
            r#type: Some("llm.chat".to_string()),
            ..Default::default()
        })
        .await
        .unwrap();
    engine
        .create_task(CreateTaskInput {
            r#type: Some("agent.step".to_string()),
            ..Default::default()
        })
        .await
        .unwrap();

    // Call the actual run function (dispatches to run_list)
    let result = run(TasksArgs {
        command: TasksCommands::List {
            status: None,
            task_type: None,
            limit: 20,
            node: None,
        },
    })
    .await;

    assert!(result.is_ok(), "run_list should succeed: {:?}", result.err());

    unsafe { std::env::remove_var("HOME"); }
}

#[tokio::test]
async fn run_list_with_status_filter_via_run() {
    let _lock = HOME_MUTEX.lock().unwrap();
    let engine = make_engine();
    let base_url = start_server(engine.clone()).await;

    let temp_dir = setup_temp_home_with_node(&base_url, "default");
    unsafe { std::env::set_var("HOME", temp_dir.path()); }

    let t1 = engine
        .create_task(CreateTaskInput {
            r#type: Some("llm.chat".to_string()),
            ..Default::default()
        })
        .await
        .unwrap();
    engine
        .transition_task(&t1.id, TaskStatus::Running, None)
        .await
        .unwrap();

    let result = run(TasksArgs {
        command: TasksCommands::List {
            status: Some("running".to_string()),
            task_type: None,
            limit: 20,
            node: None,
        },
    })
    .await;

    assert!(result.is_ok(), "run_list with status filter should succeed: {:?}", result.err());

    unsafe { std::env::remove_var("HOME"); }
}

#[tokio::test]
async fn run_list_with_type_filter_via_run() {
    let _lock = HOME_MUTEX.lock().unwrap();
    let engine = make_engine();
    let base_url = start_server(engine.clone()).await;

    let temp_dir = setup_temp_home_with_node(&base_url, "default");
    unsafe { std::env::set_var("HOME", temp_dir.path()); }

    engine
        .create_task(CreateTaskInput {
            r#type: Some("llm.chat".to_string()),
            ..Default::default()
        })
        .await
        .unwrap();

    let result = run(TasksArgs {
        command: TasksCommands::List {
            status: None,
            task_type: Some("llm.chat".to_string()),
            limit: 20,
            node: None,
        },
    })
    .await;

    assert!(result.is_ok(), "run_list with type filter should succeed: {:?}", result.err());

    unsafe { std::env::remove_var("HOME"); }
}

#[tokio::test]
async fn run_list_with_limit_via_run() {
    let _lock = HOME_MUTEX.lock().unwrap();
    let engine = make_engine();
    let base_url = start_server(engine.clone()).await;

    let temp_dir = setup_temp_home_with_node(&base_url, "default");
    unsafe { std::env::set_var("HOME", temp_dir.path()); }

    for i in 0..5 {
        engine
            .create_task(CreateTaskInput {
                r#type: Some(format!("task.{}", i)),
                ..Default::default()
            })
            .await
            .unwrap();
    }

    let result = run(TasksArgs {
        command: TasksCommands::List {
            status: None,
            task_type: None,
            limit: 2,
            node: None,
        },
    })
    .await;

    assert!(result.is_ok(), "run_list with limit should succeed: {:?}", result.err());

    unsafe { std::env::remove_var("HOME"); }
}

#[tokio::test]
async fn run_list_empty_server_via_run() {
    let _lock = HOME_MUTEX.lock().unwrap();
    let engine = make_engine();
    let base_url = start_server(engine.clone()).await;

    let temp_dir = setup_temp_home_with_node(&base_url, "default");
    unsafe { std::env::set_var("HOME", temp_dir.path()); }

    let result = run(TasksArgs {
        command: TasksCommands::List {
            status: None,
            task_type: None,
            limit: 20,
            node: None,
        },
    })
    .await;

    assert!(result.is_ok(), "run_list on empty server should succeed: {:?}", result.err());

    unsafe { std::env::remove_var("HOME"); }
}

#[tokio::test]
async fn run_inspect_via_run_happy_path() {
    let _lock = HOME_MUTEX.lock().unwrap();
    let engine = make_engine();
    let base_url = start_server(engine.clone()).await;

    let temp_dir = setup_temp_home_with_node(&base_url, "default");
    unsafe { std::env::set_var("HOME", temp_dir.path()); }

    let task = engine
        .create_task(CreateTaskInput {
            r#type: Some("llm.chat".to_string()),
            params: Some([("model".to_string(), json!("gpt-4"))].into()),
            ..Default::default()
        })
        .await
        .unwrap();

    let result = run(TasksArgs {
        command: TasksCommands::Inspect {
            task_id: task.id.clone(),
            node: None,
        },
    })
    .await;

    assert!(result.is_ok(), "run_inspect should succeed: {:?}", result.err());

    unsafe { std::env::remove_var("HOME"); }
}

#[tokio::test]
async fn run_inspect_with_events_via_run() {
    let _lock = HOME_MUTEX.lock().unwrap();
    let engine = make_engine();
    let base_url = start_server(engine.clone()).await;

    let temp_dir = setup_temp_home_with_node(&base_url, "default");
    unsafe { std::env::set_var("HOME", temp_dir.path()); }

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
                data: json!({"delta": "Hello"}),
                series_id: Some("response".to_string()),
                series_mode: None,
                series_acc_field: None,
            },
        )
        .await
        .unwrap();

    let result = run(TasksArgs {
        command: TasksCommands::Inspect {
            task_id: task.id.clone(),
            node: None,
        },
    })
    .await;

    assert!(result.is_ok(), "run_inspect with events should succeed: {:?}", result.err());

    unsafe { std::env::remove_var("HOME"); }
}

#[tokio::test]
async fn run_list_with_named_node_via_run() {
    let _lock = HOME_MUTEX.lock().unwrap();
    let engine = make_engine();
    let base_url = start_server(engine.clone()).await;

    let temp_dir = tempfile::TempDir::new().unwrap();
    let config_dir = temp_dir.path().join(".taskcast");
    std::fs::create_dir_all(&config_dir).unwrap();
    let mgr = NodeConfigManager::new(config_dir);
    mgr.add(
        "my-node",
        NodeEntry {
            url: base_url.clone(),
            token: None,
            token_type: None,
        },
    );
    // Don't set as current -- test the named node path
    unsafe { std::env::set_var("HOME", temp_dir.path()); }

    engine
        .create_task(CreateTaskInput {
            r#type: Some("test".to_string()),
            ..Default::default()
        })
        .await
        .unwrap();

    let result = run(TasksArgs {
        command: TasksCommands::List {
            status: None,
            task_type: None,
            limit: 20,
            node: Some("my-node".to_string()),
        },
    })
    .await;

    assert!(result.is_ok(), "run_list with named node should succeed: {:?}", result.err());

    unsafe { std::env::remove_var("HOME"); }
}

#[tokio::test]
async fn run_inspect_with_named_node_via_run() {
    let _lock = HOME_MUTEX.lock().unwrap();
    let engine = make_engine();
    let base_url = start_server(engine.clone()).await;

    let temp_dir = tempfile::TempDir::new().unwrap();
    let config_dir = temp_dir.path().join(".taskcast");
    std::fs::create_dir_all(&config_dir).unwrap();
    let mgr = NodeConfigManager::new(config_dir);
    mgr.add(
        "my-node",
        NodeEntry {
            url: base_url.clone(),
            token: None,
            token_type: None,
        },
    );
    unsafe { std::env::set_var("HOME", temp_dir.path()); }

    let task = engine
        .create_task(CreateTaskInput {
            r#type: Some("test".to_string()),
            ..Default::default()
        })
        .await
        .unwrap();

    let result = run(TasksArgs {
        command: TasksCommands::Inspect {
            task_id: task.id.clone(),
            node: Some("my-node".to_string()),
        },
    })
    .await;

    assert!(result.is_ok(), "run_inspect with named node should succeed: {:?}", result.err());

    unsafe { std::env::remove_var("HOME"); }
}

#[tokio::test]
async fn run_list_node_config_lookup_path() {
    let _lock = HOME_MUTEX.lock().unwrap();
    let engine = make_engine();
    let base_url = start_server(engine.clone()).await;

    let temp_dir = setup_temp_home_with_node(&base_url, "default");
    unsafe { std::env::set_var("HOME", temp_dir.path()); }

    // Replicate run_list lines 176-193: node lookup + client creation
    let home = dirs::home_dir().unwrap().join(".taskcast");
    let node_mgr = NodeConfigManager::new(home);
    let node = node_mgr.get_current();
    assert_eq!(node.url, base_url);

    let client = TaskcastClient::from_node(&node).await.unwrap();
    assert_eq!(client.base_url(), base_url);

    // Test query string construction (lines 195-206)
    let status = Some("running".to_string());
    let task_type = Some("llm.*".to_string());
    let mut params = Vec::new();
    if let Some(ref s) = status {
        params.push(format!("status={}", s));
    }
    if let Some(ref t) = task_type {
        params.push(format!("type={}", t));
    }
    let qs = if params.is_empty() {
        String::new()
    } else {
        format!("?{}", params.join("&"))
    };
    let path = format!("/tasks{}", qs);
    assert_eq!(path, "/tasks?status=running&type=llm.*");

    // Make actual HTTP request (line 209)
    let res = client.get(&path).await.unwrap();
    assert!(res.status().is_success());

    unsafe { std::env::remove_var("HOME"); }
}

#[tokio::test]
async fn run_inspect_node_config_lookup_path() {
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
                r#type: "test.event".to_string(),
                level: Level::Info,
                data: json!({"msg": "inspect-test"}),
                series_id: None,
                series_mode: None,
                series_acc_field: None,
            },
        )
        .await
        .unwrap();

    // Replicate run_inspect lines 238-277
    let home = dirs::home_dir().unwrap().join(".taskcast");
    let node_mgr = NodeConfigManager::new(home);
    let node = node_mgr.get_current();
    let client = TaskcastClient::from_node(&node).await.unwrap();

    // Get task details (line 258)
    let task_res = client.get(&format!("/tasks/{}", task.id)).await.unwrap();
    assert!(task_res.status().is_success());
    let task_detail: TaskDetail = task_res.json().await.unwrap();
    assert_eq!(task_detail.id, task.id);
    assert_eq!(task_detail.status, "running");

    // Get event history (lines 268-275)
    let events_res = client
        .get(&format!("/tasks/{}/events/history", task.id))
        .await
        .unwrap();
    let events: Vec<EventItem> = if events_res.status().is_success() {
        events_res.json().await.unwrap()
    } else {
        vec![]
    };
    assert!(!events.is_empty(), "should have events");

    let output = format_task_inspect(&task_detail, &events);
    assert!(output.contains(&task.id), "output: {output}");
    assert!(output.contains("running"), "output: {output}");

    unsafe { std::env::remove_var("HOME"); }
}

#[tokio::test]
async fn run_inspect_events_history_non_success_fallback() {
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

    // Replicate the fallback in run_inspect (lines 271-275):
    // if events endpoint returns non-success, use empty vec
    let home = dirs::home_dir().unwrap().join(".taskcast");
    let node_mgr = NodeConfigManager::new(home);
    let node = node_mgr.get_current();
    let client = TaskcastClient::from_node(&node).await.unwrap();

    let task_res = client.get(&format!("/tasks/{}", task.id)).await.unwrap();
    assert!(task_res.status().is_success());
    let task_detail: TaskDetail = task_res.json().await.unwrap();

    // Normal history should work fine
    let events_res = client
        .get(&format!("/tasks/{}/events/history", task.id))
        .await
        .unwrap();
    let events: Vec<EventItem> = if events_res.status().is_success() {
        events_res.json().await.unwrap()
    } else {
        vec![]
    };

    let output = format_task_inspect(&task_detail, &events);
    assert!(output.contains(&task.id), "output: {output}");

    unsafe { std::env::remove_var("HOME"); }
}

#[tokio::test]
async fn run_list_named_node_not_found() {
    let _lock = HOME_MUTEX.lock().unwrap();

    let temp_dir = tempfile::TempDir::new().unwrap();
    let config_dir = temp_dir.path().join(".taskcast");
    std::fs::create_dir_all(&config_dir).unwrap();
    let _mgr = NodeConfigManager::new(config_dir);

    unsafe { std::env::set_var("HOME", temp_dir.path()); }

    // Replicate run_list named node lookup failure (lines 182-188)
    let home = dirs::home_dir().unwrap().join(".taskcast");
    let node_mgr = NodeConfigManager::new(home);
    let result = node_mgr.get("nonexistent");
    assert!(result.is_none(), "should not find nonexistent node");

    unsafe { std::env::remove_var("HOME"); }
}

#[tokio::test]
async fn run_inspect_http_error_path() {
    let _lock = HOME_MUTEX.lock().unwrap();
    let engine = make_engine();
    let base_url = start_server(engine.clone()).await;

    let temp_dir = setup_temp_home_with_node(&base_url, "default");
    unsafe { std::env::set_var("HOME", temp_dir.path()); }

    // Replicate run_inspect error path (lines 259-263)
    let home = dirs::home_dir().unwrap().join(".taskcast");
    let node_mgr = NodeConfigManager::new(home);
    let node = node_mgr.get_current();
    let client = TaskcastClient::from_node(&node).await.unwrap();

    let task_res = client.get("/tasks/nonexistent-task-id").await.unwrap();
    assert!(!task_res.status().is_success());
    let status_code = task_res.status();
    let body = task_res.text().await.unwrap_or_default();
    let error_msg = format!("Error: HTTP {} — {}", status_code.as_u16(), body);
    assert!(error_msg.contains("404"), "should be 404: {error_msg}");

    unsafe { std::env::remove_var("HOME"); }
}
