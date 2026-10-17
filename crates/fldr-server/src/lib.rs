//! fldr's authoritative server.
//!
//! [`app`] builds the HTTP router over an [`AppState`]; `main.rs` only wires
//! configuration, stores, logging and shutdown around it. M0 serves `/health`
//! and `/v1`; history, refs and land arrive in later milestones.

use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::Json;
use axum::Router;
use axum::extract::{DefaultBodyLimit, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use fldr_core::api::{ApiError, ComponentStatus, HealthResponse, ServiceInfo};
use fldr_store::BlobStore;
use tower_http::timeout::TimeoutLayer;
use tower_http::trace::TraceLayer;

pub mod config;
pub mod healthcheck;
pub mod meta;
pub mod volume;

use meta::{MetaKind, MetaStore};

/// Value of `service` in [`HealthResponse`] and [`ServiceInfo`].
pub const SERVICE: &str = "fldr-server";
/// Crate version of this build.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
/// Git commit this build came from, if `build.rs` could determine it.
///
/// An empty value counts as unknown: the Dockerfile always declares the
/// `FLDR_GIT_SHA` build arg, so it is present but empty when not passed.
pub const GIT_SHA: Option<&str> = match option_env!("FLDR_GIT_SHA") {
    Some(sha) if !sha.is_empty() => Some(sha),
    _ => None,
};
/// Wire protocol major version reported by `/v1`.
pub const API_VERSION: u32 = 1;

/// Largest accepted request body. Blob uploads will get their own limit.
pub const BODY_LIMIT: usize = 16 * 1024 * 1024;
/// Requests taking longer than this get 408 Request Timeout.
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
/// Upper bound on each `/health` probe, below Docker's healthcheck timeout.
pub const PROBE_TIMEOUT: Duration = Duration::from_secs(3);

/// Shared, cheaply clonable server state.
#[derive(Clone)]
pub struct AppState {
    pub meta: Arc<dyn MetaStore>,
    pub blobs: Arc<dyn BlobStore>,
    /// Contents of `$DATA_DIR/.volume-id`.
    pub volume_id: Option<String>,
    pub started_at: Instant,
}

impl AppState {
    pub fn new(
        meta: Arc<dyn MetaStore>,
        blobs: Arc<dyn BlobStore>,
        volume_id: Option<String>,
    ) -> Self {
        Self {
            meta,
            blobs,
            volume_id,
            started_at: Instant::now(),
        }
    }
}

/// The full HTTP application.
pub fn app(state: AppState) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/v1", get(service_info))
        .fallback(not_found)
        .with_state(state)
        .layer(DefaultBodyLimit::max(BODY_LIMIT))
        .layer(TimeoutLayer::with_status_code(
            StatusCode::REQUEST_TIMEOUT,
            REQUEST_TIMEOUT,
        ))
        .layer(TraceLayer::new_for_http())
}

/// A JSON [`ApiError`] response with the given status.
pub fn api_error(status: StatusCode, code: &str, message: impl Into<String>) -> Response {
    let body = ApiError {
        code: code.to_owned(),
        message: message.into(),
    };
    (status, Json(body)).into_response()
}

/// Compute the health report. Probes run concurrently, each bounded by
/// [`PROBE_TIMEOUT`].
pub async fn health_report(state: &AppState) -> HealthResponse {
    let meta = async {
        let probe = async {
            state.meta.ping().await?;
            state.meta.schema_version().await
        };
        match tokio::time::timeout(PROBE_TIMEOUT, probe).await {
            Ok(Ok(schema)) => {
                let status = match state.meta.kind() {
                    MetaKind::Postgres => ComponentStatus::Ok,
                    MetaKind::Memory => ComponentStatus::Memory,
                };
                (status, schema)
            }
            Ok(Err(e)) => {
                tracing::warn!(error = %e, "health: metadata store failed");
                (ComponentStatus::Error, None)
            }
            Err(_) => {
                tracing::warn!("health: metadata store timed out");
                (ComponentStatus::Error, None)
            }
        }
    };
    let blobs = async {
        match tokio::time::timeout(PROBE_TIMEOUT, state.blobs.check()).await {
            Ok(Ok(())) => ComponentStatus::Ok,
            Ok(Err(e)) => {
                tracing::warn!(error = %e, "health: blob store failed");
                ComponentStatus::Error
            }
            Err(_) => {
                tracing::warn!("health: blob store timed out");
                ComponentStatus::Error
            }
        }
    };
    let ((db, schema), blobs) = tokio::join!(meta, blobs);

    HealthResponse {
        ok: db != ComponentStatus::Error && blobs != ComponentStatus::Error,
        service: SERVICE.to_owned(),
        version: VERSION.to_owned(),
        git_sha: GIT_SHA.map(str::to_owned),
        db,
        schema,
        blobs,
        volume_id: state.volume_id.clone(),
        uptime_secs: state.started_at.elapsed().as_secs(),
    }
}

async fn health(State(state): State<AppState>) -> Response {
    let report = health_report(&state).await;
    let status = if report.ok {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };
    (status, Json(report)).into_response()
}

async fn service_info() -> Json<ServiceInfo> {
    Json(ServiceInfo {
        service: SERVICE.to_owned(),
        version: VERSION.to_owned(),
        api_version: API_VERSION,
        capabilities: vec!["health".to_owned()],
    })
}

async fn not_found(uri: axum::http::Uri) -> Response {
    api_error(
        StatusCode::NOT_FOUND,
        "not_found",
        format!("no route for {}", uri.path()),
    )
}
