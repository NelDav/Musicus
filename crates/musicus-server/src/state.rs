//! Shared application state handed to every handler.

use crate::{auth::LoginLimiter, blobs::BlobStore, cache::FileCache, config::ClientCache, db::Db};
use std::sync::Arc;
use tokio::sync::Semaphore;

pub type AppState = Arc<App>;

pub struct App {
    pub db: Db,
    pub blobs: BlobStore,
    pub cache: FileCache,
    pub settings: Settings,
    pub limiter: LoginLimiter,
    /// Bounds concurrent Argon2 computations: each needs 19 MiB, so 60
    /// simultaneous logins would otherwise need over 1 GB of RAM.
    pub hashing: Semaphore,
}

#[derive(Clone, Debug)]
pub struct Settings {
    pub cookie_secure: bool,
    pub client_cache: ClientCache,
    pub session_ttl_secs: i64,
    pub invite_ttl_secs: i64,
    pub max_upload_bytes: u64,
    pub trust_proxy: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            cookie_secure: true,
            client_cache: ClientCache::None,
            session_ttl_secs: 60 * 24 * 3600,
            invite_ttl_secs: 7 * 24 * 3600,
            max_upload_bytes: 200 * 1024 * 1024,
            trust_proxy: false,
        }
    }
}

impl App {
    pub fn new(db: Db, blobs: BlobStore, cache_bytes: u64, settings: Settings) -> AppState {
        let cores = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1);
        Arc::new(App {
            db,
            blobs,
            cache: FileCache::new(cache_bytes),
            settings,
            limiter: LoginLimiter::default(),
            hashing: Semaphore::new(cores),
        })
    }
}
