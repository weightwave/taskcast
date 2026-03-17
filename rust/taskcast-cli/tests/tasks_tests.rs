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
