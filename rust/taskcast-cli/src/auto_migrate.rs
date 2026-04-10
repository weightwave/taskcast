use sqlx::PgPool;

use crate::helpers::{format_display_url, parse_boolean_env};

/// Automatically run database migrations if TASKCAST_AUTO_MIGRATE is enabled.
///
/// This is the Rust counterpart of `performAutoMigrateIfEnabled` in the TS CLI.
/// Both implementations MUST produce byte-identical log output for the same
/// outcomes so that users switching between runtimes see consistent behavior.
///
/// Log messages (spec §Error Handling & Log Messages — fixed for CI assertions):
/// - Banner:   `[taskcast] TASKCAST_AUTO_MIGRATE enabled — running Postgres migrations on <url>`
/// - Skip:     `[taskcast] TASKCAST_AUTO_MIGRATE is set but no Postgres configured — skipping`
/// - Up to date: `[taskcast] Database schema up to date (<N> migration(s) already applied)`
/// - Applied:  `[taskcast] Applied <N> new migration(s): <filename1>, <filename2>, ...`
/// - Failure:  `[taskcast] Auto-migration failed: <error_message>`
///
/// All messages go to stderr (`eprintln!`) to match the TS CLI's convention
/// (which uses `console.error`). This keeps stdout free for machine-readable output.
///
/// The presence of a `pool` is treated as proof that Postgres is configured.
/// This fixes the silent-bypass bug where auto-migrate would skip when
/// Postgres was configured only via the YAML config file (not env var).
///
/// # Arguments
/// * `pool` - An optional PgPool. If None, auto-migrate is skipped with the
///   "no Postgres configured" message.
/// * `postgres_url` - The resolved URL for the banner log (display-only).
/// * `env_auto_migrate` - TASKCAST_AUTO_MIGRATE env var value (for testability)
///
/// # Returns
/// * `Ok(())` if migration succeeds or is skipped (disabled / unconfigured)
/// * `Err` with message `"Auto-migration failed: <original_error>"` on failure
pub async fn run_auto_migrate(
    pool: Option<&PgPool>,
    postgres_url: Option<&str>,
    env_auto_migrate: Option<&str>,
) -> Result<(), Box<dyn std::error::Error>> {
    // Check if auto-migrate is enabled
    if !parse_boolean_env(env_auto_migrate) {
        return Ok(());
    }

    // Check if Postgres is actually configured: the presence of a pool is proof.
    // The env var alone is insufficient because Postgres may be configured via
    // the YAML config file only.
    let pool = match pool {
        Some(p) => p,
        None => {
            eprintln!(
                "[taskcast] TASKCAST_AUTO_MIGRATE is set but no Postgres configured — skipping"
            );
            return Ok(());
        }
    };

    // Log banner with display URL (credentials stripped).
    // The raw postgres_url may contain a password which must not leak into
    // stderr / log aggregators.
    let url_display = postgres_url
        .map(format_display_url)
        .unwrap_or_else(|| "<postgres>".to_string());
    eprintln!(
        "[taskcast] TASKCAST_AUTO_MIGRATE enabled — running Postgres migrations on {}",
        url_display
    );

    // Note on error handling: we do NOT log errors here. The caller (main.rs
    // for the CLI, or tests for direct invocations) is responsible for the
    // single user-facing "[taskcast] Auto-migration failed: ..." line.
    // Logging here would produce a duplicate when errors propagate through
    // main.rs's explicit error handler.

    // Ensure _sqlx_migrations table exists before querying it
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
    .map_err(|e| format!("Auto-migration failed: {}", e))?;

    // Fail fast on dirty (failed) migrations — must match TS dirty-check semantics.
    // TS error text: "Dirty migration found: version N (description). A previous
    // migration failed. Please fix it manually before running migrations."
    // Rust produces the same canonical wording.
    let dirty: Vec<(i64, String)> = sqlx::query_as(
        "SELECT version, description FROM _sqlx_migrations WHERE success = false ORDER BY version",
    )
    .fetch_all(pool)
    .await
    .map_err(|e| format!("Auto-migration failed: Failed to query dirty migrations: {}", e))?;

    if let Some((version, description)) = dirty.into_iter().next() {
        let inner = format!(
            "Dirty migration found: version {} ({}). A previous migration failed. Please fix it manually before running migrations.",
            version, description
        );
        return Err(format!("Auto-migration failed: {}", inner).into());
    }

    // Query currently-applied migrations BEFORE running, so we can compute the
    // precise "newly applied" list and filenames for the success log.
    let before: Vec<(i64,)> = sqlx::query_as(
        "SELECT version FROM _sqlx_migrations WHERE success = true ORDER BY version",
    )
    .fetch_all(pool)
    .await
    .map_err(|e| format!("Auto-migration failed: Failed to query applied migrations: {}", e))?;
    let before_set: std::collections::HashSet<i64> = before.iter().map(|r| r.0).collect();

    // Single sqlx::migrate!() invocation — use the returned migrator for both
    // the iter() (to know filenames) and the run() call. This avoids any
    // divergence between pre-flight count and actual applied set.
    let migrator = sqlx::migrate!("../../migrations/postgres");

    migrator
        .run(pool)
        .await
        .map_err(|err| format!("Auto-migration failed: {}", err))?;

    // Compute which migrations were newly applied by diffing against `before`.
    // Reconstruct filenames from (version, description) using the sqlx metadata:
    // filenames follow the 3-digit zero-padded convention `NNN_description.sql`
    // where description uses `_` as separator between words. sqlx stores
    // description with spaces replacing underscores, so we reverse that for the
    // filename.
    let newly_applied: Vec<String> = migrator
        .iter()
        .filter(|m| !before_set.contains(&m.version))
        .map(|m| format!("{:03}_{}.sql", m.version, m.description.replace(' ', "_")))
        .collect();

    if newly_applied.is_empty() {
        // Count of already-applied migrations in the migration set itself
        let already_applied_count = migrator
            .iter()
            .filter(|m| before_set.contains(&m.version))
            .count();
        eprintln!(
            "[taskcast] Database schema up to date ({} migration(s) already applied)",
            already_applied_count
        );
    } else {
        eprintln!(
            "[taskcast] Applied {} new migration(s): {}",
            newly_applied.len(),
            newly_applied.join(", ")
        );
    }

    Ok(())
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // Full run_auto_migrate coverage lives in integration tests with real
    // Postgres via testcontainers. Inline unit tests cover parse_boolean_env
    // dependency and the disabled/no-pool early-return branches.

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

    #[tokio::test]
    async fn returns_ok_when_auto_migrate_disabled() {
        // No pool needed — the function must return without touching the DB
        // when the env var is not truthy.
        let result = run_auto_migrate(None, None, None).await;
        assert!(result.is_ok());

        let result = run_auto_migrate(None, Some("postgres://x"), Some("false")).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn returns_ok_when_enabled_but_pool_is_none() {
        // Enabled but no pool → logs skip message and returns Ok.
        let result = run_auto_migrate(None, None, Some("true")).await;
        assert!(result.is_ok());
    }
}
