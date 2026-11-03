//! Musicus server: a sheet music library with group-based access control,
//! content-addressed file storage and private annotations.
//!
//! One process holds everything: the REST API, the static web client, the
//! SQLite database (WAL mode, one writer, several readers) and the file
//! store on disk.

pub mod access;
pub mod auth;
pub mod blobs;
pub mod cache;
pub mod config;
pub mod db;
pub mod error;
pub mod processors;
pub mod routes;
pub mod state;

use std::path::Path;

/// Opens the data directory: database (with migrations) and file store.
pub async fn open_state(
    data_dir: &Path,
    cache_bytes: u64,
    settings: state::Settings,
) -> anyhow_like::Result<state::AppState> {
    std::fs::create_dir_all(data_dir)?;
    let readers = std::thread::available_parallelism().map(|n| n.get() as u32).unwrap_or(2).clamp(2, 8);
    let db = db::Db::open(&data_dir.join("musicus.sqlite"), readers).await?;
    let blobs = blobs::BlobStore::open(data_dir)?;
    Ok(state::App::new(db, blobs, cache_bytes, settings))
}

/// When the database has no users yet, creates `admin` and returns a
/// one-time invite token for it.
pub async fn bootstrap_admin(app: &state::App) -> anyhow_like::Result<Option<String>> {
    let users: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM users").fetch_one(&app.db.read).await?;
    if users > 0 {
        return Ok(None);
    }
    let id = routes::users::insert_user(app, "admin", "Administrator", musicus_api::Role::Admin).await?;
    Ok(Some(routes::users::issue_invite(app, id).await?.token))
}

/// Minimal boxed error for setup code outside request handling.
pub mod anyhow_like {
    pub type Error = Box<dyn std::error::Error + Send + Sync>;
    pub type Result<T> = std::result::Result<T, Error>;
}
