use std::net::SocketAddr;

mod common;

use axum::{routing::get, Json, Router};
use common::config_dir::IsolatedConfigDir;
use serde_json::json;
use taskcast_cli::commands::ping::{run, PingArgs};
use taskcast_cli::node_config::{NodeConfigManager, NodeEntry};
use tokio::net::TcpListener;

async fn start_mock_server(app: Router) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr: SocketAddr = listener.local_addr().unwrap();
    let base_url = format!("http://127.0.0.1:{}", addr.port());

    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    base_url
}

fn setup_config_dir() -> IsolatedConfigDir {
    IsolatedConfigDir::new()
}

#[tokio::test]
async fn run_success_default_node() {
    let app = Router::new().route("/health", get(|| async { Json(json!({ "ok": true })) }));
    let base_url = start_mock_server(app).await;

    let dir = setup_config_dir();
    let mgr = NodeConfigManager::new(dir.path().to_path_buf());
    mgr.add(
        "mock",
        NodeEntry {
            url: base_url,
            token: None,
            token_type: None,
        },
    );
    mgr.set_current("mock").unwrap();

    let result = run(PingArgs { node: None }).await;
    assert!(result.is_ok(), "run should succeed: {:?}", result.err());
}

#[tokio::test]
async fn run_success_named_node() {
    let app = Router::new().route("/health", get(|| async { Json(json!({ "ok": true })) }));
    let base_url = start_mock_server(app).await;

    let dir = setup_config_dir();
    let mgr = NodeConfigManager::new(dir.path().to_path_buf());
    mgr.add(
        "my-server",
        NodeEntry {
            url: base_url,
            token: None,
            token_type: None,
        },
    );

    let result = run(PingArgs {
        node: Some("my-server".to_string()),
    })
    .await;
    assert!(result.is_ok(), "run should succeed: {:?}", result.err());
}

#[tokio::test]
async fn run_node_not_found_returns_error() {
    let _dir = setup_config_dir();

    let result = run(PingArgs {
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

#[tokio::test]
async fn run_ping_failure_returns_error() {
    let dir = setup_config_dir();
    let mgr = NodeConfigManager::new(dir.path().to_path_buf());
    mgr.add(
        "bad-server",
        NodeEntry {
            url: "http://127.0.0.1:19999".to_string(),
            token: None,
            token_type: None,
        },
    );
    mgr.set_current("bad-server").unwrap();

    let result = run(PingArgs { node: None }).await;
    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(
        err.contains("FAIL") || err.contains("cannot reach"),
        "error should indicate ping failure, got: {err}"
    );
}
