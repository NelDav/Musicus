//! Command line and environment configuration.

use clap::{Args, Parser, Subcommand, ValueEnum};
use std::{net::SocketAddr, path::PathBuf};

#[derive(Parser, Debug)]
#[command(name = "musicus-server", version, about = "Sheet music library with private annotations")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Run the server.
    Serve(ServeArgs),
    /// Create an admin account and print its one-time invite link.
    CreateAdmin {
        #[command(flatten)]
        data: DataArgs,
        #[arg(long)]
        username: String,
        #[arg(long)]
        display_name: Option<String>,
        /// Base URL used to print the invite link.
        #[arg(long, env = "MUSICUS_PUBLIC_URL", default_value = "http://localhost:8080")]
        public_url: String,
    },
    /// Write a consistent copy of the database (safe while the server runs).
    /// Back up the `blobs` directory separately, after the database.
    Backup {
        #[command(flatten)]
        data: DataArgs,
        /// Target file; must not exist yet.
        #[arg(long)]
        to: PathBuf,
    },
}

#[derive(Args, Debug, Clone)]
pub struct DataArgs {
    /// Directory holding `musicus.sqlite` and the `blobs` file store.
    #[arg(long, env = "MUSICUS_DATA_DIR", default_value = "data")]
    pub data_dir: PathBuf,
}

impl DataArgs {
    pub fn db_path(&self) -> PathBuf {
        self.data_dir.join("musicus.sqlite")
    }
}

#[derive(Args, Debug, Clone)]
pub struct ServeArgs {
    #[command(flatten)]
    pub data: DataArgs,

    #[arg(long, env = "MUSICUS_LISTEN", default_value = "0.0.0.0:8080")]
    pub listen: SocketAddr,

    /// Directory of the built web client (output of `dx bundle`). Without
    /// it only the API is served.
    #[arg(long, env = "MUSICUS_WEB_DIR")]
    pub web_dir: Option<PathBuf>,

    /// Base URL printed in setup links, e.g. https://sheets.example.org.
    #[arg(long, env = "MUSICUS_PUBLIC_URL", default_value = "http://localhost:8080")]
    pub public_url: String,

    /// PEM certificate chain; together with --tls-key serves HTTPS directly.
    #[arg(long, env = "MUSICUS_TLS_CERT", requires = "tls_key")]
    pub tls_cert: Option<PathBuf>,

    #[arg(long, env = "MUSICUS_TLS_KEY", requires = "tls_cert")]
    pub tls_key: Option<PathBuf>,

    /// Send the session cookie without the `Secure` flag. Only for plain-HTTP
    /// development on another host than localhost; never in production.
    #[arg(long, env = "MUSICUS_INSECURE_COOKIES")]
    pub insecure_cookies: bool,

    /// Whether browsers may keep downloaded sheets in their HTTP cache.
    #[arg(long, env = "MUSICUS_CLIENT_CACHE", value_enum, default_value_t = ClientCache::None)]
    pub client_cache: ClientCache,

    /// RAM used to keep hot sheet files (e.g. tonight's programme), in MB.
    #[arg(long, env = "MUSICUS_RAM_CACHE_MB", default_value_t = 128)]
    pub ram_cache_mb: u64,

    /// Largest accepted upload, in MB.
    #[arg(long, env = "MUSICUS_MAX_UPLOAD_MB", default_value_t = 200)]
    pub max_upload_mb: u64,

    /// Days a session stays valid without use (sliding expiry).
    #[arg(long, env = "MUSICUS_SESSION_DAYS", default_value_t = 60)]
    pub session_days: u64,

    /// Take the client address from `X-Forwarded-For` (only behind a
    /// reverse proxy that sets it).
    #[arg(long, env = "MUSICUS_TRUST_PROXY")]
    pub trust_proxy: bool,
}

#[derive(ValueEnum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClientCache {
    /// `Cache-Control: no-store`: clients keep nothing on disk.
    None,
    /// Files are immutable and may be cached privately for a year.
    Immutable,
}
