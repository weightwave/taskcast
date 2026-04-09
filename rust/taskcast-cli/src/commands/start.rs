use std::sync::Arc;

use clap::Args;

use crate::auto_migrate::run_auto_migrate;
use crate::helpers::{auth_mode_to_string, parse_jwt_algorithm, resolve_port, resolve_storage_mode};

#[derive(Args, Debug)]
pub struct StartArgs {
    /// Config file path
    #[arg(short, long)]
    pub config: Option<String>,
    /// Port to listen on
    #[arg(short, long, default_value = "3721")]
    pub port: u16,
    /// Storage backend: memory, redis, or sqlite
    #[arg(short, long, default_value = "memory")]
    pub storage: String,
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
            storage: "memory".to_string(),
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
async fn create_postgres_pool_with_auto_migrate(
    postgres_url: &str,
) -> Result<sqlx::PgPool, Box<dyn std::error::Error>> {
    let pool = sqlx::PgPool::connect(postgres_url).await?;

    // Run auto-migrate if enabled
    run_auto_migrate(
        &pool,
        std::env::var("TASKCAST_AUTO_MIGRATE").ok().as_deref(),
        std::env::var("TASKCAST_POSTGRES_URL").ok().as_deref(),
    )
    .await?;

    Ok(pool)
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

    // 1. Load config file
    let file_config =
        taskcast_core::config::load_config_file(config.as_deref()).unwrap_or_default();

    // 2. Resolve port: CLI flag > config file > default
    let port = resolve_port(port, file_config.port);

    // 3. Resolve adapter URLs
    let redis_url = std::env::var("TASKCAST_REDIS_URL")
        .ok()
        .or_else(|| file_config.adapters.as_ref()?.broadcast.as_ref()?.url.clone());
    let postgres_url = std::env::var("TASKCAST_POSTGRES_URL").ok().or_else(|| {
        file_config
            .adapters
            .as_ref()?
            .long_term_store
            .as_ref()?
            .url
            .clone()
    });

    // 4. Resolve storage mode: CLI flag > env var > auto-detect
    let env_storage = std::env::var("TASKCAST_STORAGE").ok();
    let storage_mode = resolve_storage_mode(&storage, env_storage.as_deref(), redis_url.is_some());

    // 5. Build adapters
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
            let url = redis_url
                .as_deref()
                .ok_or("--storage redis requires TASKCAST_REDIS_URL")?;
            let client = redis::Client::open(url)?;
            let pub_conn = client.get_multiplexed_async_connection().await?;
            let sub_conn = client.get_async_pubsub().await?;
            let store_conn = client.get_multiplexed_async_connection().await?;

            let adapters =
                taskcast_redis::create_redis_adapters(pub_conn, sub_conn, store_conn, None);

            let long_term_store: Option<Arc<dyn taskcast_core::LongTermStore>> =
                if let Some(ref pg_url) = postgres_url {
                    let pool = create_postgres_pool_with_auto_migrate(pg_url).await?;
                    let store = taskcast_postgres::PostgresLongTermStore::new(pool);
                    Some(Arc::new(store))
                } else {
                    None
                };

            (
                Arc::new(adapters.broadcast),
                Arc::new(adapters.short_term_store),
                long_term_store,
            )
        }
        _ => {
            eprintln!(
                "[taskcast] No TASKCAST_REDIS_URL configured \u{2014} using in-memory adapters"
            );

            let long_term_store: Option<Arc<dyn taskcast_core::LongTermStore>> =
                if let Some(ref pg_url) = postgres_url {
                    let pool = create_postgres_pool_with_auto_migrate(pg_url).await?;
                    let store = taskcast_postgres::PostgresLongTermStore::new(pool);
                    Some(Arc::new(store))
                } else {
                    None
                };

            (
                Arc::new(taskcast_core::MemoryBroadcastProvider::new()),
                Arc::new(taskcast_core::MemoryShortTermStore::new()),
                long_term_store,
            )
        }
    };

    // 6. Build engine (clone adapters for WorkerManager before moving into engine)
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

    // 7. Auth mode
    let auth_mode_str = std::env::var("TASKCAST_AUTH_MODE")
        .ok()
        .or_else(|| file_config.auth.as_ref().map(|a| auth_mode_to_string(&a.mode)));

    let auth_mode = match auth_mode_str.as_deref() {
        Some("jwt") => {
            let jwt_config = file_config
                .auth
                .as_ref()
                .and_then(|a| a.jwt.as_ref());

            let algorithm =
                parse_jwt_algorithm(jwt_config.and_then(|j| j.algorithm.as_deref()));

            taskcast_server::AuthMode::Jwt(taskcast_server::JwtConfig {
                algorithm,
                secret: std::env::var("TASKCAST_JWT_SECRET")
                    .ok()
                    .or_else(|| jwt_config?.secret.clone()),
                public_key: jwt_config.and_then(|j| j.public_key.clone()),
                issuer: jwt_config.and_then(|j| j.issuer.clone()),
                audience: jwt_config.and_then(|j| j.audience.clone()),
            })
        }
        _ => taskcast_server::AuthMode::None,
    };

    // 8. Create WorkerManager if workers enabled in config
    let workers_enabled = file_config
        .workers
        .as_ref()
        .and_then(|w| w.enabled)
        .unwrap_or(false);

    let worker_manager = if workers_enabled {
        println!("[taskcast] Worker assignment system enabled");

        let mut wm_defaults =
            taskcast_core::worker_manager::WorkerManagerDefaults::default();
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

        Some(Arc::new(
            taskcast_core::worker_manager::WorkerManager::new(
                taskcast_core::worker_manager::WorkerManagerOptions {
                    engine: Arc::clone(&engine),
                    short_term_store: short_term_for_wm,
                    broadcast: broadcast_for_wm,
                    long_term_store: long_term_for_wm,
                    hooks: None,
                    defaults: Some(wm_defaults),
                },
            ),
        ))
    } else {
        None
    };

    // 9. Create and serve app
    let (app, _ws_registry) =
        taskcast_server::create_app(engine, auth_mode, worker_manager, None, taskcast_server::CorsConfig::default());

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

    // Serve playground static files if --playground
    let app = if playground {
        println!("[taskcast] Playground UI at http://localhost:{port}/_playground/");
        app.nest(
            "/_playground",
            crate::commands::playground::playground_routes(),
        )
    } else {
        app
    };

    let listener = tokio::net::TcpListener::bind(format!("0.0.0.0:{port}")).await?;
    println!("[taskcast] Server started on http://localhost:{port}");
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;

    Ok(())
}

async fn shutdown_signal() {
    let ctrl_c = tokio::signal::ctrl_c();

    #[cfg(unix)]
    {
        let mut sigterm = tokio::signal::unix::signal(
            tokio::signal::unix::SignalKind::terminate(),
        )
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
