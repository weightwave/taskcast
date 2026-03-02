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
            let port = if port != 3721 {
                port
            } else {
                file_config.port.unwrap_or(port)
            };

            // 3. Resolve adapter URLs
            let redis_url = std::env::var("TASKCAST_REDIS_URL")
                .ok()
                .or_else(|| file_config.adapters.as_ref()?.broadcast.as_ref()?.url.clone());
            let postgres_url = std::env::var("TASKCAST_POSTGRES_URL")
                .ok()
                .or_else(|| file_config.adapters.as_ref()?.long_term.as_ref()?.url.clone());

            // 4. Resolve storage mode: CLI flag > env var > auto-detect
            let storage_mode = if storage != "memory" {
                storage.as_str()
            } else if std::env::var("TASKCAST_STORAGE").ok().as_deref() == Some("sqlite") {
                "sqlite"
            } else if redis_url.is_some() {
                "redis"
            } else {
                "memory"
            };

            // 5. Build adapters
            let (broadcast, short_term, long_term): (
                Arc<dyn taskcast_core::BroadcastProvider>,
                Arc<dyn taskcast_core::ShortTermStore>,
                Option<Arc<dyn taskcast_core::LongTermStore>>,
            ) = match storage_mode {
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

                    let long_term: Option<Arc<dyn taskcast_core::LongTermStore>> =
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
                        Arc::new(adapters.short_term),
                        long_term,
                    )
                }
                _ => {
                    eprintln!(
                        "[taskcast] No TASKCAST_REDIS_URL configured \u{2014} using in-memory adapters"
                    );

                    let long_term: Option<Arc<dyn taskcast_core::LongTermStore>> =
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
                        long_term,
                    )
                }
            };

            // 6. Build engine
            let engine = Arc::new(taskcast_core::TaskEngine::new(
                taskcast_core::TaskEngineOptions {
                    short_term,
                    broadcast,
                    long_term,
                    hooks: None,
                },
            ));

            // 7. Auth mode
            let auth_mode_str = std::env::var("TASKCAST_AUTH_MODE").ok().or_else(|| {
                file_config.auth.as_ref().map(|a| match a.mode {
                    taskcast_core::config::AuthMode::None => "none".to_string(),
                    taskcast_core::config::AuthMode::Jwt => "jwt".to_string(),
                    taskcast_core::config::AuthMode::Custom => "custom".to_string(),
                })
            });

            let auth_mode = match auth_mode_str.as_deref() {
                Some("jwt") => {
                    let jwt_config = file_config
                        .auth
                        .as_ref()
                        .and_then(|a| a.jwt.as_ref());

                    let algorithm = jwt_config
                        .and_then(|j| j.algorithm.as_deref())
                        .map(|a| match a {
                            "RS256" => jsonwebtoken::Algorithm::RS256,
                            "RS384" => jsonwebtoken::Algorithm::RS384,
                            "RS512" => jsonwebtoken::Algorithm::RS512,
                            "ES256" => jsonwebtoken::Algorithm::ES256,
                            "ES384" => jsonwebtoken::Algorithm::ES384,
                            "PS256" => jsonwebtoken::Algorithm::PS256,
                            "PS384" => jsonwebtoken::Algorithm::PS384,
                            "PS512" => jsonwebtoken::Algorithm::PS512,
                            _ => jsonwebtoken::Algorithm::HS256,
                        })
                        .unwrap_or(jsonwebtoken::Algorithm::HS256);

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

            // 8. Create and serve app
            let app = taskcast_server::create_app(engine, auth_mode);
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
