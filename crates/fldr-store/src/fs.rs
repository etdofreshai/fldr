use std::path::PathBuf;

use async_trait::async_trait;
use bytes::Bytes;
use fldr_core::BlobId;

use crate::{BlobStore, StoreError};

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
        todo!("m0/store")
    }

    async fn put_verified(&self, _expected: BlobId, _data: Bytes) -> Result<(), StoreError> {
        todo!("m0/store")
    }

    async fn get(&self, _id: &BlobId) -> Result<Option<Bytes>, StoreError> {
        todo!("m0/store")
    }

    async fn has(&self, _id: &BlobId) -> Result<bool, StoreError> {
        todo!("m0/store")
    }

    async fn missing(&self, _ids: &[BlobId]) -> Result<Vec<BlobId>, StoreError> {
        todo!("m0/store")
    }

    async fn check(&self) -> Result<(), StoreError> {
        todo!("m0/store")
    }
}
