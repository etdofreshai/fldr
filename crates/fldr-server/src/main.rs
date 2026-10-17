//! `fldr-server` binary: configuration, logging, stores and shutdown around
//! [`fldr_server::app`].
//!
//! `fldr-server --healthcheck` instead probes a running server on `$PORT` and
//! exits 0 if `/health` returns 200 (used by the Docker `HEALTHCHECK`).

use std::process::ExitCode;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Context;
use fldr_server::config::{Config, LogFormat};
use fldr_server::meta::{MemMetaStore, MetaStore, PgMetaStore};
use fldr_server::{AppState, app, healthcheck, volume};
use fldr_store::{BlobStore, FsBlobStore};
use tracing_subscriber::EnvFilter;

/// How long to keep retrying an unreachable database at boot before exiting
/// non-zero (and letting the orchestrator restart us).
const DB_BOOT_DEADLINE: Duration = Duration::from_secs(60);

#[tokio::main]
async fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        None => {}
        Some("--healthcheck") => return run_healthcheck().await,
        Some("--version" | "-V") => {
            println!("fldr-server {}", fldr_server::VERSION);
            return ExitCode::SUCCESS;
        }
        Some(other) => {
            eprintln!(
                "usage: fldr-server [--healthcheck | --version]  (unknown argument {other:?})"
            );
            return ExitCode::from(2);
        }
    }

    let config = match Config::from_env() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("fldr-server: {e}");
            return ExitCode::from(2);
        }
    };
    init_tracing(config.log_format);

    match serve(config).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            tracing::error!(error = format!("{e:#}"), "fldr-server exiting");
            ExitCode::FAILURE
        }
    }
}

async fn run_healthcheck() -> ExitCode {
    let port = match Config::from_env() {
        Ok(c) => c.port,
        Err(e) => {
            eprintln!("healthcheck: {e}");
            return ExitCode::FAILURE;
        }
    };
    match healthcheck::probe(port, Duration::from_secs(5)).await {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => {
            eprintln!("healthcheck: /health did not return 200");
            ExitCode::FAILURE
        }
        Err(e) => {
            eprintln!("healthcheck: {e}");
            ExitCode::FAILURE
        }
    }
}

fn init_tracing(format: LogFormat) {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    let builder = tracing_subscriber::fmt().with_env_filter(filter);
    match format {
        LogFormat::Json => builder.json().flatten_event(true).init(),
        LogFormat::Pretty => builder.init(),
    }
}

async fn serve(config: Config) -> anyhow::Result<()> {
    tracing::info!(
        version = fldr_server::VERSION,
        git_sha = fldr_server::GIT_SHA,
        "starting fldr-server"
    );

    let volume_id = volume::init_data_dir(&config.data_dir)
        .with_context(|| format!("initializing DATA_DIR {}", config.data_dir.display()))?;
    tracing::info!(data_dir = %config.data_dir.display(), volume_id, "data directory ready");

    let blobs: Arc<dyn BlobStore> = Arc::new(
        FsBlobStore::open(config.data_dir.join("blobs"))
            .await
            .context("opening blob store")?,
    );

    let meta: Arc<dyn MetaStore> = match &config.database_url {
        Some(url) => Arc::new(connect_db(url).await?),
        None => {
            tracing::warn!("DATABASE_URL is unset: using the in-memory metadata store");
            Arc::new(MemMetaStore::new())
        }
    };

    let state = AppState::new(meta, blobs, Some(volume_id));
    let addr = config.socket_addr();
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .with_context(|| format!("binding {addr}"))?;
    tracing::info!(%addr, "listening");

    axum::serve(listener, app(state))
        .with_graceful_shutdown(shutdown_signal())
        .await
        .context("server error")?;
    tracing::info!("shut down cleanly");
    Ok(())
}

/// Connect to Postgres and migrate, retrying with exponential backoff for up
/// to [`DB_BOOT_DEADLINE`]. Migration errors other than connectivity are
/// retried too; they are rare and the deadline bounds the cost.
async fn connect_db(url: &str) -> anyhow::Result<PgMetaStore> {
    let start = Instant::now();
    let mut delay = Duration::from_millis(500);
    let mut attempt = 1u32;
    loop {
        match PgMetaStore::connect(url).await {
            Ok(store) => {
                tracing::info!(attempt, "connected to Postgres; migrations applied");
                return Ok(store);
            }
            Err(e) if start.elapsed() + delay < DB_BOOT_DEADLINE => {
                tracing::warn!(attempt, error = %e, retry_in_ms = delay.as_millis() as u64, "database unavailable");
                tokio::time::sleep(delay).await;
                delay = (delay * 2).min(Duration::from_secs(8));
                attempt += 1;
            }
            Err(e) => {
                return Err(e).context(format!(
                    "database still unavailable after {attempt} attempts over {}s",
                    start.elapsed().as_secs()
                ));
            }
        }
    }
}

async fn shutdown_signal() {
    let ctrl_c = async {
        if let Err(e) = tokio::signal::ctrl_c().await {
            tracing::error!(error = %e, "cannot listen for Ctrl-C");
            std::future::pending::<()>().await;
        }
    };

    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut s) => {
                s.recv().await;
            }
            Err(e) => {
                tracing::error!(error = %e, "cannot listen for SIGTERM");
                std::future::pending::<()>().await;
            }
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        () = ctrl_c => tracing::info!("received Ctrl-C, shutting down"),
        () = terminate => tracing::info!("received SIGTERM, shutting down"),
    }
}
