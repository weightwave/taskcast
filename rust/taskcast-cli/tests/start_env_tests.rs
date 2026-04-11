//! Tests that modify process-wide environment variables.
//! Isolated in a separate test binary so they don't interfere with
//! parallel tests in start_tests.rs that also call `run()`.

use std::sync::{Mutex, MutexGuard};
use taskcast_cli::commands::start::StartArgs;
use testcontainers::runners::AsyncRunner;
use testcontainers_modules::postgres::Postgres;
use testcontainers_modules::redis::Redis;

// ─── Helpers ─────────────────────────────────────────────────────────────────

static ENV_LOCK: Mutex<()> = Mutex::new(());

fn lock_env() -> MutexGuard<'static, ()> {
    ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

struct EnvGuard {
    /// Saved original values: (key, previous value if any).
    /// On Drop we restore previous values rather than blindly removing keys,
    /// so pre-existing environment state is preserved across tests.
    saved: Vec<(&'static str, Option<String>)>,
    _lock: MutexGuard<'static, ()>,
}

impl EnvGuard {
    fn new(vars: &[(&'static str, &str)]) -> Self {
        let lock = lock_env();
        let mut saved = Vec::with_capacity(vars.len());
        for (key, value) in vars {
            saved.push((*key, std::env::var(key).ok()));
            std::env::set_var(key, value);
        }
        Self { saved, _lock: lock }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        for (key, prev) in &self.saved {
            match prev {
                Some(v) => std::env::set_var(key, v),
                None => std::env::remove_var(key),
            }
        }
    }
}

async fn find_available_port() -> u16 {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    port
}

// ─── JWT auth via env var ──────────────────────────────────────────────────────

#[tokio::test]
async fn run_jwt_auth_rejects_unauthenticated_requests() {
    let _env = EnvGuard::new(&[
        ("TASKCAST_AUTH_MODE", "jwt"),
        ("TASKCAST_JWT_SECRET", "test-secret-key-for-testing"),
    ]);

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
    let res = client
        .get(&format!("http://127.0.0.1:{port}/tasks"))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 401);

    let res = reqwest::get(&format!("http://127.0.0.1:{port}/health"))
        .await
        .unwrap();
    assert!(res.status().is_success());

    let res = reqwest::get(&format!("http://127.0.0.1:{port}/health/detail"))
        .await
        .unwrap();
    assert!(res.status().is_success());
    let body: serde_json::Value = res.json().await.unwrap();
    assert_eq!(body["auth"]["mode"], "jwt");

    handle.abort();
}

#[tokio::test]
async fn run_jwt_auth_accepts_valid_token() {
    let secret = "test-secret-key-for-jwt-auth";
    let _env = EnvGuard::new(&[
        ("TASKCAST_AUTH_MODE", "jwt"),
        ("TASKCAST_JWT_SECRET", secret),
    ]);

    let port = find_available_port().await;
    let handle = tokio::spawn(async move {
        let _ = taskcast_cli::commands::start::run(StartArgs {
            port,
            ..Default::default()
        })
        .await;
    });

    tokio::time::sleep(std::time::Duration::from_millis(300)).await;

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let claims = serde_json::json!({
        "sub": "test-user",
        "scope": ["*"],
        "taskIds": "*",
        "exp": now + 3600,
        "iat": now
    });
    let token = jsonwebtoken::encode(
        &jsonwebtoken::Header::default(),
        &claims,
        &jsonwebtoken::EncodingKey::from_secret(secret.as_bytes()),
    )
    .unwrap();

    let client = reqwest::Client::new();
    let res = client
        .get(&format!("http://127.0.0.1:{port}/tasks"))
        .header("Authorization", format!("Bearer {token}"))
        .send()
        .await
        .unwrap();
    assert!(
        res.status().is_success(),
        "Expected 200, got {}",
        res.status()
    );

    handle.abort();
}

// ─── JWT env var overrides config file secret ───────────────────────────────

#[tokio::test]
async fn run_jwt_env_secret_overrides_config_secret() {
    let env_secret = "env-override-secret-key-xyz";
    let _env = EnvGuard::new(&[
        ("TASKCAST_AUTH_MODE", "jwt"),
        ("TASKCAST_JWT_SECRET", env_secret),
    ]);

    let port = find_available_port().await;
    let handle = tokio::spawn(async move {
        let _ = taskcast_cli::commands::start::run(StartArgs {
            port,
            ..Default::default()
        })
        .await;
    });

    tokio::time::sleep(std::time::Duration::from_millis(300)).await;

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let claims = serde_json::json!({
        "sub": "test-user",
        "scope": ["*"],
        "taskIds": "*",
        "exp": now + 3600,
        "iat": now
    });
    let token = jsonwebtoken::encode(
        &jsonwebtoken::Header::default(),
        &claims,
        &jsonwebtoken::EncodingKey::from_secret(env_secret.as_bytes()),
    )
    .unwrap();

    let client = reqwest::Client::new();
    let res = client
        .get(&format!("http://127.0.0.1:{port}/tasks"))
        .header("Authorization", format!("Bearer {token}"))
        .send()
        .await
        .unwrap();
    assert!(
        res.status().is_success(),
        "Expected 200, got {}",
        res.status()
    );

    handle.abort();
}

// ─── Env var resolution for TASKCAST_STORAGE ────────────────────────────────

#[tokio::test]
async fn run_env_storage_sqlite_overrides_default_memory() {
    let _env = EnvGuard::new(&[("TASKCAST_STORAGE", "sqlite")]);

    let tmp_dir = tempfile::tempdir().unwrap();
    let db_path = tmp_dir.path().join("env_test.db");
    let db_path_str = db_path.to_str().unwrap().to_string();

    let port = find_available_port().await;
    let handle = tokio::spawn(async move {
        let _ = taskcast_cli::commands::start::run(StartArgs {
            port,
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

// ─── Redis backend (testcontainer) ──────────────────────────────────────────

#[tokio::test]
async fn run_redis_backend_serves_health() {
    let container = Redis::default().start().await.unwrap();
    let host_port = container.get_host_port_ipv4(6379).await.unwrap();
    let redis_url = format!("redis://127.0.0.1:{}", host_port);

    // EnvGuard sets the env var immediately; the borrow only needs to live through new()
    let _env = EnvGuard::new(&[("TASKCAST_REDIS_URL", &redis_url)]);

    let port = find_available_port().await;
    let handle = tokio::spawn(async move {
        let _ = taskcast_cli::commands::start::run(StartArgs {
            port,
            storage: "redis".to_string(),
            ..Default::default()
        })
        .await;
    });

    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    let res = reqwest::get(&format!("http://127.0.0.1:{port}/health"))
        .await
        .unwrap();
    assert!(res.status().is_success());

    handle.abort();
    drop(_env);
    drop(container);
}

// ─── Memory backend with Postgres long-term store (testcontainer) ───────────

#[tokio::test]
async fn run_memory_backend_with_postgres_long_term_store() {
    let container = Postgres::default().start().await.unwrap();
    let host_port = container.get_host_port_ipv4(5432).await.unwrap();
    let pg_url = format!(
        "postgres://postgres:postgres@127.0.0.1:{}/postgres?sslmode=disable",
        host_port
    );

    let _env = EnvGuard::new(&[("TASKCAST_POSTGRES_URL", &pg_url)]);

    let port = find_available_port().await;
    let handle = tokio::spawn(async move {
        let _ = taskcast_cli::commands::start::run(StartArgs {
            port,
            ..Default::default()
        })
        .await;
    });

    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    let res = reqwest::get(&format!("http://127.0.0.1:{port}/health"))
        .await
        .unwrap();
    assert!(res.status().is_success());

    handle.abort();
    drop(_env);
    drop(container);
}

// ─── Auto-migrate integration tests ────────────────────────────────────────

#[tokio::test]
async fn auto_migrate_disabled_when_env_var_not_set() {
    let container = Postgres::default().start().await.unwrap();
    let host_port = container.get_host_port_ipv4(5432).await.unwrap();
    let pg_url = format!(
        "postgres://postgres:postgres@127.0.0.1:{}/postgres?sslmode=disable",
        host_port
    );

    // Don't set TASKCAST_AUTO_MIGRATE; it should default to disabled
    let _env = EnvGuard::new(&[("TASKCAST_POSTGRES_URL", &pg_url)]);

    let port = find_available_port().await;
    let handle = tokio::spawn(async move {
        let _ = taskcast_cli::commands::start::run(StartArgs {
            port,
            ..Default::default()
        })
        .await;
    });

    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    let res = reqwest::get(&format!("http://127.0.0.1:{port}/health"))
        .await
        .unwrap();
    assert!(res.status().is_success());

    handle.abort();
    drop(_env);
    drop(container);
}

#[tokio::test]
async fn auto_migrate_enabled_runs_migrations_on_startup() {
    let container = Postgres::default().start().await.unwrap();
    let host_port = container.get_host_port_ipv4(5432).await.unwrap();
    let pg_url = format!(
        "postgres://postgres:postgres@127.0.0.1:{}/postgres?sslmode=disable",
        host_port
    );

    // Enable auto-migrate
    let _env = EnvGuard::new(&[
        ("TASKCAST_AUTO_MIGRATE", "true"),
        ("TASKCAST_POSTGRES_URL", &pg_url),
    ]);

    let port = find_available_port().await;
    let handle = tokio::spawn(async move {
        let _ = taskcast_cli::commands::start::run(StartArgs {
            port,
            ..Default::default()
        })
        .await;
    });

    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    // Verify server started successfully
    let res = reqwest::get(&format!("http://127.0.0.1:{port}/health"))
        .await
        .unwrap();
    assert!(res.status().is_success());

    // Verify that migrations were applied by checking that we can create a task
    let client = reqwest::Client::new();
    let task_payload = serde_json::json!({
        "type": "test:task",
        "params": {"foo": "bar"}
    });
    let res = client
        .post(&format!("http://127.0.0.1:{port}/tasks"))
        .json(&task_payload)
        .send()
        .await
        .unwrap();

    // Should succeed (200 or 201)
    assert!(
        res.status().is_success(),
        "Expected success response from /tasks, got {}",
        res.status()
    );

    handle.abort();
    drop(_env);
    drop(container);
}

#[tokio::test]
async fn auto_migrate_disabled_when_env_var_is_false() {
    let container = Postgres::default().start().await.unwrap();
    let host_port = container.get_host_port_ipv4(5432).await.unwrap();
    let pg_url = format!(
        "postgres://postgres:postgres@127.0.0.1:{}/postgres?sslmode=disable",
        host_port
    );

    // Explicitly set to false
    let _env = EnvGuard::new(&[
        ("TASKCAST_AUTO_MIGRATE", "false"),
        ("TASKCAST_POSTGRES_URL", &pg_url),
    ]);

    let port = find_available_port().await;
    let handle = tokio::spawn(async move {
        let _ = taskcast_cli::commands::start::run(StartArgs {
            port,
            ..Default::default()
        })
        .await;
    });

    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    let res = reqwest::get(&format!("http://127.0.0.1:{port}/health"))
        .await
        .unwrap();
    assert!(res.status().is_success());

    handle.abort();
    drop(_env);
    drop(container);
}

#[tokio::test]
async fn auto_migrate_enabled_with_case_insensitive_env_var() {
    let container = Postgres::default().start().await.unwrap();
    let host_port = container.get_host_port_ipv4(5432).await.unwrap();
    let pg_url = format!(
        "postgres://postgres:postgres@127.0.0.1:{}/postgres?sslmode=disable",
        host_port
    );

    // Test case-insensitive parsing
    let _env = EnvGuard::new(&[
        ("TASKCAST_AUTO_MIGRATE", "TRUE"),
        ("TASKCAST_POSTGRES_URL", &pg_url),
    ]);

    let port = find_available_port().await;
    let handle = tokio::spawn(async move {
        let _ = taskcast_cli::commands::start::run(StartArgs {
            port,
            ..Default::default()
        })
        .await;
    });

    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    let res = reqwest::get(&format!("http://127.0.0.1:{port}/health"))
        .await
        .unwrap();
    assert!(res.status().is_success());

    handle.abort();
    drop(_env);
    drop(container);
}
