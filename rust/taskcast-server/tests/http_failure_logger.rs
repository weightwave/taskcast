use std::io;
use std::sync::Arc;

use axum::http::StatusCode;
use axum::middleware;
use axum::routing::{get, post};
use axum::{Json, Router};
use axum_test::TestServer;
use serde_json::{json, Value};
use taskcast_core::{
    BroadcastProvider, EngineError, MemoryShortTermStore, TaskEngine, TaskEngineOptions, TaskEvent,
};
use taskcast_server::{
    create_app_with_failure_logger, create_app_with_failure_logger_and_routes,
    http_failure_logger_middleware, AppError, AuthMode, CollectingHttpFailureLogger, CorsConfig,
    HttpFailureKind, HttpFailureLogger, LogLevel,
};

struct UnsupportedBroadcast;

#[async_trait::async_trait]
impl BroadcastProvider for UnsupportedBroadcast {
    async fn publish(
        &self,
        _channel: &str,
        _event: TaskEvent,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        Ok(())
    }

    async fn subscribe(
        &self,
        _channel: &str,
        _handler: Box<dyn Fn(TaskEvent) + Send + Sync>,
    ) -> Box<dyn Fn() + Send + Sync> {
        Box::new(|| {})
    }
}

async fn broken_pipe() -> Result<Json<Value>, AppError> {
    Err(AppError::Engine(EngineError::Store(Box::new(
        io::Error::new(
            io::ErrorKind::BrokenPipe,
            "redis://admin:secret@redis.example.com:6379 broken pipe",
        ),
    ))))
}

async fn manual_500() -> (StatusCode, &'static str) {
    (StatusCode::INTERNAL_SERVER_ERROR, "existing response")
}

#[tokio::test]
async fn logs_typed_store_500_once_and_preserves_response() {
    let logger = CollectingHttpFailureLogger::default();
    let logger_arc: Arc<dyn HttpFailureLogger> = Arc::new(logger.clone());
    let app =
        Router::new()
            .route("/tasks", get(broken_pipe))
            .layer(middleware::from_fn_with_state(
                logger_arc,
                http_failure_logger_middleware,
            ));
    let server = TestServer::new(app);

    let response = server
        .get("/tasks?access_token=do-not-log")
        .add_header(
            axum::http::header::AUTHORIZATION,
            axum::http::HeaderValue::from_static("Bearer do-not-log"),
        )
        .await;

    response.assert_status(StatusCode::INTERNAL_SERVER_ERROR);
    response.assert_json(&json!({
        "error": "redis://admin:secret@redis.example.com:6379 broken pipe"
    }));

    let records = logger.records();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].method, "GET");
    assert_eq!(records[0].path, "/tasks");
    assert_eq!(records[0].status, 500);
    assert_eq!(records[0].error_kind, Some(HttpFailureKind::Store));
    assert_eq!(
        records[0].error.as_deref(),
        Some("redis://***@redis.example.com:6379 broken pipe")
    );
    let serialized = serde_json::to_string(&records[0]).unwrap();
    assert!(!serialized.contains("access_token"));
    assert!(!serialized.contains("Bearer"));
    assert!(!serialized.contains("secret"));
}

#[tokio::test]
async fn logs_manual_500_once_without_invented_details() {
    let logger = CollectingHttpFailureLogger::default();
    let logger_arc: Arc<dyn HttpFailureLogger> = Arc::new(logger.clone());
    let app =
        Router::new()
            .route("/manual", post(manual_500))
            .layer(middleware::from_fn_with_state(
                logger_arc,
                http_failure_logger_middleware,
            ));
    let server = TestServer::new(app);

    let response = server
        .post("/manual?secret=query-secret")
        .add_header(
            axum::http::header::AUTHORIZATION,
            axum::http::HeaderValue::from_static("Bearer header-secret"),
        )
        .text("body-secret")
        .await;

    response.assert_status(StatusCode::INTERNAL_SERVER_ERROR);
    response.assert_text("existing response");
    let records = logger.records();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].method, "POST");
    assert_eq!(records[0].path, "/manual");
    assert_eq!(records[0].status, 500);
    assert!(records[0].error_kind.is_none());
    assert!(records[0].error.is_none());
    let serialized = serde_json::to_string(&records[0]).unwrap();
    assert!(!serialized.contains("query-secret"));
    assert!(!serialized.contains("header-secret"));
    assert!(!serialized.contains("body-secret"));
}

#[tokio::test]
async fn logs_the_upper_5xx_boundary() {
    let logger = CollectingHttpFailureLogger::default();
    let logger_arc: Arc<dyn HttpFailureLogger> = Arc::new(logger.clone());
    let app = Router::new()
        .route(
            "/upper-bound",
            get(|| async { StatusCode::from_u16(599).unwrap() }),
        )
        .layer(middleware::from_fn_with_state(
            logger_arc,
            http_failure_logger_middleware,
        ));
    let server = TestServer::new(app);

    server
        .get("/upper-bound")
        .await
        .assert_status(StatusCode::from_u16(599).unwrap());

    let records = logger.records();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].status, 599);
}

#[tokio::test]
async fn injectable_app_constructor_installs_logger_and_marks_internal_errors() {
    let logger = CollectingHttpFailureLogger::default();
    let logger_arc: Arc<dyn HttpFailureLogger> = Arc::new(logger.clone());
    let engine = Arc::new(TaskEngine::new(TaskEngineOptions {
        short_term_store: Arc::new(MemoryShortTermStore::new()),
        broadcast: Arc::new(UnsupportedBroadcast),
        long_term_store: None,
        hooks: None,
    }));
    let (app, _) = create_app_with_failure_logger(
        engine,
        AuthMode::None,
        None,
        None,
        CorsConfig::default(),
        logger_arc,
    );
    let server = TestServer::new(app);

    let response = server.get("/events").await;

    response.assert_status(StatusCode::NOT_IMPLEMENTED);
    let records = logger.records();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].error_kind, Some(HttpFailureKind::Internal));
    assert_eq!(
        records[0].error.as_deref(),
        Some("Global SSE not supported with this broadcast provider")
    );
}

#[tokio::test]
async fn additional_routes_are_inside_the_single_failure_logger_layer() {
    let logger = CollectingHttpFailureLogger::default();
    let logger_arc: Arc<dyn HttpFailureLogger> = Arc::new(logger.clone());
    let engine = Arc::new(TaskEngine::new(TaskEngineOptions {
        short_term_store: Arc::new(MemoryShortTermStore::new()),
        broadcast: Arc::new(UnsupportedBroadcast),
        long_term_store: None,
        hooks: None,
    }));
    let additional_routes = Router::new().route(
        "/_playground/failure",
        get(|| async { StatusCode::BAD_GATEWAY }),
    );
    let (app, _) = create_app_with_failure_logger_and_routes(
        engine,
        AuthMode::None,
        None,
        None,
        CorsConfig::default(),
        logger_arc,
        additional_routes,
    );
    let server = TestServer::new(app);

    server
        .get("/_playground/failure")
        .await
        .assert_status(StatusCode::BAD_GATEWAY);

    let records = logger.records();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].path, "/_playground/failure");
    assert_eq!(records[0].status, 502);
}

#[tokio::test]
async fn does_not_log_success_redirect_or_client_error() {
    let logger = CollectingHttpFailureLogger::default();
    let logger_arc: Arc<dyn HttpFailureLogger> = Arc::new(logger.clone());
    let app = Router::new()
        .route("/ok", get(|| async { StatusCode::OK }))
        .route("/redirect", get(|| async { StatusCode::FOUND }))
        .route("/bad", get(|| async { StatusCode::BAD_REQUEST }))
        .layer(middleware::from_fn_with_state(
            logger_arc,
            http_failure_logger_middleware,
        ));
    let server = TestServer::new(app);

    server.get("/ok").await.assert_status(StatusCode::OK);
    server
        .get("/redirect")
        .await
        .assert_status(StatusCode::FOUND);
    server
        .get("/bad")
        .await
        .assert_status(StatusCode::BAD_REQUEST);

    assert!(logger.records().is_empty());
}

#[test]
fn parses_levels_and_truncates_unicode() {
    assert_eq!(LogLevel::parse(None).unwrap(), LogLevel::Info);
    assert_eq!(LogLevel::parse(Some("DEBUG")).unwrap(), LogLevel::Debug);
    assert_eq!(LogLevel::parse(Some("Info")).unwrap(), LogLevel::Info);
    assert_eq!(LogLevel::parse(Some("Warn")).unwrap(), LogLevel::Warn);
    assert_eq!(LogLevel::parse(Some("error")).unwrap(), LogLevel::Error);
    assert!(LogLevel::parse(Some("trace")).is_err());
    for level in [
        LogLevel::Debug,
        LogLevel::Info,
        LogLevel::Warn,
        LogLevel::Error,
    ] {
        assert!(level.allows_error());
    }

    let message = format!("{}tail", "😀".repeat(2048));
    let sanitized = taskcast_server::sanitize_error_message(&message).unwrap();
    assert_eq!(sanitized.chars().count(), 2048);
    assert!(!sanitized.contains("tail"));
    assert!(taskcast_server::sanitize_error_message("").is_none());
}
