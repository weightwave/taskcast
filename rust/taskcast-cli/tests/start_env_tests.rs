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
        Self::with_removed(vars, &[])
    }

    fn with_removed(vars: &[(&'static str, &str)], removed: &[&'static str]) -> Self {
        let lock = lock_env();
        let mut saved = Vec::with_capacity(vars.len() + removed.len());
        for (key, value) in vars {
            saved.push((*key, std::env::var(key).ok()));
            std::env::set_var(key, value);
        }
        for key in removed {
            if vars.iter().any(|(existing, _)| existing == key) {
                continue;
            }
            saved.push((*key, std::env::var(key).ok()));
            std::env::remove_var(key);
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

const TEST_RSA_PRIVATE_KEY: &str = r#"-----BEGIN PRIVATE KEY-----
MIIEvAIBADANBgkqhkiG9w0BAQEFAASCBKYwggSiAgEAAoIBAQC5OOCe0r9EjEoD
rqN0UlG2vv6z0u8SRKxpATooDIlnWFmiSab39pH63UmHTHdr2INOl/EmjcGlBfHm
jp+3rJKNISKGRgL5h0tbDj3E5Wy9Zfhryecejbq4SOvVX/+pQNkIPAEZYL6wLhtw
h/6Lz0K6h+rztpYg4UKK9BrAZr6YKCi1PWAaFTrXUBNM8HRQlSyo9+9hqFuyGNzi
kXd+Rc84KmJl9GZcunOvthYbh55iwsR18aPo/vwWmZ0qieeKsJVmHU1wFv62AIs7
xI8VLmZJ/YOCFs8TT4qhUfB5mtX4a4zPGtKei9cOuFGyplXOpZNH91HLGfDyBfMK
/VX+n6pDAgMBAAECggEAA3L5sjtxo5C+BOXvPHuwv3Rv2pPM++W0SDUYcVjg0VnZ
B9qgD1htTRY3bk7DbCPmBHdz3o65ONETH7X8NDfOETuF7XYt5ZoNXy61G8HuwNm+
diAv+5oScttFkpc52lvPLtLfOl4nO7FzTvWMjR6/VI9/YpAKqT+vAfA1wOt1rr0+
+FhmhMXqKegcy01qIxVtqNXFTer0hRGY4Sc3pvsMSRvYOfPbsQndIF04u2z1y4Bc
uHiY6RTAsIzztSzZY0JHBKoXB3l2P239TJ2JzfQQ2hbTAcSsJkaiTAaapeTOFMTS
PHS1VhzB4DH+r184EC1srJQQoC8DHdUpVzJX8XWYsQKBgQDr/sHzAFlXdrWYnh7c
IlUuj+Nj3sMD6wv7cBh7u3yLlOjSR6j4MZj3yhDdLuf1I+e0ix95ZoXem9/dNDyz
/ESYIsZ0wsa+qWDzxwKaqwiMC4IXd0WxS0mHXeL5D7hOFsIsqTSblAm3q0N17gAV
VMhzJxeAeaN6LkSXbX4nKqX3zwKBgQDI7FCJTH2ZvprraKD2TbjqEqewPnURSmyL
nuxFOMe9cAuql3BBl/ep6gT7XmHRQZd4dVOYqbpxXVl0VdawzoNXWW0P5S0mxiz9
XCo/MsvnN5X1W4THPgAE4xZNpj6qdOWT5wXOvYvGP1HaOQxmskXUnJDcQgQT6bTx
/DXG6KEPTQKBgEkOVHwlX4Lz/MOCL4t2FWiUopAIJdbQrKTpzqp/H88WCf0OsgAj
Wnda1l2iZ6w7sT7y0ouCcW64UlToFuKg9ZsjKMx8f4oGZT0SHnxC9iJkbaFWCv0X
kWuWZO01MJj78qBgwShoa5mwKvIW+2+fD26Wa3AaN8FbEWDPRH5bdYWBAoGAEo0X
JoYkdqSNozym1/b3Is2UJAawQmdvvDhxMjb64jfNK/QNjlDcshiEWz0spOh8dsfG
bysEpuDqmH4wc2St5cvA8R3E3HahwsbWs70Z7IBKXTwU91x3Hfxlm8fEs3JVnCFR
fPQtSqGgChkIVxcQsX+/NEb4H2qNpWYXBQWHkWUCgYBJnOJTUphM2cpZlR6rtGu5
NgZ2F/I/QutO6nizjTq6J4U/UBs9qGH2olKxmuDVEp4VrVqLdNxRVz22w1HU4/6k
8y7/u7w33zou9ienJN1dZrm+EzmxAOqk28b2Nrb1gUA+MfxGVP83URDPIIp0vVIY
N1w/tV0JBRl2ihDWvUv6PQ==
-----END PRIVATE KEY-----"#;

const TEST_RSA_PUBLIC_KEY: &str = r#"-----BEGIN PUBLIC KEY-----
MIIBIjANBgkqhkiG9w0BAQEFAAOCAQ8AMIIBCgKCAQEAuTjgntK/RIxKA66jdFJR
tr7+s9LvEkSsaQE6KAyJZ1hZokmm9/aR+t1Jh0x3a9iDTpfxJo3BpQXx5o6ft6yS
jSEihkYC+YdLWw49xOVsvWX4a8nnHo26uEjr1V//qUDZCDwBGWC+sC4bcIf+i89C
uofq87aWIOFCivQawGa+mCgotT1gGhU611ATTPB0UJUsqPfvYahbshjc4pF3fkXP
OCpiZfRmXLpzr7YWG4eeYsLEdfGj6P78FpmdKonnirCVZh1NcBb+tgCLO8SPFS5m
Sf2DghbPE0+KoVHweZrV+GuMzxrSnovXDrhRsqZVzqWTR/dRyxnw8gXzCv1V/p+q
QwIDAQAB
-----END PUBLIC KEY-----"#;

fn make_rs256_token(issuer: &str, audience: &str) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let claims = serde_json::json!({
        "sub": "test-user",
        "scope": ["*"],
        "taskIds": "*",
        "iss": issuer,
        "aud": audience,
        "exp": now + 3600,
        "iat": now
    });
    jsonwebtoken::encode(
        &jsonwebtoken::Header::new(jsonwebtoken::Algorithm::RS256),
        &claims,
        &jsonwebtoken::EncodingKey::from_rsa_pem(TEST_RSA_PRIVATE_KEY.as_bytes()).unwrap(),
    )
    .unwrap()
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

#[tokio::test]
async fn run_jwt_auth_accepts_trusted_service_key_from_config() {
    let _env = EnvGuard::with_removed(
        &[],
        &[
            "TASKCAST_AUTH_MODE",
            "TASKCAST_JWT_SECRET",
            "TASKCAST_JWT_ALGORITHM",
            "TASKCAST_JWT_PUBLIC_KEY",
            "TASKCAST_JWT_PUBLIC_KEY_FILE",
            "TASKCAST_JWT_ISSUER",
            "TASKCAST_JWT_AUDIENCE",
        ],
    );
    let dir = tempfile::tempdir().unwrap();
    let config_path = dir.path().join("taskcast.config.yaml");
    std::fs::write(
        &config_path,
        r#"
auth:
  mode: jwt
  jwt:
    secret: config-secret-key-for-testing
trustedServices:
  - name: backend
    key: service-key-that-is-long-enough
    taskIds: "*"
    scope: ["*"]
"#,
    )
    .unwrap();

    let port = find_available_port().await;
    let config = config_path.to_string_lossy().into_owned();
    let handle = tokio::spawn(async move {
        let _ = taskcast_cli::commands::start::run(StartArgs {
            port,
            config: Some(config),
            ..Default::default()
        })
        .await;
    });

    tokio::time::sleep(std::time::Duration::from_millis(300)).await;

    let client = reqwest::Client::new();
    let res = client
        .get(&format!("http://127.0.0.1:{port}/tasks"))
        .header("X-Taskcast-Service-Key", "service-key-that-is-long-enough")
        .send()
        .await
        .unwrap();
    assert!(
        res.status().is_success(),
        "Expected 200, got {}",
        res.status()
    );

    let res = client
        .get(&format!("http://127.0.0.1:{port}/tasks"))
        .header("X-Taskcast-Service-Key", "wrong-service-key")
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 401);

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

#[tokio::test]
async fn run_jwt_auth_accepts_rs256_public_key_from_env_and_enforces_audience() {
    let _env = EnvGuard::with_removed(
        &[
            ("TASKCAST_AUTH_MODE", "jwt"),
            ("TASKCAST_JWT_ALGORITHM", "RS256"),
            ("TASKCAST_JWT_PUBLIC_KEY", TEST_RSA_PUBLIC_KEY),
            ("TASKCAST_JWT_ISSUER", "railway-auth"),
            ("TASKCAST_JWT_AUDIENCE", "taskcast"),
        ],
        &["TASKCAST_JWT_SECRET", "TASKCAST_JWT_PUBLIC_KEY_FILE"],
    );

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
    let valid_token = make_rs256_token("railway-auth", "taskcast");
    let res = client
        .get(&format!("http://127.0.0.1:{port}/tasks"))
        .header("Authorization", format!("Bearer {valid_token}"))
        .send()
        .await
        .unwrap();
    assert!(
        res.status().is_success(),
        "Expected 200, got {}",
        res.status()
    );

    let wrong_audience_token = make_rs256_token("railway-auth", "other-service");
    let res = client
        .get(&format!("http://127.0.0.1:{port}/tasks"))
        .header("Authorization", format!("Bearer {wrong_audience_token}"))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 401);

    handle.abort();
}

#[tokio::test]
async fn run_jwt_auth_accepts_rs256_public_key_file_from_env() {
    let tmp_dir = tempfile::tempdir().unwrap();
    let public_key_path = tmp_dir.path().join("jwt.pub");
    std::fs::write(&public_key_path, TEST_RSA_PUBLIC_KEY).unwrap();
    let public_key_path = public_key_path.to_str().unwrap().to_string();

    let _env = EnvGuard::with_removed(
        &[
            ("TASKCAST_AUTH_MODE", "jwt"),
            ("TASKCAST_JWT_ALGORITHM", "RS256"),
            ("TASKCAST_JWT_PUBLIC_KEY_FILE", &public_key_path),
        ],
        &[
            "TASKCAST_JWT_SECRET",
            "TASKCAST_JWT_PUBLIC_KEY",
            "TASKCAST_JWT_ISSUER",
            "TASKCAST_JWT_AUDIENCE",
        ],
    );

    let port = find_available_port().await;
    let handle = tokio::spawn(async move {
        let _ = taskcast_cli::commands::start::run(StartArgs {
            port,
            ..Default::default()
        })
        .await;
    });

    tokio::time::sleep(std::time::Duration::from_millis(300)).await;

    let token = make_rs256_token("any-issuer", "any-audience");
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
