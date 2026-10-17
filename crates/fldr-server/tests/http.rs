//! HTTP-level tests against the router with in-memory stores.

use std::sync::Arc;

use async_trait::async_trait;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use bytes::Bytes;
use fldr_core::BlobId;
use fldr_core::api::{ApiError, ComponentStatus, HealthResponse, ServiceInfo};
use fldr_server::meta::MemMetaStore;
use fldr_server::{AppState, app, volume};
use fldr_store::{BlobStore, MemBlobStore, StoreError};
use http_body_util::BodyExt;
use tower::ServiceExt;

/// A blob store whose every operation fails, to exercise the 503 path.
struct FailingBlobStore;

fn broken() -> StoreError {
    StoreError::Io(std::io::Error::other("disk on fire"))
}

#[async_trait]
impl BlobStore for FailingBlobStore {
    async fn put(&self, _: Bytes) -> Result<BlobId, StoreError> {
        Err(broken())
    }
    async fn put_verified(&self, _: BlobId, _: Bytes) -> Result<(), StoreError> {
        Err(broken())
    }
    async fn get(&self, _: &BlobId) -> Result<Option<Bytes>, StoreError> {
        Err(broken())
    }
    async fn has(&self, _: &BlobId) -> Result<bool, StoreError> {
        Err(broken())
    }
    async fn missing(&self, _: &[BlobId]) -> Result<Vec<BlobId>, StoreError> {
        Err(broken())
    }
    async fn check(&self) -> Result<(), StoreError> {
        Err(broken())
    }
}

fn state(blobs: Arc<dyn BlobStore>, data_dir: &std::path::Path) -> AppState {
    let volume_id = volume::init_data_dir(data_dir).unwrap();
    AppState::new(Arc::new(MemMetaStore::new()), blobs, Some(volume_id))
}

async fn get(state: AppState, uri: &str) -> (StatusCode, serde_json::Value) {
    let resp = app(state)
        .oneshot(Request::get(uri).body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = resp.status();
    assert_eq!(
        resp.headers()["content-type"],
        "application/json",
        "{uri} must answer JSON"
    );
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    (status, serde_json::from_slice(&body).unwrap())
}

#[tokio::test]
async fn health_ok_with_memory_stores() {
    let dir = tempfile::tempdir().unwrap();
    let st = state(Arc::new(MemBlobStore::new()), dir.path());
    let volume_id = st.volume_id.clone().unwrap();
    let (status, json) = get(st, "/health").await;
    assert_eq!(status, StatusCode::OK);

    // Exact wire shape (camelCase, lowercase enums).
    let obj = json.as_object().unwrap();
    let mut keys: Vec<&str> = obj.keys().map(String::as_str).collect();
    keys.sort_unstable();
    let mut expected = vec![
        "blobs",
        "db",
        "ok",
        "schema",
        "service",
        "uptimeSecs",
        "version",
        "volumeId",
    ];
    if fldr_server::GIT_SHA.is_some() {
        expected.push("gitSha");
        expected.sort_unstable();
    }
    assert_eq!(keys, expected);
    assert_eq!(json["ok"], true);
    assert_eq!(json["service"], "fldr-server");
    assert_eq!(json["version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(json["db"], "memory");
    assert_eq!(json["blobs"], "ok");
    assert_eq!(json["schema"], 1);
    assert_eq!(json["volumeId"], volume_id.as_str());
    assert!(json["uptimeSecs"].is_u64());

    let typed: HealthResponse = serde_json::from_value(json).unwrap();
    assert_eq!(typed.db, ComponentStatus::Memory);
}

#[tokio::test]
async fn health_503_when_blobs_fail() {
    let dir = tempfile::tempdir().unwrap();
    let (status, json) = get(state(Arc::new(FailingBlobStore), dir.path()), "/health").await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    let h: HealthResponse = serde_json::from_value(json).unwrap();
    assert!(!h.ok);
    assert_eq!(h.blobs, ComponentStatus::Error);
    assert_eq!(h.db, ComponentStatus::Memory);
}

#[tokio::test]
async fn v1_service_info() {
    let dir = tempfile::tempdir().unwrap();
    let (status, json) = get(state(Arc::new(MemBlobStore::new()), dir.path()), "/v1").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        json,
        serde_json::json!({
            "service": "fldr-server",
            "version": env!("CARGO_PKG_VERSION"),
            "apiVersion": 1,
            "capabilities": ["health"],
        })
    );
    let _: ServiceInfo = serde_json::from_value(json).unwrap();
}

#[tokio::test]
async fn unknown_route_is_json_404() {
    let dir = tempfile::tempdir().unwrap();
    let (status, json) = get(
        state(Arc::new(MemBlobStore::new()), dir.path()),
        "/no/such/route",
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let err: ApiError = serde_json::from_value(json).unwrap();
    assert_eq!(err.code, "not_found");
    assert!(err.message.contains("/no/such/route"));
}

#[test]
fn volume_id_stable_across_inits() {
    let dir = tempfile::tempdir().unwrap();
    let a = volume::init_data_dir(dir.path()).unwrap();
    let b = volume::init_data_dir(dir.path()).unwrap();
    assert_eq!(a, b);
    assert!(dir.path().join("blobs").is_dir());

    let other = tempfile::tempdir().unwrap();
    assert_ne!(volume::init_data_dir(other.path()).unwrap(), a);
}
