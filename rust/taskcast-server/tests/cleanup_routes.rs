use axum_test::TestServer;
use serde_json::{json, Value};
use std::sync::Arc;
use taskcast_core::*;
use taskcast_server::{create_app, AuthMode, CorsConfig};

async fn fixture() -> TestServer {
    let hot = Arc::new(MemoryShortTermStore::new());
    let durable = Arc::new(MemoryLongTermStore::new());
    let engine = Arc::new(
        TaskEngine::new(TaskEngineOptions {
            short_term_store: hot.clone(),
            long_term_store: Some(durable.clone()),
            broadcast: Arc::new(MemoryBroadcastProvider::new()),
            hooks: None,
        })
        .with_cleanup_config(ResolvedCleanupConfig {
            enabled: true,
            rules: vec![serde_json::from_value(json!({"target":"events","trigger":{}})).unwrap()],
        })
        .unwrap(),
    );
    engine
        .create_task(CreateTaskInput {
            id: Some("expired".into()),
            ..Default::default()
        })
        .await
        .unwrap();
    engine
        .transition_task("expired", TaskStatus::Running, None)
        .await
        .unwrap();
    let original = engine
        .transition_task("expired", TaskStatus::Failed, None)
        .await
        .unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while durable.get_events("expired", None).await.unwrap().len() != 2 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let events = hot.get_events("expired", None).await.unwrap();
    assert_eq!(
        engine
            .sweep_cleanup(100, 1, 30000)
            .await
            .unwrap()
            .deleted_events,
        1
    );
    hot.save_task(original).await.unwrap();
    for event in events {
        hot.append_event("expired", event).await.unwrap();
    }
    let (app, _) = create_app(engine, AuthMode::None, None, None, CorsConfig::AllowAll);
    TestServer::new(app)
}

#[tokio::test]
async fn expiry_overrides_stale_cache_and_old_cursors() {
    let server = fixture().await;
    let task: Value = server.get("/tasks/expired").await.json();
    assert!(task["historyExpiredAt"].as_f64().unwrap() > 0.0);
    for query in [
        "",
        "?since.id=old",
        "?since.index=0",
        "?since.timestamp=1",
        "?seriesFormat=accumulated",
    ] {
        let response = server
            .get(&format!("/tasks/expired/events/history{query}"))
            .await;
        response.assert_status_ok();
        response.assert_header("x-taskcast-history-expired", "true");
        assert_eq!(response.json::<Value>(), json!([]));
    }
}

#[tokio::test]
async fn expired_sse_signals_before_original_terminal_done() {
    let server = fixture().await;
    for query in ["", "?since.id=old", "?since.index=0", "?since.timestamp=1"] {
        let body = server
            .get(&format!("/tasks/expired/events{query}"))
            .await
            .text();
        assert!(body.contains("event: taskcast.history_expired"), "{body}");
        assert!(body.contains("\"reason\":\"failed\""));
        assert!(body.find("taskcast.history_expired") < body.find("taskcast.done"));
        assert!(!body.contains("event: taskcast.event"));
    }
}

#[tokio::test]
async fn expired_export_has_stable_conflict_code() {
    let server = fixture().await;
    let response = server.get("/tasks/expired/archive").await;
    response.assert_status(axum_test::http::StatusCode::CONFLICT);
    assert_eq!(response.json::<Value>()["code"], "TASKCAST_HISTORY_EXPIRED");
}
