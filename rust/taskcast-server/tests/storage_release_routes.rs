use std::sync::Arc;

use axum_test::http::HeaderValue;
use axum_test::TestServer;
use jsonwebtoken::{encode, EncodingKey, Header};
use serde_json::json;
use taskcast_core::{
    CreateTaskInput, Level, LongTermStore, MemoryBroadcastProvider, MemoryLongTermStore,
    MemoryShortTermStore, PublishEventInput, ShortTermStore, StorageWriterRegistration, TaskEngine,
    TaskEngineOptions,
};
use taskcast_server::{create_app, AuthMode, CorsConfig, JwtConfig};

const JWT_SECRET: &str = "storage-release-route-test-secret";

fn make_engine(with_long_term: bool) -> Arc<TaskEngine> {
    Arc::new(TaskEngine::new(TaskEngineOptions {
        short_term_store: Arc::new(MemoryShortTermStore::new()),
        broadcast: Arc::new(MemoryBroadcastProvider::new()),
        long_term_store: with_long_term
            .then(|| Arc::new(MemoryLongTermStore::new()) as Arc<dyn taskcast_core::LongTermStore>),
        hooks: None,
    }))
}

fn make_server(engine: Arc<TaskEngine>, auth_mode: AuthMode) -> TestServer {
    let (app, _) = create_app(engine, auth_mode, None, None, CorsConfig::default());
    TestServer::new(app)
}

fn token(scope: &[&str]) -> String {
    encode(
        &Header::default(),
        &json!({
            "scope": scope,
            "taskIds": "*",
            "exp": 9999999999u64
        }),
        &EncodingKey::from_secret(JWT_SECRET.as_bytes()),
    )
    .unwrap()
}

#[tokio::test]
async fn release_route_is_idempotent_and_uses_camel_case_response() {
    let engine = make_engine(true);
    let server = make_server(engine.clone(), AuthMode::None);
    engine
        .create_task(CreateTaskInput {
            id: Some("release-me".to_string()),
            ..Default::default()
        })
        .await
        .unwrap();
    let event = engine
        .publish_event(
            "release-me",
            PublishEventInput {
                r#type: "demo.event".to_string(),
                level: Level::Info,
                data: json!({ "value": 1 }),
                series_id: None,
                series_mode: None,
                series_acc_field: None,
            },
        )
        .await
        .unwrap();

    let first = server
        .post("/tasks/release-me/storage/release")
        .json(&json!({
            "expectedLastEventIndex": event.index,
            "inactiveSince": event.timestamp + 1_000.0
        }))
        .await;
    first.assert_status_ok();
    assert_eq!(
        first.json::<serde_json::Value>(),
        json!({
            "taskId": "release-me",
            "storageState": "cold",
            "archiveWatermark": event.index,
            "released": true
        })
    );

    let second = server
        .post("/tasks/release-me/storage/release")
        .json(&json!({
            "expectedLastEventIndex": event.index,
            "inactiveSince": event.timestamp + 1_000.0
        }))
        .await;
    second.assert_status_ok();
    assert_eq!(second.json::<serde_json::Value>()["released"], false);
}

#[tokio::test]
async fn release_route_maps_missing_stale_and_unsupported_errors() {
    let engine = make_engine(true);
    let server = make_server(engine.clone(), AuthMode::None);
    let missing = server
        .post("/tasks/missing/storage/release")
        .json(&json!({ "expectedLastEventIndex": -1, "inactiveSince": 1_000.0 }))
        .await;
    missing.assert_status(axum_test::http::StatusCode::NOT_FOUND);

    engine
        .create_task(CreateTaskInput {
            id: Some("stale".to_string()),
            ..Default::default()
        })
        .await
        .unwrap();
    engine
        .publish_event(
            "stale",
            PublishEventInput {
                r#type: "demo.event".to_string(),
                level: Level::Info,
                data: json!(null),
                series_id: None,
                series_mode: None,
                series_acc_field: None,
            },
        )
        .await
        .unwrap();
    let stale = server
        .post("/tasks/stale/storage/release")
        .json(&json!({ "expectedLastEventIndex": -1, "inactiveSince": 9_999_999_999_999f64 }))
        .await;
    stale.assert_status(axum_test::http::StatusCode::CONFLICT);
    assert_eq!(
        stale.json::<serde_json::Value>()["code"],
        "storage_precondition_failed"
    );

    let unsupported_engine = make_engine(false);
    unsupported_engine
        .create_task(CreateTaskInput {
            id: Some("unsupported".to_string()),
            ..Default::default()
        })
        .await
        .unwrap();
    let unsupported = make_server(unsupported_engine, AuthMode::None)
        .post("/tasks/unsupported/storage/release")
        .json(&json!({ "expectedLastEventIndex": -1, "inactiveSince": 9_999_999_999_999f64 }))
        .await;
    unsupported.assert_status(axum_test::http::StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(
        unsupported.json::<serde_json::Value>()["code"],
        "storage_release_unsupported"
    );
}

#[tokio::test]
async fn release_route_requires_manage_scope_and_is_in_openapi() {
    let engine = make_engine(true);
    engine
        .create_task(CreateTaskInput {
            id: Some("managed".to_string()),
            ..Default::default()
        })
        .await
        .unwrap();
    let server = make_server(
        engine,
        AuthMode::Jwt(JwtConfig {
            algorithm: jsonwebtoken::Algorithm::HS256,
            secret: Some(JWT_SECRET.to_string()),
            public_key: None,
            issuer: None,
            audience: None,
        }),
    );
    let denied = server
        .post("/tasks/managed/storage/release")
        .add_header(
            axum_test::http::header::AUTHORIZATION,
            HeaderValue::from_str(&format!("Bearer {}", token(&["event:subscribe"]))).unwrap(),
        )
        .json(&json!({ "expectedLastEventIndex": -1, "inactiveSince": 1_000.0 }))
        .await;
    denied.assert_status(axum_test::http::StatusCode::FORBIDDEN);

    let spec = server
        .get("/openapi.json")
        .await
        .json::<serde_json::Value>();
    assert!(spec["paths"]["/tasks/{taskId}/storage/release"]["post"].is_object());
}

#[tokio::test]
async fn release_route_retains_busy_request_and_blocks_old_writers() {
    let hot = Arc::new(MemoryShortTermStore::new());
    let durable = Arc::new(MemoryLongTermStore::new());
    let engine = Arc::new(TaskEngine::new(TaskEngineOptions {
        short_term_store: hot.clone(),
        broadcast: Arc::new(MemoryBroadcastProvider::new()),
        long_term_store: Some(durable.clone()),
        hooks: None,
    }));
    engine
        .create_task(CreateTaskInput {
            id: Some("busy".to_string()),
            ..Default::default()
        })
        .await
        .unwrap();
    let lease = hot
        .acquire_storage_lock("busy", "other-lock", "other-generation", 30_000)
        .await
        .unwrap()
        .unwrap();
    let busy = make_server(engine, AuthMode::None)
        .post("/tasks/busy/storage/release")
        .json(&json!({
            "expectedLastEventIndex": -1,
            "inactiveSince": 9_999_999_999_999f64
        }))
        .await;
    busy.assert_status(axum_test::http::StatusCode::CONFLICT);
    assert_eq!(busy.json::<serde_json::Value>()["code"], "storage_busy");
    assert_eq!(
        durable
            .list_storage_release_requests(10)
            .await
            .unwrap()
            .len(),
        1
    );
    hot.release_storage_lock(&lease).await.unwrap();

    let hot = Arc::new(MemoryShortTermStore::new());
    let durable = Arc::new(MemoryLongTermStore::new());
    let engine = Arc::new(TaskEngine::new(TaskEngineOptions {
        short_term_store: hot.clone(),
        broadcast: Arc::new(MemoryBroadcastProvider::new()),
        long_term_store: Some(durable.clone()),
        hooks: None,
    }));
    engine
        .create_task(CreateTaskInput {
            id: Some("old-writer".to_string()),
            ..Default::default()
        })
        .await
        .unwrap();
    hot.register_storage_writer(
        StorageWriterRegistration {
            instance_id: "legacy-writer".to_string(),
            storage_protocol_version: 1,
            build: "old".to_string(),
            expires_at: 0.0,
        },
        30_000,
    )
    .await
    .unwrap();
    let blocked = make_server(engine, AuthMode::None)
        .post("/tasks/old-writer/storage/release")
        .json(&json!({
            "expectedLastEventIndex": -1,
            "inactiveSince": 9_999_999_999_999f64
        }))
        .await;
    blocked.assert_status(axum_test::http::StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(
        blocked.json::<serde_json::Value>()["code"],
        "storage_unavailable"
    );
    assert!(durable
        .list_storage_release_requests(10)
        .await
        .unwrap()
        .is_empty());
}
