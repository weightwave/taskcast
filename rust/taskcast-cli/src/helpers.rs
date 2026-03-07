use taskcast_core::config::AuthMode;

pub const DEFAULT_PORT: u16 = 3721;

/// Resolve the port: CLI flag (if changed from default) > config file > default.
pub fn resolve_port(cli_port: u16, config_port: Option<u16>) -> u16 {
    if cli_port != DEFAULT_PORT {
        cli_port
    } else {
        config_port.unwrap_or(cli_port)
    }
}

/// Resolve storage mode: CLI flag (if not "memory") > env var > auto-detect from redis_url.
pub fn resolve_storage_mode<'a>(
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
pub fn parse_jwt_algorithm(alg: Option<&str>) -> jsonwebtoken::Algorithm {
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
pub fn auth_mode_to_string(mode: &AuthMode) -> String {
    match mode {
        AuthMode::None => "none".to_string(),
        AuthMode::Jwt => "jwt".to_string(),
        AuthMode::Custom => "custom".to_string(),
    }
}

/// Resolve the Postgres URL: explicit URL > env var > config file.
pub fn resolve_postgres_url(
    cli_url: Option<String>,
    env_url: Option<String>,
    config_url: Option<String>,
) -> Option<String> {
    cli_url.or(env_url).or(config_url)
}

/// Format a Postgres URL for human-readable display (host:port/dbname).
pub fn format_display_url(postgres_url: &str) -> String {
    match url::Url::parse(postgres_url) {
        Ok(parsed) => {
            let host = parsed.host_str().unwrap_or("unknown");
            let port = parsed.port().unwrap_or(5432);
            let path = parsed.path().trim_start_matches('/');
            let db = if path.is_empty() { "postgres" } else { path };
            format!("{host}:{port}/{db}")
        }
        Err(_) => postgres_url.to_string(),
    }
}
