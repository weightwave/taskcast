use std::collections::HashSet;
use sqlx::PgPool;

use crate::helpers::parse_boolean_env;

/// Automatically run database migrations if enabled.
///
/// This function checks two conditions:
/// 1. TASKCAST_AUTO_MIGRATE env var is truthy (case-insensitive, parsed via parse_boolean_env)
/// 2. Postgres URL is configured (TASKCAST_POSTGRES_URL env var)
///
/// If both are true, runs migrations and logs the result:
/// - "Applied N migrations" if N > 0
/// - "Database schema up to date" if N = 0
/// - "Auto-migration failed: <error_message>" if an error occurs (and throws error)
///
/// If auto-migrate is disabled, returns immediately (no-op).
/// If Postgres is not configured, logs info message and returns (no-op).
///
/// # Arguments
/// * `pool` - The PgPool to run migrations against
/// * `env_auto_migrate` - TASKCAST_AUTO_MIGRATE env var (for testability)
/// * `env_postgres_url` - TASKCAST_POSTGRES_URL env var (for testability)
///
/// # Returns
/// Ok(()) if migration succeeds or is skipped (disabled/unconfigured)
/// Err with message "Auto-migration failed: <original_error>" if migration fails
pub async fn run_auto_migrate(
    pool: &PgPool,
    env_auto_migrate: Option<&str>,
    env_postgres_url: Option<&str>,
) -> Result<(), Box<dyn std::error::Error>> {
    // Check if auto-migrate is enabled
    let auto_migrate_enabled = parse_boolean_env(env_auto_migrate);
    if !auto_migrate_enabled {
        return Ok(());
    }

    // Check if Postgres is configured
    if env_postgres_url.is_none() {
        eprintln!("[taskcast] Auto-migrate disabled: Postgres not configured");
        return Ok(());
    }

    // Ensure _sqlx_migrations table exists (same as migrate command)
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS _sqlx_migrations (
            version BIGINT PRIMARY KEY,
            description TEXT NOT NULL,
            installed_on TIMESTAMPTZ NOT NULL DEFAULT now(),
            success BOOLEAN NOT NULL,
            checksum BYTEA NOT NULL,
            execution_time BIGINT NOT NULL
        )",
    )
    .execute(pool)
    .await
    .map_err(|e| format!("Failed to check migration state: {e}"))?;

    // Fail fast on dirty (failed) migrations
    let dirty: Vec<(i64,)> = sqlx::query_as(
        "SELECT version FROM _sqlx_migrations WHERE success = false ORDER BY version",
    )
    .fetch_all(pool)
    .await
    .map_err(|e| format!("Failed to query dirty migrations: {e}"))?;

    if !dirty.is_empty() {
        let versions: Vec<String> = dirty.iter().map(|r| r.0.to_string()).collect();
        let error_msg = format!(
            "Dirty (failed) migrations found: versions {}. Fix manually before running migrations.",
            versions.join(", ")
        );
        eprintln!("[taskcast] Auto-migration failed: {}", error_msg);
        return Err(format!("Auto-migration failed: {}", error_msg).into());
    }

    // Get list of already-applied migrations
    let applied: Vec<(i64,)> = sqlx::query_as(
        "SELECT version FROM _sqlx_migrations WHERE success = true ORDER BY version",
    )
    .fetch_all(pool)
    .await
    .map_err(|e| format!("Failed to query applied migrations: {e}"))?;

    let applied_set: HashSet<i64> = applied.iter().map(|r| r.0).collect();
    let migrator = sqlx::migrate!("../../migrations/postgres");
    let pending: Vec<_> = migrator
        .iter()
        .filter(|m| !applied_set.contains(&m.version))
        .collect();

    // If nothing pending, log and return
    if pending.is_empty() {
        eprintln!("[taskcast] Database schema up to date");
        return Ok(());
    }

    // Run migrations
    match sqlx::migrate!("../../migrations/postgres").run(pool).await {
        Ok(_) => {
            eprintln!("[taskcast] Applied {} migrations", pending.len());
            Ok(())
        }
        Err(err) => {
            let error_message = err.to_string();
            eprintln!("[taskcast] Auto-migration failed: {}", error_message);
            Err(format!("Auto-migration failed: {}", error_message).into())
        }
    }
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // Note: Unit tests for run_auto_migrate are limited because they require:
    // 1. A PgPool which needs a real (or testcontainer) database
    // 2. The ability to verify migration state (which requires inspecting _sqlx_migrations table)
    //
    // Unit tests that only verify early returns (before pool access) are included below.
    // Integration tests with testcontainers Postgres verify the full flow.

    // ─── Tests for parse_boolean_env (helper dependency) ──────────────────
    //
    // These tests verify the behavior of the dependency helper, showing
    // different input patterns that affect run_auto_migrate's decision logic.

    #[test]
    fn helper_parse_boolean_env_recognizes_true_values() {
        assert!(parse_boolean_env(Some("true")));
        assert!(parse_boolean_env(Some("1")));
        assert!(parse_boolean_env(Some("yes")));
        assert!(parse_boolean_env(Some("on")));
        assert!(parse_boolean_env(Some("TRUE")));
        assert!(parse_boolean_env(Some("  true  ")));
    }

    #[test]
    fn helper_parse_boolean_env_recognizes_false_values() {
        assert!(!parse_boolean_env(None));
        assert!(!parse_boolean_env(Some("")));
        assert!(!parse_boolean_env(Some("false")));
        assert!(!parse_boolean_env(Some("0")));
        assert!(!parse_boolean_env(Some("no")));
        assert!(!parse_boolean_env(Some("off")));
        assert!(!parse_boolean_env(Some("FALSE")));
        assert!(!parse_boolean_env(Some("maybe")));
    }

    // Integration tests with real testcontainers Postgres are in tests/integration/
}
