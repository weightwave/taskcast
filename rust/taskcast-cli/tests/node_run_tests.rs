mod common;

use common::config_dir::IsolatedConfigDir;
use taskcast_cli::commands::node::{run, NodeCommands};
use taskcast_cli::node_config::{NodeConfigManager, TokenType};

fn setup_config_dir() -> IsolatedConfigDir {
    IsolatedConfigDir::new()
}

#[test]
fn run_add_without_token() {
    let dir = setup_config_dir();
    run(NodeCommands::Add {
        name: "local".to_string(),
        url: "http://localhost:3721".to_string(),
        token: None,
        token_type: "jwt".to_string(),
    })
    .unwrap();
    let mgr = NodeConfigManager::new(dir.path().to_path_buf());
    let node = mgr.get("local").unwrap();
    assert_eq!(node.url, "http://localhost:3721");
    assert!(node.token.is_none());
    assert!(node.token_type.is_none());
}

#[test]
fn run_add_with_jwt_token() {
    let dir = setup_config_dir();
    run(NodeCommands::Add {
        name: "prod".to_string(),
        url: "https://prod.example.com".to_string(),
        token: Some("eyJ...".to_string()),
        token_type: "jwt".to_string(),
    })
    .unwrap();
    let mgr = NodeConfigManager::new(dir.path().to_path_buf());
    let node = mgr.get("prod").unwrap();
    assert_eq!(node.url, "https://prod.example.com");
    assert_eq!(node.token, Some("eyJ...".to_string()));
    assert_eq!(node.token_type, Some(TokenType::Jwt));
}

#[test]
fn run_add_with_admin_token() {
    let dir = setup_config_dir();
    run(NodeCommands::Add {
        name: "staging".to_string(),
        url: "https://staging.example.com".to_string(),
        token: Some("admin_xxx".to_string()),
        token_type: "admin".to_string(),
    })
    .unwrap();
    let mgr = NodeConfigManager::new(dir.path().to_path_buf());
    let node = mgr.get("staging").unwrap();
    assert_eq!(node.url, "https://staging.example.com");
    assert_eq!(node.token, Some("admin_xxx".to_string()));
    assert_eq!(node.token_type, Some(TokenType::Admin));
}

#[test]
fn run_add_with_unknown_token_type_defaults_to_jwt() {
    let dir = setup_config_dir();
    run(NodeCommands::Add {
        name: "test".to_string(),
        url: "http://localhost:3721".to_string(),
        token: Some("tok".to_string()),
        token_type: "bearer".to_string(),
    })
    .unwrap();
    let mgr = NodeConfigManager::new(dir.path().to_path_buf());
    let node = mgr.get("test").unwrap();
    assert_eq!(node.token_type, Some(TokenType::Jwt));
}

#[test]
fn run_list_empty() {
    let _dir = setup_config_dir();
    run(NodeCommands::List).unwrap();
}

#[test]
fn run_list_with_nodes() {
    let _dir = setup_config_dir();
    run(NodeCommands::Add {
        name: "test".to_string(),
        url: "http://localhost:3721".to_string(),
        token: None,
        token_type: "jwt".to_string(),
    })
    .unwrap();
    run(NodeCommands::List).unwrap();
}

#[test]
fn run_use_existing_node() {
    let dir = setup_config_dir();
    run(NodeCommands::Add {
        name: "test".to_string(),
        url: "http://localhost:3721".to_string(),
        token: None,
        token_type: "jwt".to_string(),
    })
    .unwrap();
    run(NodeCommands::Use {
        name: "test".to_string(),
    })
    .unwrap();
    let mgr = NodeConfigManager::new(dir.path().to_path_buf());
    let current = mgr.get_current();
    assert_eq!(current.url, "http://localhost:3721");
}

#[test]
fn run_remove_existing_node() {
    let dir = setup_config_dir();
    run(NodeCommands::Add {
        name: "test".to_string(),
        url: "http://localhost:3721".to_string(),
        token: None,
        token_type: "jwt".to_string(),
    })
    .unwrap();
    run(NodeCommands::Remove {
        name: "test".to_string(),
    })
    .unwrap();
    let mgr = NodeConfigManager::new(dir.path().to_path_buf());
    assert!(mgr.get("test").is_none());
}

#[test]
fn run_remove_nonexistent_returns_error() {
    let _dir = setup_config_dir();
    let result = run(NodeCommands::Remove {
        name: "ghost".to_string(),
    });
    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(
        err.contains("ghost"),
        "error should mention the node name, got: {err}"
    );
}

#[test]
fn run_use_nonexistent_returns_error() {
    let _dir = setup_config_dir();
    let result = run(NodeCommands::Use {
        name: "ghost".to_string(),
    });
    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(
        err.contains("ghost"),
        "error should mention the node name, got: {err}"
    );
}
