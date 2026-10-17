//! JSON wire types shared by `fldr-server` and `fldr-cli`.
//!
//! Field names are camelCase on the wire. Additive changes only: clients must
//! ignore unknown fields, and new fields must be optional.

use serde::{Deserialize, Serialize};

/// Status of one server dependency in [`HealthResponse`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ComponentStatus {
    /// Reachable and working.
    Ok,
    /// Configured but failing.
    Error,
    /// Running on the in-memory fallback (no `DATABASE_URL`); local dev only.
    Memory,
}

/// `GET /health`. HTTP 200 when `ok`, otherwise 503 with the same body.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HealthResponse {
    pub ok: bool,
    /// Always `"fldr-server"`.
    pub service: String,
    /// Crate version of the running server.
    pub version: String,
    /// Git commit the server was built from, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git_sha: Option<String>,
    pub db: ComponentStatus,
    /// Latest applied metadata-schema migration, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schema: Option<i64>,
    pub blobs: ComponentStatus,
    /// Random id persisted in `$DATA_DIR/.volume-id` on first boot. Stable
    /// across restarts and redeploys iff the data volume persists.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub volume_id: Option<String>,
    pub uptime_secs: u64,
}

/// `GET /v1`: service discovery.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServiceInfo {
    /// Always `"fldr-server"`.
    pub service: String,
    pub version: String,
    /// Wire protocol major version; bumped only for breaking changes.
    pub api_version: u32,
    /// Feature flags clients may probe, e.g. `"health"`, `"blobs"`.
    pub capabilities: Vec<String>,
}

/// Error body for every non-2xx JSON response.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApiError {
    /// Stable, machine-readable code such as `"not_found"` or `"conflict"`.
    pub code: String,
    /// Human-readable description.
    pub message: String,
}
