mod long_term;
mod row_helpers;
mod short_term;

pub use long_term::SqliteLongTermStore;
pub use short_term::SqliteShortTermStore;

use sqlx::sqlite::SqlitePoolOptions;
use sqlx::{Row, SqlitePool};

pub struct SqliteAdapters {
    pub short_term_store: SqliteShortTermStore,
    pub long_term_store: SqliteLongTermStore,
}

pub async fn create_sqlite_adapters(
    db_path: &str,
) -> Result<SqliteAdapters, Box<dyn std::error::Error>> {
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect(&format!("sqlite:{}?mode=rwc", db_path))
        .await?;

    // Enable WAL mode and foreign keys
    sqlx::query("PRAGMA journal_mode=WAL")
        .execute(&pool)
        .await?;
    sqlx::query("PRAGMA foreign_keys=ON")
        .execute(&pool)
        .await?;

    // Run migration — split into individual statements because
    // SQLite's sqlx driver only executes one statement per call.
    run_migrations(&pool).await?;

    Ok(SqliteAdapters {
        short_term_store: SqliteShortTermStore::new(pool.clone()),
        long_term_store: SqliteLongTermStore::new_shared_archive_restore_storage(pool),
    })
}

async fn run_migrations(pool: &SqlitePool) -> Result<(), Box<dyn std::error::Error>> {
    let migration_sql = include_str!("../migrations/001_initial.sql");

    // Split on semicolons and execute each statement individually
    for statement in migration_sql.split(';') {
        let trimmed = statement.trim();
        if !trimmed.is_empty() {
            sqlx::query(trimmed).execute(pool).await?;
        }
    }

    let columns = sqlx::query("PRAGMA table_info(taskcast_tasks)")
        .fetch_all(pool)
        .await?;
    let existing = columns
        .iter()
        .map(|row| row.get::<String, _>("name"))
        .collect::<std::collections::HashSet<_>>();
    for (name, definition) in [
        ("storage_state", "TEXT NOT NULL DEFAULT 'hot'"),
        ("storage_epoch", "INTEGER NOT NULL DEFAULT 1"),
        ("active_release_generation", "TEXT"),
        ("archive_watermark", "INTEGER NOT NULL DEFAULT -1"),
        ("last_event_at", "INTEGER"),
        ("cold_at", "INTEGER"),
        ("execution_deadline_at", "INTEGER"),
        ("task_version", "INTEGER NOT NULL DEFAULT 0"),
        ("ttl_claim_token", "TEXT"),
        ("ttl_claim_until", "INTEGER"),
    ] {
        if !existing.contains(name) {
            sqlx::query(&format!(
                "ALTER TABLE taskcast_tasks ADD COLUMN {name} {definition}"
            ))
            .execute(pool)
            .await?;
        }
    }

    let lifecycle_sql = include_str!("../migrations/002_storage_lifecycle.sql");
    for statement in lifecycle_sql.split(';') {
        let trimmed = statement.trim();
        if !trimmed.is_empty() {
            sqlx::query(trimmed).execute(pool).await?;
        }
    }

    Ok(())
}
