mod commands;
mod helpers;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "taskcast",
    version,
    about = "Taskcast \u{2014} unified task tracking and streaming service"
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    /// Start the taskcast server in foreground (default)
    Start(commands::start::StartArgs),
    /// Serve only the playground UI (no engine)
    Playground(commands::playground::PlaygroundArgs),
    /// Run Postgres database migrations
    Migrate(commands::migrate::MigrateArgs),
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

    match cli.command {
        None => {
            commands::start::run(commands::start::StartArgs::default()).await?;
        }
        Some(Commands::Start(args)) => {
            commands::start::run(args).await?;
        }
        Some(Commands::Migrate(args)) => {
            commands::migrate::run(args).await?;
        }
        Some(Commands::Playground(args)) => {
            commands::playground::run(args).await?;
        }
        Some(Commands::Daemon) => {
            eprintln!("[taskcast] daemon mode is not yet implemented, use `taskcast start` for foreground mode");
            std::process::exit(1);
        }
        Some(Commands::Stop) => {
            eprintln!("[taskcast] stop is not yet implemented");
            std::process::exit(1);
        }
        Some(Commands::Status) => {
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
    use crate::commands::playground::{serve_playground_file, PlaygroundAssets};
    use crate::helpers::*;

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
            Commands::Start(args) => {
                assert_eq!(args.port, 8080);
                assert_eq!(args.storage, "sqlite");
            }
            _ => panic!("expected Start command"),
        }
    }

    #[test]
    fn cli_start_default_values() {
        let cli = Cli::parse_from(["taskcast", "start"]);
        match cli.command.unwrap() {
            Commands::Start(args) => {
                assert!(args.config.is_none());
                assert_eq!(args.port, 3721);
                assert_eq!(args.storage, "memory");
                assert_eq!(args.db_path, "./taskcast.db");
                assert!(!args.playground);
            }
            _ => panic!("expected Start command"),
        }
    }

    #[test]
    fn cli_start_with_playground_flag() {
        let cli = Cli::parse_from(["taskcast", "start", "--playground"]);
        match cli.command.unwrap() {
            Commands::Start(args) => {
                assert!(args.playground);
            }
            _ => panic!("expected Start command"),
        }
    }

    #[test]
    fn cli_playground_subcommand_parses() {
        let cli = Cli::parse_from(["taskcast", "playground"]);
        match cli.command.unwrap() {
            Commands::Playground(args) => {
                assert_eq!(args.port, 5173);
            }
            _ => panic!("expected Playground command"),
        }
    }

    #[test]
    fn cli_playground_with_port() {
        let cli = Cli::parse_from(["taskcast", "playground", "--port", "8080"]);
        match cli.command.unwrap() {
            Commands::Playground(args) => {
                assert_eq!(args.port, 8080);
            }
            _ => panic!("expected Playground command"),
        }
    }

    #[test]
    fn cli_start_with_config_flag() {
        let cli = Cli::parse_from(["taskcast", "start", "-c", "/etc/taskcast.yaml"]);
        match cli.command.unwrap() {
            Commands::Start(args) => {
                assert_eq!(args.config, Some("/etc/taskcast.yaml".to_string()));
            }
            _ => panic!("expected Start command"),
        }
    }

    #[test]
    fn cli_start_with_db_path() {
        let cli = Cli::parse_from(["taskcast", "start", "--db-path", "/data/tasks.db"]);
        match cli.command.unwrap() {
            Commands::Start(args) => {
                assert_eq!(args.db_path, "/data/tasks.db");
            }
            _ => panic!("expected Start command"),
        }
    }

    // ─── Migrate subcommand parsing ────────────────────────────────────────

    #[test]
    fn cli_migrate_subcommand_parses() {
        let cli = Cli::parse_from(["taskcast", "migrate", "--url", "postgres://localhost/db"]);
        match cli.command.unwrap() {
            Commands::Migrate(args) => {
                assert_eq!(args.url, Some("postgres://localhost/db".to_string()));
                assert!(args.config.is_none());
                assert!(!args.yes);
            }
            _ => panic!("expected Migrate command"),
        }
    }

    #[test]
    fn cli_migrate_with_yes_flag() {
        let cli =
            Cli::parse_from(["taskcast", "migrate", "-y", "--url", "postgres://localhost/db"]);
        match cli.command.unwrap() {
            Commands::Migrate(args) => assert!(args.yes),
            _ => panic!("expected Migrate command"),
        }
    }

    #[test]
    fn cli_migrate_with_config_flag() {
        let cli = Cli::parse_from(["taskcast", "migrate", "-c", "/etc/taskcast.yaml"]);
        match cli.command.unwrap() {
            Commands::Migrate(args) => {
                assert_eq!(args.config, Some("/etc/taskcast.yaml".to_string()));
                assert!(args.url.is_none());
            }
            _ => panic!("expected Migrate command"),
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

    // ─── resolve_postgres_url ────────────────────────────────────────────

    #[test]
    fn postgres_url_prefers_cli_flag() {
        assert_eq!(
            resolve_postgres_url(
                Some("postgres://flag".to_string()),
                Some("postgres://env".to_string()),
                Some("postgres://config".to_string()),
            ),
            Some("postgres://flag".to_string())
        );
    }

    #[test]
    fn postgres_url_falls_back_to_env() {
        assert_eq!(
            resolve_postgres_url(
                None,
                Some("postgres://env".to_string()),
                Some("postgres://config".to_string()),
            ),
            Some("postgres://env".to_string())
        );
    }

    #[test]
    fn postgres_url_falls_back_to_config() {
        assert_eq!(
            resolve_postgres_url(
                None,
                None,
                Some("postgres://config".to_string()),
            ),
            Some("postgres://config".to_string())
        );
    }

    #[test]
    fn postgres_url_returns_none_when_all_missing() {
        assert_eq!(resolve_postgres_url(None, None, None), None);
    }

    // ─── format_display_url ──────────────────────────────────────────────

    #[test]
    fn display_url_formats_standard() {
        assert_eq!(
            format_display_url("postgres://user:pass@myhost:5433/mydb"),
            "myhost:5433/mydb"
        );
    }

    #[test]
    fn display_url_uses_default_port() {
        assert_eq!(
            format_display_url("postgres://user@myhost/mydb"),
            "myhost:5432/mydb"
        );
    }

    #[test]
    fn display_url_defaults_db_name() {
        assert_eq!(
            format_display_url("postgres://user@myhost:5432"),
            "myhost:5432/postgres"
        );
        assert_eq!(
            format_display_url("postgres://user@myhost:5432/"),
            "myhost:5432/postgres"
        );
    }

    #[test]
    fn display_url_returns_raw_for_invalid() {
        assert_eq!(
            format_display_url("not-a-url"),
            "not-a-url"
        );
    }

    // ─── Embedded playground assets ─────────────────────────────────────

    #[test]
    fn playground_assets_contains_index_html() {
        assert!(
            PlaygroundAssets::get("index.html").is_some(),
            "index.html must be embedded"
        );
    }

    #[test]
    fn playground_assets_index_html_has_content() {
        let asset = PlaygroundAssets::get("index.html").unwrap();
        let content = std::str::from_utf8(&asset.data).unwrap();
        assert!(content.contains("<!DOCTYPE html>") || content.contains("<html"),
            "index.html should contain HTML");
    }

    #[test]
    fn serve_playground_file_returns_html_for_index() {
        let response = serve_playground_file("index.html");
        assert_eq!(response.status(), axum::http::StatusCode::OK);
    }

    #[test]
    fn serve_playground_file_spa_fallback_for_unknown_path() {
        let response = serve_playground_file("nonexistent/route");
        assert_eq!(response.status(), axum::http::StatusCode::OK);
    }

    #[test]
    fn serve_playground_file_returns_css_with_correct_mime() {
        let css_file = PlaygroundAssets::iter()
            .find(|f| f.ends_with(".css"))
            .expect("should have a CSS file");
        let response = serve_playground_file(&css_file);
        assert_eq!(response.status(), axum::http::StatusCode::OK);
        let content_type = response.headers().get("content-type").expect("should have content-type");
        assert!(content_type.to_str().unwrap().starts_with("text/css"), "expected text/css, got {content_type:?}");
    }

    #[test]
    fn serve_playground_file_returns_js_with_correct_mime() {
        let js_file = PlaygroundAssets::iter()
            .find(|f| f.ends_with(".js"))
            .expect("should have a JS file");
        let response = serve_playground_file(&js_file);
        assert_eq!(response.status(), axum::http::StatusCode::OK);
        let content_type = response.headers().get("content-type").expect("should have content-type");
        assert!(content_type.to_str().unwrap().contains("javascript"), "expected javascript mime, got {content_type:?}");
    }
}
