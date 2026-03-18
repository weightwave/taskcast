use taskcast_cli::commands::migrate::MigrateArgs;
use taskcast_cli::helpers::{format_display_url, resolve_postgres_url};

// ─── MigrateArgs construction ─────────────────────────────────────────────────

#[test]
fn migrate_args_default_values() {
    let args = MigrateArgs {
        url: None,
        config: None,
        yes: false,
    };
    assert!(args.url.is_none());
    assert!(args.config.is_none());
    assert!(!args.yes);
}

#[test]
fn migrate_args_with_url() {
    let args = MigrateArgs {
        url: Some("postgres://user:pass@localhost:5432/testdb".to_string()),
        config: None,
        yes: false,
    };
    assert_eq!(
        args.url.as_deref(),
        Some("postgres://user:pass@localhost:5432/testdb")
    );
}

#[test]
fn migrate_args_with_config_path() {
    let args = MigrateArgs {
        url: None,
        config: Some("/etc/taskcast/config.yaml".to_string()),
        yes: false,
    };
    assert_eq!(
        args.config.as_deref(),
        Some("/etc/taskcast/config.yaml")
    );
}

#[test]
fn migrate_args_with_yes_flag() {
    let args = MigrateArgs {
        url: None,
        config: None,
        yes: true,
    };
    assert!(args.yes);
}

#[test]
fn migrate_args_all_fields_set() {
    let args = MigrateArgs {
        url: Some("postgres://localhost/db".to_string()),
        config: Some("./taskcast.yaml".to_string()),
        yes: true,
    };
    assert!(args.url.is_some());
    assert!(args.config.is_some());
    assert!(args.yes);
}

// ─── URL resolution for migrate (same logic as run()) ─────────────────────────

#[test]
fn migrate_url_resolution_cli_takes_priority() {
    let result = resolve_postgres_url(
        Some("postgres://cli-url".to_string()),
        Some("postgres://env-url".to_string()),
        Some("postgres://config-url".to_string()),
    );
    assert_eq!(result, Some("postgres://cli-url".to_string()));
}

#[test]
fn migrate_url_resolution_env_fallback() {
    let result = resolve_postgres_url(
        None,
        Some("postgres://env-url".to_string()),
        Some("postgres://config-url".to_string()),
    );
    assert_eq!(result, Some("postgres://env-url".to_string()));
}

#[test]
fn migrate_url_resolution_config_fallback() {
    let result = resolve_postgres_url(
        None,
        None,
        Some("postgres://config-url".to_string()),
    );
    assert_eq!(result, Some("postgres://config-url".to_string()));
}

#[test]
fn migrate_url_resolution_none_when_all_missing() {
    let result = resolve_postgres_url(None, None, None);
    assert!(result.is_none());
}

// ─── Display URL formatting for migrate ───────────────────────────────────────

#[test]
fn migrate_display_url_standard() {
    let display = format_display_url("postgres://user:pass@db.example.com:5432/mydb");
    assert_eq!(display, "db.example.com:5432/mydb");
}

#[test]
fn migrate_display_url_localhost_wildcard() {
    let display = format_display_url("postgres://user@0.0.0.0:5432/taskcast");
    assert_eq!(display, "localhost:5432/taskcast");
}

#[test]
fn migrate_display_url_ipv6_wildcard() {
    let display = format_display_url("postgres://user@[::]:5432/taskcast");
    assert_eq!(display, "localhost:5432/taskcast");
}

#[test]
fn migrate_display_url_default_port_and_db() {
    let display = format_display_url("postgres://user@myhost");
    assert_eq!(display, "myhost:5432/postgres");
}

#[test]
fn migrate_display_url_invalid_url_returns_raw() {
    let display = format_display_url("not-a-valid-url");
    assert_eq!(display, "not-a-valid-url");
}

// ─── MigrateArgs debug trait ──────────────────────────────────────────────────

#[test]
fn migrate_args_implements_debug() {
    let args = MigrateArgs {
        url: Some("postgres://localhost/db".to_string()),
        config: None,
        yes: true,
    };
    let debug_output = format!("{:?}", args);
    assert!(debug_output.contains("MigrateArgs"));
    assert!(debug_output.contains("postgres://localhost/db"));
    assert!(debug_output.contains("yes: true"));
}

// ─── Integration tests with testcontainers (real Postgres) ────────────────────

use sqlx::postgres::PgPoolOptions;
use testcontainers::runners::AsyncRunner;
use testcontainers_modules::postgres::Postgres;

use taskcast_cli::commands::migrate;

async fn start_postgres() -> (String, testcontainers::ContainerAsync<Postgres>) {
    let container = Postgres::default().start().await.unwrap();
    let host_port = container.get_host_port_ipv4(5432).await.unwrap();
    let url = format!(
        "postgres://postgres:postgres@127.0.0.1:{}/postgres?sslmode=disable",
        host_port
    );
    (url, container)
}

#[tokio::test]
async fn migrate_run_applies_pending_migrations() {
    let (url, _container) = start_postgres().await;

    // First run: should apply all pending migrations
    let result = migrate::run(MigrateArgs {
        url: Some(url.clone()),
        config: None,
        yes: true,
    })
    .await;
    assert!(result.is_ok(), "migrate should succeed: {:?}", result.err());

    // Verify migrations were applied
    let pool = PgPoolOptions::new()
        .max_connections(1)
        .connect(&url)
        .await
        .unwrap();
    let applied: Vec<(i64,)> = sqlx::query_as(
        "SELECT version FROM _sqlx_migrations WHERE success = true ORDER BY version",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert!(
        !applied.is_empty(),
        "at least one migration should have been applied"
    );
    pool.close().await;
}

#[tokio::test]
async fn migrate_run_already_up_to_date() {
    let (url, _container) = start_postgres().await;

    // First run: apply all migrations
    migrate::run(MigrateArgs {
        url: Some(url.clone()),
        config: None,
        yes: true,
    })
    .await
    .unwrap();

    // Second run: should report "up to date" and return Ok
    let result = migrate::run(MigrateArgs {
        url: Some(url.clone()),
        config: None,
        yes: true,
    })
    .await;
    assert!(
        result.is_ok(),
        "second migrate should succeed (already up to date): {:?}",
        result.err()
    );
}

#[tokio::test]
async fn migrate_run_no_tty_without_yes_exits_via_error_path() {
    // In CI/tests, stdin is NOT a terminal, so without --yes the function
    // calls process::exit(1). We can't test that in-process, but we CAN
    // test the --yes path which is the primary coverage target.
    // This test documents the expected behavior.
    let (url, _container) = start_postgres().await;

    // With --yes=true, should work fine
    let result = migrate::run(MigrateArgs {
        url: Some(url),
        config: None,
        yes: true,
    })
    .await;
    assert!(result.is_ok());
}

#[tokio::test]
async fn migrate_run_with_invalid_url_returns_error() {
    // An unreachable Postgres URL should return an error (not panic)
    let result = migrate::run(MigrateArgs {
        url: Some("postgres://user:pass@127.0.0.1:19999/nonexistent".to_string()),
        config: None,
        yes: true,
    })
    .await;
    assert!(result.is_err(), "should fail with connection error");
    let err = result.err().unwrap().to_string();
    assert!(
        err.contains("Failed to connect") || err.contains("error"),
        "got: {err}"
    );
}

#[tokio::test]
async fn migrate_run_detects_dirty_migrations() {
    let (url, _container) = start_postgres().await;

    // Create the _sqlx_migrations table with a dirty (failed) record
    let pool = PgPoolOptions::new()
        .max_connections(1)
        .connect(&url)
        .await
        .unwrap();

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
    .execute(&pool)
    .await
    .unwrap();

    // Insert a failed migration record
    sqlx::query(
        "INSERT INTO _sqlx_migrations (version, description, success, checksum, execution_time)
         VALUES (99999, 'dirty_test', false, '\\x00', 0)",
    )
    .execute(&pool)
    .await
    .unwrap();
    pool.close().await;

    // Verify the dirty record is detectable by querying directly
    // (we can't test process::exit(1) in-process, but we can confirm
    // the detection query finds the dirty record)
    let pool2 = PgPoolOptions::new()
        .max_connections(1)
        .connect(&url)
        .await
        .unwrap();

    let dirty: Vec<(i64,)> = sqlx::query_as(
        "SELECT version FROM _sqlx_migrations WHERE success = false ORDER BY version",
    )
    .fetch_all(&pool2)
    .await
    .unwrap();

    assert!(
        !dirty.is_empty(),
        "should detect at least one dirty migration"
    );
    assert_eq!(
        dirty[0].0, 99999,
        "dirty migration version should be 99999"
    );
    pool2.close().await;
}

// ─── Direct run() error path tests ──────────────────────────────────────────

#[tokio::test]
async fn migrate_run_no_url_returns_error() {
    // No URL, no env var, no config -> should return Err
    // Clear the env var to ensure no fallback
    let saved = std::env::var("TASKCAST_POSTGRES_URL").ok();
    std::env::remove_var("TASKCAST_POSTGRES_URL");

    let result = migrate::run(MigrateArgs {
        url: None,
        config: None,
        yes: true,
    })
    .await;

    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(
        err.contains("No Postgres URL"),
        "error should mention missing URL, got: {err}"
    );

    // Restore env var if it was set
    if let Some(val) = saved {
        std::env::set_var("TASKCAST_POSTGRES_URL", val);
    }
}

#[tokio::test]
async fn migrate_run_bad_config_returns_error() {
    // Point to a nonexistent config file
    let result = migrate::run(MigrateArgs {
        url: None,
        config: Some("/nonexistent/config.yaml".to_string()),
        yes: true,
    })
    .await;

    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(
        err.contains("config") || err.contains("Failed to load") || err.contains("No Postgres URL"),
        "error should mention config loading failure, got: {err}"
    );
}

#[tokio::test]
async fn migrate_run_invalid_config_file_returns_error() {
    // An existing file with invalid YAML triggers the config load error path (lines 26-27)
    let tmp = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(tmp.path(), "{{invalid yaml content!!!").unwrap();
    let result = migrate::run(MigrateArgs {
        url: None,
        config: Some(tmp.path().to_str().unwrap().to_string()),
        yes: true,
    })
    .await;

    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(
        err.contains("Failed to load") || err.contains("config"),
        "error should mention config loading failure, got: {err}"
    );
}

#[tokio::test]
async fn migrate_run_returns_error_on_dirty_migrations() {
    let (url, _container) = start_postgres().await;

    // Create the _sqlx_migrations table with a dirty (failed) record
    let pool = PgPoolOptions::new()
        .max_connections(1)
        .connect(&url)
        .await
        .unwrap();

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
    .execute(&pool)
    .await
    .unwrap();

    sqlx::query(
        "INSERT INTO _sqlx_migrations (version, description, success, checksum, execution_time)
         VALUES (99999, 'dirty_test', false, '\\x00', 0)",
    )
    .execute(&pool)
    .await
    .unwrap();
    pool.close().await;

    // Now call run() which should detect dirty migrations and return Err (lines 86-91)
    let result = migrate::run(MigrateArgs {
        url: Some(url),
        config: None,
        yes: true,
    })
    .await;
    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(
        err.contains("Dirty") || err.contains("dirty"),
        "expected dirty migration error, got: {err}"
    );
}

#[tokio::test]
async fn migrate_run_no_tty_without_yes_returns_error() {
    let (url, _container) = start_postgres().await;

    // Without --yes, and no TTY (test environment), should return error
    let result = migrate::run(MigrateArgs {
        url: Some(url),
        config: None,
        yes: false,
    })
    .await;

    assert!(result.is_err(), "should fail without TTY and without --yes");
    let err = result.unwrap_err().to_string();
    assert!(
        err.contains("No TTY") || err.contains("TTY"),
        "expected TTY error, got: {err}"
    );
}
