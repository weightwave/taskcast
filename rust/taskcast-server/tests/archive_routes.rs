use std::sync::Arc;

use axum_test::http::{header, HeaderValue, StatusCode};
use axum_test::TestServer;
use jsonwebtoken::{encode, EncodingKey, Header};
use serde_json::json;
use taskcast_core::{
    CreateTaskInput, Level, MemoryBroadcastProvider, MemoryShortTermStore, PublishEventInput,
    TaskArchive, TaskEngine, TaskEngineOptions, TaskStatus,
};
use taskcast_server::{create_app, AuthMode, CorsConfig, JwtConfig};

const JWT_SECRET: &str = "archive-route-test-secret-key-needs-to-be-long-enough";

fn make_engine() -> Arc<TaskEngine> {
    Arc::new(TaskEngine::new(TaskEngineOptions {
        short_term_store: Arc::new(MemoryShortTermStore::new()),
        broadcast: Arc::new(MemoryBroadcastProvider::new()),
        long_term_store: None,
        hooks: None,
    }))
}

fn make_no_auth_server() -> (Arc<TaskEngine>, TestServer) {
    let engine = make_engine();
    let (app, _) = create_app(
        Arc::clone(&engine),
        AuthMode::None,
        None,
        None,
        CorsConfig::default(),
    );
    (engine, TestServer::new(app))
}

fn make_jwt_server() -> (Arc<TaskEngine>, TestServer) {
    let engine = make_engine();
    let auth = AuthMode::Jwt(JwtConfig {
        algorithm: jsonwebtoken::Algorithm::HS256,
        secret: Some(JWT_SECRET.to_string()),
        public_key: None,
        issuer: None,
        audience: None,
    });
    let (app, _) = create_app(engine.clone(), auth, None, None, CorsConfig::default());
    (engine, TestServer::new(app))
}

fn make_token(scope: &[&str], task_ids: serde_json::Value) -> String {
    encode(
        &Header::default(),
        &json!({
            "sub": "archive-route-test",
            "scope": scope,
            "taskIds": task_ids,
            "exp": 9999999999u64
        }),
        &EncodingKey::from_secret(JWT_SECRET.as_bytes()),
    )
    .unwrap()
}

fn bearer_header(token: &str) -> HeaderValue {
    HeaderValue::from_str(&format!("Bearer {token}")).unwrap()
}

async fn build_archive(task_id: &str) -> TaskArchive {
    let engine = make_engine();
    create_running_task(&engine, task_id).await;
    publish_event(&engine, task_id, "log", json!({ "message": "first" })).await;
    publish_event(&engine, task_id, "progress", json!({ "pct": 50 })).await;
    engine.export_task_archive(task_id).await.unwrap()
}

async fn create_running_task(engine: &TaskEngine, task_id: &str) {
    engine
        .create_task(CreateTaskInput {
            id: Some(task_id.to_string()),
            r#type: Some("archive-test".to_string()),
            ..Default::default()
        })
        .await
        .unwrap();
    engine
        .transition_task(task_id, TaskStatus::Running, None)
        .await
        .unwrap();
}

async fn publish_event(
    engine: &TaskEngine,
    task_id: &str,
    event_type: &str,
    data: serde_json::Value,
) {
    engine
        .publish_event(
            task_id,
            PublishEventInput {
                r#type: event_type.to_string(),
                level: Level::Info,
                data,
                series_id: None,
                series_mode: None,
                series_acc_field: None,
            },
        )
        .await
        .unwrap();
}

#[tokio::test]
async fn import_archive_restores_task_and_events() {
    let archive = build_archive("archive-import-ok").await;
    let event_count = archive.events.len();
    let (engine, server) = make_no_auth_server();

    let response = server
        .post("/tasks/import")
        .json(&json!({ "archive": archive }))
        .await;

    response.assert_status_ok();
    let body: serde_json::Value = response.json();
    assert_eq!(body["ok"], true);
    assert_eq!(body["taskId"], "archive-import-ok");
    assert_eq!(body["eventCount"], event_count);
    assert_eq!(body["overwritten"], false);

    assert!(engine
        .get_task("archive-import-ok")
        .await
        .unwrap()
        .is_some());
    let events = engine.get_events("archive-import-ok", None).await.unwrap();
    assert_eq!(events.len(), event_count);
}

#[tokio::test]
async fn export_missing_task_archive_returns_404() {
    let (_engine, server) = make_no_auth_server();

    let response = server.get("/tasks/missing-task/archive").await;

    response.assert_status(StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn import_archive_conflict_returns_409_without_overwrite() {
    let archive = build_archive("archive-conflict").await;
    let (_engine, server) = make_no_auth_server();

    server
        .post("/tasks/import")
        .json(&json!({ "archive": archive.clone() }))
        .await
        .assert_status_ok();

    let response = server
        .post("/tasks/import")
        .json(&json!({ "archive": archive }))
        .await;

    response.assert_status(StatusCode::CONFLICT);
}

#[tokio::test]
async fn exported_archive_can_roundtrip_through_import_route() {
    let (source_engine, source_server) = make_no_auth_server();
    create_running_task(&source_engine, "archive-roundtrip").await;
    publish_event(
        &source_engine,
        "archive-roundtrip",
        "log",
        json!({ "message": "roundtrip" }),
    )
    .await;

    let export_response = source_server.get("/tasks/archive-roundtrip/archive").await;
    export_response.assert_status_ok();
    let archive: TaskArchive = export_response.json();
    assert_eq!(archive.schema, "taskcast.taskArchive");
    assert_eq!(archive.version, 1);
    assert_eq!(archive.task.id, "archive-roundtrip");
    assert!(archive.events.iter().any(|event| event.r#type == "log"));
    let event_count = archive.events.len();

    let (target_engine, target_server) = make_no_auth_server();
    let import_response = target_server
        .post("/tasks/import")
        .json(&json!({ "archive": archive }))
        .await;
    import_response.assert_status_ok();

    let imported_events = target_engine
        .get_events("archive-roundtrip", None)
        .await
        .unwrap();
    assert_eq!(imported_events.len(), event_count);
}

#[tokio::test]
async fn import_malformed_archive_returns_400() {
    let archive = build_archive("archive-malformed").await;
    let mut body = json!({ "archive": archive });
    body["archive"]["schema"] = json!("taskcast.unsupportedArchive");
    let (_engine, server) = make_no_auth_server();

    let response = server.post("/tasks/import").json(&body).await;

    response.assert_status(StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn import_archive_deserialize_error_returns_400() {
    let archive = build_archive("archive-deserialize-error").await;
    let mut body = json!({ "archive": archive });
    body["archive"]["events"][0]["seriesSnapshot"] = json!(null);
    let (_engine, server) = make_no_auth_server();

    let response = server.post("/tasks/import").json(&body).await;

    response.assert_status(StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn archive_routes_enforce_required_jwt_scopes() {
    let (engine, server) = make_jwt_server();
    create_running_task(&engine, "archive-scope").await;
    let archive = engine.export_task_archive("archive-scope").await.unwrap();

    let no_history_token = make_token(&["event:subscribe"], json!("*"));
    let export_response = server
        .get("/tasks/archive-scope/archive")
        .add_header(header::AUTHORIZATION, bearer_header(&no_history_token))
        .await;
    export_response.assert_status(StatusCode::FORBIDDEN);

    let no_manage_token = make_token(&["event:history"], json!("*"));
    let import_response = server
        .post("/tasks/import")
        .add_header(header::AUTHORIZATION, bearer_header(&no_manage_token))
        .json(&json!({ "archive": archive, "overwrite": true }))
        .await;
    import_response.assert_status(StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn openapi_includes_archive_paths_and_schemas() {
    let (_engine, server) = make_no_auth_server();

    let response = server.get("/openapi.json").await;
    response.assert_status_ok();
    let body: serde_json::Value = response.json();

    assert!(body["paths"]["/tasks/{task_id}/archive"].is_object());
    assert!(body["paths"]["/tasks/import"].is_object());
    assert_eq!(body["info"]["version"], env!("CARGO_PKG_VERSION"));
    assert!(body["components"]["schemas"]["TaskArchive"].is_object());
    assert!(body["components"]["schemas"]["TaskArchiveImportResult"].is_object());
    assert!(body["components"]["schemas"]["ImportTaskArchiveBody"].is_object());
    assert!(body["components"]["schemas"]["ImportTaskArchiveResponse"].is_object());

    let schemas = &body["components"]["schemas"];
    let archive_event_schema = resolve_archive_event_schema(schemas);
    let archive_event_properties = archive_event_schema["properties"]
        .as_object()
        .expect("archive event schema should have properties");
    assert!(
        !archive_event_properties.contains_key("seriesSnapshot"),
        "archive event schema must not expose seriesSnapshot"
    );
    assert!(
        !archive_event_properties.contains_key("_accumulatedData"),
        "archive event schema must not expose _accumulatedData"
    );
}

fn resolve_archive_event_schema(schemas: &serde_json::Value) -> &serde_json::Value {
    let item_schema = &schemas["TaskArchive"]["properties"]["events"]["items"];
    if let Some(ref_path) = item_schema["$ref"].as_str() {
        let schema_name = ref_path
            .strip_prefix("#/components/schemas/")
            .expect("archive event schema ref should point at components.schemas");
        &schemas[schema_name]
    } else {
        item_schema
    }
}
