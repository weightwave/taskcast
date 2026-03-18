use std::sync::Mutex;

use taskcast_cli::commands::node::{run, NodeCommands};
use taskcast_cli::node_config::{NodeConfigManager, TokenType};
use tempfile::TempDir;

/// Global lock to serialize tests that modify the HOME env var.
static HOME_LOCK: Mutex<()> = Mutex::new(());

fn setup_home() -> TempDir {
    let dir = TempDir::new().unwrap();
    std::env::set_var("HOME", dir.path());
    dir
}

// ─── run(Add) without token ────────────────────────────────────────────────

#[test]
fn run_add_without_token() {
    let _lock = HOME_LOCK.lock().unwrap();
    let dir = setup_home();
    run(NodeCommands::Add {
        name: "local".to_string(),
        url: "http://localhost:3721".to_string(),
        token: None,
        token_type: "jwt".to_string(),
    });
    // Verify via NodeConfigManager
    let mgr = NodeConfigManager::new(dir.path().join(".taskcast"));
    let node = mgr.get("local").unwrap();
    assert_eq!(node.url, "http://localhost:3721");
    assert!(node.token.is_none());
    assert!(node.token_type.is_none()); // No token -> no token_type stored
}

// ─── run(Add) with jwt token ───────────────────────────────────────────────

#[test]
fn run_add_with_jwt_token() {
    let _lock = HOME_LOCK.lock().unwrap();
    let dir = setup_home();
    run(NodeCommands::Add {
        name: "prod".to_string(),
        url: "https://prod.example.com".to_string(),
        token: Some("eyJ...".to_string()),
        token_type: "jwt".to_string(),
    });
    let mgr = NodeConfigManager::new(dir.path().join(".taskcast"));
    let node = mgr.get("prod").unwrap();
    assert_eq!(node.url, "https://prod.example.com");
    assert_eq!(node.token, Some("eyJ...".to_string()));
    assert_eq!(node.token_type, Some(TokenType::Jwt));
}

// ─── run(Add) with admin token ─────────────────────────────────────────────

#[test]
fn run_add_with_admin_token() {
    let _lock = HOME_LOCK.lock().unwrap();
    let dir = setup_home();
    run(NodeCommands::Add {
        name: "staging".to_string(),
        url: "https://staging.example.com".to_string(),
        token: Some("admin_xxx".to_string()),
        token_type: "admin".to_string(),
    });
    let mgr = NodeConfigManager::new(dir.path().join(".taskcast"));
    let node = mgr.get("staging").unwrap();
    assert_eq!(node.url, "https://staging.example.com");
    assert_eq!(node.token, Some("admin_xxx".to_string()));
    assert_eq!(node.token_type, Some(TokenType::Admin));
}

// ─── run(Add) with unknown token_type defaults to jwt ──────────────────────

#[test]
fn run_add_with_unknown_token_type_defaults_to_jwt() {
    let _lock = HOME_LOCK.lock().unwrap();
    let dir = setup_home();
    run(NodeCommands::Add {
        name: "test".to_string(),
        url: "http://localhost:3721".to_string(),
        token: Some("tok".to_string()),
        token_type: "bearer".to_string(), // unknown type
    });
    let mgr = NodeConfigManager::new(dir.path().join(".taskcast"));
    let node = mgr.get("test").unwrap();
    assert_eq!(node.token_type, Some(TokenType::Jwt)); // defaults to JWT
}

// ─── run(List) empty ───────────────────────────────────────────────────────

#[test]
fn run_list_empty() {
    let _lock = HOME_LOCK.lock().unwrap();
    let _dir = setup_home();
    // Should not panic, just prints "No nodes configured..."
    run(NodeCommands::List);
}

// ─── run(List) with nodes ──────────────────────────────────────────────────

#[test]
fn run_list_with_nodes() {
    let _lock = HOME_LOCK.lock().unwrap();
    let _dir = setup_home();
    run(NodeCommands::Add {
        name: "test".to_string(),
        url: "http://localhost:3721".to_string(),
        token: None,
        token_type: "jwt".to_string(),
    });
    // Should not panic, prints the node list
    run(NodeCommands::List);
}

// ─── run(Use) existing node ────────────────────────────────────────────────

#[test]
fn run_use_existing_node() {
    let _lock = HOME_LOCK.lock().unwrap();
    let dir = setup_home();
    run(NodeCommands::Add {
        name: "test".to_string(),
        url: "http://localhost:3721".to_string(),
        token: None,
        token_type: "jwt".to_string(),
    });
    run(NodeCommands::Use {
        name: "test".to_string(),
    });
    // Verify current node was set
    let mgr = NodeConfigManager::new(dir.path().join(".taskcast"));
    let current = mgr.get_current();
    assert_eq!(current.url, "http://localhost:3721");
}

// ─── run(Remove) existing node ─────────────────────────────────────────────

#[test]
fn run_remove_existing_node() {
    let _lock = HOME_LOCK.lock().unwrap();
    let dir = setup_home();
    run(NodeCommands::Add {
        name: "test".to_string(),
        url: "http://localhost:3721".to_string(),
        token: None,
        token_type: "jwt".to_string(),
    });
    run(NodeCommands::Remove {
        name: "test".to_string(),
    });
    // Verify node was removed
    let mgr = NodeConfigManager::new(dir.path().join(".taskcast"));
    assert!(mgr.get("test").is_none());
}

// NOTE: run(Remove) for non-existent node and run(Use) for non-existent node
// call std::process::exit(1) which would kill the test process.
// Those error paths are tested indirectly through NodeConfigManager unit tests
// in node_config.rs (error_on_remove_nonexistent, error_on_use_nonexistent).
