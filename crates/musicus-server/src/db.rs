//! SQLite access: one writer connection (SQLite allows one writer at a time,
//! so the pool of one *is* the write queue) and a small pool of readers,
//! which never block on the writer in WAL mode.

use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePool, SqlitePoolOptions, SqliteSynchronous};
use std::{path::Path, time::Duration};

#[derive(Clone)]
pub struct Db {
    pub read: SqlitePool,
    pub write: SqlitePool,
}

impl Db {
    /// Opens (or creates) the database file and applies pending migrations.
    pub async fn open(path: &Path, readers: u32) -> Result<Self, sqlx::Error> {
        let base = SqliteConnectOptions::new()
            .filename(path)
            .journal_mode(SqliteJournalMode::Wal)
            .synchronous(SqliteSynchronous::Normal)
            .foreign_keys(true)
            .busy_timeout(Duration::from_secs(5))
            .pragma("cache_size", "-16000");
        let write =
            SqlitePoolOptions::new().max_connections(1).connect_with(base.clone().create_if_missing(true)).await?;
        sqlx::migrate!("./migrations").run(&write).await?;
        let read = SqlitePoolOptions::new().max_connections(readers.max(1)).connect_with(base.read_only(true)).await?;
        Ok(Self { read, write })
    }

    /// Writes a consistent copy of the database while the server may run.
    pub async fn backup_to(&self, target: &Path) -> Result<(), sqlx::Error> {
        sqlx::query("VACUUM INTO ?1").bind(target.to_string_lossy().into_owned()).execute(&self.write).await?;
        Ok(())
    }
}

/// Current time in Unix seconds.
pub fn now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

/// JSON array literal for `json_each(?)`, the way to pass id lists to SQLite.
pub fn json_ids(ids: &[i64]) -> String {
    serde_json::to_string(ids).expect("ids serialise")
}
