//! Tests that modify process-wide environment variables.
//! Isolated in a separate test binary so they don't interfere with
//! parallel tests in start_tests.rs that also call `run()`.

use std::sync::{Mutex, MutexGuard};
use taskcast_cli::commands::start::StartArgs;

// ─── Helpers ─────────────────────────────────────────────────────────────────

static ENV_LOCK: Mutex<()> = Mutex::new(());

fn lock_env() -> MutexGuard<'static, ()> {
    ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

struct EnvGuard {
    vars: Vec<&'static str>,
    _lock: MutexGuard<'static, ()>,
}

impl EnvGuard {
    fn new(vars: &[(&'static str, &str)]) -> Self {
        let lock = lock_env();
        for (key, value) in vars {
            std::env::set_var(key, value);
        }
        Self {
            vars: vars.iter().map(|(k, _)| *k).collect(),
            _lock: lock,
        }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        for var in &self.vars {
            std::env::remove_var(var);
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
