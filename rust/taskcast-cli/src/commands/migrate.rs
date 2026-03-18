use std::collections::HashSet;

use clap::Args;

use crate::helpers::{format_display_url, resolve_postgres_url};

#[derive(Args, Debug)]
pub struct MigrateArgs {
    /// Postgres connection URL (highest priority)
    #[arg(long)]
    pub url: Option<String>,
    /// Config file path
    #[arg(short, long)]
    pub config: Option<String>,
    /// Skip confirmation prompt
    #[arg(short, long)]
    pub yes: bool,
}

pub async fn run(args: MigrateArgs) -> Result<(), Box<dyn std::error::Error>> {
    let MigrateArgs { url, config, yes } = args;

    // 1. Resolve postgres URL: --url > env var > config file
    let file_config = match taskcast_core::config::load_config_file(config.as_deref()) {
        Ok(cfg) => cfg,
        Err(e) => {
            return Err(format!("[taskcast] Failed to load config file: {e}").into());
        }
    };

    let config_url = file_config
        .adapters
        .as_ref()
        .and_then(|a| a.long_term_store.as_ref())
        .and_then(|lt| lt.url.clone());

    let postgres_url = resolve_postgres_url(
        url,
        std::env::var("TASKCAST_POSTGRES_URL").ok(),
        config_url,
    );

    let postgres_url = match postgres_url {
        Some(u) => u,
        None => {
            return Err("[taskcast] No Postgres URL found. Provide one via --url, TASKCAST_POSTGRES_URL, or config file.".into());
        }
    };

    // 2. Display target info
    let display_url = format_display_url(&postgres_url);
    eprintln!("[taskcast] Target database: {display_url}");

    // 3. Connect to database
    let pool = sqlx::PgPool::connect(&postgres_url)
        .await
        .map_err(|e| format!("Failed to connect to database: {e}"))?;

    // 4. Check pending migrations
    let migrator = sqlx::migrate!("../../migrations/postgres");

    // Ensure _sqlx_migrations table exists so the query doesn't fail on fresh databases
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
    .map_err(|e| format!("Failed to check migration state: {e}"))?;

    // Fail fast on dirty (failed) migrations
    let dirty: Vec<(i64,)> = sqlx::query_as(
        "SELECT version FROM _sqlx_migrations WHERE success = false ORDER BY version",
    )
    .fetch_all(&pool)
    .await
    .map_err(|e| format!("Failed to query dirty migrations: {e}"))?;

    if !dirty.is_empty() {
        let versions: Vec<String> = dirty.iter().map(|r| r.0.to_string()).collect();
        pool.close().await;
        return Err(format!(
            "[taskcast] Dirty (failed) migrations found: versions {}. Fix manually before running migrations.",
            versions.join(", ")
        ).into());
    }

    let applied: Vec<(i64,)> = sqlx::query_as(
        "SELECT version FROM _sqlx_migrations WHERE success = true ORDER BY version",
    )
    .fetch_all(&pool)
    .await
    .map_err(|e| format!("Failed to query applied migrations: {e}"))?;

    let applied_set: HashSet<i64> = applied.iter().map(|r| r.0).collect();
    let pending: Vec<_> = migrator
        .iter()
        .filter(|m| !applied_set.contains(&m.version))
        .collect();

    // 5. If nothing pending, exit early
    if pending.is_empty() {
        eprintln!("[taskcast] Database is up to date.");
        pool.close().await;
        return Ok(());
    }

    // 6. List pending migrations
    eprintln!("[taskcast] {} pending migration(s):", pending.len());
    for m in &pending {
        eprintln!(
            "  - {:03}_{}.sql",
            m.version,
            m.description.replace(' ', "_")
        );
    }

    // 7. Prompt for confirmation unless -y
    if !yes {
        use std::io::IsTerminal;
        use std::io::Write;
        if !std::io::stdin().is_terminal() {
            pool.close().await;
            return Err("[taskcast] No TTY detected. Re-run with --yes (-y) to skip confirmation.".into());
        }
        eprint!(
            "Apply {} migration(s) to {}? (Y/n) ",
            pending.len(),
            display_url
        );
        std::io::stderr().flush()?;
        let mut input = String::new();
        std::io::stdin().read_line(&mut input)?;
        let trimmed = input.trim().to_lowercase();
        if !(trimmed.is_empty() || trimmed == "y" || trimmed == "yes") {
            eprintln!("[taskcast] Migration cancelled.");
            pool.close().await;
            return Ok(());
        }
    }

    // 8. Run migrations
    let store = taskcast_postgres::PostgresLongTermStore::new(pool.clone());
    store
        .migrate()
        .await
        .map_err(|e| format!("Migration failed: {e}"))?;

    // 9. Print summary
    eprintln!(
        "[taskcast] Successfully applied {} migration(s).",
        pending.len()
    );
    pool.close().await;

    Ok(())
}
