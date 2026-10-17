use std::collections::{HashMap, HashSet};
use std::sync::RwLock;

use async_trait::async_trait;
use bytes::Bytes;
use fldr_core::BlobId;

use crate::{BlobStore, StoreError};

/// In-memory store for tests and local development.
#[derive(Default)]
pub struct MemBlobStore {
    blobs: RwLock<HashMap<BlobId, Bytes>>,
}

impl MemBlobStore {
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl BlobStore for MemBlobStore {
    async fn put(&self, data: Bytes) -> Result<BlobId, StoreError> {
        let id = BlobId::of(&data);
        self.blobs.write().unwrap().entry(id).or_insert(data);
        Ok(id)
    }

    async fn put_verified(&self, expected: BlobId, data: Bytes) -> Result<(), StoreError> {
        let actual = BlobId::of(&data);
        if actual != expected {
            return Err(StoreError::HashMismatch { expected, actual });
        }
        self.blobs.write().unwrap().entry(actual).or_insert(data);
        Ok(())
    }

    async fn get(&self, id: &BlobId) -> Result<Option<Bytes>, StoreError> {
        Ok(self.blobs.read().unwrap().get(id).cloned())
    }

    async fn has(&self, id: &BlobId) -> Result<bool, StoreError> {
        Ok(self.blobs.read().unwrap().contains_key(id))
    }

    async fn missing(&self, ids: &[BlobId]) -> Result<Vec<BlobId>, StoreError> {
        let blobs = self.blobs.read().unwrap();
        let mut seen = HashSet::new();
        Ok(ids
            .iter()
            .filter(|id| !blobs.contains_key(id) && seen.insert(**id))
            .copied()
            .collect())
    }

    async fn check(&self) -> Result<(), StoreError> {
        Ok(())
    }
}
