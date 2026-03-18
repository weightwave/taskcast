//! Tests for the global SSE endpoint `GET /events`.
//!
//! Verifies that the endpoint streams events from tasks created after the
//! SSE connection is established, supports type/level filters, and requires
//! the `event:subscribe` auth scope.

use std::sync::Arc;

use axum_test::http::HeaderValue;
use serde_json::json;
use taskcast_core::{
    Level, MemoryBroadcastProvider, MemoryShortTermStore, TaskEngine, TaskEngineOptions,
    TaskStatus,
};
use taskcast_server::{create_app, AuthMode, CorsConfig, JwtConfig};

// ─── Helpers ────────────────────────────────────────────────────────────────

fn make_engine() -> Arc<TaskEngine> {
    Arc::new(TaskEngine::new(TaskEngineOptions {
        short_term_store: Arc::new(MemoryShortTermStore::new()),
        broadcast: Arc::new(MemoryBroadcastProvider::new()),
        long_term_store: None,
        hooks: None,
    }))
}

fn make_app(engine: Arc<TaskEngine>) -> axum::Router {
    let (app, _) = create_app(engine, AuthMode::None, None, None, CorsConfig::default());
    app
}

/// Spin up a real TCP listener so we can use reqwest for SSE streaming.
async fn serve_app(app: axum::Router) -> std::net::SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    addr
}

async fn create_task(engine: &TaskEngine, task_id: &str) {
    engine
        .create_task(taskcast_core::engine::CreateTaskInput {
            id: Some(task_id.to_string()),
            r#type: Some("test".to_string()),
            ..Default::default()
        })
        .await
        .expect("create_task failed");
}

async fn publish_event(
    engine: &TaskEngine,
    task_id: &str,
    event_type: &str,
    level: Level,
    data: serde_json::Value,
) {
    engine
        .publish_event(
            task_id,
            taskcast_core::PublishEventInput {
                r#type: event_type.to_string(),
                level,
                data,
                series_id: None,
                series_mode: None,
                series_acc_field: None,
            },
        )
        .await
        .expect("publish_event failed");
}

const JWT_SECRET: &str = "test-secret-key-for-jwt-signing-needs-to-be-long-enough";

fn make_jwt_engine_and_app() -> (Arc<TaskEngine>, axum::Router) {
    let engine = make_engine();
    let auth_mode = AuthMode::Jwt(JwtConfig {
        algorithm: jsonwebtoken::Algorithm::HS256,
        secret: Some(JWT_SECRET.to_string()),
        public_key: None,
        issuer: None,
        audience: None,
    });
    let (app, _) = create_app(Arc::clone(&engine), auth_mode, None, None, CorsConfig::default());
    (engine, app)
}

fn make_token(claims: serde_json::Value) -> String {
    jsonwebtoken::encode(
        &jsonwebtoken::Header::default(),
        &claims,
        &jsonwebtoken::EncodingKey::from_secret(JWT_SECRET.as_bytes()),
    )
    .unwrap()
}

fn bearer_header(token: &str) -> HeaderValue {
    HeaderValue::from_str(&format!("Bearer {token}")).unwrap()
}

// ─── Tests ──────────────────────────────────────────────────────────────────

#[tokio::test]
async fn global_sse_returns_sse_content_type() {
    let engine = make_engine();
    let app = make_app(Arc::clone(&engine));
    let addr = serve_app(app).await;
    let client = reqwest::Client::new();

    // Spawn a task that creates a task and completes it after a short delay,
    // so we can verify the SSE stream works. We'll drop the client to close.
    let engine_clone = Arc::clone(&engine);
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        create_task(&engine_clone, "global-sse-ct-1").await;
        publish_event(
            &engine_clone,
            "global-sse-ct-1",
            "progress",
            Level::Info,
            json!({ "step": 1 }),
        )
        .await;
    });

    let response = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        client.get(format!("http://{addr}/events")).send(),
    )
    .await
    .expect("connect timed out")
    .unwrap();

    assert_eq!(response.status(), 200);
    let content_type = response
        .headers()
        .get("content-type")
        .expect("missing content-type header")
        .to_str()
        .unwrap();
    assert!(
        content_type.contains("text/event-stream"),
        "expected text/event-stream, got: {content_type}"
    );
}

#[tokio::test]
async fn global_sse_streams_events_from_newly_created_tasks() {
    let engine = make_engine();
    let app = make_app(Arc::clone(&engine));
    let addr = serve_app(app).await;

    // Connect to /events from main task
    let client = reqwest::Client::new();
    let mut response = client.get(format!("http://{addr}/events")).send().await.unwrap();
    assert_eq!(response.status(), 200);

    // Spawn producer
    let engine_clone = Arc::clone(&engine);
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        // Create two tasks and publish events to both
        create_task(&engine_clone, "global-sse-1").await;
        engine_clone
            .transition_task("global-sse-1", TaskStatus::Running, None)
            .await
            .unwrap();
        publish_event(
            &engine_clone,
            "global-sse-1",
            "progress",
            Level::Info,
            json!({ "task": 1, "step": "hello" }),
        )
        .await;

        create_task(&engine_clone, "global-sse-2").await;
        engine_clone
            .transition_task("global-sse-2", TaskStatus::Running, None)
            .await
            .unwrap();
        publish_event(
            &engine_clone,
            "global-sse-2",
            "log",
            Level::Info,
            json!({ "task": 2, "msg": "world" }),
        )
        .await;
    });

    // Read chunks in main task body
    let mut collected = String::new();
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(3);
    while tokio::time::Instant::now() < deadline {
        match tokio::time::timeout(
            std::time::Duration::from_millis(200),
            response.chunk(),
        )
        .await
        {
            Ok(Ok(Some(chunk))) => {
                collected.push_str(&String::from_utf8_lossy(&chunk));
                // Wait until we see both events
                if collected.contains("hello") && collected.contains("world") {
                    break;
                }
            }
            Ok(Ok(None)) => break,
            Ok(Err(_)) => break,
            Err(_) => continue,
        }
    }

    assert!(
        collected.contains("event: taskcast.event"),
        "should contain SSE event lines. Got:\n{collected}"
    );
    assert!(
        collected.contains("hello"),
        "should contain event from task 1. Got:\n{collected}"
    );
    assert!(
        collected.contains("world"),
        "should contain event from task 2. Got:\n{collected}"
    );
}

#[tokio::test]
async fn global_sse_streams_events_with_task_id_in_envelope() {
    let engine = make_engine();
    let app = make_app(Arc::clone(&engine));
    let addr = serve_app(app).await;

    // Connect to /events from main task
    let client = reqwest::Client::new();
    let mut response = client.get(format!("http://{addr}/events")).send().await.unwrap();
    assert_eq!(response.status(), 200);

    // Spawn producer
    let engine_clone = Arc::clone(&engine);
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        create_task(&engine_clone, "global-sse-envelope-1").await;
        engine_clone
            .transition_task("global-sse-envelope-1", TaskStatus::Running, None)
            .await
            .unwrap();

        publish_event(
            &engine_clone,
            "global-sse-envelope-1",
            "progress",
            Level::Info,
            json!({ "value": 42 }),
        )
        .await;
    });

    // Read chunks in main task body
    let mut collected = String::new();
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(3);
    while tokio::time::Instant::now() < deadline {
        match tokio::time::timeout(
            std::time::Duration::from_millis(200),
            response.chunk(),
        )
        .await
        {
            Ok(Ok(Some(chunk))) => {
                collected.push_str(&String::from_utf8_lossy(&chunk));
                if collected.contains("\"value\":42") || collected.contains("\"value\": 42") {
                    break;
                }
            }
            Ok(Ok(None)) => break,
            Ok(Err(_)) => break,
            Err(_) => continue,
        }
    }

    // Verify the envelope contains taskId
    assert!(
        collected.contains("global-sse-envelope-1"),
        "envelope should contain taskId. Got:\n{collected}"
    );
    assert!(
        collected.contains("event: taskcast.event"),
        "should have SSE event type. Got:\n{collected}"
    );
}

#[tokio::test]
async fn global_sse_type_filter_with_wildcard() {
    let engine = make_engine();
    let app = make_app(Arc::clone(&engine));
    let addr = serve_app(app).await;

    // Connect with type filter from main task
    let client = reqwest::Client::new();
    let mut response = client
        .get(format!("http://{addr}/events?types=llm.*"))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);

    // Spawn producer
    let engine_clone = Arc::clone(&engine);
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        create_task(&engine_clone, "global-sse-filter-1").await;
        engine_clone
            .transition_task("global-sse-filter-1", TaskStatus::Running, None)
            .await
            .unwrap();

        // This event should NOT match the filter "llm.*"
        publish_event(
            &engine_clone,
            "global-sse-filter-1",
            "progress",
            Level::Info,
            json!({ "should_be_filtered": true }),
        )
        .await;

        // This event SHOULD match the filter "llm.*"
        publish_event(
            &engine_clone,
            "global-sse-filter-1",
            "llm.delta",
            Level::Info,
            json!({ "token": "hello" }),
        )
        .await;

        // This event should also NOT match
        publish_event(
            &engine_clone,
            "global-sse-filter-1",
            "log",
            Level::Info,
            json!({ "also_filtered": true }),
        )
        .await;
    });

    // Read chunks in main task body
    let mut collected = String::new();
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(3);
    while tokio::time::Instant::now() < deadline {
        match tokio::time::timeout(
            std::time::Duration::from_millis(200),
            response.chunk(),
        )
        .await
        {
            Ok(Ok(Some(chunk))) => {
                collected.push_str(&String::from_utf8_lossy(&chunk));
                if collected.contains("hello") {
                    // Wait a bit more to ensure no additional events sneak through
                    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
                    // Collect any remaining chunks
                    while let Ok(Ok(Some(chunk))) = tokio::time::timeout(
                        std::time::Duration::from_millis(100),
                        response.chunk(),
                    )
                    .await
                    {
                        collected.push_str(&String::from_utf8_lossy(&chunk));
                    }
                    break;
                }
            }
            Ok(Ok(None)) => break,
            Ok(Err(_)) => break,
            Err(_) => continue,
        }
    }

    // Should contain the llm.delta event
    assert!(
        collected.contains("hello"),
        "should contain the llm.delta event data. Got:\n{collected}"
    );

    // Should NOT contain filtered-out events
    assert!(
        !collected.contains("should_be_filtered"),
        "should not contain progress event (filtered out). Got:\n{collected}"
    );
    assert!(
        !collected.contains("also_filtered"),
        "should not contain log event (filtered out). Got:\n{collected}"
    );
}

#[tokio::test]
async fn global_sse_level_filter() {
    let engine = make_engine();
    let app = make_app(Arc::clone(&engine));
    let addr = serve_app(app).await;

    // Connect with level filter from main task
    let client = reqwest::Client::new();
    let mut response = client
        .get(format!("http://{addr}/events?levels=warn"))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);

    // Spawn producer
    let engine_clone = Arc::clone(&engine);
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        create_task(&engine_clone, "global-sse-level-1").await;
        engine_clone
            .transition_task("global-sse-level-1", TaskStatus::Running, None)
            .await
            .unwrap();

        // Info event — should NOT pass filter (we filter for warn only)
        publish_event(
            &engine_clone,
            "global-sse-level-1",
            "progress",
            Level::Info,
            json!({ "info_event": true }),
        )
        .await;

        // Warn event — SHOULD pass filter
        publish_event(
            &engine_clone,
            "global-sse-level-1",
            "warning",
            Level::Warn,
            json!({ "warn_event": true }),
        )
        .await;
    });

    // Read chunks in main task body
    let mut collected = String::new();
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(3);
    while tokio::time::Instant::now() < deadline {
        match tokio::time::timeout(
            std::time::Duration::from_millis(200),
            response.chunk(),
        )
        .await
        {
            Ok(Ok(Some(chunk))) => {
                collected.push_str(&String::from_utf8_lossy(&chunk));
                if collected.contains("warn_event") {
                    // Wait a bit more to ensure no additional events sneak through
                    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
                    // Collect any remaining chunks
                    while let Ok(Ok(Some(chunk))) = tokio::time::timeout(
                        std::time::Duration::from_millis(100),
                        response.chunk(),
                    )
                    .await
                    {
                        collected.push_str(&String::from_utf8_lossy(&chunk));
                    }
                    break;
                }
            }
            Ok(Ok(None)) => break,
            Ok(Err(_)) => break,
            Err(_) => continue,
        }
    }

    // Should contain the warn event
    assert!(
        collected.contains("warn_event"),
        "should contain the warn-level event. Got:\n{collected}"
    );

    // Should NOT contain the info event
    assert!(
        !collected.contains("info_event"),
        "should not contain the info-level event (filtered out). Got:\n{collected}"
    );
}

#[tokio::test]
async fn global_sse_requires_event_subscribe_scope() {
    let (engine, app) = make_jwt_engine_and_app();
    let addr = serve_app(app).await;
    let client = reqwest::Client::new();

    // Token with only task:create scope (not event:subscribe)
    let limited_token = make_token(json!({
        "sub": "test-user",
        "scope": ["task:create"],
        "taskIds": "*",
        "exp": 9999999999u64
    }));

    // Should be forbidden
    let response = client
        .get(format!("http://{addr}/events"))
        .header("Authorization", bearer_header(&limited_token))
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), 403);

    // Token with event:subscribe scope should work
    let valid_token = make_token(json!({
        "sub": "test-user",
        "scope": ["event:subscribe"],
        "taskIds": "*",
        "exp": 9999999999u64
    }));

    // Spawn a task to create something so SSE has data
    let engine_clone = Arc::clone(&engine);
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        create_task(&engine_clone, "auth-sse-task").await;
    });

    let response = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        client
            .get(format!("http://{addr}/events"))
            .header("Authorization", bearer_header(&valid_token))
            .send(),
    )
    .await
    .expect("connect timed out")
    .unwrap();

    assert_eq!(response.status(), 200);
}

#[tokio::test]
async fn global_sse_does_not_replay_existing_tasks() {
    let engine = make_engine();

    // Create a task BEFORE establishing SSE connection
    create_task(&engine, "pre-existing-task").await;
    engine
        .transition_task("pre-existing-task", TaskStatus::Running, None)
        .await
        .unwrap();
    publish_event(
        &engine,
        "pre-existing-task",
        "progress",
        Level::Info,
        json!({ "pre_existing": true }),
    )
    .await;

    let app = make_app(Arc::clone(&engine));
    let addr = serve_app(app).await;

    // Connect to /events from main task
    let client = reqwest::Client::new();
    let mut response = client.get(format!("http://{addr}/events")).send().await.unwrap();
    assert_eq!(response.status(), 200);

    // Spawn producer
    let engine_clone = Arc::clone(&engine);
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        // Create a NEW task after SSE connection
        create_task(&engine_clone, "post-connect-task").await;
        engine_clone
            .transition_task("post-connect-task", TaskStatus::Running, None)
            .await
            .unwrap();
        publish_event(
            &engine_clone,
            "post-connect-task",
            "progress",
            Level::Info,
            json!({ "post_connect": true }),
        )
        .await;
    });

    // Read chunks in main task body
    let mut collected = String::new();
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(3);
    while tokio::time::Instant::now() < deadline {
        match tokio::time::timeout(
            std::time::Duration::from_millis(200),
            response.chunk(),
        )
        .await
        {
            Ok(Ok(Some(chunk))) => {
                collected.push_str(&String::from_utf8_lossy(&chunk));
                if collected.contains("post_connect") {
                    // Wait a bit more to ensure no pre-existing events sneak through
                    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
                    // Collect any remaining chunks
                    while let Ok(Ok(Some(chunk))) = tokio::time::timeout(
                        std::time::Duration::from_millis(100),
                        response.chunk(),
                    )
                    .await
                    {
                        collected.push_str(&String::from_utf8_lossy(&chunk));
                    }
                    break;
                }
            }
            Ok(Ok(None)) => break,
            Ok(Err(_)) => break,
            Err(_) => continue,
        }
    }

    // Should contain events from the post-connect task
    assert!(
        collected.contains("post_connect"),
        "should contain events from newly created task. Got:\n{collected}"
    );

    // Should NOT contain events from the pre-existing task
    assert!(
        !collected.contains("pre_existing"),
        "should NOT contain events from pre-existing task. Got:\n{collected}"
    );
}

// ─── Global SSE 501 when subscribe_sync is unsupported ───────────────────────

/// A broadcast provider that does NOT implement subscribe_sync (uses the default
/// trait method which returns Err), used to test the 501 error path.
struct NoSyncBroadcastProvider;

#[async_trait::async_trait]
impl taskcast_core::BroadcastProvider for NoSyncBroadcastProvider {
    async fn publish(
        &self,
        _channel: &str,
        _event: taskcast_core::TaskEvent,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        Ok(())
    }

    async fn subscribe(
        &self,
        _channel: &str,
        _handler: Box<dyn Fn(taskcast_core::TaskEvent) + Send + Sync>,
    ) -> Box<dyn Fn() + Send + Sync> {
        Box::new(|| {})
    }
    // subscribe_sync intentionally NOT overridden — uses default Err
}

#[tokio::test]
async fn global_sse_returns_501_when_subscribe_sync_unsupported() {
    let engine = Arc::new(TaskEngine::new(TaskEngineOptions {
        short_term_store: Arc::new(MemoryShortTermStore::new()),
        broadcast: Arc::new(NoSyncBroadcastProvider),
        long_term_store: None,
        hooks: None,
    }));
    let app = make_app(Arc::clone(&engine));
    let addr = serve_app(app).await;
    let client = reqwest::Client::new();

    let response = client
        .get(format!("http://{addr}/events"))
        .send()
        .await
        .unwrap();

    assert_eq!(
        response.status(),
        501,
        "should return 501 when subscribe_sync is not supported"
    );
    let body = response.text().await.unwrap();
    assert!(
        body.contains("not supported"),
        "response should mention unsupported. Got: {body}"
    );
}

// ─── Engine-level creation listener tests ───────────────────────────────────

#[tokio::test]
async fn engine_creation_listener_fires_on_task_creation() {
    let engine = make_engine();

    let received = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let received_clone = Arc::clone(&received);

    let listener: taskcast_core::CreationListener = Arc::new(move |task| {
        received_clone.lock().unwrap().push(task.id.clone());
    });

    engine.add_creation_listener(listener);

    create_task(&engine, "listen-1").await;
    create_task(&engine, "listen-2").await;

    let ids = received.lock().unwrap().clone();
    assert_eq!(ids, vec!["listen-1", "listen-2"]);
}

#[tokio::test]
async fn engine_remove_creation_listener_stops_notifications() {
    let engine = make_engine();

    let received = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let received_clone = Arc::clone(&received);

    let listener: taskcast_core::CreationListener = Arc::new(move |task| {
        received_clone.lock().unwrap().push(task.id.clone());
    });

    engine.add_creation_listener(listener.clone());
    create_task(&engine, "before-remove").await;

    engine.remove_creation_listener(&listener);
    create_task(&engine, "after-remove").await;

    let ids = received.lock().unwrap().clone();
    assert_eq!(ids, vec!["before-remove"]);
}
