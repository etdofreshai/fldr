//! Immutable, content-addressed blob storage.
//!
//! Blobs are file contents keyed by [`BlobId`] (plain SHA-256). Once written a
//! blob never changes, so stores need no locking and any cache tier can serve
//! them. Trees, changesets and refs are metadata and live in the server's
//! metadata store, not here.

use async_trait::async_trait;
use bytes::Bytes;
use fldr_core::BlobId;

pub use fs::FsBlobStore;
pub use mem::MemBlobStore;

mod fs;
mod mem;

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    /// Uploaded bytes do not hash to the id the caller claimed.
    #[error("hash mismatch: expected {expected}, got {actual}")]
    HashMismatch { expected: BlobId, actual: BlobId },
    /// Stored bytes no longer hash to their id (bit rot, truncation, tampering).
    #[error("blob {0} is corrupt")]
    Corrupt(BlobId),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

#[async_trait]
pub trait BlobStore: Send + Sync + 'static {
    /// Store `data` and return its id. Idempotent: storing existing content
    /// is a cheap no-op.
    async fn put(&self, data: Bytes) -> Result<BlobId, StoreError>;

    /// Store `data` only if it hashes to `expected`; otherwise
    /// [`StoreError::HashMismatch`] and nothing is written.
    async fn put_verified(&self, expected: BlobId, data: Bytes) -> Result<(), StoreError>;

    /// Fetch a blob, verifying its hash. `Ok(None)` if absent;
    /// [`StoreError::Corrupt`] if present but damaged.
    async fn get(&self, id: &BlobId) -> Result<Option<Bytes>, StoreError>;

    async fn has(&self, id: &BlobId) -> Result<bool, StoreError>;

    /// The subset of `ids` not present, in input order, without duplicates.
    /// Clients call this before uploading so they only send what is missing.
    async fn missing(&self, ids: &[BlobId]) -> Result<Vec<BlobId>, StoreError>;

    /// Cheap liveness probe used by `/health` (for example a scratch-file
    /// write and delete). Must not create or modify any blob.
    async fn check(&self) -> Result<(), StoreError>;
}
