use clap::Parser;
use musicus_server::{
    anyhow_like::Result,
    bootstrap_admin,
    config::{Cli, Command, ServeArgs},
    open_state, routes,
    state::Settings,
};
use std::net::SocketAddr;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info,tower_http=warn,sqlx=warn")),
        )
        .init();
    match Cli::parse().command {
        Command::Serve(args) => serve(args).await,
        Command::CreateAdmin { data, username, display_name, public_url } => {
            let app = open_state(&data.data_dir, 0, Settings::default()).await?;
            let display_name = display_name.unwrap_or_else(|| username.clone());
            let id = routes::users::insert_user(&app, &username, &display_name, musicus_api::Role::Admin).await?;
            let invite = routes::users::issue_invite(&app, id).await?;
            println!("Admin '{username}' created. Set the password here (valid 7 days):");
            println!("{}/invite/{}", public_url.trim_end_matches('/'), invite.token);
            Ok(())
        }
        Command::Backup { data, to } => {
            if to.exists() {
                return Err(format!("{} already exists", to.display()).into());
            }
            let app = open_state(&data.data_dir, 0, Settings::default()).await?;
            app.db.backup_to(&to).await?;
            println!("Database written to {}.", to.display());
            println!(
                "Now back up {} (immutable files; copy after the database).",
                data.data_dir.join("blobs").display()
            );
            Ok(())
        }
        Command::Healthcheck { listen } => healthcheck(listen),
    }
}

/// A minimal HTTP/1.0 request, so the runtime image needs no curl or shell.
fn healthcheck(listen: SocketAddr) -> Result<()> {
    use std::io::{Read, Write};
    use std::time::Duration;
    let mut target = listen;
    if target.ip().is_unspecified() {
        target.set_ip(std::net::Ipv4Addr::LOCALHOST.into());
    }
    let mut stream = std::net::TcpStream::connect_timeout(&target, Duration::from_secs(3))?;
    stream.set_read_timeout(Some(Duration::from_secs(3)))?;
    stream.write_all(b"GET /api/health HTTP/1.0\r\nHost: localhost\r\n\r\n")?;
    let mut head = [0u8; 12];
    stream.read_exact(&mut head)?;
    if head.ends_with(b" 200") { Ok(()) } else { Err(format!("unhealthy: {}", String::from_utf8_lossy(&head)).into()) }
}

async fn serve(args: ServeArgs) -> Result<()> {
    let settings = Settings {
        cookie_secure: !args.insecure_cookies,
        client_cache: args.client_cache,
        session_ttl_secs: args.session_days as i64 * 24 * 3600,
        max_upload_bytes: args.max_upload_mb * 1024 * 1024,
        trust_proxy: args.trust_proxy,
        ..Settings::default()
    };
    let state = open_state(&args.data.data_dir, args.ram_cache_mb * 1024 * 1024, settings).await?;
    if let Some(token) = bootstrap_admin(&state).await? {
        tracing::warn!("No users yet. Created 'admin'; set its password at:");
        tracing::warn!("{}/invite/{token}", args.public_url.trim_end_matches('/'));
    }
    if let Some(dir) = &args.web_dir
        && !dir.join("index.html").exists()
    {
        return Err(format!("{} contains no index.html (build the web client first)", dir.display()).into());
    }
    let app = routes::app(state, args.web_dir.as_deref()).into_make_service_with_connect_info::<SocketAddr>();

    match (&args.tls_cert, &args.tls_key) {
        (Some(cert), Some(key)) => {
            let _ = rustls::crypto::ring::default_provider().install_default();
            let config = axum_server::tls_rustls::RustlsConfig::from_pem_file(cert, key).await?;
            let handle = axum_server::Handle::new();
            let shutdown = handle.clone();
            tokio::spawn(async move {
                shutdown_signal().await;
                shutdown.graceful_shutdown(Some(std::time::Duration::from_secs(10)));
            });
            tracing::info!("listening on https://{}", args.listen);
            axum_server::bind_rustls(args.listen, config).handle(handle).serve(app).await?;
        }
        _ => {
            let listener = tokio::net::TcpListener::bind(args.listen).await?;
            tracing::info!("listening on http://{}", args.listen);
            axum::serve(listener, app).with_graceful_shutdown(shutdown_signal()).await?;
        }
    }
    Ok(())
}

async fn shutdown_signal() {
    let ctrl_c = tokio::signal::ctrl_c();
    #[cfg(unix)]
    let terminate = async {
        if let Ok(mut s) = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            s.recv().await;
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();
    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }
    tracing::info!("shutting down");
}
