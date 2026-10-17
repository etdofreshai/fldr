//! fldr's pure data model.
//!
//! Everything in fldr is content-addressed and immutable: file contents
//! (blobs), directory listings (trees) and changesets are identified by the
//! SHA-256 of their bytes, so any tier can cache them without coordination.
//! This crate has no I/O; storage lives in `fldr-store` and history in
//! `fldr-server`.

pub mod api;
pub mod hash;

pub use hash::{BlobId, ChangesetId, Hash, HashParseError, ObjectKind, TreeId};
