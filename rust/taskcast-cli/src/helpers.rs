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
/// Replaces wildcard bind addresses (0.0.0.0, ::) with "localhost" for readability.
pub fn format_display_url(postgres_url: &str) -> String {
    match url::Url::parse(postgres_url) {
        Ok(parsed) => {
            let raw_host = parsed.host_str().unwrap_or("unknown");
            // url::Url::host_str() returns IPv6 addresses in brackets, e.g. "[::] "
            let normalized = raw_host.trim_start_matches('[').trim_end_matches(']');
            let host = match normalized {
                "0.0.0.0" | "::" => "localhost",
                _ => raw_host,
            };
            let port = parsed.port().unwrap_or(5432);
            let path = parsed.path().trim_start_matches('/');
            let db = if path.is_empty() { "postgres" } else { path };
            format!("{host}:{port}/{db}")
        }
        Err(_) => postgres_url.to_string(),
    }
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use jsonwebtoken::Algorithm;
    use taskcast_core::config::AuthMode;

    // ─── resolve_port ────────────────────────────────────────────────────────

    #[test]
    fn port_default_value_is_3721() {
        assert_eq!(DEFAULT_PORT, 3721);
    }

    #[test]
    fn port_returns_default_when_no_overrides() {
        assert_eq!(resolve_port(DEFAULT_PORT, None), DEFAULT_PORT);
    }

    #[test]
    fn port_cli_overrides_config() {
        assert_eq!(resolve_port(8080, Some(9090)), 8080);
    }

    #[test]
    fn port_cli_overrides_none_config() {
        assert_eq!(resolve_port(8080, None), 8080);
    }

    #[test]
    fn port_config_used_when_cli_is_default() {
        assert_eq!(resolve_port(DEFAULT_PORT, Some(9090)), 9090);
    }

    #[test]
    fn port_falls_back_to_default_when_both_absent() {
        assert_eq!(resolve_port(DEFAULT_PORT, None), DEFAULT_PORT);
    }

    // ─── resolve_storage_mode ────────────────────────────────────────────────

    #[test]
    fn storage_defaults_to_memory() {
        assert_eq!(resolve_storage_mode("memory", None, false), "memory");
    }

    #[test]
    fn storage_cli_flag_overrides_all() {
        assert_eq!(resolve_storage_mode("sqlite", None, true), "sqlite");
        assert_eq!(resolve_storage_mode("redis", Some("sqlite"), false), "redis");
    }

    #[test]
    fn storage_env_sqlite_overrides_auto_detect() {
        assert_eq!(resolve_storage_mode("memory", Some("sqlite"), true), "sqlite");
    }

    #[test]
    fn storage_env_sqlite_when_no_redis() {
        assert_eq!(resolve_storage_mode("memory", Some("sqlite"), false), "sqlite");
    }

    #[test]
    fn storage_auto_detects_redis_from_url() {
        assert_eq!(resolve_storage_mode("memory", None, true), "redis");
    }

    #[test]
    fn storage_env_non_sqlite_falls_through_to_redis() {
        // Only "sqlite" env var triggers sqlite mode; other values fall through
        assert_eq!(resolve_storage_mode("memory", Some("redis"), true), "redis");
    }

    #[test]
    fn storage_env_non_sqlite_falls_through_to_memory() {
        assert_eq!(resolve_storage_mode("memory", Some("other"), false), "memory");
    }

    // ─── parse_jwt_algorithm ─────────────────────────────────────────────────

    #[test]
    fn algorithm_hs256_is_default_for_none() {
        assert_eq!(parse_jwt_algorithm(None), Algorithm::HS256);
    }

    #[test]
    fn algorithm_hs256_is_default_for_unknown() {
        assert_eq!(parse_jwt_algorithm(Some("UNKNOWN")), Algorithm::HS256);
    }

    #[test]
    fn algorithm_hs256_not_explicitly_mapped() {
        // HS256 is the catch-all default, not an explicit match arm
        assert_eq!(parse_jwt_algorithm(Some("HS256")), Algorithm::HS256);
    }

    #[test]
    fn algorithm_hs384_falls_to_default() {
        // HS384 is not explicitly mapped, falls through to HS256 default
        assert_eq!(parse_jwt_algorithm(Some("HS384")), Algorithm::HS256);
    }

    #[test]
    fn algorithm_hs512_falls_to_default() {
        // HS512 is not explicitly mapped, falls through to HS256 default
        assert_eq!(parse_jwt_algorithm(Some("HS512")), Algorithm::HS256);
    }

    #[test]
    fn algorithm_rs256() {
        assert_eq!(parse_jwt_algorithm(Some("RS256")), Algorithm::RS256);
    }

    #[test]
    fn algorithm_rs384() {
        assert_eq!(parse_jwt_algorithm(Some("RS384")), Algorithm::RS384);
    }

    #[test]
    fn algorithm_rs512() {
        assert_eq!(parse_jwt_algorithm(Some("RS512")), Algorithm::RS512);
    }

    #[test]
    fn algorithm_es256() {
        assert_eq!(parse_jwt_algorithm(Some("ES256")), Algorithm::ES256);
    }

    #[test]
    fn algorithm_es384() {
        assert_eq!(parse_jwt_algorithm(Some("ES384")), Algorithm::ES384);
    }

    #[test]
    fn algorithm_ps256() {
        assert_eq!(parse_jwt_algorithm(Some("PS256")), Algorithm::PS256);
    }

    #[test]
    fn algorithm_ps384() {
        assert_eq!(parse_jwt_algorithm(Some("PS384")), Algorithm::PS384);
    }

    #[test]
    fn algorithm_ps512() {
        assert_eq!(parse_jwt_algorithm(Some("PS512")), Algorithm::PS512);
    }

    #[test]
    fn algorithm_case_sensitive_lowercase_rejected() {
        assert_eq!(parse_jwt_algorithm(Some("rs256")), Algorithm::HS256);
        assert_eq!(parse_jwt_algorithm(Some("es256")), Algorithm::HS256);
    }

    #[test]
    fn algorithm_case_sensitive_mixed_case_rejected() {
        assert_eq!(parse_jwt_algorithm(Some("Rs256")), Algorithm::HS256);
    }

    #[test]
    fn algorithm_empty_string_falls_to_default() {
        assert_eq!(parse_jwt_algorithm(Some("")), Algorithm::HS256);
    }

    // ─── auth_mode_to_string ─────────────────────────────────────────────────

    #[test]
    fn auth_mode_none() {
        assert_eq!(auth_mode_to_string(&AuthMode::None), "none");
    }

    #[test]
    fn auth_mode_jwt() {
        assert_eq!(auth_mode_to_string(&AuthMode::Jwt), "jwt");
    }

    #[test]
    fn auth_mode_custom() {
        assert_eq!(auth_mode_to_string(&AuthMode::Custom), "custom");
    }

    // ─── resolve_postgres_url ────────────────────────────────────────────────

    #[test]
    fn postgres_url_cli_takes_priority() {
        assert_eq!(
            resolve_postgres_url(
                Some("postgres://cli".to_string()),
                Some("postgres://env".to_string()),
                Some("postgres://config".to_string()),
            ),
            Some("postgres://cli".to_string())
        );
    }

    #[test]
    fn postgres_url_env_when_no_cli() {
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
    fn postgres_url_config_when_no_cli_or_env() {
        assert_eq!(
            resolve_postgres_url(None, None, Some("postgres://config".to_string())),
            Some("postgres://config".to_string())
        );
    }

    #[test]
    fn postgres_url_none_when_all_missing() {
        assert_eq!(resolve_postgres_url(None, None, None), None);
    }

    // ─── format_display_url ──────────────────────────────────────────────────

    #[test]
    fn display_url_standard_format() {
        assert_eq!(
            format_display_url("postgres://user:pass@myhost:5433/mydb"),
            "myhost:5433/mydb"
        );
    }

    #[test]
    fn display_url_default_port() {
        assert_eq!(
            format_display_url("postgres://user@myhost/mydb"),
            "myhost:5432/mydb"
        );
    }

    #[test]
    fn display_url_default_db_name() {
        assert_eq!(
            format_display_url("postgres://user@myhost:5432"),
            "myhost:5432/postgres"
        );
    }

    #[test]
    fn display_url_empty_path_defaults_db() {
        assert_eq!(
            format_display_url("postgres://user@myhost:5432/"),
            "myhost:5432/postgres"
        );
    }

    #[test]
    fn display_url_invalid_returns_raw() {
        assert_eq!(format_display_url("not-a-url"), "not-a-url");
    }

    #[test]
    fn display_url_0000_becomes_localhost() {
        assert_eq!(
            format_display_url("postgres://user@0.0.0.0:5432/mydb"),
            "localhost:5432/mydb"
        );
    }

    #[test]
    fn display_url_ipv6_wildcard_becomes_localhost() {
        // url::Url parses [::] as host "::"
        assert_eq!(
            format_display_url("postgres://user@[::]:5432/mydb"),
            "localhost:5432/mydb"
        );
    }

    #[test]
    fn display_url_normal_host_unchanged() {
        assert_eq!(
            format_display_url("postgres://user@db.example.com:5432/prod"),
            "db.example.com:5432/prod"
        );
    }
}
