use std::sync::Arc;

use axum_test::TestServer;
use serde_json::json;
use taskcast_core::{
    CreateTaskInput, MemoryBroadcastProvider, MemoryShortTermStore, TaskEngine, TaskEngineOptions,
    TaskStatus,
};
use taskcast_server::{create_app, AuthMode};

use taskcast_cli::commands::doctor::{
    format_doctor_result, AdapterStatus, AuthStatus, DoctorResult, ServerStatus,
};
use taskcast_cli::commands::logs::format_event;
use taskcast_cli::commands::tasks::{
    format_task_inspect, format_task_list, EventItem, TaskDetail, TaskListItem,
};

// ─── Helper ──────────────────────────────────────────────────────────────────

fn make_server() -> (Arc<TaskEngine>, TestServer) {
    let engine = Arc::new(TaskEngine::new(TaskEngineOptions {
        short_term_store: Arc::new(MemoryShortTermStore::new()),
        broadcast: Arc::new(MemoryBroadcastProvider::new()),
        long_term_store: None,
        hooks: None,
    }));
    let (app, _) = create_app(Arc::clone(&engine), AuthMode::None, None, None);
    (engine, TestServer::new(app))
}

// ─── 1. Health endpoint responds OK ──────────────────────────────────────────

#[tokio::test]
async fn health_returns_ok() {
    let (_engine, server) = make_server();
    let res = server.get("/health").await;
    res.assert_status_ok();
    let body: serde_json::Value = res.json();
    assert_eq!(body, json!({ "ok": true }));
}

// ─── 2. Health detail returns adapters ───────────────────────────────────────

#[tokio::test]
async fn health_detail_returns_adapters_and_auth() {
    let (_engine, server) = make_server();
    let res = server.get("/health/detail").await;
    res.assert_status_ok();
    let body: serde_json::Value = res.json();
    assert_eq!(body["ok"], true);
    assert!(body["uptime"].is_number());
    assert_eq!(body["auth"]["mode"], "none");
    assert_eq!(body["adapters"]["broadcast"]["provider"], "memory");
    assert_eq!(body["adapters"]["broadcast"]["status"], "ok");
    assert_eq!(body["adapters"]["shortTermStore"]["provider"], "memory");
    assert_eq!(body["adapters"]["shortTermStore"]["status"], "ok");
}

// ─── 3. Create task via POST, get via GET ────────────────────────────────────

#[tokio::test]
async fn create_and_get_task() {
    let (_engine, server) = make_server();

    // Create task
    let res = server
        .post("/tasks")
        .json(&json!({ "type": "llm.chat", "params": { "prompt": "hello" } }))
        .await;
    res.assert_status(axum_test::http::StatusCode::CREATED);
    let created: serde_json::Value = res.json();
    assert_eq!(created["type"], "llm.chat");
    assert_eq!(created["status"], "pending");
    assert_eq!(created["params"]["prompt"], "hello");

    let task_id = created["id"].as_str().unwrap();

    // Get task by ID
    let res = server.get(&format!("/tasks/{task_id}")).await;
    res.assert_status_ok();
    let fetched: serde_json::Value = res.json();
    assert_eq!(fetched["id"], task_id);
    assert_eq!(fetched["type"], "llm.chat");
    assert_eq!(fetched["status"], "pending");
}

#[tokio::test]
async fn get_nonexistent_task_returns_404() {
    let (_engine, server) = make_server();
    let res = server.get("/tasks/nonexistent").await;
    res.assert_status(axum_test::http::StatusCode::NOT_FOUND);
}

// ─── 4. Transition task status ───────────────────────────────────────────────

#[tokio::test]
async fn transition_task_to_running() {
    let (engine, server) = make_server();
    let task = engine
        .create_task(CreateTaskInput {
            r#type: Some("test".to_string()),
            ..Default::default()
        })
        .await
        .unwrap();

    let res = server
        .patch(&format!("/tasks/{}/status", task.id))
        .json(&json!({ "status": "running" }))
        .await;
    res.assert_status_ok();
    let body: serde_json::Value = res.json();
    assert_eq!(body["status"], "running");
}

#[tokio::test]
async fn invalid_transition_returns_400() {
    let (engine, server) = make_server();
    let task = engine
        .create_task(CreateTaskInput {
            r#type: Some("test".to_string()),
            ..Default::default()
        })
        .await
        .unwrap();

    // pending -> completed is invalid
    let res = server
        .patch(&format!("/tasks/{}/status", task.id))
        .json(&json!({ "status": "completed" }))
        .await;
    res.assert_status(axum_test::http::StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn cannot_transition_terminal_task() {
    let (engine, server) = make_server();
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

    let res = server
        .patch(&format!("/tasks/{}/status", task.id))
        .json(&json!({ "status": "running" }))
        .await;
    res.assert_status(axum_test::http::StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn transition_nonexistent_task_returns_404() {
    let (_engine, server) = make_server();
    let res = server
        .patch("/tasks/nonexistent/status")
        .json(&json!({ "status": "running" }))
        .await;
    res.assert_status(axum_test::http::StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn transition_with_invalid_status_returns_422() {
    let (engine, server) = make_server();
    let task = engine
        .create_task(CreateTaskInput {
            r#type: Some("test".to_string()),
            ..Default::default()
        })
        .await
        .unwrap();

    // Axum returns 422 Unprocessable Entity for serde deserialization failures
    let res = server
        .patch(&format!("/tasks/{}/status", task.id))
        .json(&json!({ "status": "invalid_status_value" }))
        .await;
    res.assert_status(axum_test::http::StatusCode::UNPROCESSABLE_ENTITY);
}

// ─── 5. Publish events and retrieve history ──────────────────────────────────

#[tokio::test]
async fn publish_event_and_get_history() {
    let (engine, server) = make_server();
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

    // Publish event
    let res = server
        .post(&format!("/tasks/{}/events", task.id))
        .json(&json!({ "type": "llm.delta", "level": "info", "data": { "delta": "Hello" } }))
        .await;
    res.assert_status(axum_test::http::StatusCode::CREATED);
    let evt: serde_json::Value = res.json();
    assert_eq!(evt["type"], "llm.delta");
    assert_eq!(evt["level"], "info");
    assert_eq!(evt["data"]["delta"], "Hello");

    // Get history
    let res = server
        .get(&format!("/tasks/{}/events/history", task.id))
        .await;
    res.assert_status_ok();
    let events: Vec<serde_json::Value> = res.json();
    // 1 taskcast:status (from transition) + 1 llm.delta = 2 events
    assert_eq!(events.len(), 2);
    assert_eq!(events[0]["type"], "taskcast:status");
    assert_eq!(events[1]["type"], "llm.delta");
    assert_eq!(events[1]["data"]["delta"], "Hello");
}

#[tokio::test]
async fn batch_event_publishing() {
    let (engine, server) = make_server();
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

    let res = server
        .post(&format!("/tasks/{}/events", task.id))
        .json(&json!([
            { "type": "step", "level": "info", "data": { "step": 1 } },
            { "type": "step", "level": "info", "data": { "step": 2 } },
            { "type": "step", "level": "info", "data": { "step": 3 } },
        ]))
        .await;
    res.assert_status(axum_test::http::StatusCode::CREATED);
    let events: Vec<serde_json::Value> = res.json();
    assert_eq!(events.len(), 3);
    assert_eq!(events[0]["data"]["step"], 1);
    assert_eq!(events[2]["data"]["step"], 3);

    // Verify in history
    let res = server
        .get(&format!("/tasks/{}/events/history", task.id))
        .await;
    let history: Vec<serde_json::Value> = res.json();
    // 1 taskcast:status + 3 step events = 4
    assert_eq!(history.len(), 4);
    assert_eq!(history[0]["type"], "taskcast:status");
    assert_eq!(history[1]["data"]["step"], 1);
    assert_eq!(history[3]["data"]["step"], 3);
}

#[tokio::test]
async fn publish_event_to_nonexistent_task_returns_404() {
    let (_engine, server) = make_server();
    let res = server
        .post("/tasks/nonexistent/events")
        .json(&json!({ "type": "test", "level": "info", "data": {} }))
        .await;
    res.assert_status(axum_test::http::StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn event_history_for_nonexistent_task_returns_404() {
    let (_engine, server) = make_server();
    let res = server.get("/tasks/nonexistent/events/history").await;
    res.assert_status(axum_test::http::StatusCode::NOT_FOUND);
}

// ─── 6. List tasks with filters ──────────────────────────────────────────────

#[tokio::test]
async fn list_tasks_empty() {
    let (_engine, server) = make_server();
    let res = server.get("/tasks").await;
    res.assert_status_ok();
    let body: serde_json::Value = res.json();
    assert_eq!(body["tasks"], json!([]));
}

#[tokio::test]
async fn list_tasks_returns_created_tasks() {
    let (engine, server) = make_server();
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

    let res = server.get("/tasks").await;
    res.assert_status_ok();
    let body: serde_json::Value = res.json();
    let tasks = body["tasks"].as_array().unwrap();
    assert_eq!(tasks.len(), 2);

    let types: Vec<&str> = tasks.iter().map(|t| t["type"].as_str().unwrap()).collect();
    assert!(types.contains(&"llm.chat"));
    assert!(types.contains(&"agent.step"));
}

#[tokio::test]
async fn list_tasks_filter_by_type() {
    let (engine, server) = make_server();
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

    let res = server.get("/tasks?type=llm.chat").await;
    res.assert_status_ok();
    let body: serde_json::Value = res.json();
    let tasks = body["tasks"].as_array().unwrap();
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0]["type"], "llm.chat");
}

#[tokio::test]
async fn list_tasks_filter_by_status() {
    let (engine, server) = make_server();
    let t1 = engine
        .create_task(CreateTaskInput {
            r#type: Some("a".to_string()),
            ..Default::default()
        })
        .await
        .unwrap();
    engine
        .create_task(CreateTaskInput {
            r#type: Some("b".to_string()),
            ..Default::default()
        })
        .await
        .unwrap();
    engine
        .transition_task(&t1.id, TaskStatus::Running, None)
        .await
        .unwrap();

    let res = server.get("/tasks?status=running").await;
    res.assert_status_ok();
    let body: serde_json::Value = res.json();
    let tasks = body["tasks"].as_array().unwrap();
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0]["id"], t1.id);
    assert_eq!(tasks[0]["status"], "running");
}

#[tokio::test]
async fn list_tasks_combined_filters() {
    let (engine, server) = make_server();
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

    let res = server.get("/tasks?status=running&type=llm.chat").await;
    res.assert_status_ok();
    let body: serde_json::Value = res.json();
    let tasks = body["tasks"].as_array().unwrap();
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0]["id"], t1.id);
}

// ─── 7. Full lifecycle ───────────────────────────────────────────────────────

#[tokio::test]
async fn full_lifecycle_create_run_publish_complete() {
    let (_engine, server) = make_server();

    // 1. Create task via HTTP
    let res = server
        .post("/tasks")
        .json(&json!({ "type": "llm.chat", "params": { "prompt": "hello" } }))
        .await;
    res.assert_status(axum_test::http::StatusCode::CREATED);
    let created: serde_json::Value = res.json();
    assert_eq!(created["status"], "pending");
    assert_eq!(created["type"], "llm.chat");
    let task_id = created["id"].as_str().unwrap().to_string();

    // 2. Transition to running
    let res = server
        .patch(&format!("/tasks/{task_id}/status"))
        .json(&json!({ "status": "running" }))
        .await;
    res.assert_status_ok();
    let body: serde_json::Value = res.json();
    assert_eq!(body["status"], "running");

    // 3. Publish event
    let res = server
        .post(&format!("/tasks/{task_id}/events"))
        .json(&json!({ "type": "llm.delta", "level": "info", "data": { "delta": "response text" } }))
        .await;
    res.assert_status(axum_test::http::StatusCode::CREATED);
    let evt: serde_json::Value = res.json();
    assert_eq!(evt["type"], "llm.delta");
    assert_eq!(evt["data"]["delta"], "response text");

    // 4. Complete the task
    let res = server
        .patch(&format!("/tasks/{task_id}/status"))
        .json(&json!({ "status": "completed", "result": { "tokens": 42 } }))
        .await;
    res.assert_status_ok();
    let body: serde_json::Value = res.json();
    assert_eq!(body["status"], "completed");
    assert_eq!(body["result"]["tokens"], 42);

    // 5. Verify final state
    let res = server.get(&format!("/tasks/{task_id}")).await;
    res.assert_status_ok();
    let final_task: serde_json::Value = res.json();
    assert_eq!(final_task["status"], "completed");
    assert_eq!(final_task["result"]["tokens"], 42);

    // 6. Verify event history
    // taskcast:status (running) + llm.delta + taskcast:status (completed) = 3
    let res = server
        .get(&format!("/tasks/{task_id}/events/history"))
        .await;
    res.assert_status_ok();
    let events: Vec<serde_json::Value> = res.json();
    assert_eq!(events.len(), 3);
    assert_eq!(events[0]["type"], "taskcast:status");
    assert_eq!(events[1]["type"], "llm.delta");
    assert_eq!(events[2]["type"], "taskcast:status");
}

#[tokio::test]
async fn lifecycle_create_run_fail() {
    let (engine, server) = make_server();
    let task = engine
        .create_task(CreateTaskInput {
            r#type: Some("flaky-task".to_string()),
            ..Default::default()
        })
        .await
        .unwrap();
    engine
        .transition_task(&task.id, TaskStatus::Running, None)
        .await
        .unwrap();

    let res = server
        .patch(&format!("/tasks/{}/status", task.id))
        .json(&json!({
            "status": "failed",
            "error": { "message": "out of memory", "code": "OOM" },
        }))
        .await;
    res.assert_status_ok();
    let body: serde_json::Value = res.json();
    assert_eq!(body["status"], "failed");
    assert_eq!(body["error"]["message"], "out of memory");
    assert_eq!(body["error"]["code"], "OOM");
}

// ─── 8. Format verification with real server data ────────────────────────────

#[tokio::test]
async fn format_task_list_from_server_data() {
    let (engine, server) = make_server();
    let t1 = engine
        .create_task(CreateTaskInput {
            r#type: Some("llm.chat".to_string()),
            ..Default::default()
        })
        .await
        .unwrap();
    let t2 = engine
        .create_task(CreateTaskInput {
            r#type: Some("batch.job".to_string()),
            ..Default::default()
        })
        .await
        .unwrap();

    let res = server.get("/tasks").await;
    let body: serde_json::Value = res.json();
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

    let output = format_task_list(&tasks);
    assert!(output.contains("ID"));
    assert!(output.contains("TYPE"));
    assert!(output.contains("STATUS"));
    assert!(output.contains(&t1.id));
    assert!(output.contains(&t2.id));
    assert!(output.contains("llm.chat"));
    assert!(output.contains("batch.job"));
    assert!(output.contains("pending"));
}

#[tokio::test]
async fn format_task_list_empty_from_server() {
    let (_engine, server) = make_server();
    let res = server.get("/tasks").await;
    let body: serde_json::Value = res.json();
    let tasks: Vec<TaskListItem> = vec![];
    let _ = body; // just to verify the endpoint works
    let output = format_task_list(&tasks);
    assert_eq!(output, "No tasks found.");
}

#[tokio::test]
async fn format_task_inspect_from_server_data() {
    let (engine, server) = make_server();
    let task = engine
        .create_task(CreateTaskInput {
            r#type: Some("llm.chat".to_string()),
            params: Some([("model".to_string(), json!("gpt-4"))].into()),
            ..Default::default()
        })
        .await
        .unwrap();
    engine
        .transition_task(&task.id, TaskStatus::Running, None)
        .await
        .unwrap();

    let task_res = server.get(&format!("/tasks/{}", task.id)).await;
    let task_body: serde_json::Value = task_res.json();

    let events_res = server
        .get(&format!("/tasks/{}/events/history", task.id))
        .await;
    let events_body: Vec<serde_json::Value> = events_res.json();

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
    assert!(output.contains("Type:    llm.chat"), "output: {output}");
    assert!(output.contains("Status:  running"), "output: {output}");
    assert!(output.contains("model"), "output: {output}");
    // 1 event: taskcast:status from the transition
    assert!(
        output.contains("Recent Events (last 1):"),
        "output: {output}"
    );
    assert!(output.contains("taskcast:status"), "output: {output}");
}

#[tokio::test]
async fn format_task_inspect_no_events() {
    let (engine, _server) = make_server();
    let task = engine
        .create_task(CreateTaskInput {
            r#type: Some("llm.chat".to_string()),
            ..Default::default()
        })
        .await
        .unwrap();

    let detail = TaskDetail {
        id: task.id.clone(),
        task_type: task.r#type.clone(),
        status: "pending".to_string(),
        params: None,
        created_at: Some(task.created_at as f64),
    };

    let output = format_task_inspect(&detail, &[]);
    assert!(output.contains("No events."));
    assert!(!output.contains("Recent Events"));
}

#[tokio::test]
async fn format_event_with_server_data() {
    let (engine, server) = make_server();
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

    // Publish a user event
    server
        .post(&format!("/tasks/{}/events", task.id))
        .json(&json!({ "type": "llm.delta", "level": "info", "data": { "delta": "Hello" } }))
        .await;

    let res = server
        .get(&format!("/tasks/{}/events/history", task.id))
        .await;
    let events: Vec<serde_json::Value> = res.json();

    // events[0] is taskcast:status, events[1] is llm.delta
    let user_event = &events[1];
    let formatted = format_event(
        user_event["type"].as_str().unwrap(),
        user_event["level"].as_str().unwrap(),
        user_event["timestamp"].as_f64().unwrap() as i64,
        user_event.get("data").unwrap(),
        None,
    );
    assert!(formatted.contains("llm.delta"), "got: {formatted}");
    assert!(formatted.contains("info"), "got: {formatted}");
    assert!(
        formatted.contains(r#""delta":"Hello""#),
        "got: {formatted}"
    );

    // Format with task ID prefix
    let formatted_with_id = format_event(
        user_event["type"].as_str().unwrap(),
        user_event["level"].as_str().unwrap(),
        user_event["timestamp"].as_f64().unwrap() as i64,
        user_event.get("data").unwrap(),
        Some(&task.id),
    );
    assert!(
        formatted_with_id.contains(&format!("{}..  ", &task.id[..7])),
        "got: {formatted_with_id}"
    );
}

#[tokio::test]
async fn format_doctor_result_from_server_response() {
    let (_engine, server) = make_server();
    let res = server.get("/health/detail").await;
    let body: serde_json::Value = res.json();

    let result = DoctorResult {
        server: ServerStatus {
            ok: true,
            url: "http://localhost:3721".to_string(),
            uptime: body["uptime"].as_u64(),
            error: None,
        },
        auth: AuthStatus {
            status: "ok".to_string(),
            mode: body["auth"]["mode"].as_str().map(|s| s.to_string()),
            message: None,
        },
        adapters: vec![
            AdapterStatus {
                name: "broadcast".to_string(),
                provider: body["adapters"]["broadcast"]["provider"]
                    .as_str()
                    .unwrap()
                    .to_string(),
                status: "ok".to_string(),
            },
            AdapterStatus {
                name: "shortTermStore".to_string(),
                provider: body["adapters"]["shortTermStore"]["provider"]
                    .as_str()
                    .unwrap()
                    .to_string(),
                status: "ok".to_string(),
            },
        ],
    };

    let output = format_doctor_result(&result);
    assert!(output.contains("Server:    OK  taskcast at http://localhost:3721"));
    assert!(output.contains("Auth:      OK  none"));
    assert!(output.contains("Broadcast: OK  memory"));
    assert!(output.contains("ShortTerm: OK  memory"));
    assert!(output.contains("LongTerm:  SKIP  not configured"));
}

// ─── 9. Error cases via HTTP ─────────────────────────────────────────────────

#[tokio::test]
async fn reject_invalid_task_body() {
    let (_engine, server) = make_server();
    // Axum returns 422 Unprocessable Entity for serde deserialization failures
    let res = server
        .post("/tasks")
        .json(&json!({ "ttl": "not-a-number" }))
        .await;
    res.assert_status(axum_test::http::StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn reject_malformed_transition_body() {
    let (engine, server) = make_server();
    let task = engine
        .create_task(CreateTaskInput {
            r#type: Some("test".to_string()),
            ..Default::default()
        })
        .await
        .unwrap();

    // Axum returns 422 Unprocessable Entity for serde deserialization failures
    let res = server
        .patch(&format!("/tasks/{}/status", task.id))
        .json(&json!({ "status": "invalid_status_value" }))
        .await;
    res.assert_status(axum_test::http::StatusCode::UNPROCESSABLE_ENTITY);
}
