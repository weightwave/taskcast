use std::net::SocketAddr;
use std::sync::Mutex;

use axum::{routing::get, Json, Router};
use serde_json::json;
use tempfile::TempDir;
use tokio::net::TcpListener;

use taskcast_cli::commands::ping::{run, PingArgs};
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

// ─── run() success with default node ────────────────────────────────────────

#[tokio::test]
async fn run_success_default_node() {
    let _lock = HOME_LOCK.lock().unwrap();

    // Start a mock server that returns 200 OK on /health
    let app = Router::new().route(
        "/health",
        get(|| async { Json(json!({ "ok": true })) }),
    );
    let base_url = start_mock_server(app).await;

    // Set up HOME and configure the default node to point to our mock server
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
    run(PingArgs { node: None }).await;
}

// ─── run() success with named node ──────────────────────────────────────────

#[tokio::test]
async fn run_success_named_node() {
    let _lock = HOME_LOCK.lock().unwrap();

    let app = Router::new().route(
        "/health",
        get(|| async { Json(json!({ "ok": true })) }),
    );
    let base_url = start_mock_server(app).await;

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

    // Explicitly name the node via the --node flag
    run(PingArgs {
        node: Some("my-server".to_string()),
    })
    .await;
}

// NOTE: run() with a non-existent named node calls std::process::exit(1),
// and run() with a server that returns non-200 also calls std::process::exit(1).
// These error paths cannot be tested in-process without killing the test runner.
// The underlying ping_server() function is already fully tested in ping_tests.rs.
