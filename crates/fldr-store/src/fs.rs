use std::path::PathBuf;

use async_trait::async_trait;
use bytes::Bytes;
use fldr_core::BlobId;

use crate::{BlobStore, StoreError};

// Placeholder until m0/store lands; errors instead of panicking so callers
// such as `/health` degrade gracefully.
fn not_yet() -> StoreError {
    StoreError::Io(std::io::Error::other("FsBlobStore is not implemented yet"))
}

/// Blobs as zstd-compressed files under a root directory.
pub struct FsBlobStore {
    #[allow(dead_code)]
    root: PathBuf,
}

impl FsBlobStore {
    /// Open (creating if needed) a store rooted at `root`. The server passes
    /// `$DATA_DIR/blobs`.
    pub async fn open(root: impl Into<PathBuf>) -> Result<Self, StoreError> {
        let root = root.into();
        tokio::fs::create_dir_all(&root).await?;
        Ok(Self { root })
    }
}

#[async_trait]
impl BlobStore for FsBlobStore {
    async fn put(&self, _data: Bytes) -> Result<BlobId, StoreError> {
        Err(not_yet())
    }

    async fn put_verified(&self, _expected: BlobId, _data: Bytes) -> Result<(), StoreError> {
        Err(not_yet())
    }

    async fn get(&self, _id: &BlobId) -> Result<Option<Bytes>, StoreError> {
        Err(not_yet())
    }

    async fn has(&self, _id: &BlobId) -> Result<bool, StoreError> {
        Err(not_yet())
    }

    async fn missing(&self, _ids: &[BlobId]) -> Result<Vec<BlobId>, StoreError> {
        Err(not_yet())
    }

    async fn check(&self) -> Result<(), StoreError> {
        Err(not_yet())
    }
}
