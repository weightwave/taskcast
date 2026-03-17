use std::sync::Arc;

use axum::middleware;
use axum_test::TestServer;
use serde_json::json;
use taskcast_core::{
    BlockedRequest, CreateTaskInput, MemoryBroadcastProvider, MemoryShortTermStore, TaskEngine,
    TaskEngineOptions, TaskStatus, TransitionPayload,
};
use taskcast_server::{
    create_app, verbose_logger_middleware, AuthMode, CollectingLogger, CorsConfig, VerboseLogger,
};

fn make_verbose_server() -> (Arc<TaskEngine>, TestServer, CollectingLogger) {
    let engine = Arc::new(TaskEngine::new(TaskEngineOptions {
        short_term_store: Arc::new(MemoryShortTermStore::new()),
        broadcast: Arc::new(MemoryBroadcastProvider::new()),
        long_term_store: None,
        hooks: None,
    }));
    let (app, _) = create_app(Arc::clone(&engine), AuthMode::None, None, None, CorsConfig::default());

    let logger = CollectingLogger::default();
    let logger_arc: Arc<dyn VerboseLogger> = Arc::new(logger.clone());
    let app = app.layer(middleware::from_fn_with_state(
        logger_arc,
        verbose_logger_middleware,
    ));

    let server = TestServer::new(app);
    (engine, server, logger)
}

#[tokio::test]
async fn logs_post_tasks_with_201() {
    let (_engine, server, logger) = make_verbose_server();
    let res = server
        .post("/tasks")
        .json(&json!({ "type": "test" }))
        .await;
    res.assert_status(axum_test::http::StatusCode::CREATED);

    let lines = logger.lines.lock().unwrap();
    assert_eq!(lines.len(), 1);
    assert!(lines[0].contains("POST"));
    assert!(lines[0].contains("/tasks"));
    assert!(lines[0].contains("201"));
    assert!(lines[0].contains("task created"));
}

#[tokio::test]
async fn logs_patch_status_transition() {
    let (engine, server, logger) = make_verbose_server();
    let task = engine
        .create_task(CreateTaskInput {
            r#type: Some("test".to_string()),
            ..Default::default()
        })
        .await
        .unwrap();

    // Clear creation log
    logger.lines.lock().unwrap().clear();

    let res = server
        .patch(&format!("/tasks/{}/status", task.id))
        .json(&json!({ "status": "running" }))
        .await;
    res.assert_status_ok();

    let lines = logger.lines.lock().unwrap();
    assert_eq!(lines.len(), 1);
    assert!(lines[0].contains("PATCH"));
    assert!(lines[0].contains("/status"));
    assert!(lines[0].contains("200"));
    assert!(lines[0].contains("\u{2192} running"));
}

#[tokio::test]
async fn logs_post_events_with_type() {
    let (engine, server, logger) = make_verbose_server();
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

    logger.lines.lock().unwrap().clear();

    let res = server
        .post(&format!("/tasks/{}/events", task.id))
        .json(&json!({ "type": "llm.delta", "level": "info", "data": { "text": "hi" } }))
        .await;
    res.assert_status(axum_test::http::StatusCode::CREATED);

    let lines = logger.lines.lock().unwrap();
    assert_eq!(lines.len(), 1);
    assert!(lines[0].contains("POST"));
    assert!(lines[0].contains("type: llm.delta"));
}

#[tokio::test]
async fn logs_get_task() {
    let (engine, server, logger) = make_verbose_server();
    let task = engine
        .create_task(CreateTaskInput {
            r#type: Some("test".to_string()),
            ..Default::default()
        })
        .await
        .unwrap();

    logger.lines.lock().unwrap().clear();

    let res = server.get(&format!("/tasks/{}", task.id)).await;
    res.assert_status_ok();

    let lines = logger.lines.lock().unwrap();
    assert_eq!(lines.len(), 1);
    assert!(lines[0].contains("GET"));
    assert!(lines[0].contains("200"));
}

#[tokio::test]
async fn logs_health_check() {
    let (_engine, server, logger) = make_verbose_server();
    let res = server.get("/health").await;
    res.assert_status_ok();

    let lines = logger.lines.lock().unwrap();
    assert_eq!(lines.len(), 1);
    assert!(lines[0].contains("GET"));
    assert!(lines[0].contains("/health"));
    assert!(lines[0].contains("200"));
}

#[tokio::test]
async fn logs_404_for_unknown_task() {
    let (_engine, server, logger) = make_verbose_server();
    let res = server.get("/tasks/nonexistent").await;
    res.assert_status(axum_test::http::StatusCode::NOT_FOUND);

    let lines = logger.lines.lock().unwrap();
    assert_eq!(lines.len(), 1);
    assert!(lines[0].contains("404"));
}

#[tokio::test]
async fn includes_timestamp_in_log() {
    let (_engine, server, logger) = make_verbose_server();
    server.get("/health").await;

    let lines = logger.lines.lock().unwrap();
    assert_eq!(lines.len(), 1);
    // Should match [YYYY-MM-DD HH:MM:SS]
    let re = regex::Regex::new(r"^\[\d{4}-\d{2}-\d{2} \d{2}:\d{2}:\d{2}\]").unwrap();
    assert!(re.is_match(&lines[0]), "Line did not match timestamp pattern: {}", &lines[0]);
}

#[tokio::test]
async fn includes_duration_in_ms() {
    let (_engine, server, logger) = make_verbose_server();
    server.get("/health").await;

    let lines = logger.lines.lock().unwrap();
    assert_eq!(lines.len(), 1);
    assert!(lines[0].contains("ms"), "Line should contain 'ms': {}", &lines[0]);
}

#[tokio::test]
async fn logs_multiple_requests() {
    let (_engine, server, logger) = make_verbose_server();
    server.get("/health").await;
    server.get("/health").await;
    server.get("/health").await;

    let lines = logger.lines.lock().unwrap();
    assert_eq!(lines.len(), 3);
}

#[tokio::test]
async fn logs_body_too_large_when_content_length_exceeds_64kb() {
    let (engine, server, logger) = make_verbose_server();
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

    logger.lines.lock().unwrap().clear();

    // Create a body larger than 64KB
    let large_text = "x".repeat(65537);
    let body = json!({
        "type": "llm.delta",
        "level": "info",
        "data": { "text": large_text }
    });

    // Explicitly set Content-Length so the middleware detects the oversized body
    // without consuming the stream (axum_test may not set it automatically).
    let body_bytes = serde_json::to_vec(&body).unwrap();
    let content_length = body_bytes.len().to_string();
    let res = server
        .post(&format!("/tasks/{}/events", task.id))
        .content_type("application/json")
        .add_header(
            axum_test::http::header::CONTENT_LENGTH,
            axum_test::http::HeaderValue::from_str(&content_length).unwrap(),
        )
        .bytes(body_bytes.into())
        .await;
    // Handler should still receive the full body and process the request normally
    res.assert_status(axum_test::http::StatusCode::CREATED);

    let lines = logger.lines.lock().unwrap();
    assert_eq!(lines.len(), 1);
    assert!(
        lines[0].contains("body too large to log"),
        "Expected 'body too large to log' in: {}",
        &lines[0]
    );
}

#[tokio::test]
async fn logs_post_without_content_length_still_parses_body() {
    let (engine, server, logger) = make_verbose_server();
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

    logger.lines.lock().unwrap().clear();

    // Send a POST with body but without explicitly setting Content-Length.
    // The middleware should still attempt to parse the body for context.
    let body = json!({ "type": "llm.chunk", "level": "info", "data": { "text": "hello" } });
    let body_bytes = serde_json::to_vec(&body).unwrap();

    let res = server
        .post(&format!("/tasks/{}/events", task.id))
        .content_type("application/json")
        .bytes(body_bytes.into())
        .await;
    res.assert_status(axum_test::http::StatusCode::CREATED);

    let lines = logger.lines.lock().unwrap();
    assert_eq!(lines.len(), 1);
    // Body should have been parsed even without Content-Length, so we see the event type
    assert!(
        lines[0].contains("type: llm.chunk"),
        "Expected 'type: llm.chunk' in: {}",
        &lines[0]
    );
}

#[tokio::test]
async fn logs_global_sse_endpoint() {
    let (_engine, server, logger) = make_verbose_server();

    // GET /events is an SSE endpoint that keeps the connection open.
    // We spawn the request in a background task and wait briefly for the
    // verbose middleware to log, since it logs after next.run() returns
    // the response headers (before the SSE body stream completes).
    let server = std::sync::Arc::new(server);
    let s = std::sync::Arc::clone(&server);
    let handle = tokio::spawn(async move {
        let _res = s.get("/events").await;
    });

    // Give the middleware time to log
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;

    let lines = logger.lines.lock().unwrap();
    assert!(
        !lines.is_empty(),
        "Expected at least one log line from the SSE endpoint"
    );
    assert!(
        lines[0].contains("SSE"),
        "Expected 'SSE' status for global events endpoint in: {}",
        &lines[0]
    );
    assert!(
        lines[0].contains("global subscriber connected"),
        "Expected 'global subscriber connected' in: {}",
        &lines[0]
    );

    handle.abort();
}

#[tokio::test]
async fn logs_post_resolve_with_context() {
    let (engine, server, logger) = make_verbose_server();
    let task = engine
        .create_task(CreateTaskInput {
            r#type: Some("test".to_string()),
            ..Default::default()
        })
        .await
        .unwrap();
    // Transition: pending -> running -> blocked
    engine
        .transition_task(&task.id, TaskStatus::Running, None)
        .await
        .unwrap();
    engine
        .transition_task(
            &task.id,
            TaskStatus::Blocked,
            Some(TransitionPayload {
                blocked_request: Some(BlockedRequest {
                    request_type: "confirm".to_string(),
                    data: json!({ "message": "proceed?" }),
                }),
                ..Default::default()
            }),
        )
        .await
        .unwrap();

    logger.lines.lock().unwrap().clear();

    let res = server
        .post(&format!("/tasks/{}/resolve", task.id))
        .json(&json!({ "data": { "answer": "yes" } }))
        .await;
    // The resolve endpoint transitions blocked -> running
    res.assert_status_ok();

    let lines = logger.lines.lock().unwrap();
    assert_eq!(lines.len(), 1);
    assert!(
        lines[0].contains("resolve"),
        "Expected 'resolve' context in: {}",
        &lines[0]
    );
}
