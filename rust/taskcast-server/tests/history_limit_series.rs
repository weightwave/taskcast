//! Integration tests for history endpoint limit + seriesFormat query parameters.

use std::sync::Arc;

use axum_test::TestServer;
use serde_json::json;
use taskcast_core::{
    CreateTaskInput, Level, MemoryBroadcastProvider, MemoryShortTermStore, PublishEventInput,
    SeriesMode, TaskEngine, TaskEngineOptions, TaskStatus,
};
use taskcast_server::{create_app, AuthMode, CorsConfig};

fn make_engine() -> Arc<TaskEngine> {
    Arc::new(TaskEngine::new(TaskEngineOptions {
        short_term_store: Arc::new(MemoryShortTermStore::new()),
        broadcast: Arc::new(MemoryBroadcastProvider::new()),
        long_term_store: None,
        hooks: None,
    }))
}

fn make_server(engine: Arc<TaskEngine>) -> TestServer {
    let (app, _) = create_app(engine, AuthMode::None, None, None, CorsConfig::default());
    TestServer::new(app)
}

async fn create_running_task(engine: &TaskEngine, id: &str) {
    engine
        .create_task(CreateTaskInput {
            id: Some(id.to_string()),
            ..Default::default()
        })
        .await
        .unwrap();
    engine
        .transition_task(id, TaskStatus::Running, None)
        .await
        .unwrap();
}

// ─── limit parameter ────────────────────────────────────────────────────────

#[tokio::test]
async fn limit_caps_returned_events() {
    let engine = make_engine();
    let server = make_server(Arc::clone(&engine));
    create_running_task(&engine, "limit-cap").await;

    for i in 0..5 {
        engine
            .publish_event(
                "limit-cap",
                PublishEventInput {
                    r#type: "progress".to_string(),
                    level: Level::Info,
                    data: json!({ "i": i }),
                    series_id: None,
                    series_mode: None,
                    series_acc_field: None,
                },
            )
            .await
            .unwrap();
    }

    let resp = server
        .get("/tasks/limit-cap/events/history?limit=3")
        .await;
    resp.assert_status(axum_test::http::StatusCode::OK);
    let body: Vec<serde_json::Value> = resp.json();
    assert_eq!(body.len(), 3);
    // First event is the taskcast:status event from transition
    assert_eq!(body[0]["type"], "taskcast:status");
    assert_eq!(body[1]["data"]["i"], 0);
    assert_eq!(body[2]["data"]["i"], 1);
}

#[tokio::test]
async fn returns_all_events_when_limit_not_specified() {
    let engine = make_engine();
    let server = make_server(Arc::clone(&engine));
    create_running_task(&engine, "no-limit").await;

    for i in 0..5 {
        engine
            .publish_event(
                "no-limit",
                PublishEventInput {
                    r#type: "progress".to_string(),
                    level: Level::Info,
                    data: json!({ "i": i }),
                    series_id: None,
                    series_mode: None,
                    series_acc_field: None,
                },
            )
            .await
            .unwrap();
    }

    let resp = server.get("/tasks/no-limit/events/history").await;
    let body: Vec<serde_json::Value> = resp.json();
    // 1 status + 5 published = 6
    assert_eq!(body.len(), 6);
}

// ─── seriesFormat=accumulated ───────────────────────────────────────────────

#[tokio::test]
async fn series_format_accumulated_collapses_series() {
    let engine = make_engine();
    let server = make_server(Arc::clone(&engine));
    create_running_task(&engine, "acc-collapse").await;

    for delta in &["A", "B", "C"] {
        engine
            .publish_event(
                "acc-collapse",
                PublishEventInput {
                    r#type: "llm.token".to_string(),
                    level: Level::Info,
                    data: json!({ "delta": delta }),
                    series_id: Some("tokens".to_string()),
                    series_mode: Some(SeriesMode::Accumulate),
                    series_acc_field: Some("delta".to_string()),
                },
            )
            .await
            .unwrap();
    }

    engine
        .publish_event(
            "acc-collapse",
            PublishEventInput {
                r#type: "log".to_string(),
                level: Level::Info,
                data: json!({ "msg": "done" }),
                series_id: None,
                series_mode: None,
                series_acc_field: None,
            },
        )
        .await
        .unwrap();

    let resp = server
        .get("/tasks/acc-collapse/events/history?seriesFormat=accumulated")
        .await;
    resp.assert_status(axum_test::http::StatusCode::OK);
    let body: Vec<serde_json::Value> = resp.json();

    // 1 status + 1 collapsed snapshot + 1 non-series = 3 events
    assert_eq!(body.len(), 3);
    let snapshot = body
        .iter()
        .find(|e| e["seriesId"] == "tokens")
        .expect("should have tokens series event");
    assert_eq!(snapshot["seriesSnapshot"], true);
    assert_eq!(snapshot["data"]["delta"], "ABC");
}

// ─── seriesFormat=delta (default) ───────────────────────────────────────────

#[tokio::test]
async fn series_format_delta_returns_raw_deltas() {
    let engine = make_engine();
    let server = make_server(Arc::clone(&engine));
    create_running_task(&engine, "delta-fmt").await;

    for delta in &["A", "B"] {
        engine
            .publish_event(
                "delta-fmt",
                PublishEventInput {
                    r#type: "llm.token".to_string(),
                    level: Level::Info,
                    data: json!({ "delta": delta }),
                    series_id: Some("tokens".to_string()),
                    series_mode: Some(SeriesMode::Accumulate),
                    series_acc_field: Some("delta".to_string()),
                },
            )
            .await
            .unwrap();
    }

    let resp = server
        .get("/tasks/delta-fmt/events/history?seriesFormat=delta")
        .await;
    let body: Vec<serde_json::Value> = resp.json();
    // 1 status + 2 deltas = 3
    assert_eq!(body.len(), 3);
    let deltas: Vec<&serde_json::Value> = body
        .iter()
        .filter(|e| e["type"] == "llm.token")
        .collect();
    assert_eq!(deltas[0]["data"]["delta"], "A");
    assert_eq!(deltas[1]["data"]["delta"], "B");
}

// ─── limit + seriesFormat=accumulated combined ──────────────────────────────

#[tokio::test]
async fn limit_with_series_format_accumulated() {
    let engine = make_engine();
    let server = make_server(Arc::clone(&engine));
    create_running_task(&engine, "limit-acc").await;

    for delta in &["A", "B", "C"] {
        engine
            .publish_event(
                "limit-acc",
                PublishEventInput {
                    r#type: "llm.token".to_string(),
                    level: Level::Info,
                    data: json!({ "delta": delta }),
                    series_id: Some("tokens".to_string()),
                    series_mode: Some(SeriesMode::Accumulate),
                    series_acc_field: Some("delta".to_string()),
                },
            )
            .await
            .unwrap();
    }

    for i in 0..2 {
        engine
            .publish_event(
                "limit-acc",
                PublishEventInput {
                    r#type: "log".to_string(),
                    level: Level::Info,
                    data: json!({ "i": i }),
                    series_id: None,
                    series_mode: None,
                    series_acc_field: None,
                },
            )
            .await
            .unwrap();
    }

    let resp = server
        .get("/tasks/limit-acc/events/history?limit=5&seriesFormat=accumulated")
        .await;
    resp.assert_status(axum_test::http::StatusCode::OK);
    let body: Vec<serde_json::Value> = resp.json();
    assert!(body.len() <= 5);
    let snapshot = body.iter().find(|e| e["seriesSnapshot"] == true);
    assert!(snapshot.is_some(), "should have a series snapshot");
}

// ─── invalid seriesFormat treated as delta ───────────────────────────────────

#[tokio::test]
async fn invalid_series_format_treated_as_delta() {
    let engine = make_engine();
    let server = make_server(Arc::clone(&engine));
    create_running_task(&engine, "inv-fmt").await;

    engine
        .publish_event(
            "inv-fmt",
            PublishEventInput {
                r#type: "llm.token".to_string(),
                level: Level::Info,
                data: json!({ "delta": "A" }),
                series_id: Some("s1".to_string()),
                series_mode: Some(SeriesMode::Accumulate),
                series_acc_field: Some("delta".to_string()),
            },
        )
        .await
        .unwrap();

    let resp = server
        .get("/tasks/inv-fmt/events/history?seriesFormat=invalid")
        .await;
    resp.assert_status(axum_test::http::StatusCode::OK);
    let body: Vec<serde_json::Value> = resp.json();
    // 1 status + 1 delta = 2, no collapse
    assert_eq!(body.len(), 2);
    assert!(
        body.iter().all(|e| e.get("seriesSnapshot").is_none() || e["seriesSnapshot"].is_null()),
        "no snapshot events should be present"
    );
}

// ─── limit with since cursor ────────────────────────────────────────────────

#[tokio::test]
async fn limit_with_since_cursor() {
    let engine = make_engine();
    let server = make_server(Arc::clone(&engine));
    create_running_task(&engine, "limit-since").await;

    let first = engine
        .publish_event(
            "limit-since",
            PublishEventInput {
                r#type: "a".to_string(),
                level: Level::Info,
                data: json!(null),
                series_id: None,
                series_mode: None,
                series_acc_field: None,
            },
        )
        .await
        .unwrap();

    engine
        .publish_event(
            "limit-since",
            PublishEventInput {
                r#type: "b".to_string(),
                level: Level::Info,
                data: json!(null),
                series_id: None,
                series_mode: None,
                series_acc_field: None,
            },
        )
        .await
        .unwrap();

    engine
        .publish_event(
            "limit-since",
            PublishEventInput {
                r#type: "c".to_string(),
                level: Level::Info,
                data: json!(null),
                series_id: None,
                series_mode: None,
                series_acc_field: None,
            },
        )
        .await
        .unwrap();

    let resp = server
        .get(&format!(
            "/tasks/limit-since/events/history?since.id={}&limit=1",
            first.id
        ))
        .await;
    resp.assert_status(axum_test::http::StatusCode::OK);
    let body: Vec<serde_json::Value> = resp.json();
    assert_eq!(body.len(), 1);
    assert_eq!(body[0]["type"], "b");
}
