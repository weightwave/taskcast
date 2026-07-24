use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use taskcast_cli::commands::start::StartArgs;
use testcontainers::{runners::AsyncRunner, ImageExt};
use testcontainers_modules::postgres::Postgres;
use testcontainers_modules::redis::Redis;

static ENV_LOCK: Mutex<()> = Mutex::new(());

struct EnvGuard {
    saved: Vec<(&'static str, Option<String>)>,
    _lock: MutexGuard<'static, ()>,
}

impl EnvGuard {
    fn isolated(vars: &[(&'static str, &str)]) -> Self {
        let lock = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
        let keys = [
            "TASKCAST_STORAGE",
            "TASKCAST_REDIS_URL",
            "TASKCAST_POSTGRES_URL",
            "TASKCAST_POSTGRES_MAX_CONNECTIONS",
            "TASKCAST_AUTO_MIGRATE",
        ];
        let saved = keys
            .iter()
            .map(|key| (*key, std::env::var(key).ok()))
            .collect();
        for key in keys {
            std::env::remove_var(key);
        }
        for (key, value) in vars {
            std::env::set_var(key, value);
        }
        Self { saved, _lock: lock }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        for (key, value) in &self.saved {
            match value {
                Some(value) => std::env::set_var(key, value),
                None => std::env::remove_var(key),
            }
        }
    }
}

async fn available_port() -> u16 {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    port
}

async fn connection_probe() -> (u16, Arc<AtomicUsize>, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let count = Arc::new(AtomicUsize::new(0));
    let count_for_task = Arc::clone(&count);
    let task = tokio::spawn(async move {
        while let Ok((socket, _)) = listener.accept().await {
            count_for_task.fetch_add(1, Ordering::SeqCst);
            drop(socket);
        }
    });
    (port, count, task)
}

async fn wait_for_health(port: u16) {
    let client = reqwest::Client::builder().no_proxy().build().unwrap();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        if let Ok(response) = client
            .get(format!("http://127.0.0.1:{port}/health"))
            .send()
            .await
        {
            if response.status().is_success() {
                return;
            }
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "server did not become healthy"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

fn write_config(contents: &str) -> (tempfile::TempDir, String) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("taskcast.config.yaml");
    std::fs::write(&path, contents).unwrap();
    (dir, path.to_string_lossy().into_owned())
}

#[tokio::test]
async fn memory_with_unrelated_redis_url_opens_no_redis_connection() {
    let (dependency_port, connections, probe) = connection_probe().await;
    let redis_url = format!("redis://127.0.0.1:{dependency_port}");
    let _env = EnvGuard::isolated(&[("TASKCAST_REDIS_URL", &redis_url)]);
    let (_dir, config) = write_config("{}");
    let port = available_port().await;
    let server = tokio::spawn(async move {
        taskcast_cli::commands::start::run(StartArgs {
            config: Some(config),
            port,
            storage: Some("memory".to_string()),
            ..Default::default()
        })
        .await
        .map_err(|error| error.to_string())
    });

    wait_for_health(port).await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(connections.load(Ordering::SeqCst), 0);

    server.abort();
    probe.abort();
}

#[tokio::test]
async fn sqlite_with_postgres_url_opens_no_postgres_connection() {
    let (dependency_port, connections, probe) = connection_probe().await;
    let postgres_url = format!("postgres://user:pass@127.0.0.1:{dependency_port}/taskcast");
    let _env = EnvGuard::isolated(&[("TASKCAST_POSTGRES_URL", &postgres_url)]);
    let (dir, config) = write_config("{}");
    let port = available_port().await;
    let db_path = dir
        .path()
        .join("taskcast.db")
        .to_string_lossy()
        .into_owned();
    let server = tokio::spawn(async move {
        taskcast_cli::commands::start::run(StartArgs {
            config: Some(config),
            port,
            storage: Some("sqlite".to_string()),
            db_path,
            ..Default::default()
        })
        .await
        .map_err(|error| error.to_string())
    });

    wait_for_health(port).await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(connections.load(Ordering::SeqCst), 0);

    server.abort();
    probe.abort();
}

#[tokio::test]
async fn explicit_non_postgres_long_term_provider_ignores_env_url() {
    let (dependency_port, connections, probe) = connection_probe().await;
    let postgres_url = format!("postgres://user:pass@127.0.0.1:{dependency_port}/taskcast");
    let _env = EnvGuard::isolated(&[("TASKCAST_POSTGRES_URL", &postgres_url)]);
    let (_dir, config) = write_config(
        r#"
adapters:
  longTermStore:
    provider: memory
"#,
    );
    let port = available_port().await;
    let server = tokio::spawn(async move {
        taskcast_cli::commands::start::run(StartArgs {
            config: Some(config),
            port,
            storage: Some("memory".to_string()),
            ..Default::default()
        })
        .await
        .map_err(|error| error.to_string())
    });

    wait_for_health(port).await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(connections.load(Ordering::SeqCst), 0);

    server.abort();
    probe.abort();
}

#[tokio::test]
async fn explicit_postgres_provider_without_url_fails_before_http_bind() {
    let _env = EnvGuard::isolated(&[]);
    let (_dir, config) = write_config(
        r#"
adapters:
  longTermStore:
    provider: postgres
    url: ""
"#,
    );
    let http_port = available_port().await;

    let result = taskcast_cli::commands::start::run(StartArgs {
        config: Some(config),
        port: http_port,
        ..Default::default()
    })
    .await;
    assert!(result
        .unwrap_err()
        .to_string()
        .contains("configured PostgreSQL long-term store requires TASKCAST_POSTGRES_URL"));

    let listener = tokio::net::TcpListener::bind(("127.0.0.1", http_port))
        .await
        .expect("HTTP port must remain unbound");
    drop(listener);
}

#[tokio::test]
async fn active_unreachable_redis_fails_before_http_bind() {
    let dependency_port = available_port().await;
    let redis_url = format!("redis://127.0.0.1:{dependency_port}");
    let _env = EnvGuard::isolated(&[("TASKCAST_REDIS_URL", &redis_url)]);
    let (_dir, config) = write_config("{}");
    let http_port = available_port().await;

    let result = tokio::time::timeout(
        Duration::from_secs(16),
        taskcast_cli::commands::start::run(StartArgs {
            config: Some(config),
            port: http_port,
            storage: Some("redis".to_string()),
            ..Default::default()
        }),
    )
    .await
    .expect("Redis startup must respect its 15 second deadline");
    assert!(result.is_err());

    let listener = tokio::net::TcpListener::bind(("127.0.0.1", http_port))
        .await
        .expect("HTTP port must remain unbound");
    drop(listener);
}

#[tokio::test]
async fn active_unreachable_postgres_fails_before_http_bind() {
    let dependency_port = available_port().await;
    let postgres_url = format!("postgres://user:pass@127.0.0.1:{dependency_port}/taskcast");
    let _env = EnvGuard::isolated(&[("TASKCAST_POSTGRES_URL", &postgres_url)]);
    let (_dir, config) = write_config("{}");
    let http_port = available_port().await;

    let result = tokio::time::timeout(
        Duration::from_secs(6),
        taskcast_cli::commands::start::run(StartArgs {
            config: Some(config),
            port: http_port,
            ..Default::default()
        }),
    )
    .await
    .expect("PostgreSQL startup must respect its 5 second acquire timeout");
    assert!(result.is_err());

    let listener = tokio::net::TcpListener::bind(("127.0.0.1", http_port))
        .await
        .expect("HTTP port must remain unbound");
    drop(listener);
}

#[tokio::test]
async fn config_file_redis_and_postgres_activate_without_env_urls() {
    let _env = EnvGuard::isolated(&[]);
    let redis = Redis::default().with_tag("7-alpine").start().await.unwrap();
    let redis_port = redis.get_host_port_ipv4(6379).await.unwrap();
    let postgres = Postgres::default()
        .with_tag("16-alpine")
        .start()
        .await
        .unwrap();
    let postgres_port = postgres.get_host_port_ipv4(5432).await.unwrap();
    let (_dir, config) = write_config(&format!(
        r#"
adapters:
  broadcast:
    provider: redis
    url: redis://127.0.0.1:{redis_port}
  shortTermStore:
    provider: redis
    url: redis://127.0.0.1:{redis_port}
  longTermStore:
    provider: postgres
    url: postgres://postgres:postgres@127.0.0.1:{postgres_port}/postgres?sslmode=disable
"#
    ));
    let port = available_port().await;
    let server = tokio::spawn(async move {
        taskcast_cli::commands::start::run(StartArgs {
            config: Some(config),
            port,
            ..Default::default()
        })
        .await
        .map_err(|error| error.to_string())
    });

    wait_for_health(port).await;
    let readiness: serde_json::Value = reqwest::Client::builder()
        .no_proxy()
        .build()
        .unwrap()
        .get(format!("http://127.0.0.1:{port}/health/ready"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let dependencies = readiness["dependencies"].as_object().unwrap();
    let mut names: Vec<_> = dependencies.keys().map(String::as_str).collect();
    names.sort_unstable();
    assert_eq!(names, ["postgres", "redisCommand", "redisPubSub"]);

    server.abort();
}
