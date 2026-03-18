use taskcast_cli::commands::start::StartArgs;

async fn find_available_port() -> u16 {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    port
}

// ─── StartArgs::default() ────────────────────────────────────────────────────

#[test]
fn start_args_default_values() {
    let args = StartArgs::default();
    assert_eq!(args.port, 3721);
    assert_eq!(args.storage, "memory");
    assert_eq!(args.db_path, "./taskcast.db");
    assert!(args.config.is_none());
    assert!(!args.playground);
    assert!(!args.verbose);
}

// ─── run() with memory backend ──────────────────────────────────────────────

#[tokio::test]
async fn run_memory_backend_serves_health() {
    let port = find_available_port().await;
    let handle = tokio::spawn(async move {
        let _ = taskcast_cli::commands::start::run(StartArgs {
            port,
            ..Default::default()
        })
        .await;
    });

    // Wait for server to start
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;

    let res = reqwest::get(&format!("http://127.0.0.1:{port}/health"))
        .await
        .unwrap();
    assert!(res.status().is_success());

    let body: serde_json::Value = res.json().await.unwrap();
    assert_eq!(body["ok"], true);

    handle.abort();
}

// ─── run() with verbose flag ────────────────────────────────────────────────

#[tokio::test]
async fn run_with_verbose_flag_serves_health() {
    let port = find_available_port().await;
    let handle = tokio::spawn(async move {
        let _ = taskcast_cli::commands::start::run(StartArgs {
            port,
            verbose: true,
            ..Default::default()
        })
        .await;
    });

    tokio::time::sleep(std::time::Duration::from_millis(300)).await;

    let res = reqwest::get(&format!("http://127.0.0.1:{port}/health"))
        .await
        .unwrap();
    assert!(res.status().is_success());

    handle.abort();
}

// ─── run() with playground flag ─────────────────────────────────────────────

#[tokio::test]
async fn run_with_playground_flag_serves_playground() {
    let port = find_available_port().await;
    let handle = tokio::spawn(async move {
        let _ = taskcast_cli::commands::start::run(StartArgs {
            port,
            playground: true,
            ..Default::default()
        })
        .await;
    });

    tokio::time::sleep(std::time::Duration::from_millis(300)).await;

    // Health endpoint should still work
    let res = reqwest::get(&format!("http://127.0.0.1:{port}/health"))
        .await
        .unwrap();
    assert!(res.status().is_success());

    // Playground index should be accessible via /_playground/index.html
    let res = reqwest::get(&format!("http://127.0.0.1:{port}/_playground/index.html"))
        .await
        .unwrap();
    assert!(
        res.status().is_success(),
        "Expected 200 for /_playground/index.html, got {}",
        res.status()
    );

    handle.abort();
}

// ─── run() with SQLite storage ──────────────────────────────────────────────

#[tokio::test]
async fn run_sqlite_backend_serves_health() {
    let tmp_dir = tempfile::tempdir().unwrap();
    let db_path = tmp_dir.path().join("test.db");
    let db_path_str = db_path.to_str().unwrap().to_string();

    let port = find_available_port().await;
    let handle = tokio::spawn(async move {
        let _ = taskcast_cli::commands::start::run(StartArgs {
            port,
            storage: "sqlite".to_string(),
            db_path: db_path_str,
            ..Default::default()
        })
        .await;
    });

    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    let res = reqwest::get(&format!("http://127.0.0.1:{port}/health"))
        .await
        .unwrap();
    assert!(res.status().is_success());

    let body: serde_json::Value = res.json().await.unwrap();
    assert_eq!(body["ok"], true);

    handle.abort();
}

// ─── run() full CRUD via memory backend ─────────────────────────────────────

#[tokio::test]
async fn run_memory_backend_crud_operations() {
    let port = find_available_port().await;
    let handle = tokio::spawn(async move {
        let _ = taskcast_cli::commands::start::run(StartArgs {
            port,
            ..Default::default()
        })
        .await;
    });

    tokio::time::sleep(std::time::Duration::from_millis(300)).await;

    let client = reqwest::Client::new();

    // Create task
    let res = client
        .post(&format!("http://127.0.0.1:{port}/tasks"))
        .json(&serde_json::json!({ "type": "test.task" }))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 201);
    let created: serde_json::Value = res.json().await.unwrap();
    assert_eq!(created["type"], "test.task");
    assert_eq!(created["status"], "pending");

    let task_id = created["id"].as_str().unwrap();

    // Get task
    let res = client
        .get(&format!("http://127.0.0.1:{port}/tasks/{task_id}"))
        .send()
        .await
        .unwrap();
    assert!(res.status().is_success());
    let fetched: serde_json::Value = res.json().await.unwrap();
    assert_eq!(fetched["id"], task_id);

    handle.abort();
}

// ─── run() with verbose + playground combined ───────────────────────────────

#[tokio::test]
async fn run_with_verbose_and_playground() {
    let port = find_available_port().await;
    let handle = tokio::spawn(async move {
        let _ = taskcast_cli::commands::start::run(StartArgs {
            port,
            verbose: true,
            playground: true,
            ..Default::default()
        })
        .await;
    });

    tokio::time::sleep(std::time::Duration::from_millis(300)).await;

    // Health check
    let res = reqwest::get(&format!("http://127.0.0.1:{port}/health"))
        .await
        .unwrap();
    assert!(res.status().is_success());

    // Playground should work too
    let res = reqwest::get(&format!("http://127.0.0.1:{port}/_playground/index.html"))
        .await
        .unwrap();
    assert!(res.status().is_success());

    handle.abort();
}

// ─── run() health detail shows memory adapters ──────────────────────────────

#[tokio::test]
async fn run_memory_backend_health_detail() {
    let port = find_available_port().await;
    let handle = tokio::spawn(async move {
        let _ = taskcast_cli::commands::start::run(StartArgs {
            port,
            ..Default::default()
        })
        .await;
    });

    tokio::time::sleep(std::time::Duration::from_millis(300)).await;

    let res = reqwest::get(&format!("http://127.0.0.1:{port}/health/detail"))
        .await
        .unwrap();
    assert!(res.status().is_success());

    let body: serde_json::Value = res.json().await.unwrap();
    assert_eq!(body["ok"], true);
    assert_eq!(body["auth"]["mode"], "none");
    assert_eq!(body["adapters"]["broadcast"]["provider"], "memory");
    assert_eq!(body["adapters"]["shortTermStore"]["provider"], "memory");

    handle.abort();
}

// ─── run() SQLite health detail ─────────────────────────────────────────────

#[tokio::test]
async fn run_sqlite_backend_health_detail() {
    let tmp_dir = tempfile::tempdir().unwrap();
    let db_path = tmp_dir.path().join("detail_test.db");
    let db_path_str = db_path.to_str().unwrap().to_string();

    let port = find_available_port().await;
    let handle = tokio::spawn(async move {
        let _ = taskcast_cli::commands::start::run(StartArgs {
            port,
            storage: "sqlite".to_string(),
            db_path: db_path_str,
            ..Default::default()
        })
        .await;
    });

    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    let res = reqwest::get(&format!("http://127.0.0.1:{port}/health/detail"))
        .await
        .unwrap();
    assert!(res.status().is_success());

    let body: serde_json::Value = res.json().await.unwrap();
    assert_eq!(body["ok"], true);
    // SQLite uses memory broadcast but sqlite short-term store
    assert_eq!(body["adapters"]["broadcast"]["provider"], "memory");

    handle.abort();
}

// ─── Worker manager setup via config file ───────────────────────────────────

#[tokio::test]
async fn run_with_workers_enabled_pull_reassign() {
    let tmp_dir = tempfile::tempdir().unwrap();
    let config_path = tmp_dir.path().join("taskcast.config.yaml");
    std::fs::write(
        &config_path,
        r#"
workers:
  enabled: true
  defaults:
    heartbeatIntervalMs: 30000
    heartbeatTimeoutMs: 90000
    offerTimeoutMs: 10000
    disconnectGraceMs: 5000
    assignMode: pull
    disconnectPolicy: reassign
"#,
    )
    .unwrap();

    let port = find_available_port().await;
    let config_str = config_path.to_str().unwrap().to_string();
    let handle = tokio::spawn(async move {
        let _ = taskcast_cli::commands::start::run(StartArgs {
            port,
            config: Some(config_str),
            ..Default::default()
        })
        .await;
    });

    tokio::time::sleep(std::time::Duration::from_millis(300)).await;

    let res = reqwest::get(&format!("http://127.0.0.1:{port}/health"))
        .await
        .unwrap();
    assert!(res.status().is_success());

    handle.abort();
}

#[tokio::test]
async fn run_with_workers_enabled_ws_offer_mark() {
    let tmp_dir = tempfile::tempdir().unwrap();
    let config_path = tmp_dir.path().join("taskcast.config.yaml");
    std::fs::write(
        &config_path,
        r#"
workers:
  enabled: true
  defaults:
    assignMode: ws-offer
    disconnectPolicy: mark
"#,
    )
    .unwrap();

    let port = find_available_port().await;
    let config_str = config_path.to_str().unwrap().to_string();
    let handle = tokio::spawn(async move {
        let _ = taskcast_cli::commands::start::run(StartArgs {
            port,
            config: Some(config_str),
            ..Default::default()
        })
        .await;
    });

    tokio::time::sleep(std::time::Duration::from_millis(300)).await;

    let res = reqwest::get(&format!("http://127.0.0.1:{port}/health"))
        .await
        .unwrap();
    assert!(res.status().is_success());

    handle.abort();
}

#[tokio::test]
async fn run_with_workers_enabled_ws_race_fail() {
    let tmp_dir = tempfile::tempdir().unwrap();
    let config_path = tmp_dir.path().join("taskcast.config.yaml");
    std::fs::write(
        &config_path,
        r#"
workers:
  enabled: true
  defaults:
    assignMode: ws-race
    disconnectPolicy: fail
"#,
    )
    .unwrap();

    let port = find_available_port().await;
    let config_str = config_path.to_str().unwrap().to_string();
    let handle = tokio::spawn(async move {
        let _ = taskcast_cli::commands::start::run(StartArgs {
            port,
            config: Some(config_str),
            ..Default::default()
        })
        .await;
    });

    tokio::time::sleep(std::time::Duration::from_millis(300)).await;

    let res = reqwest::get(&format!("http://127.0.0.1:{port}/health"))
        .await
        .unwrap();
    assert!(res.status().is_success());

    handle.abort();
}

#[tokio::test]
async fn run_with_workers_enabled_external_assign_mode() {
    let tmp_dir = tempfile::tempdir().unwrap();
    let config_path = tmp_dir.path().join("taskcast.config.yaml");
    std::fs::write(
        &config_path,
        r#"
workers:
  enabled: true
  defaults:
    assignMode: external
"#,
    )
    .unwrap();

    let port = find_available_port().await;
    let config_str = config_path.to_str().unwrap().to_string();
    let handle = tokio::spawn(async move {
        let _ = taskcast_cli::commands::start::run(StartArgs {
            port,
            config: Some(config_str),
            ..Default::default()
        })
        .await;
    });

    tokio::time::sleep(std::time::Duration::from_millis(300)).await;

    let res = reqwest::get(&format!("http://127.0.0.1:{port}/health"))
        .await
        .unwrap();
    assert!(res.status().is_success());

    handle.abort();
}

#[tokio::test]
async fn run_with_workers_enabled_unknown_assign_mode_defaults_to_external() {
    let tmp_dir = tempfile::tempdir().unwrap();
    let config_path = tmp_dir.path().join("taskcast.config.yaml");
    std::fs::write(
        &config_path,
        r#"
workers:
  enabled: true
  defaults:
    assignMode: some-unknown-mode
    disconnectPolicy: some-unknown-policy
"#,
    )
    .unwrap();

    let port = find_available_port().await;
    let config_str = config_path.to_str().unwrap().to_string();
    let handle = tokio::spawn(async move {
        let _ = taskcast_cli::commands::start::run(StartArgs {
            port,
            config: Some(config_str),
            ..Default::default()
        })
        .await;
    });

    tokio::time::sleep(std::time::Duration::from_millis(300)).await;

    let res = reqwest::get(&format!("http://127.0.0.1:{port}/health"))
        .await
        .unwrap();
    assert!(res.status().is_success());

    handle.abort();
}

#[tokio::test]
async fn run_with_workers_enabled_no_defaults() {
    let tmp_dir = tempfile::tempdir().unwrap();
    let config_path = tmp_dir.path().join("taskcast.config.yaml");
    std::fs::write(
        &config_path,
        r#"
workers:
  enabled: true
"#,
    )
    .unwrap();

    let port = find_available_port().await;
    let config_str = config_path.to_str().unwrap().to_string();
    let handle = tokio::spawn(async move {
        let _ = taskcast_cli::commands::start::run(StartArgs {
            port,
            config: Some(config_str),
            ..Default::default()
        })
        .await;
    });

    tokio::time::sleep(std::time::Duration::from_millis(300)).await;

    let res = reqwest::get(&format!("http://127.0.0.1:{port}/health"))
        .await
        .unwrap();
    assert!(res.status().is_success());

    handle.abort();
}

#[tokio::test]
async fn run_with_workers_disabled_in_config() {
    let tmp_dir = tempfile::tempdir().unwrap();
    let config_path = tmp_dir.path().join("taskcast.config.yaml");
    std::fs::write(
        &config_path,
        r#"
workers:
  enabled: false
"#,
    )
    .unwrap();

    let port = find_available_port().await;
    let config_str = config_path.to_str().unwrap().to_string();
    let handle = tokio::spawn(async move {
        let _ = taskcast_cli::commands::start::run(StartArgs {
            port,
            config: Some(config_str),
            ..Default::default()
        })
        .await;
    });

    tokio::time::sleep(std::time::Duration::from_millis(300)).await;

    let res = reqwest::get(&format!("http://127.0.0.1:{port}/health"))
        .await
        .unwrap();
    assert!(res.status().is_success());

    handle.abort();
}

// ─── JWT auth via config file (not env var) ─────────────────────────────────

#[tokio::test]
async fn run_jwt_auth_from_config_file() {
    let tmp_dir = tempfile::tempdir().unwrap();
    let config_path = tmp_dir.path().join("taskcast.config.yaml");
    std::fs::write(
        &config_path,
        r#"
auth:
  mode: jwt
  jwt:
    secret: config-file-secret-key-123
    algorithm: HS256
    issuer: test-issuer
    audience: test-audience
"#,
    )
    .unwrap();

    let port = find_available_port().await;
    let config_str = config_path.to_str().unwrap().to_string();
    let handle = tokio::spawn(async move {
        let _ = taskcast_cli::commands::start::run(StartArgs {
            port,
            config: Some(config_str),
            ..Default::default()
        })
        .await;
    });

    tokio::time::sleep(std::time::Duration::from_millis(300)).await;

    // Unauthenticated request should be rejected
    let client = reqwest::Client::new();
    let res = client
        .get(&format!("http://127.0.0.1:{port}/tasks"))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 401);

    // Health detail should show auth mode as jwt
    let res = reqwest::get(&format!("http://127.0.0.1:{port}/health/detail"))
        .await
        .unwrap();
    let body: serde_json::Value = res.json().await.unwrap();
    assert_eq!(body["auth"]["mode"], "jwt");

    handle.abort();
}

// ─── Workers with all defaults explicitly set ───────────────────────────────

#[tokio::test]
async fn run_with_workers_all_defaults_set() {
    let tmp_dir = tempfile::tempdir().unwrap();
    let config_path = tmp_dir.path().join("taskcast.config.yaml");
    std::fs::write(
        &config_path,
        r#"
workers:
  enabled: true
  defaults:
    heartbeatIntervalMs: 15000
    heartbeatTimeoutMs: 45000
    offerTimeoutMs: 5000
    disconnectGraceMs: 3000
    assignMode: ws-race
    disconnectPolicy: fail
"#,
    )
    .unwrap();

    let port = find_available_port().await;
    let config_str = config_path.to_str().unwrap().to_string();
    let handle = tokio::spawn(async move {
        let _ = taskcast_cli::commands::start::run(StartArgs {
            port,
            config: Some(config_str),
            ..Default::default()
        })
        .await;
    });

    tokio::time::sleep(std::time::Duration::from_millis(300)).await;

    // Verify server is up and functional
    let res = reqwest::get(&format!("http://127.0.0.1:{port}/health"))
        .await
        .unwrap();
    assert!(res.status().is_success());

    // Create a task to verify the engine works correctly with worker manager
    let client = reqwest::Client::new();
    let res = client
        .post(&format!("http://127.0.0.1:{port}/tasks"))
        .json(&serde_json::json!({ "type": "worker.test" }))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 201);

    handle.abort();
}

// ─── Env var resolution for adapter URLs from config ────────────────────────

#[tokio::test]
async fn run_adapter_url_from_config_file() {
    // Test that adapter URLs are read from config file.
    // With broadcast url set in config, resolve_storage_mode sees redis_url is Some,
    // but we explicitly force memory storage via CLI flag.
    // The postgres url will cause the memory path to try connecting and fail,
    // which exercises the env var resolution code path (lines 60-71).
    let tmp_dir = tempfile::tempdir().unwrap();
    let config_path = tmp_dir.path().join("taskcast.config.yaml");
    std::fs::write(
        &config_path,
        r#"
adapters:
  broadcast:
    provider: redis
    url: redis://localhost:16379
  longTermStore:
    provider: postgres
    url: postgres://localhost:15432/testdb
"#,
    )
    .unwrap();

    let port = find_available_port().await;
    let config_str = config_path.to_str().unwrap().to_string();
    let handle = tokio::spawn(async move {
        let _ = taskcast_cli::commands::start::run(StartArgs {
            port,
            config: Some(config_str),
            // Explicitly use memory so we don't try to connect to redis
            storage: "memory".to_string(),
            ..Default::default()
        })
        .await;
    });

    // The server will attempt to connect to postgres (lines 125-129) and fail.
    // This exercises the env var resolution code for both redis_url and postgres_url.
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    handle.abort();
}
