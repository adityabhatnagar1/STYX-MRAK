//! SQLite connection management and migrations.

use crate::error::Result;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous};
use sqlx::SqlitePool;
use std::str::FromStr;
use std::time::Duration;

pub mod repositories;

/// Open (creating if needed) the SQLite database and run pending migrations.
pub async fn connect(database_url: &str) -> Result<SqlitePool> {
    let opts = SqliteConnectOptions::from_str(database_url)?
        .create_if_missing(true)
        .journal_mode(SqliteJournalMode::Wal)
        .synchronous(SqliteSynchronous::Normal)
        .foreign_keys(true)
        .busy_timeout(Duration::from_secs(5));

    // Make sure the parent directory exists for file-backed databases.
    if let Some(path) = opts.get_filename().parent() {
        if !path.as_os_str().is_empty() {
            let _ = std::fs::create_dir_all(path);
        }
    }

    let pool = SqlitePoolOptions::new().max_connections(5).connect_with(opts).await?;
    migrate(&pool).await?;
    Ok(pool)
}

pub async fn migrate(pool: &SqlitePool) -> Result<()> {
    sqlx::migrate!("./migrations").run(pool).await?;
    Ok(())
}

/// Cheap liveness probe used by `/health`.
pub async fn ping(pool: &SqlitePool) -> bool {
    sqlx::query_scalar::<_, i64>("SELECT 1").fetch_one(pool).await.is_ok()
}

/// In-memory database with migrations applied (tests and tooling). One
/// connection so every query sees the same database.
pub async fn connect_memory() -> Result<SqlitePool> {
    let opts = SqliteConnectOptions::from_str("sqlite::memory:")?.foreign_keys(true);
    let pool = SqlitePoolOptions::new().max_connections(1).connect_with(opts).await?;
    migrate(&pool).await?;
    Ok(pool)
}

#[cfg(test)]
pub async fn test_pool() -> SqlitePool {
    connect_memory().await.expect("in-memory database")
}
