mod long_term;
mod row_helpers;
mod short_term;

pub use long_term::SqliteLongTermStore;
pub use short_term::SqliteShortTermStore;

use sqlx::sqlite::SqlitePoolOptions;
use sqlx::SqlitePool;

pub struct SqliteAdapters {
    pub short_term: SqliteShortTermStore,
    pub long_term: SqliteLongTermStore,
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
        short_term: SqliteShortTermStore::new(pool.clone()),
        long_term: SqliteLongTermStore::new(pool),
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

    Ok(())
}
