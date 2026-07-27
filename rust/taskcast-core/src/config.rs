use crate::PermissionScope;
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;

// ─── Config Types ────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct TaskcastConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub log_level: Option<LogLevel>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub admin_token: Option<String>,
    /// Enable the admin API endpoint (POST /admin/token). Defaults to false.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub admin_api: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auth: Option<AuthConfig>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trusted_services: Option<Vec<TrustedServiceConfig>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub adapters: Option<AdaptersConfig>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sentry: Option<SentryConfig>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub webhook: Option<WebhookGlobalConfig>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cleanup: Option<CleanupGlobalConfig>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub workers: Option<WorkersConfig>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub storage_lifecycle: Option<StorageLifecycleConfig>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct StorageLifecycleConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hot_retention_enabled: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hot_retention_terminal_seconds: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hot_retention_idle_seconds: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rehydrate_replay_events: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub storage_lock_ttl_seconds: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ttl_sweep_interval_seconds: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ttl_sweep_batch_size: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedStorageLifecycleConfig {
    pub hot_retention_enabled: bool,
    pub hot_retention_terminal_seconds: u64,
    pub hot_retention_idle_seconds: u64,
    pub rehydrate_replay_events: u64,
    pub storage_lock_ttl_seconds: u64,
    pub ttl_sweep_interval_seconds: u64,
    pub ttl_sweep_batch_size: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct WorkersConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub defaults: Option<WorkerDefaultsConfig>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct WorkerDefaultsConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub assign_mode: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub heartbeat_interval_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub heartbeat_timeout_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub offer_timeout_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub disconnect_policy: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub disconnect_grace_ms: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum LogLevel {
    Debug,
    Info,
    Warn,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthConfig {
    pub mode: AuthMode,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub jwt: Option<JwtConfig>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AuthMode {
    None,
    Jwt,
    Custom,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JwtConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub algorithm: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub secret: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub public_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub public_key_file: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub issuer: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub audience: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TrustedServiceConfig {
    pub name: String,
    pub key: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub task_ids: Option<TrustedServiceTaskIds>,
    #[serde(default)]
    pub scope: Vec<PermissionScope>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum TrustedServiceTaskIds {
    Wildcard(String),
    List(Vec<String>),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AdaptersConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub broadcast: Option<AdapterEntry>,
    #[serde(alias = "shortTerm", skip_serializing_if = "Option::is_none")]
    pub short_term_store: Option<AdapterEntry>,
    #[serde(alias = "longTerm", skip_serializing_if = "Option::is_none")]
    pub long_term_store: Option<AdapterEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AdapterEntry {
    pub provider: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SentryConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dsn: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub capture_task_failures: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub capture_task_timeouts: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub capture_unhandled_errors: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub capture_dropped_events: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub capture_storage_errors: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub capture_broadcast_errors: Option<bool>,
    #[serde(
        rename = "traceSSEConnections",
        skip_serializing_if = "Option::is_none"
    )]
    pub trace_sse_connections: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trace_event_publish: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WebhookGlobalConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_retry: Option<WebhookRetryConfig>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WebhookRetryConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retries: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub backoff: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub initial_delay_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_delay_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CleanupGlobalConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rules: Option<Vec<serde_json::Value>>,
}

// ─── Config Format ───────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigFormat {
    Json,
    Yaml,
}

// ─── Error ───────────────────────────────────────────────────────────────────

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("JSON parse error: {0}")]
    JsonParse(#[from] serde_json::Error),

    #[error("YAML parse error: {0}")]
    YamlParse(#[from] serde_yaml::Error),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("{0}")]
    InvalidValue(String),
}

pub fn resolve_storage_lifecycle_config(
    config: &TaskcastConfig,
    env: &HashMap<String, String>,
) -> Result<ResolvedStorageLifecycleConfig, ConfigError> {
    let file = config.storage_lifecycle.as_ref();
    let parse_positive = |key: &str, file_value: Option<u64>, default: u64, maximum: u64| {
        let value = match env.get(key) {
            Some(value) => value.parse::<u64>().map_err(|_| {
                ConfigError::InvalidValue(format!("{key} must be a positive integer"))
            })?,
            None => file_value.unwrap_or(default),
        };
        if value == 0 {
            return Err(ConfigError::InvalidValue(format!(
                "{key} must be a positive integer"
            )));
        }
        if value > maximum {
            return Err(ConfigError::InvalidValue(format!(
                "{key} must be a positive integer no greater than {maximum}"
            )));
        }
        Ok(value)
    };
    let maximum_seconds = u64::MAX / 1_000;
    let hot_retention_enabled = match env.get("TASKCAST_HOT_RETENTION_ENABLED") {
        Some(value) if value == "true" => true,
        Some(value) if value == "false" => false,
        Some(_) => {
            return Err(ConfigError::InvalidValue(
                "TASKCAST_HOT_RETENTION_ENABLED must be true or false".to_string(),
            ))
        }
        None => file
            .and_then(|value| value.hot_retention_enabled)
            .unwrap_or(false),
    };

    Ok(ResolvedStorageLifecycleConfig {
        hot_retention_enabled,
        hot_retention_terminal_seconds: parse_positive(
            "TASKCAST_HOT_RETENTION_TERMINAL_SECONDS",
            file.and_then(|value| value.hot_retention_terminal_seconds),
            86_400,
            maximum_seconds,
        )?,
        hot_retention_idle_seconds: parse_positive(
            "TASKCAST_HOT_RETENTION_IDLE_SECONDS",
            file.and_then(|value| value.hot_retention_idle_seconds),
            3_600,
            maximum_seconds,
        )?,
        rehydrate_replay_events: parse_positive(
            "TASKCAST_REHYDRATE_REPLAY_EVENTS",
            file.and_then(|value| value.rehydrate_replay_events),
            1_000,
            u64::MAX,
        )?,
        storage_lock_ttl_seconds: parse_positive(
            "TASKCAST_STORAGE_LOCK_TTL_SECONDS",
            file.and_then(|value| value.storage_lock_ttl_seconds),
            30,
            maximum_seconds,
        )?,
        ttl_sweep_interval_seconds: parse_positive(
            "TASKCAST_TTL_SWEEP_INTERVAL_SECONDS",
            file.and_then(|value| value.ttl_sweep_interval_seconds),
            5,
            maximum_seconds,
        )?,
        ttl_sweep_batch_size: parse_positive(
            "TASKCAST_TTL_SWEEP_BATCH_SIZE",
            file.and_then(|value| value.ttl_sweep_batch_size),
            100,
            u64::MAX,
        )?,
    })
}

// ─── Environment Variable Interpolation ──────────────────────────────────────

/// Replace `${VAR_NAME}` patterns in a string with environment variable values.
/// If the environment variable is not set, the original `${VAR_NAME}` is kept.
pub fn interpolate_env_vars(value: &str) -> String {
    let re = Regex::new(r"\$\{([^}]+)\}").expect("invalid regex");
    re.replace_all(value, |caps: &regex::Captures| {
        let var_name = &caps[1];
        std::env::var(var_name).unwrap_or_else(|_| caps[0].to_string())
    })
    .into_owned()
}

/// Recursively interpolate environment variables in a serde_json::Value tree.
/// Strings get `${VAR}` replacement; arrays and objects are traversed recursively;
/// other types (numbers, booleans, null) pass through unchanged.
fn interpolate_value(value: serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::String(s) => serde_json::Value::String(interpolate_env_vars(&s)),
        serde_json::Value::Array(arr) => {
            serde_json::Value::Array(arr.into_iter().map(interpolate_value).collect())
        }
        serde_json::Value::Object(map) => serde_json::Value::Object(
            map.into_iter()
                .map(|(k, v)| (k, interpolate_value(v)))
                .collect(),
        ),
        other => other,
    }
}

// ─── Parsing ─────────────────────────────────────────────────────────────────

/// Parse a config string in the given format, with environment variable interpolation.
///
/// - **YAML**: env vars are interpolated in the raw string *before* YAML parsing.
/// - **JSON**: the string is parsed first, then env vars are interpolated in values.
///
/// After interpolation, if `port` ended up as a string (from env var substitution),
/// it is coerced to a number. If coercion fails, the port field is cleared.
pub fn parse_config(content: &str, format: ConfigFormat) -> Result<TaskcastConfig, ConfigError> {
    let raw: serde_json::Value = match format {
        ConfigFormat::Json => serde_json::from_str(content)?,
        ConfigFormat::Yaml => {
            let interpolated = interpolate_env_vars(content);
            let parsed: serde_json::Value = serde_yaml::from_str(&interpolated)?;
            // Empty YAML content parses to null; treat as empty config
            if parsed.is_null() {
                return Ok(TaskcastConfig::default());
            }
            parsed
        }
    };

    let interpolated = interpolate_value(raw);

    // Handle port coercion: if port is a string, try to parse it as a number
    let final_value = coerce_port(interpolated);

    let config: TaskcastConfig =
        serde_json::from_value(final_value).map_err(ConfigError::JsonParse)?;
    Ok(config)
}

/// If the `port` field is a JSON string, attempt to parse it as an integer.
/// If parsing succeeds, replace it with the numeric value.
/// If parsing fails, remove the port field entirely.
fn coerce_port(mut value: serde_json::Value) -> serde_json::Value {
    if let serde_json::Value::Object(ref mut map) = value {
        if let Some(serde_json::Value::String(s)) = map.get("port") {
            match s.parse::<u64>() {
                Ok(n) => {
                    map.insert("port".to_string(), serde_json::Value::Number(n.into()));
                }
                Err(_) => {
                    map.remove("port");
                }
            }
        }
    }
    value
}

// ─── Admin Token Resolution ──────────────────────────────────────────────────

/// Resolves the admin token based on config.
/// - If `admin_api` is false/unset, returns None (admin API disabled, no token needed).
/// - If `admin_api` is true and `admin_token` is set, returns it.
/// - If `admin_api` is true and `admin_token` is not set, auto-generates a ULID and logs it.
///
/// Mutates the config in place.
pub fn resolve_admin_token(config: &mut TaskcastConfig) -> Option<String> {
    if config.admin_api != Some(true) {
        return None;
    }
    if let Some(ref token) = config.admin_token {
        return Some(token.clone());
    }
    let token = ulid::Ulid::new().to_string();
    println!("[taskcast] Admin token (auto-generated): {}", token);
    config.admin_token = Some(token.clone());
    Some(token)
}

// ─── File Loading ────────────────────────────────────────────────────────────

/// Default config file candidate names, checked in order.
const DEFAULT_CANDIDATES: &[&str] = &[
    "taskcast.config.ts",
    "taskcast.config.js",
    "taskcast.config.mjs",
    "taskcast.config.yaml",
    "taskcast.config.yml",
    "taskcast.config.json",
];

/// Load a config file from disk. If `config_path` is provided, only that path
/// is tried. Otherwise, a list of default candidates is checked in order
/// relative to the current working directory.
///
/// JS/TS config files (.ts, .js, .mjs) are skipped in the Rust version.
/// If no matching file is found, returns a default (empty) config.
pub fn load_config_file(config_path: Option<&str>) -> Result<TaskcastConfig, ConfigError> {
    let base_dir = std::env::current_dir()?;
    load_config_file_from_dir(config_path, &base_dir)
}

/// Internal: load config searching from a specific base directory.
fn load_config_file_from_dir(
    config_path: Option<&str>,
    base_dir: &Path,
) -> Result<TaskcastConfig, ConfigError> {
    let candidates: Vec<&str> = match config_path {
        Some(path) => vec![path],
        None => DEFAULT_CANDIDATES.to_vec(),
    };

    for candidate in candidates {
        let full_path = if Path::new(candidate).is_absolute() {
            std::path::PathBuf::from(candidate)
        } else {
            base_dir.join(candidate)
        };

        if !full_path.exists() {
            continue;
        }

        let ext = full_path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_lowercase();

        // Skip JS/TS config files in Rust version
        if ext == "ts" || ext == "js" || ext == "mjs" {
            continue;
        }

        let content = std::fs::read_to_string(&full_path)?;
        let format = if ext == "json" {
            ConfigFormat::Json
        } else {
            ConfigFormat::Yaml
        };

        return parse_config(&content, format);
    }

    Ok(TaskcastConfig::default())
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::env;
    use std::io::Write;

    // ─── interpolate_env_vars ────────────────────────────────────────────────

    #[test]
    fn interpolate_basic_substitution() {
        env::set_var("TASKCAST_TEST_HOST", "localhost");
        let result = interpolate_env_vars("host: ${TASKCAST_TEST_HOST}");
        assert_eq!(result, "host: localhost");
        env::remove_var("TASKCAST_TEST_HOST");
    }

    #[test]
    fn interpolate_missing_var_stays_as_is() {
        // Use a variable name that is extremely unlikely to exist
        let result = interpolate_env_vars("val: ${TASKCAST_NONEXISTENT_VAR_XYZ_12345}");
        assert_eq!(result, "val: ${TASKCAST_NONEXISTENT_VAR_XYZ_12345}");
    }

    #[test]
    fn interpolate_multiple_vars() {
        env::set_var("TASKCAST_TEST_A", "alpha");
        env::set_var("TASKCAST_TEST_B", "beta");
        let result = interpolate_env_vars("${TASKCAST_TEST_A} and ${TASKCAST_TEST_B}");
        assert_eq!(result, "alpha and beta");
        env::remove_var("TASKCAST_TEST_A");
        env::remove_var("TASKCAST_TEST_B");
    }

    #[test]
    fn interpolate_no_vars_unchanged() {
        let result = interpolate_env_vars("no variables here");
        assert_eq!(result, "no variables here");
    }

    #[test]
    fn interpolate_mixed_present_and_missing() {
        env::set_var("TASKCAST_TEST_PRESENT", "found");
        let result =
            interpolate_env_vars("${TASKCAST_TEST_PRESENT} and ${TASKCAST_NONEXISTENT_MISSING_99}");
        assert_eq!(result, "found and ${TASKCAST_NONEXISTENT_MISSING_99}");
        env::remove_var("TASKCAST_TEST_PRESENT");
    }

    // ─── parse_config JSON ──────────────────────────────────────────────────

    #[test]
    fn parse_json_basic_config() {
        let json = r#"{"port": 3000, "logLevel": "info"}"#;
        let config = parse_config(json, ConfigFormat::Json).unwrap();
        assert_eq!(config.port, Some(3000));
        assert_eq!(config.log_level, Some(LogLevel::Info));
    }

    #[test]
    fn parse_json_with_env_vars() {
        env::set_var("TASKCAST_TEST_SECRET", "my-secret");
        let json = r#"{
            "auth": {
                "mode": "jwt",
                "jwt": {
                    "secret": "${TASKCAST_TEST_SECRET}"
                }
            }
        }"#;
        let config = parse_config(json, ConfigFormat::Json).unwrap();
        assert_eq!(config.auth.as_ref().unwrap().mode, AuthMode::Jwt);
        assert_eq!(
            config.auth.as_ref().unwrap().jwt.as_ref().unwrap().secret,
            Some("my-secret".to_string())
        );
        env::remove_var("TASKCAST_TEST_SECRET");
    }

    #[test]
    fn parse_json_with_adapters() {
        let json = r#"{
            "adapters": {
                "broadcast": { "provider": "redis", "url": "redis://localhost:6379" },
                "shortTerm": { "provider": "memory" },
                "longTerm": { "provider": "postgres", "url": "postgres://localhost/db" }
            }
        }"#;
        let config = parse_config(json, ConfigFormat::Json).unwrap();
        let adapters = config.adapters.unwrap();
        assert_eq!(adapters.broadcast.as_ref().unwrap().provider, "redis");
        assert_eq!(
            adapters.broadcast.as_ref().unwrap().url,
            Some("redis://localhost:6379".to_string())
        );
        assert_eq!(
            adapters.short_term_store.as_ref().unwrap().provider,
            "memory"
        );
        assert_eq!(adapters.short_term_store.as_ref().unwrap().url, None);
        assert_eq!(
            adapters.long_term_store.as_ref().unwrap().provider,
            "postgres"
        );
    }

    #[test]
    fn parse_json_empty_object() {
        let config = parse_config("{}", ConfigFormat::Json).unwrap();
        assert_eq!(config, TaskcastConfig::default());
    }

    #[test]
    fn parse_json_with_sentry() {
        let json = r#"{
            "sentry": {
                "dsn": "https://examplePublicKey@o0.ingest.sentry.io/0",
                "captureTaskFailures": true,
                "traceSSEConnections": false
            }
        }"#;
        let config = parse_config(json, ConfigFormat::Json).unwrap();
        let sentry = config.sentry.unwrap();
        assert_eq!(
            sentry.dsn,
            Some("https://examplePublicKey@o0.ingest.sentry.io/0".to_string())
        );
        assert_eq!(sentry.capture_task_failures, Some(true));
        assert_eq!(sentry.trace_sse_connections, Some(false));
    }

    #[test]
    fn parse_json_with_webhook_retry() {
        let json = r#"{
            "webhook": {
                "defaultRetry": {
                    "retries": 3,
                    "backoff": "exponential",
                    "initialDelayMs": 1000,
                    "maxDelayMs": 30000,
                    "timeoutMs": 5000
                }
            }
        }"#;
        let config = parse_config(json, ConfigFormat::Json).unwrap();
        let retry = config.webhook.unwrap().default_retry.unwrap();
        assert_eq!(retry.retries, Some(3));
        assert_eq!(retry.backoff, Some("exponential".to_string()));
        assert_eq!(retry.initial_delay_ms, Some(1000));
        assert_eq!(retry.max_delay_ms, Some(30000));
        assert_eq!(retry.timeout_ms, Some(5000));
    }

    // ─── parse_config YAML ──────────────────────────────────────────────────

    #[test]
    fn parse_yaml_basic_config() {
        let yaml = "port: 8080\nlogLevel: debug\n";
        let config = parse_config(yaml, ConfigFormat::Yaml).unwrap();
        assert_eq!(config.port, Some(8080));
        assert_eq!(config.log_level, Some(LogLevel::Debug));
    }

    #[test]
    fn parse_yaml_with_env_vars() {
        env::set_var("TASKCAST_TEST_REDIS_URL", "redis://prod:6379");
        let yaml = r#"
adapters:
  broadcast:
    provider: redis
    url: ${TASKCAST_TEST_REDIS_URL}
"#;
        let config = parse_config(yaml, ConfigFormat::Yaml).unwrap();
        let adapters = config.adapters.unwrap();
        assert_eq!(
            adapters.broadcast.as_ref().unwrap().url,
            Some("redis://prod:6379".to_string())
        );
        env::remove_var("TASKCAST_TEST_REDIS_URL");
    }

    #[test]
    fn parse_yaml_with_auth() {
        let yaml = r#"
auth:
  mode: jwt
  jwt:
    algorithm: RS256
    publicKeyFile: /etc/keys/public.pem
    issuer: my-app
    audience: api
"#;
        let config = parse_config(yaml, ConfigFormat::Yaml).unwrap();
        let auth = config.auth.unwrap();
        assert_eq!(auth.mode, AuthMode::Jwt);
        let jwt = auth.jwt.unwrap();
        assert_eq!(jwt.algorithm, Some("RS256".to_string()));
        assert_eq!(
            jwt.public_key_file,
            Some("/etc/keys/public.pem".to_string())
        );
        assert_eq!(jwt.issuer, Some("my-app".to_string()));
        assert_eq!(jwt.audience, Some("api".to_string()));
    }

    #[test]
    fn parse_yaml_with_trusted_services() {
        env::set_var("TASKCAST_TEST_SERVICE_KEY", "backend-service-secret");
        let yaml = r#"
trustedServices:
  - name: backend
    key: ${TASKCAST_TEST_SERVICE_KEY}
    taskIds: "*"
    scope: ["*"]
  - name: worker
    key: worker-service-secret
    taskIds: ["task-1", "task-2"]
    scope: ["event:publish"]
"#;
        let config = parse_config(yaml, ConfigFormat::Yaml).unwrap();
        let trusted_services = config.trusted_services.unwrap();
        assert_eq!(trusted_services.len(), 2);
        assert_eq!(trusted_services[0].name, "backend");
        assert_eq!(trusted_services[0].key, "backend-service-secret");
        assert_eq!(
            trusted_services[0].task_ids,
            Some(TrustedServiceTaskIds::Wildcard("*".to_string()))
        );
        assert_eq!(trusted_services[0].scope, vec![PermissionScope::All]);
        assert_eq!(
            trusted_services[1].task_ids,
            Some(TrustedServiceTaskIds::List(vec![
                "task-1".to_string(),
                "task-2".to_string(),
            ]))
        );
        assert_eq!(
            trusted_services[1].scope,
            vec![PermissionScope::EventPublish]
        );
        env::remove_var("TASKCAST_TEST_SERVICE_KEY");
    }

    #[test]
    fn parse_yaml_empty() {
        let config = parse_config("", ConfigFormat::Yaml).unwrap();
        assert_eq!(config, TaskcastConfig::default());
    }

    // ─── Port coercion ──────────────────────────────────────────────────────

    #[test]
    fn port_string_coerced_to_number() {
        env::set_var("TASKCAST_TEST_PORT", "4000");
        let json = r#"{"port": "${TASKCAST_TEST_PORT}"}"#;
        let config = parse_config(json, ConfigFormat::Json).unwrap();
        assert_eq!(config.port, Some(4000));
        env::remove_var("TASKCAST_TEST_PORT");
    }

    #[test]
    fn port_invalid_string_removed() {
        let json_val = serde_json::json!({"port": "not-a-number"});
        let coerced = coerce_port(json_val);
        assert!(coerced.get("port").is_none());
    }

    #[test]
    fn port_numeric_stays_as_is() {
        let json = r#"{"port": 5000}"#;
        let config = parse_config(json, ConfigFormat::Json).unwrap();
        assert_eq!(config.port, Some(5000));
    }

    #[test]
    fn port_string_in_yaml_coerced() {
        env::set_var("TASKCAST_TEST_YAML_PORT", "9090");
        let yaml = "port: ${TASKCAST_TEST_YAML_PORT}\n";
        let config = parse_config(yaml, ConfigFormat::Yaml).unwrap();
        assert_eq!(config.port, Some(9090));
        env::remove_var("TASKCAST_TEST_YAML_PORT");
    }

    // ─── load_config_file ───────────────────────────────────────────────────

    #[test]
    fn load_config_file_missing_returns_empty() {
        let config = load_config_file(Some("/tmp/taskcast_nonexistent_config_file.yaml")).unwrap();
        assert_eq!(config, TaskcastConfig::default());
    }

    #[test]
    fn load_config_file_explicit_json_path() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("test-config.json");
        let mut f = std::fs::File::create(&file_path).unwrap();
        writeln!(f, r#"{{"port": 7777, "logLevel": "warn"}}"#).unwrap();

        let config = load_config_file(Some(file_path.to_str().unwrap())).unwrap();
        assert_eq!(config.port, Some(7777));
        assert_eq!(config.log_level, Some(LogLevel::Warn));
    }

    #[test]
    fn load_config_file_explicit_yaml_path() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("test-config.yaml");
        let mut f = std::fs::File::create(&file_path).unwrap();
        writeln!(f, "port: 6666\nlogLevel: error").unwrap();

        let config = load_config_file(Some(file_path.to_str().unwrap())).unwrap();
        assert_eq!(config.port, Some(6666));
        assert_eq!(config.log_level, Some(LogLevel::Error));
    }

    #[test]
    fn load_config_file_skips_ts_js_mjs() {
        let dir = tempfile::tempdir().unwrap();

        // Create a .ts file (should be skipped)
        let ts_path = dir.path().join("taskcast.config.ts");
        std::fs::write(&ts_path, "export default { port: 1111 }").unwrap();

        // Create a .yaml file (should be picked up after .ts is skipped)
        let yaml_path = dir.path().join("taskcast.config.yaml");
        std::fs::write(&yaml_path, "port: 2222").unwrap();

        let config = load_config_file_from_dir(None, dir.path()).unwrap();
        assert_eq!(config.port, Some(2222));
    }

    #[test]
    fn load_config_file_default_candidates_no_files() {
        let dir = tempfile::tempdir().unwrap();
        let config = load_config_file_from_dir(None, dir.path()).unwrap();
        assert_eq!(config, TaskcastConfig::default());
    }

    #[test]
    fn load_config_file_default_candidates_yml() {
        let dir = tempfile::tempdir().unwrap();
        let yml_path = dir.path().join("taskcast.config.yml");
        std::fs::write(&yml_path, "port: 3333\nlogLevel: info").unwrap();

        let config = load_config_file_from_dir(None, dir.path()).unwrap();
        assert_eq!(config.port, Some(3333));
        assert_eq!(config.log_level, Some(LogLevel::Info));
    }

    #[test]
    fn load_config_file_default_candidates_json() {
        let dir = tempfile::tempdir().unwrap();
        let json_path = dir.path().join("taskcast.config.json");
        std::fs::write(&json_path, r#"{"port": 4444}"#).unwrap();

        let config = load_config_file_from_dir(None, dir.path()).unwrap();
        assert_eq!(config.port, Some(4444));
    }

    // ─── Full config roundtrip ──────────────────────────────────────────────

    #[test]
    fn full_config_json_roundtrip() {
        let json = r#"{
            "port": 3000,
            "logLevel": "info",
            "auth": {
                "mode": "jwt",
                "jwt": {
                    "algorithm": "HS256",
                    "secret": "super-secret"
                }
            },
            "adapters": {
                "broadcast": { "provider": "redis", "url": "redis://localhost:6379" },
                "shortTerm": { "provider": "redis", "url": "redis://localhost:6379" },
                "longTerm": { "provider": "postgres", "url": "postgres://localhost/taskcast" }
            },
            "sentry": {
                "dsn": "https://key@sentry.io/123",
                "captureTaskFailures": true,
                "captureTaskTimeouts": true,
                "captureUnhandledErrors": true,
                "captureDroppedEvents": false,
                "captureStorageErrors": true,
                "captureBroadcastErrors": false,
                "traceSSEConnections": true,
                "traceEventPublish": false
            },
            "webhook": {
                "defaultRetry": {
                    "retries": 5,
                    "backoff": "exponential",
                    "initialDelayMs": 500,
                    "maxDelayMs": 60000,
                    "timeoutMs": 10000
                }
            },
            "cleanup": {
                "rules": [{"name": "test-rule"}]
            }
        }"#;
        let config = parse_config(json, ConfigFormat::Json).unwrap();
        assert_eq!(config.port, Some(3000));
        assert_eq!(config.log_level, Some(LogLevel::Info));
        assert!(config.auth.is_some());
        assert!(config.adapters.is_some());
        assert!(config.sentry.is_some());
        assert!(config.webhook.is_some());
        assert!(config.cleanup.is_some());

        // Re-serialize and re-parse to verify roundtrip
        let serialized = serde_json::to_string(&config).unwrap();
        let reparsed: TaskcastConfig = serde_json::from_str(&serialized).unwrap();
        assert_eq!(config, reparsed);
    }

    // ─── interpolate_value ──────────────────────────────────────────────────

    #[test]
    fn interpolate_value_numbers_and_bools_unchanged() {
        let val = serde_json::json!({
            "count": 42,
            "enabled": true,
            "nothing": null
        });
        let result = interpolate_value(val.clone());
        assert_eq!(result, val);
    }

    #[test]
    fn interpolate_value_nested_arrays() {
        env::set_var("TASKCAST_TEST_NESTED", "replaced");
        let val = serde_json::json!(["${TASKCAST_TEST_NESTED}", [1, "${TASKCAST_TEST_NESTED}"]]);
        let result = interpolate_value(val);
        assert_eq!(result[0], "replaced");
        assert_eq!(result[1][0], 1);
        assert_eq!(result[1][1], "replaced");
        env::remove_var("TASKCAST_TEST_NESTED");
    }

    // ─── YAML-specific env var interpolation behavior ───────────────────────

    #[test]
    fn yaml_env_var_interpolation_happens_before_parsing() {
        // In YAML mode, env vars are interpolated in the raw string before YAML parsing.
        // This means env vars can affect YAML structure (e.g., a var could contain a number
        // and YAML would parse it as a number).
        env::set_var("TASKCAST_TEST_YAML_NUM", "42");
        let yaml = "port: ${TASKCAST_TEST_YAML_NUM}\n";
        let config = parse_config(yaml, ConfigFormat::Yaml).unwrap();
        // YAML parser sees "port: 42" and parses it as a number
        assert_eq!(config.port, Some(42));
        env::remove_var("TASKCAST_TEST_YAML_NUM");
    }

    // ─── resolve_admin_token ─────────────────────────────────────────────────

    #[test]
    fn resolve_admin_token_returns_none_when_admin_api_not_enabled() {
        let mut config = TaskcastConfig::default();
        assert!(config.admin_api.is_none());

        let token = resolve_admin_token(&mut config);
        assert!(token.is_none());
        assert!(config.admin_token.is_none());
    }

    #[test]
    fn resolve_admin_token_returns_none_when_admin_api_false() {
        let mut config = TaskcastConfig {
            admin_api: Some(false),
            ..Default::default()
        };

        let token = resolve_admin_token(&mut config);
        assert!(token.is_none());
    }

    #[test]
    fn resolve_admin_token_returns_none_when_admin_api_false_even_with_token() {
        let mut config = TaskcastConfig {
            admin_api: Some(false),
            admin_token: Some("my-token".to_string()),
            ..Default::default()
        };

        let token = resolve_admin_token(&mut config);
        assert!(token.is_none());
    }

    #[test]
    fn resolve_admin_token_generates_when_admin_api_true_and_no_token() {
        let mut config = TaskcastConfig {
            admin_api: Some(true),
            ..Default::default()
        };

        let token = resolve_admin_token(&mut config);

        assert!(token.is_some());
        let t = token.unwrap();
        assert_eq!(t.len(), 26); // ULID is 26 chars
        assert_eq!(config.admin_token, Some(t));
    }

    #[test]
    fn resolve_admin_token_preserves_explicit_value_when_admin_api_true() {
        let mut config = TaskcastConfig {
            admin_api: Some(true),
            admin_token: Some("my-secret-token".to_string()),
            ..Default::default()
        };

        let token = resolve_admin_token(&mut config);

        assert_eq!(token, Some("my-secret-token".to_string()));
    }

    #[test]
    fn resolve_admin_token_generates_unique_tokens() {
        let mut config1 = TaskcastConfig {
            admin_api: Some(true),
            ..Default::default()
        };
        let mut config2 = TaskcastConfig {
            admin_api: Some(true),
            ..Default::default()
        };

        let token1 = resolve_admin_token(&mut config1).unwrap();
        let token2 = resolve_admin_token(&mut config2).unwrap();

        assert_ne!(token1, token2);
    }

    #[test]
    fn resolve_admin_token_idempotent_on_same_config() {
        let mut config = TaskcastConfig {
            admin_api: Some(true),
            ..Default::default()
        };

        let token1 = resolve_admin_token(&mut config).unwrap();
        let token2 = resolve_admin_token(&mut config).unwrap();

        assert_eq!(token1, token2);
    }

    #[test]
    fn admin_token_parsed_from_json_config() {
        let json = r#"{"adminToken": "from-config-file"}"#;
        let config = parse_config(json, ConfigFormat::Json).unwrap();
        assert_eq!(config.admin_token, Some("from-config-file".to_string()));
    }

    #[test]
    fn admin_token_parsed_from_yaml_config() {
        let yaml = "adminToken: from-yaml-config\n";
        let config = parse_config(yaml, ConfigFormat::Yaml).unwrap();
        assert_eq!(config.admin_token, Some("from-yaml-config".to_string()));
    }

    #[test]
    fn admin_api_parsed_from_json_config() {
        let json = r#"{"adminApi": true, "adminToken": "test"}"#;
        let config = parse_config(json, ConfigFormat::Json).unwrap();
        assert_eq!(config.admin_api, Some(true));
        assert_eq!(config.admin_token, Some("test".to_string()));
    }

    #[test]
    fn admin_api_parsed_from_yaml_config() {
        let yaml = "adminApi: true\nadminToken: from-yaml\n";
        let config = parse_config(yaml, ConfigFormat::Yaml).unwrap();
        assert_eq!(config.admin_api, Some(true));
        assert_eq!(config.admin_token, Some("from-yaml".to_string()));
    }

    #[test]
    fn admin_api_defaults_to_none_when_not_specified() {
        let json = r#"{"port": 3000}"#;
        let config = parse_config(json, ConfigFormat::Json).unwrap();
        assert!(config.admin_api.is_none());
    }

    #[test]
    fn storage_lifecycle_uses_conservative_defaults() {
        let resolved =
            resolve_storage_lifecycle_config(&TaskcastConfig::default(), &HashMap::new()).unwrap();
        assert_eq!(
            resolved,
            ResolvedStorageLifecycleConfig {
                hot_retention_enabled: false,
                hot_retention_terminal_seconds: 86_400,
                hot_retention_idle_seconds: 3_600,
                rehydrate_replay_events: 1_000,
                storage_lock_ttl_seconds: 30,
                ttl_sweep_interval_seconds: 5,
                ttl_sweep_batch_size: 100,
            }
        );
    }

    #[test]
    fn storage_lifecycle_env_overrides_config_file() {
        let config = parse_config(
            r#"
storageLifecycle:
  hotRetentionEnabled: true
  hotRetentionTerminalSeconds: 7200
  hotRetentionIdleSeconds: 1800
  rehydrateReplayEvents: 250
  storageLockTtlSeconds: 45
  ttlSweepIntervalSeconds: 10
  ttlSweepBatchSize: 25
"#,
            ConfigFormat::Yaml,
        )
        .unwrap();
        let env = HashMap::from([
            (
                "TASKCAST_HOT_RETENTION_ENABLED".to_string(),
                "false".to_string(),
            ),
            (
                "TASKCAST_REHYDRATE_REPLAY_EVENTS".to_string(),
                "500".to_string(),
            ),
            (
                "TASKCAST_TTL_SWEEP_BATCH_SIZE".to_string(),
                "50".to_string(),
            ),
        ]);

        assert_eq!(
            resolve_storage_lifecycle_config(&config, &env).unwrap(),
            ResolvedStorageLifecycleConfig {
                hot_retention_enabled: false,
                hot_retention_terminal_seconds: 7_200,
                hot_retention_idle_seconds: 1_800,
                rehydrate_replay_events: 500,
                storage_lock_ttl_seconds: 45,
                ttl_sweep_interval_seconds: 10,
                ttl_sweep_batch_size: 50,
            }
        );
    }

    #[test]
    fn storage_lifecycle_rejects_invalid_values() {
        for (key, value) in [
            ("TASKCAST_HOT_RETENTION_ENABLED", "yes"),
            ("TASKCAST_HOT_RETENTION_TERMINAL_SECONDS", "0"),
            ("TASKCAST_HOT_RETENTION_IDLE_SECONDS", "-1"),
            ("TASKCAST_REHYDRATE_REPLAY_EVENTS", "NaN"),
            ("TASKCAST_STORAGE_LOCK_TTL_SECONDS", "1.5"),
            ("TASKCAST_TTL_SWEEP_INTERVAL_SECONDS", "0"),
            ("TASKCAST_TTL_SWEEP_BATCH_SIZE", ""),
        ] {
            let env = HashMap::from([(key.to_string(), value.to_string())]);
            let error =
                resolve_storage_lifecycle_config(&TaskcastConfig::default(), &env).unwrap_err();
            assert!(error.to_string().contains(key));
        }

        let env = HashMap::from([(
            "TASKCAST_STORAGE_LOCK_TTL_SECONDS".to_string(),
            u64::MAX.to_string(),
        )]);
        let error = resolve_storage_lifecycle_config(&TaskcastConfig::default(), &env).unwrap_err();
        assert!(error.to_string().starts_with(
            "TASKCAST_STORAGE_LOCK_TTL_SECONDS must be a positive integer no greater than"
        ));
    }
}
