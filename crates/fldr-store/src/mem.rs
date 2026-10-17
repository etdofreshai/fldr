use async_trait::async_trait;
use bytes::Bytes;
use fldr_core::BlobId;

use crate::{BlobStore, StoreError};

/// In-memory store for tests and local development.
#[derive(Default)]
pub struct MemBlobStore {}

impl MemBlobStore {
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl BlobStore for MemBlobStore {
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
