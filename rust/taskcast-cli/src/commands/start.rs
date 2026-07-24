use std::sync::Arc;
use std::time::Duration;

use clap::Args;

use crate::auto_migrate::run_auto_migrate;
use crate::helpers::{
    auth_mode_to_string, parse_jwt_algorithm, resolve_port, resolve_storage_mode,
};

#[derive(Args, Debug)]
pub struct StartArgs {
    /// Config file path
    #[arg(short, long)]
    pub config: Option<String>,
    /// Port to listen on
    #[arg(short, long, default_value = "3721")]
    pub port: u16,
    /// Storage backend: memory, redis, or sqlite
    #[arg(short, long)]
    pub storage: Option<String>,
    /// SQLite database file path (default: ./taskcast.db)
    #[arg(long, default_value = "./taskcast.db")]
    pub db_path: String,
    /// Serve the interactive playground UI at /_playground/
    #[arg(long)]
    pub playground: bool,
    /// Enable verbose output
    #[arg(short, long)]
    pub verbose: bool,
}

impl Default for StartArgs {
    fn default() -> Self {
        Self {
            config: None,
            port: 3721,
            storage: None,
            db_path: "./taskcast.db".to_string(),
            playground: false,
            verbose: false,
        }
    }
}

/// Create a Postgres pool and run auto-migrations if enabled.
///
/// This helper encapsulates the pool creation + auto-migrate flow.
/// It's called from the main `run()` function in multiple places.
///
/// The pool itself (not the env var) is passed to `run_auto_migrate` as
/// proof that Postgres is configured, so the helper works correctly
/// regardless of whether the URL came from an env var or the config file.
async fn create_postgres_pool_with_auto_migrate(
    postgres_url: &str,
    max_connections: u32,
) -> Result<sqlx::PgPool, std::io::Error> {
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(max_connections)
        .min_connections(0)
        .acquire_timeout(Duration::from_secs(5))
        .connect(postgres_url)
        .await
        .map_err(|error| std::io::Error::other(error.to_string()))?;

    match tokio::time::timeout(
        Duration::from_secs(5),
        sqlx::query("SELECT 1").execute(&pool),
    )
    .await
    {
        Ok(Ok(_)) => {}
        Ok(Err(error)) => {
            pool.close().await;
            return Err(std::io::Error::other(error.to_string()));
        }
        Err(error) => {
            pool.close().await;
            return Err(std::io::Error::other(error.to_string()));
        }
    }

    let migration_result: Result<(), std::io::Error> = run_auto_migrate(
        Some(&pool),
        Some(postgres_url),
        std::env::var("TASKCAST_AUTO_MIGRATE").ok().as_deref(),
    )
    .await
    .map_err(|error| std::io::Error::other(error.to_string()));
    if let Err(error) = migration_result {
        pool.close().await;
        return Err(error);
    }

    Ok(pool)
}

fn env_non_empty(key: &str) -> Option<String> {
    std::env::var(key).ok().filter(|value| !value.is_empty())
}

fn resolve_log_level(value: Option<&str>) -> Result<taskcast_server::LogLevel, String> {
    taskcast_server::LogLevel::parse(value)
}

pub fn parse_postgres_max_connections(value: Option<&str>) -> Result<u32, String> {
    match value.filter(|value| !value.is_empty()) {
        None => Ok(10),
        Some(value) => value
            .parse::<u32>()
            .ok()
            .filter(|parsed| *parsed > 0)
            .ok_or_else(|| {
                "TASKCAST_POSTGRES_MAX_CONNECTIONS must be a positive integer".to_string()
            }),
    }
}

fn effective_runtime_adapters(
    storage_mode: &str,
    postgres_active: bool,
) -> taskcast_server::RuntimeAdapterDescriptors {
    if storage_mode == "sqlite" {
        return taskcast_server::RuntimeAdapterDescriptors {
            broadcast: "memory".to_string(),
            short_term_store: "sqlite".to_string(),
            long_term_store: Some("sqlite".to_string()),
        };
    }
    taskcast_server::RuntimeAdapterDescriptors {
        broadcast: storage_mode.to_string(),
        short_term_store: storage_mode.to_string(),
        long_term_store: postgres_active.then(|| "postgres".to_string()),
    }
}

fn configured_storage_provider(
    config: &taskcast_core::config::TaskcastConfig,
) -> Result<Option<&str>, String> {
    let short_term = config
        .adapters
        .as_ref()
        .and_then(|adapters| adapters.short_term_store.as_ref())
        .map(|entry| entry.provider.as_str());
    let broadcast = config
        .adapters
        .as_ref()
        .and_then(|adapters| adapters.broadcast.as_ref())
        .map(|entry| entry.provider.as_str());
    if short_term.is_some() && broadcast.is_some() && short_term != broadcast {
        return Err("configured short-term and broadcast providers must match".to_string());
    }
    Ok(short_term.or(broadcast))
}

fn configured_redis_url(config: &taskcast_core::config::TaskcastConfig) -> Option<String> {
    config
        .adapters
        .as_ref()
        .and_then(|adapters| adapters.broadcast.as_ref())
        .and_then(|entry| entry.url.clone())
        .filter(|url| !url.is_empty())
        .or_else(|| {
            config
                .adapters
                .as_ref()
                .and_then(|adapters| adapters.short_term_store.as_ref())
                .and_then(|entry| entry.url.clone())
                .filter(|url| !url.is_empty())
        })
}

fn postgres_activation(
    storage_mode: &str,
    configured_provider: Option<&str>,
    env_url: Option<String>,
    configured_url: Option<String>,
) -> Result<Option<String>, String> {
    if storage_mode == "sqlite" {
        return Ok(None);
    }
    if let Some(provider) = configured_provider {
        if provider != "postgres" {
            return Ok(None);
        }
        return env_url
            .or(configured_url)
            .filter(|url| !url.is_empty())
            .map(Some)
            .ok_or_else(|| {
                "configured PostgreSQL long-term store requires TASKCAST_POSTGRES_URL or adapters.longTermStore.url"
                    .to_string()
            });
    }
    Ok(env_url)
}

#[cfg(test)]
mod log_level_tests {
    use taskcast_server::LogLevel;

    use super::{effective_runtime_adapters, resolve_log_level};

    #[test]
    fn defaults_to_info() {
        assert_eq!(resolve_log_level(None).unwrap(), LogLevel::Info);
    }

    #[test]
    fn accepts_case_insensitive_levels() {
        assert_eq!(resolve_log_level(Some("DEBUG")).unwrap(), LogLevel::Debug);
        assert_eq!(resolve_log_level(Some("Info")).unwrap(), LogLevel::Info);
        assert_eq!(resolve_log_level(Some("Warn")).unwrap(), LogLevel::Warn);
        assert_eq!(resolve_log_level(Some("error")).unwrap(), LogLevel::Error);
    }

    #[test]
    fn rejects_invalid_level() {
        assert!(resolve_log_level(Some("trace"))
            .unwrap_err()
            .contains("invalid TASKCAST_LOG_LEVEL"));
    }

    #[test]
    fn runtime_adapters_follow_selected_storage_not_raw_config() {
        let memory = effective_runtime_adapters("memory", false);
        assert_eq!(memory.broadcast, "memory");
        assert_eq!(memory.short_term_store, "memory");
        assert_eq!(memory.long_term_store, None);

        let redis = effective_runtime_adapters("redis", true);
        assert_eq!(redis.broadcast, "redis");
        assert_eq!(redis.short_term_store, "redis");
        assert_eq!(redis.long_term_store.as_deref(), Some("postgres"));

        let sqlite = effective_runtime_adapters("sqlite", true);
        assert_eq!(sqlite.broadcast, "memory");
        assert_eq!(sqlite.short_term_store, "sqlite");
        assert_eq!(sqlite.long_term_store.as_deref(), Some("sqlite"));
    }
}

fn trusted_service_task_ids(
    task_ids: Option<&taskcast_core::config::TrustedServiceTaskIds>,
) -> taskcast_server::TaskIdAccess {
    match task_ids {
        Some(taskcast_core::config::TrustedServiceTaskIds::List(ids)) => {
            taskcast_server::TaskIdAccess::List(ids.clone())
        }
        Some(taskcast_core::config::TrustedServiceTaskIds::Wildcard(_)) | None => {
            taskcast_server::TaskIdAccess::All
        }
    }
}

fn trusted_services_from_config(
    services: Option<&[taskcast_core::config::TrustedServiceConfig]>,
) -> Vec<taskcast_server::TrustedServiceConfig> {
    services
        .unwrap_or_default()
        .iter()
        .map(|service| taskcast_server::TrustedServiceConfig {
            name: service.name.clone(),
            key: service.key.clone(),
            task_ids: trusted_service_task_ids(service.task_ids.as_ref()),
            scope: service.scope.clone(),
        })
        .collect()
}

fn build_auth_mode(
    file_config: &taskcast_core::config::TaskcastConfig,
) -> Result<taskcast_server::AuthMode, Box<dyn std::error::Error>> {
    let auth_mode_str = std::env::var("TASKCAST_AUTH_MODE").ok().or_else(|| {
        file_config
            .auth
            .as_ref()
            .map(|auth| auth_mode_to_string(&auth.mode))
    });

    if auth_mode_str.as_deref() != Some("jwt") {
        return Ok(taskcast_server::AuthMode::None);
    }

    let jwt_config = file_config.auth.as_ref().and_then(|auth| auth.jwt.as_ref());
    let env_algorithm = env_non_empty("TASKCAST_JWT_ALGORITHM");
    let algorithm = parse_jwt_algorithm(
        env_algorithm
            .as_deref()
            .or_else(|| jwt_config.and_then(|jwt| jwt.algorithm.as_deref())),
    );
    let public_key = if let Some(key) = env_non_empty("TASKCAST_JWT_PUBLIC_KEY") {
        Some(key)
    } else if let Some(path) = env_non_empty("TASKCAST_JWT_PUBLIC_KEY_FILE") {
        Some(std::fs::read_to_string(path)?)
    } else if let Some(key) = jwt_config.and_then(|jwt| jwt.public_key.clone()) {
        Some(key)
    } else if let Some(path) = jwt_config.and_then(|jwt| jwt.public_key_file.clone()) {
        Some(std::fs::read_to_string(path)?)
    } else {
        None
    };
    let jwt = taskcast_server::JwtConfig {
        algorithm,
        secret: env_non_empty("TASKCAST_JWT_SECRET")
            .or_else(|| jwt_config.and_then(|jwt| jwt.secret.clone())),
        public_key,
        issuer: env_non_empty("TASKCAST_JWT_ISSUER")
            .or_else(|| jwt_config.and_then(|jwt| jwt.issuer.clone())),
        audience: env_non_empty("TASKCAST_JWT_AUDIENCE")
            .or_else(|| jwt_config.and_then(|jwt| jwt.audience.clone())),
    };
    let trusted_services = trusted_services_from_config(file_config.trusted_services.as_deref());

    Ok(if trusted_services.is_empty() {
        taskcast_server::AuthMode::Jwt(jwt)
    } else {
        taskcast_server::AuthMode::JwtWithTrustedServices {
            jwt,
            trusted_services,
        }
    })
}

pub async fn run(args: StartArgs) -> Result<(), Box<dyn std::error::Error>> {
    let StartArgs {
        config,
        port,
        storage,
        db_path,
        playground,
        verbose,
    } = args;

    let log_level = resolve_log_level(env_non_empty("TASKCAST_LOG_LEVEL").as_deref())?;

    // 1. Load config file
    let file_config =
        taskcast_core::config::load_config_file(config.as_deref()).unwrap_or_default();

    // 2. Resolve port: CLI flag > config file > default
    let port = resolve_port(port, file_config.port);

    // 3. Resolve activation before opening any network connection.
    let redis_url =
        env_non_empty("TASKCAST_REDIS_URL").or_else(|| configured_redis_url(&file_config));
    let env_storage = env_non_empty("TASKCAST_STORAGE");
    let configured_provider = if storage.is_none() && env_storage.is_none() {
        configured_storage_provider(&file_config)?
    } else {
        None
    };
    let storage_mode = resolve_storage_mode(
        storage.as_deref(),
        env_storage.as_deref(),
        configured_provider,
        redis_url.is_some(),
    )?;
    if storage_mode == "redis" && redis_url.is_none() {
        return Err(
            "storage mode redis requires TASKCAST_REDIS_URL or a configured Redis URL".into(),
        );
    }

    let configured_long_term = file_config
        .adapters
        .as_ref()
        .and_then(|adapters| adapters.long_term_store.as_ref());
    let postgres_url = postgres_activation(
        storage_mode,
        configured_long_term.map(|entry| entry.provider.as_str()),
        env_non_empty("TASKCAST_POSTGRES_URL"),
        configured_long_term.and_then(|entry| entry.url.clone()),
    )?;
    let max_connections = if postgres_url.is_some() {
        parse_postgres_max_connections(
            std::env::var("TASKCAST_POSTGRES_MAX_CONNECTIONS")
                .ok()
                .as_deref(),
        )?
    } else {
        10
    };
    let auth_mode = build_auth_mode(&file_config)?;

    let dependency_health = Arc::new(taskcast_server::DependencyHealthRegistry::new());
    let observer: Arc<dyn taskcast_core::DependencyObserver> = dependency_health.clone();
    let managed_redis = if storage_mode == "redis" {
        let client = redis::Client::open(
            redis_url
                .as_deref()
                .expect("Redis URL checked for active Redis storage"),
        )?;
        Some(
            taskcast_redis::create_managed_redis_adapters(client, None, Some(observer.clone()))
                .await
                .map_err(|error| {
                    Box::new(std::io::Error::other(error.to_string())) as Box<dyn std::error::Error>
                })?,
        )
    } else {
        None
    };

    let (redis_adapters, redis_command_manager, redis_pubsub) = if let Some(managed) = managed_redis
    {
        let taskcast_redis::ManagedRedisAdapters {
            adapters,
            command_manager,
            pubsub,
        } = managed;
        let pubsub = Arc::new(pubsub);
        let manager_for_check = command_manager.clone();
        let command_check: taskcast_server::DependencyCheck = Arc::new(move || {
            let manager = manager_for_check.clone();
            Box::pin(async move { taskcast_redis::command_check(&manager).await })
        });
        if let Err(error) =
            dependency_health.register(taskcast_core::DependencyName::RedisCommand, command_check)
        {
            pubsub.shutdown().await;
            drop(command_manager);
            return Err(error.into());
        }
        let pubsub_for_check = Arc::clone(&pubsub);
        let pubsub_check: taskcast_server::DependencyCheck = Arc::new(move || {
            let pubsub = Arc::clone(&pubsub_for_check);
            Box::pin(async move {
                if pubsub.is_subscribed() {
                    Ok(())
                } else {
                    Err(taskcast_core::DependencyUnavailableError::new(
                        taskcast_core::DependencyName::RedisPubSub,
                        taskcast_core::DependencyErrorKind::ConnectionClosed,
                        std::io::Error::new(
                            std::io::ErrorKind::NotConnected,
                            "Redis PubSub is not subscribed",
                        ),
                    ))
                }
            })
        });
        if let Err(error) =
            dependency_health.register(taskcast_core::DependencyName::RedisPubSub, pubsub_check)
        {
            pubsub.shutdown().await;
            drop(command_manager);
            return Err(error.into());
        }
        (Some(adapters), Some(command_manager), Some(pubsub))
    } else {
        (None, None, None)
    };

    let postgres_pool = if let Some(postgres_url) = postgres_url.as_deref() {
        match create_postgres_pool_with_auto_migrate(postgres_url, max_connections).await {
            Ok(pool) => Some(pool),
            Err(error) => {
                if let Some(pubsub) = redis_pubsub.as_ref() {
                    pubsub.shutdown().await;
                }
                return Err(Box::new(error));
            }
        }
    } else {
        None
    };
    if let Some(pool) = postgres_pool.as_ref() {
        let pool_for_check = pool.clone();
        let check: taskcast_server::DependencyCheck = Arc::new(move || {
            let pool = pool_for_check.clone();
            Box::pin(async move { taskcast_postgres::postgres_check(&pool).await })
        });
        if let Err(error) =
            dependency_health.register(taskcast_core::DependencyName::Postgres, check)
        {
            close_runtime_dependencies(redis_pubsub.as_ref(), postgres_pool.as_ref()).await;
            drop(redis_command_manager);
            return Err(error.into());
        }
    }

    // 4. Build adapters.
    type StorageAdapters = (
        Arc<dyn taskcast_core::BroadcastProvider>,
        Arc<dyn taskcast_core::ShortTermStore>,
        Option<Arc<dyn taskcast_core::LongTermStore>>,
    );
    let (broadcast, short_term_store, long_term_store): StorageAdapters = match storage_mode {
        "sqlite" => {
            let adapters = taskcast_sqlite::create_sqlite_adapters(&db_path).await?;
            eprintln!("[taskcast] Using SQLite storage at {db_path}");
            (
                Arc::new(taskcast_core::MemoryBroadcastProvider::new()),
                Arc::new(adapters.short_term_store),
                Some(Arc::new(adapters.long_term_store) as Arc<dyn taskcast_core::LongTermStore>),
            )
        }
        "redis" => {
            let adapters = redis_adapters.expect("managed Redis adapters must exist");
            let long_term_store = postgres_pool.as_ref().map(|pool| {
                Arc::new(taskcast_postgres::PostgresLongTermStore::new_observed(
                    pool.clone(),
                    observer.clone(),
                )) as Arc<dyn taskcast_core::LongTermStore>
            });
            (
                Arc::new(adapters.broadcast),
                Arc::new(adapters.short_term_store),
                long_term_store,
            )
        }
        _ => {
            eprintln!("[taskcast] Using in-memory adapters");
            let long_term_store = postgres_pool.as_ref().map(|pool| {
                Arc::new(taskcast_postgres::PostgresLongTermStore::new_observed(
                    pool.clone(),
                    observer.clone(),
                )) as Arc<dyn taskcast_core::LongTermStore>
            });

            (
                Arc::new(taskcast_core::MemoryBroadcastProvider::new()),
                Arc::new(taskcast_core::MemoryShortTermStore::new()),
                long_term_store,
            )
        }
    };

    // 5. Build engine (clone adapters for WorkerManager before moving into engine)
    let short_term_for_wm = Arc::clone(&short_term_store);
    let broadcast_for_wm = Arc::clone(&broadcast);
    let long_term_for_wm = long_term_store.clone();

    let engine = Arc::new(taskcast_core::TaskEngine::new(
        taskcast_core::TaskEngineOptions {
            short_term_store,
            broadcast,
            long_term_store,
            hooks: None,
        },
    ));

    // 6. Create WorkerManager if workers enabled in config
    let workers_enabled = file_config
        .workers
        .as_ref()
        .and_then(|w| w.enabled)
        .unwrap_or(false);

    let worker_manager = if workers_enabled {
        println!("[taskcast] Worker assignment system enabled");

        let mut wm_defaults = taskcast_core::worker_manager::WorkerManagerDefaults::default();
        if let Some(cfg_defaults) = file_config
            .workers
            .as_ref()
            .and_then(|w| w.defaults.as_ref())
        {
            if let Some(v) = cfg_defaults.heartbeat_interval_ms {
                wm_defaults.heartbeat_interval_ms = Some(v);
            }
            if let Some(v) = cfg_defaults.heartbeat_timeout_ms {
                wm_defaults.heartbeat_timeout_ms = Some(v);
            }
            if let Some(v) = cfg_defaults.offer_timeout_ms {
                wm_defaults.offer_timeout_ms = Some(v);
            }
            if let Some(v) = cfg_defaults.disconnect_grace_ms {
                wm_defaults.disconnect_grace_ms = Some(v);
            }
            if let Some(ref mode) = cfg_defaults.assign_mode {
                wm_defaults.assign_mode = match mode.as_str() {
                    "pull" => Some(taskcast_core::AssignMode::Pull),
                    "ws-offer" => Some(taskcast_core::AssignMode::WsOffer),
                    "ws-race" => Some(taskcast_core::AssignMode::WsRace),
                    _ => Some(taskcast_core::AssignMode::External),
                };
            }
            if let Some(ref policy) = cfg_defaults.disconnect_policy {
                wm_defaults.disconnect_policy = match policy.as_str() {
                    "mark" => Some(taskcast_core::DisconnectPolicy::Mark),
                    "fail" => Some(taskcast_core::DisconnectPolicy::Fail),
                    _ => Some(taskcast_core::DisconnectPolicy::Reassign),
                };
            }
        }

        Some(Arc::new(taskcast_core::worker_manager::WorkerManager::new(
            taskcast_core::worker_manager::WorkerManagerOptions {
                engine: Arc::clone(&engine),
                short_term_store: short_term_for_wm,
                broadcast: broadcast_for_wm,
                long_term_store: long_term_for_wm,
                hooks: None,
                defaults: Some(wm_defaults),
            },
        )))
    } else {
        None
    };

    // 7. Compose all routes before applying the single outer failure logger.
    let additional_routes = if playground {
        println!("[taskcast] Playground UI at http://localhost:{port}/_playground/");
        axum::Router::new().nest(
            "/_playground",
            crate::commands::playground::playground_routes(),
        )
    } else {
        axum::Router::new()
    };
    let failure_logger: Arc<dyn taskcast_server::HttpFailureLogger> =
        Arc::new(taskcast_server::StderrHttpFailureLogger::new(log_level));
    let effective_adapters = effective_runtime_adapters(storage_mode, postgres_pool.is_some());
    let runtime_health = taskcast_server::RuntimeHealth {
        registry: Some(dependency_health),
        effective_adapters: Some(effective_adapters),
    };
    let (app, _ws_registry) = taskcast_server::create_app_with_runtime_health_and_routes(
        engine,
        auth_mode,
        worker_manager,
        Some(file_config.clone()),
        taskcast_server::CorsConfig::default(),
        Arc::clone(&failure_logger),
        runtime_health,
        additional_routes,
    );

    // Apply verbose request logging middleware if --verbose
    let app = if verbose {
        eprintln!("[taskcast] Verbose request logging enabled");
        let logger: std::sync::Arc<dyn taskcast_server::VerboseLogger> =
            std::sync::Arc::new(taskcast_server::StderrLogger);
        app.layer(axum::middleware::from_fn_with_state(
            logger,
            taskcast_server::verbose_logger_middleware,
        ))
    } else {
        app
    };

    let listener = match tokio::net::TcpListener::bind(format!("0.0.0.0:{port}")).await {
        Ok(listener) => listener,
        Err(error) => {
            close_runtime_dependencies(redis_pubsub.as_ref(), postgres_pool.as_ref()).await;
            drop(redis_command_manager);
            return Err(Box::new(error));
        }
    };
    println!("[taskcast] Server started on http://localhost:{port}");
    let serve_result = axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await;
    close_runtime_dependencies(redis_pubsub.as_ref(), postgres_pool.as_ref()).await;
    drop(redis_command_manager);
    serve_result?;

    Ok(())
}

async fn close_runtime_dependencies(
    redis_pubsub: Option<&Arc<taskcast_redis::RedisPubSubHandle>>,
    postgres_pool: Option<&sqlx::PgPool>,
) {
    if let Some(pubsub) = redis_pubsub {
        pubsub.shutdown().await;
    }
    if let Some(pool) = postgres_pool {
        pool.close().await;
    }
}

async fn shutdown_signal() {
    let ctrl_c = tokio::signal::ctrl_c();

    #[cfg(unix)]
    {
        let mut sigterm = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("failed to register SIGTERM handler");

        tokio::select! {
            _ = ctrl_c => {},
            _ = sigterm.recv() => {},
        }
    }

    #[cfg(not(unix))]
    {
        ctrl_c.await.ok();
    }

    eprintln!("[taskcast] Shutting down gracefully...");
}
