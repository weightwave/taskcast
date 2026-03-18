use std::net::SocketAddr;
use std::sync::Mutex;

use axum::{routing::get, Json, Router};
use serde_json::json;
use tempfile::TempDir;
use tokio::net::TcpListener;

use taskcast_cli::commands::doctor::{run, DoctorArgs};
use taskcast_cli::node_config::{NodeConfigManager, NodeEntry};

/// Global lock to serialize tests that modify the HOME env var.
static HOME_LOCK: Mutex<()> = Mutex::new(());

// ─── Helpers ─────────────────────────────────────────────────────────────────

async fn start_mock_server(app: Router) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr: SocketAddr = listener.local_addr().unwrap();
    let base_url = format!("http://127.0.0.1:{}", addr.port());

    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    base_url
}

fn setup_home() -> TempDir {
    let dir = TempDir::new().unwrap();
    std::env::set_var("HOME", dir.path());
    dir
}

fn healthy_detail_app() -> Router {
    Router::new().route(
        "/health/detail",
        get(|| async {
            Json(json!({
                "ok": true,
                "uptime": 120,
                "auth": { "mode": "none" },
                "adapters": {
                    "broadcast": { "provider": "memory", "status": "ok" },
                    "shortTermStore": { "provider": "memory", "status": "ok" }
                }
            }))
        }),
    )
}

// ─── run() success with default node ────────────────────────────────────────

#[tokio::test]
async fn run_success_default_node() {
    let _lock = HOME_LOCK.lock().unwrap();

    let base_url = start_mock_server(healthy_detail_app()).await;

    let dir = setup_home();
    let mgr = NodeConfigManager::new(dir.path().join(".taskcast"));
    mgr.add(
        "mock",
        NodeEntry {
            url: base_url,
            token: None,
            token_type: None,
        },
    );
    mgr.set_current("mock").unwrap();

    // run() should succeed without calling process::exit
    let result = run(DoctorArgs { node: None }).await;
    assert!(result.is_ok());
}

// ─── run() success with named node ──────────────────────────────────────────

#[tokio::test]
async fn run_success_named_node() {
    let _lock = HOME_LOCK.lock().unwrap();

    let base_url = start_mock_server(healthy_detail_app()).await;

    let dir = setup_home();
    let mgr = NodeConfigManager::new(dir.path().join(".taskcast"));
    mgr.add(
        "my-server",
        NodeEntry {
            url: base_url,
            token: None,
            token_type: None,
        },
    );

    // Explicitly name the node
    let result = run(DoctorArgs {
        node: Some("my-server".to_string()),
    })
    .await;
    assert!(result.is_ok());
}

// ─── run() with non-existent named node returns error ───────────────────────

#[tokio::test]
async fn run_node_not_found_returns_error() {
    let _lock = HOME_LOCK.lock().unwrap();
    let _dir = setup_home();

    let result = run(DoctorArgs {
        node: Some("nonexistent".to_string()),
    })
    .await;
    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(
        err.contains("nonexistent"),
        "error should mention the node name, got: {err}"
    );
}

// ─── run() with unreachable server returns error ────────────────────────────

#[tokio::test]
async fn run_server_unreachable_returns_error() {
    let _lock = HOME_LOCK.lock().unwrap();

    let dir = setup_home();
    let mgr = NodeConfigManager::new(dir.path().join(".taskcast"));
    mgr.add(
        "dead-server",
        NodeEntry {
            url: "http://127.0.0.1:19999".to_string(),
            token: None,
            token_type: None,
        },
    );
    mgr.set_current("dead-server").unwrap();

    let result = run(DoctorArgs { node: None }).await;
    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(
        err.contains("Server check failed"),
        "error should indicate server failure, got: {err}"
    );
}
