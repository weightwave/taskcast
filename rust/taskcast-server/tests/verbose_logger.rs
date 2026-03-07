use std::sync::Arc;

use axum::middleware;
use axum_test::TestServer;
use serde_json::json;
use taskcast_core::{
    CreateTaskInput, MemoryBroadcastProvider, MemoryShortTermStore, TaskEngine, TaskEngineOptions,
    TaskStatus,
};
use taskcast_server::{
    create_app, verbose_logger_middleware, AuthMode, CollectingLogger, VerboseLogger,
};

fn make_verbose_server() -> (Arc<TaskEngine>, TestServer, CollectingLogger) {
    let engine = Arc::new(TaskEngine::new(TaskEngineOptions {
        short_term_store: Arc::new(MemoryShortTermStore::new()),
        broadcast: Arc::new(MemoryBroadcastProvider::new()),
        long_term_store: None,
        hooks: None,
    }));
    let (app, _) = create_app(Arc::clone(&engine), AuthMode::None, None, None);

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
