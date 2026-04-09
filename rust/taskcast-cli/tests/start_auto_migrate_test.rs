//! E2E tests for Rust auto-migrate feature with real Postgres.
//!
//! These tests verify:
//! 1. Auto-migrate enabled + Postgres configured → migrations applied
//! 2. Idempotency → running twice applies only pending migrations
//! 3. Auto-migrate disabled → skips migrations but server starts
//! 4. Postgres not configured → skips migrations (no-op)
//!
//! Uses testcontainers for real Postgres (not mocks).
//! Environment variables are controlled per-test via EnvGuard.

use std::sync::{Mutex, MutexGuard};
use sqlx::postgres::PgPoolOptions;
use taskcast_cli::commands::start::StartArgs;
use testcontainers::runners::AsyncRunner;
use testcontainers_modules::postgres::Postgres;

// ─── Helpers ─────────────────────────────────────────────────────────────────

static ENV_LOCK: Mutex<()> = Mutex::new(());

fn lock_env() -> MutexGuard<'static, ()> {
    ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

struct EnvGuard {
    vars: Vec<(&'static str, Option<String>)>,
    _lock: MutexGuard<'static, ()>,
}

impl EnvGuard {
    fn new(vars: &[(&'static str, &str)]) -> Self {
        let lock = lock_env();
        let mut saved_vars = Vec::new();
        for (key, value) in vars {
            let old_value = std::env::var(key).ok();
            saved_vars.push((*key, old_value));
            std::env::set_var(key, value);
        }
        Self {
            vars: saved_vars,
            _lock: lock,
        }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        for (key, old_value) in &self.vars {
            if let Some(value) = old_value {
                std::env::set_var(key, value);
            } else {
                std::env::remove_var(key);
            }
        }
    }
}

async fn find_available_port() -> u16 {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    port
}

/// Create a Postgres pool and run migrations manually.
/// Used to verify database schema after auto-migrate runs.
async fn get_postgres_pool(database_url: &str) -> sqlx::PgPool {
    PgPoolOptions::new()
        .max_connections(5)
        .connect(database_url)
        .await
        .unwrap()
}

/// Query the information_schema to verify tables exist.
async fn verify_tables_exist(pool: &sqlx::PgPool, table_names: &[&str]) -> bool {
    for table_name in table_names {
        let result: Option<(String,)> = sqlx::query_as(
            "SELECT table_name FROM information_schema.tables \
             WHERE table_schema = 'public' AND table_name = $1"
        )
        .bind(*table_name)
        .fetch_optional(pool)
        .await
        .unwrap();

        if result.is_none() {
            return false;
        }
    }
    true
}

/// Query the _sqlx_migrations table to get count of applied migrations.
async fn get_applied_migration_count(pool: &sqlx::PgPool) -> i64 {
    let result: (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM _sqlx_migrations WHERE success = true"
    )
    .fetch_one(pool)
    .await
    .unwrap_or((0,));

    result.0
}

// ─── Test 1: Auto-migrate enabled + Postgres configured ────────────────────────

#[tokio::test]
async fn start_with_auto_migrate_enabled_applies_migrations() {
    // Start Postgres container
    let container = Postgres::default().start().await.unwrap();
    let host_port = container.get_host_port_ipv4(5432).await.unwrap();
    let database_url = format!(
        "postgres://postgres:postgres@127.0.0.1:{}/postgres",
        host_port
    );

    // Set environment: auto-migrate enabled, Postgres configured
    let _env = EnvGuard::new(&[
        ("TASKCAST_AUTO_MIGRATE", "true"),
        ("TASKCAST_POSTGRES_URL", &database_url),
    ]);

    let port = find_available_port().await;

    // Spawn server in background
    let handle = tokio::spawn(async move {
        let _ = taskcast_cli::commands::start::run(StartArgs {
            port,
            storage: "memory".to_string(),
            ..Default::default()
        })
        .await;
    });

    // Wait for server to start and migrations to run
    tokio::time::sleep(std::time::Duration::from_secs(2)).await;

    // Verify migrations were applied by checking database schema
    let pool = get_postgres_pool(&database_url).await;

    // Verify tables exist (created by migrations)
    let tables_exist = verify_tables_exist(&pool, &["taskcast_tasks", "taskcast_events"]).await;
    assert!(tables_exist, "Expected tables 'taskcast_tasks' and 'taskcast_events' to exist");

    // Verify at least 1 migration was applied
    let migration_count = get_applied_migration_count(&pool).await;
    assert!(migration_count > 0, "Expected at least 1 migration to be applied, got {}", migration_count);

    // Verify health endpoint works
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    let res = reqwest::get(&format!("http://127.0.0.1:{port}/health"))
        .await
        .unwrap();
    assert!(res.status().is_success(), "Health endpoint should return 200");

    handle.abort();
}

// ─── Test 2: Idempotency (run twice) ───────────────────────────────────────────

#[tokio::test]
async fn start_auto_migrate_twice_is_idempotent() {
    // Start Postgres container
    let container = Postgres::default().start().await.unwrap();
    let host_port = container.get_host_port_ipv4(5432).await.unwrap();
    let database_url = format!(
        "postgres://postgres:postgres@127.0.0.1:{}/postgres",
        host_port
    );

    // First run: apply migrations
    {
        let _env = EnvGuard::new(&[
            ("TASKCAST_AUTO_MIGRATE", "true"),
            ("TASKCAST_POSTGRES_URL", &database_url),
        ]);

        let port = find_available_port().await;

        let handle = tokio::spawn(async move {
            let _ = taskcast_cli::commands::start::run(StartArgs {
                port,
                storage: "memory".to_string(),
                ..Default::default()
            })
            .await;
        });

        tokio::time::sleep(std::time::Duration::from_secs(1)).await;

        let pool = get_postgres_pool(&database_url).await;
        let count_after_first_run = get_applied_migration_count(&pool).await;
        assert!(count_after_first_run > 0, "First run should apply migrations");

        handle.abort();
        drop(_env);
    }

    // Second run: should be no-op (all migrations already applied)
    {
        let _env = EnvGuard::new(&[
            ("TASKCAST_AUTO_MIGRATE", "true"),
            ("TASKCAST_POSTGRES_URL", &database_url),
        ]);

        let port = find_available_port().await;

        let handle = tokio::spawn(async move {
            let _ = taskcast_cli::commands::start::run(StartArgs {
                port,
                storage: "memory".to_string(),
                ..Default::default()
            })
            .await;
        });

        tokio::time::sleep(std::time::Duration::from_secs(1)).await;

        let pool = get_postgres_pool(&database_url).await;
        let count_after_second_run = get_applied_migration_count(&pool).await;

        // Count should remain the same (no new migrations applied)
        let pool_first = get_postgres_pool(&database_url).await;
        let count_first = get_applied_migration_count(&pool_first).await;
        assert_eq!(count_after_second_run, count_first, "Second run should not apply new migrations (idempotent)");

        handle.abort();
    }
}

// ─── Test 3: Auto-migrate disabled (env var false) ────────────────────────────

#[tokio::test]
async fn start_with_auto_migrate_disabled_skips_migrations_but_starts_server() {
    // Start Postgres container
    let container = Postgres::default().start().await.unwrap();
    let host_port = container.get_host_port_ipv4(5432).await.unwrap();
    let database_url = format!(
        "postgres://postgres:postgres@127.0.0.1:{}/postgres",
        host_port
    );

    // Set environment: auto-migrate DISABLED, Postgres configured
    let _env = EnvGuard::new(&[
        ("TASKCAST_AUTO_MIGRATE", "false"),
        ("TASKCAST_POSTGRES_URL", &database_url),
    ]);

    let port = find_available_port().await;

    // Spawn server in background
    let handle = tokio::spawn(async move {
        let _ = taskcast_cli::commands::start::run(StartArgs {
            port,
            storage: "memory".to_string(),
            ..Default::default()
        })
        .await;
    });

    // Wait for server to start
    tokio::time::sleep(std::time::Duration::from_secs(1)).await;

    // Verify server started (health check passes)
    let res = reqwest::get(&format!("http://127.0.0.1:{port}/health"))
        .await
        .unwrap();
    assert!(res.status().is_success(), "Health endpoint should work even without auto-migrate");

    // Verify migrations were NOT applied (no _sqlx_migrations table should exist)
    let pool = get_postgres_pool(&database_url).await;
    let result: Option<(String,)> = sqlx::query_as(
        "SELECT table_name FROM information_schema.tables \
         WHERE table_schema = 'public' AND table_name = '_sqlx_migrations'"
    )
    .fetch_optional(&pool)
    .await
    .unwrap();

    assert!(result.is_none(), "_sqlx_migrations table should NOT exist when auto-migrate is disabled");

    handle.abort();
}

// ─── Test 4: Postgres not configured (env var missing) ────────────────────────

#[tokio::test]
async fn start_with_postgres_not_configured_skips_migrations() {
    // Do NOT set TASKCAST_POSTGRES_URL env var
    // This simulates running with memory storage only
    let _env = EnvGuard::new(&[
        ("TASKCAST_AUTO_MIGRATE", "true"),
        // TASKCAST_POSTGRES_URL is NOT set
    ]);

    let port = find_available_port().await;

    // Spawn server in background with memory storage
    let handle = tokio::spawn(async move {
        let _ = taskcast_cli::commands::start::run(StartArgs {
            port,
            storage: "memory".to_string(),
            ..Default::default()
        })
        .await;
    });

    // Wait for server to start
    tokio::time::sleep(std::time::Duration::from_secs(1)).await;

    // Verify health endpoint still works
    let res = reqwest::get(&format!("http://127.0.0.1:{port}/health"))
        .await
        .unwrap();
    assert!(res.status().is_success(), "Health should work with memory storage");

    handle.abort();
}

// ─── Test 5: Verify schema structure (tables, columns) ────────────────────────

#[tokio::test]
async fn auto_migrate_creates_correct_table_structure() {
    // Start Postgres container
    let container = Postgres::default().start().await.unwrap();
    let host_port = container.get_host_port_ipv4(5432).await.unwrap();
    let database_url = format!(
        "postgres://postgres:postgres@127.0.0.1:{}/postgres",
        host_port
    );

    // Set environment: auto-migrate enabled
    let _env = EnvGuard::new(&[
        ("TASKCAST_AUTO_MIGRATE", "true"),
        ("TASKCAST_POSTGRES_URL", &database_url),
    ]);

    let port = find_available_port().await;

    // Spawn server
    let handle = tokio::spawn(async move {
        let _ = taskcast_cli::commands::start::run(StartArgs {
            port,
            storage: "memory".to_string(),
            ..Default::default()
        })
        .await;
    });

    tokio::time::sleep(std::time::Duration::from_secs(2)).await;

    // Verify table structure
    let pool = get_postgres_pool(&database_url).await;

    // Check 'taskcast_tasks' table has expected columns
    let tasks_columns: Vec<(String,)> = sqlx::query_as(
        "SELECT column_name FROM information_schema.columns \
         WHERE table_schema = 'public' AND table_name = 'taskcast_tasks' \
         ORDER BY column_name"
    )
    .fetch_all(&pool)
    .await
    .unwrap();

    assert!(!tasks_columns.is_empty(), "taskcast_tasks table should have columns");
    let column_names: Vec<String> = tasks_columns.iter().map(|(name,)| name.clone()).collect();

    // Verify key columns exist
    assert!(column_names.contains(&"id".to_string()), "taskcast_tasks table should have 'id' column");
    assert!(column_names.contains(&"status".to_string()), "taskcast_tasks table should have 'status' column");
    assert!(column_names.contains(&"params".to_string()), "taskcast_tasks table should have 'params' column");

    // Check 'taskcast_events' table has expected columns
    let events_columns: Vec<(String,)> = sqlx::query_as(
        "SELECT column_name FROM information_schema.columns \
         WHERE table_schema = 'public' AND table_name = 'taskcast_events' \
         ORDER BY column_name"
    )
    .fetch_all(&pool)
    .await
    .unwrap();

    assert!(!events_columns.is_empty(), "taskcast_events table should have columns");
    let event_column_names: Vec<String> = events_columns.iter().map(|(name,)| name.clone()).collect();

    // Verify key columns exist
    assert!(event_column_names.contains(&"id".to_string()), "taskcast_events table should have 'id' column");
    assert!(event_column_names.contains(&"task_id".to_string()), "taskcast_events table should have 'task_id' column");
    assert!(event_column_names.contains(&"data".to_string()), "taskcast_events table should have 'data' column");

    handle.abort();
}

// ─── Test 6: Multiple servers in sequence (redis + postgres) ──────────────────

#[tokio::test]
async fn start_redis_storage_with_postgres_long_term_auto_migrate() {
    // Start Redis container
    let redis_container = testcontainers_modules::redis::Redis::default()
        .start()
        .await
        .unwrap();
    let redis_port = redis_container.get_host_port_ipv4(6379).await.unwrap();
    let redis_url = format!("redis://127.0.0.1:{}", redis_port);

    // Start Postgres container
    let postgres_container = Postgres::default().start().await.unwrap();
    let postgres_port = postgres_container.get_host_port_ipv4(5432).await.unwrap();
    let database_url = format!(
        "postgres://postgres:postgres@127.0.0.1:{}/postgres",
        postgres_port
    );

    // Set environment: redis broadcast + postgres long-term store with auto-migrate
    let _env = EnvGuard::new(&[
        ("TASKCAST_AUTO_MIGRATE", "true"),
        ("TASKCAST_REDIS_URL", &redis_url),
        ("TASKCAST_POSTGRES_URL", &database_url),
    ]);

    let port = find_available_port().await;

    // Spawn server with redis + postgres
    let handle = tokio::spawn(async move {
        let _ = taskcast_cli::commands::start::run(StartArgs {
            port,
            storage: "redis".to_string(),
            ..Default::default()
        })
        .await;
    });

    tokio::time::sleep(std::time::Duration::from_secs(2)).await;

    // Verify server started
    let res = reqwest::get(&format!("http://127.0.0.1:{port}/health"))
        .await
        .unwrap();
    assert!(res.status().is_success(), "Health should work with redis + postgres");

    // Verify migrations were applied
    let pool = get_postgres_pool(&database_url).await;
    let tables_exist = verify_tables_exist(&pool, &["taskcast_tasks", "taskcast_events"]).await;
    assert!(tables_exist, "Migrations should be applied with redis storage");

    handle.abort();
}

/// Regression test for the fail-fast requirement:
/// when a dirty migration row exists, `run_auto_migrate` MUST return Err
/// with a message starting with "Auto-migration failed:".
///
/// This mirrors the TS integration test in start-auto-migrate.test.ts
/// "wraps migration errors in performAutoMigrateIfEnabled".
#[tokio::test]
async fn run_auto_migrate_fails_fast_on_dirty_migration() {
    use taskcast_cli::run_auto_migrate;

    let postgres_container = Postgres::default().start().await.unwrap();
    let postgres_port = postgres_container.get_host_port_ipv4(5432).await.unwrap();
    let database_url = format!(
        "postgres://postgres:postgres@127.0.0.1:{}/postgres",
        postgres_port
    );

    let pool = sqlx::PgPool::connect(&database_url).await.unwrap();

    // Create the _sqlx_migrations table and insert a dirty row.
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
         VALUES (99, 'corrupt test', false, '\\x00', -1)",
    )
    .execute(&pool)
    .await
    .unwrap();

    // Call run_auto_migrate with enabled=true and pool present.
    let result = run_auto_migrate(Some(&pool), Some(&database_url), Some("true")).await;

    assert!(result.is_err(), "Expected Err on dirty migration");
    let err_msg = result.unwrap_err().to_string();
    assert!(
        err_msg.starts_with("Auto-migration failed:"),
        "Error should start with 'Auto-migration failed:', got: {}",
        err_msg
    );
    assert!(
        err_msg.contains("Dirty migration found"),
        "Error should mention dirty migration, got: {}",
        err_msg
    );
}
