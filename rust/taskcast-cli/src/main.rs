use std::sync::Arc;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "taskcast",
    version = "0.1.0",
    about = "Taskcast \u{2014} unified task tracking and streaming service"
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    /// Start the taskcast server in foreground (default)
    Start {
        /// Config file path
        #[arg(short, long)]
        config: Option<String>,
        /// Port to listen on
        #[arg(short, long, default_value = "3721")]
        port: u16,
        /// Storage backend: memory, redis, or sqlite
        #[arg(short, long, default_value = "memory")]
        storage: String,
        /// SQLite database file path (default: ./taskcast.db)
        #[arg(long, default_value = "./taskcast.db")]
        db_path: String,
    },
    /// Start the server as a background service (not yet implemented)
    Daemon,
    /// Stop the background service (not yet implemented)
    Stop,
    /// Show server status (not yet implemented)
    Status,
}

// ─── Pure Helper Functions (testable) ────────────────────────────────────────

const DEFAULT_PORT: u16 = 3721;

/// Resolve the port: CLI flag (if changed from default) > config file > default.
fn resolve_port(cli_port: u16, config_port: Option<u16>) -> u16 {
    if cli_port != DEFAULT_PORT {
        cli_port
    } else {
        config_port.unwrap_or(cli_port)
    }
}

/// Resolve storage mode: CLI flag (if not "memory") > env var > auto-detect from redis_url.
fn resolve_storage_mode<'a>(
    cli_storage: &'a str,
    env_storage: Option<&'a str>,
    has_redis_url: bool,
) -> &'a str {
    if cli_storage != "memory" {
        cli_storage
    } else if env_storage == Some("sqlite") {
        "sqlite"
    } else if has_redis_url {
        "redis"
    } else {
        "memory"
    }
}

/// Map a JWT algorithm string to jsonwebtoken::Algorithm. Defaults to HS256.
fn parse_jwt_algorithm(alg: Option<&str>) -> jsonwebtoken::Algorithm {
    match alg {
        Some("RS256") => jsonwebtoken::Algorithm::RS256,
        Some("RS384") => jsonwebtoken::Algorithm::RS384,
        Some("RS512") => jsonwebtoken::Algorithm::RS512,
        Some("ES256") => jsonwebtoken::Algorithm::ES256,
        Some("ES384") => jsonwebtoken::Algorithm::ES384,
        Some("PS256") => jsonwebtoken::Algorithm::PS256,
        Some("PS384") => jsonwebtoken::Algorithm::PS384,
        Some("PS512") => jsonwebtoken::Algorithm::PS512,
        _ => jsonwebtoken::Algorithm::HS256,
    }
}

/// Convert a config AuthMode enum to its string representation.
fn auth_mode_to_string(mode: &taskcast_core::config::AuthMode) -> String {
    match mode {
        taskcast_core::config::AuthMode::None => "none".to_string(),
        taskcast_core::config::AuthMode::Jwt => "jwt".to_string(),
        taskcast_core::config::AuthMode::Custom => "custom".to_string(),
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();
    let cmd = cli.command.unwrap_or(Commands::Start {
        config: None,
        port: 3721,
        storage: "memory".to_string(),
        db_path: "./taskcast.db".to_string(),
    });

    match cmd {
        Commands::Start {
            config,
            port,
            storage,
            db_path,
        } => {
            // 1. Load config file
            let file_config = taskcast_core::config::load_config_file(config.as_deref())
                .unwrap_or_default();

            // 2. Resolve port: CLI flag > config file > default
            let port = resolve_port(port, file_config.port);

            // 3. Resolve adapter URLs
            let redis_url = std::env::var("TASKCAST_REDIS_URL")
                .ok()
                .or_else(|| file_config.adapters.as_ref()?.broadcast.as_ref()?.url.clone());
            let postgres_url = std::env::var("TASKCAST_POSTGRES_URL")
                .ok()
                .or_else(|| file_config.adapters.as_ref()?.long_term_store.as_ref()?.url.clone());

            // 4. Resolve storage mode: CLI flag > env var > auto-detect
            let env_storage = std::env::var("TASKCAST_STORAGE").ok();
            let storage_mode =
                resolve_storage_mode(&storage, env_storage.as_deref(), redis_url.is_some());

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
                        Arc::new(adapters.short_term),
                        Some(Arc::new(adapters.long_term) as Arc<dyn taskcast_core::LongTermStore>),
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
                            let pool = sqlx::PgPool::connect(pg_url).await?;
                            let store =
                                taskcast_postgres::PostgresLongTermStore::new(pool, None);
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
                            let pool = sqlx::PgPool::connect(pg_url).await?;
                            let store =
                                taskcast_postgres::PostgresLongTermStore::new(pool, None);
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
            let short_term_for_wm = Arc::clone(&short_term);
            let broadcast_for_wm = Arc::clone(&broadcast);
            let long_term_for_wm = long_term.clone();

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

                let mut wm_defaults = taskcast_core::worker_manager::WorkerManagerDefaults::default();
                if let Some(ref cfg_defaults) = file_config.workers.as_ref().and_then(|w| w.defaults.as_ref()) {
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
                        short_term: short_term_for_wm,
                        broadcast: broadcast_for_wm,
                        long_term: long_term_for_wm,
                        hooks: None,
                        defaults: Some(wm_defaults),
                    },
                )))
            } else {
                None
            };

            // 9. Create and serve app
            let app = taskcast_server::create_app(engine, auth_mode, worker_manager);
            let listener = tokio::net::TcpListener::bind(format!("0.0.0.0:{port}")).await?;
            println!("[taskcast] Server started on http://localhost:{port}");
            axum::serve(listener, app).await?;
        }
        Commands::Daemon => {
            eprintln!("[taskcast] daemon mode is not yet implemented, use `taskcast start` for foreground mode");
            std::process::exit(1);
        }
        Commands::Stop => {
            eprintln!("[taskcast] stop is not yet implemented");
            std::process::exit(1);
        }
        Commands::Status => {
            eprintln!("[taskcast] status is not yet implemented");
            std::process::exit(1);
        }
    }

    Ok(())
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ─── resolve_port ────────────────────────────────────────────────────────

    #[test]
    fn port_cli_flag_overrides_config() {
        assert_eq!(resolve_port(8080, Some(9090)), 8080);
    }

    #[test]
    fn port_uses_config_when_cli_is_default() {
        assert_eq!(resolve_port(DEFAULT_PORT, Some(9090)), 9090);
    }

    #[test]
    fn port_falls_back_to_default_when_both_absent() {
        assert_eq!(resolve_port(DEFAULT_PORT, None), DEFAULT_PORT);
    }

    #[test]
    fn port_cli_default_value_is_3721() {
        assert_eq!(DEFAULT_PORT, 3721);
    }

    // ─── resolve_storage_mode ────────────────────────────────────────────────

    #[test]
    fn storage_cli_flag_overrides_all() {
        assert_eq!(resolve_storage_mode("sqlite", None, true), "sqlite");
        assert_eq!(resolve_storage_mode("redis", Some("sqlite"), false), "redis");
    }

    #[test]
    fn storage_env_var_sqlite_overrides_auto_detect() {
        assert_eq!(resolve_storage_mode("memory", Some("sqlite"), true), "sqlite");
    }

    #[test]
    fn storage_auto_detects_redis_from_url() {
        assert_eq!(resolve_storage_mode("memory", None, true), "redis");
    }

    #[test]
    fn storage_defaults_to_memory() {
        assert_eq!(resolve_storage_mode("memory", None, false), "memory");
    }

    #[test]
    fn storage_env_var_non_sqlite_falls_through() {
        // Only "sqlite" env var triggers sqlite mode, other values fall through
        assert_eq!(resolve_storage_mode("memory", Some("redis"), true), "redis");
        assert_eq!(resolve_storage_mode("memory", Some("other"), false), "memory");
    }

    // ─── parse_jwt_algorithm ─────────────────────────────────────────────────

    #[test]
    fn algorithm_maps_all_known_values() {
        assert_eq!(parse_jwt_algorithm(Some("RS256")), jsonwebtoken::Algorithm::RS256);
        assert_eq!(parse_jwt_algorithm(Some("RS384")), jsonwebtoken::Algorithm::RS384);
        assert_eq!(parse_jwt_algorithm(Some("RS512")), jsonwebtoken::Algorithm::RS512);
        assert_eq!(parse_jwt_algorithm(Some("ES256")), jsonwebtoken::Algorithm::ES256);
        assert_eq!(parse_jwt_algorithm(Some("ES384")), jsonwebtoken::Algorithm::ES384);
        assert_eq!(parse_jwt_algorithm(Some("PS256")), jsonwebtoken::Algorithm::PS256);
        assert_eq!(parse_jwt_algorithm(Some("PS384")), jsonwebtoken::Algorithm::PS384);
        assert_eq!(parse_jwt_algorithm(Some("PS512")), jsonwebtoken::Algorithm::PS512);
    }

    #[test]
    fn algorithm_defaults_to_hs256_for_unknown() {
        assert_eq!(parse_jwt_algorithm(Some("UNKNOWN")), jsonwebtoken::Algorithm::HS256);
        assert_eq!(parse_jwt_algorithm(Some("HS384")), jsonwebtoken::Algorithm::HS256);
    }

    #[test]
    fn algorithm_defaults_to_hs256_when_none() {
        assert_eq!(parse_jwt_algorithm(None), jsonwebtoken::Algorithm::HS256);
    }

    // ─── auth_mode_to_string ─────────────────────────────────────────────────

    #[test]
    fn auth_mode_none_to_string() {
        assert_eq!(
            auth_mode_to_string(&taskcast_core::config::AuthMode::None),
            "none"
        );
    }

    #[test]
    fn auth_mode_jwt_to_string() {
        assert_eq!(
            auth_mode_to_string(&taskcast_core::config::AuthMode::Jwt),
            "jwt"
        );
    }

    #[test]
    fn auth_mode_custom_to_string() {
        assert_eq!(
            auth_mode_to_string(&taskcast_core::config::AuthMode::Custom),
            "custom"
        );
    }

    // ─── CLI struct parsing ──────────────────────────────────────────────────

    #[test]
    fn cli_default_command_is_start() {
        let cli = Cli::parse_from(["taskcast"]);
        assert!(cli.command.is_none());
    }

    #[test]
    fn cli_start_subcommand_parses() {
        let cli = Cli::parse_from(["taskcast", "start", "--port", "8080", "--storage", "sqlite"]);
        match cli.command.unwrap() {
            Commands::Start {
                port, storage, ..
            } => {
                assert_eq!(port, 8080);
                assert_eq!(storage, "sqlite");
            }
            _ => panic!("expected Start command"),
        }
    }

    #[test]
    fn cli_start_default_values() {
        let cli = Cli::parse_from(["taskcast", "start"]);
        match cli.command.unwrap() {
            Commands::Start {
                config,
                port,
                storage,
                db_path,
            } => {
                assert!(config.is_none());
                assert_eq!(port, 3721);
                assert_eq!(storage, "memory");
                assert_eq!(db_path, "./taskcast.db");
            }
            _ => panic!("expected Start command"),
        }
    }

    #[test]
    fn cli_start_with_config_flag() {
        let cli = Cli::parse_from(["taskcast", "start", "-c", "/etc/taskcast.yaml"]);
        match cli.command.unwrap() {
            Commands::Start { config, .. } => {
                assert_eq!(config, Some("/etc/taskcast.yaml".to_string()));
            }
            _ => panic!("expected Start command"),
        }
    }

    #[test]
    fn cli_start_with_db_path() {
        let cli = Cli::parse_from(["taskcast", "start", "--db-path", "/data/tasks.db"]);
        match cli.command.unwrap() {
            Commands::Start { db_path, .. } => {
                assert_eq!(db_path, "/data/tasks.db");
            }
            _ => panic!("expected Start command"),
        }
    }

    #[test]
    fn cli_daemon_subcommand_parses() {
        let cli = Cli::parse_from(["taskcast", "daemon"]);
        assert!(matches!(cli.command.unwrap(), Commands::Daemon));
    }

    #[test]
    fn cli_stop_subcommand_parses() {
        let cli = Cli::parse_from(["taskcast", "stop"]);
        assert!(matches!(cli.command.unwrap(), Commands::Stop));
    }

    #[test]
    fn cli_status_subcommand_parses() {
        let cli = Cli::parse_from(["taskcast", "status"]);
        assert!(matches!(cli.command.unwrap(), Commands::Status));
    }
}
