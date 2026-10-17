//! fldr's pure data model.
//!
//! Everything in fldr is content-addressed and immutable: file contents
//! (blobs), directory listings (trees) and changesets are identified by the
//! SHA-256 of their bytes, so any tier can cache them without coordination.
//! This crate has no I/O; storage lives in `fldr-store` and history in
//! `fldr-server`.

pub mod api;
pub mod canonical;
pub mod changeset;
pub mod hash;
pub mod ops;
pub mod path;
pub mod tree;

pub use canonical::{CanonicalError, from_canonical, to_canonical};
pub use changeset::{Actor, ActorKind, Changes, Changeset, ChangesetError, FileChange};
pub use hash::{BlobId, ChangesetId, Hash, HashParseError, ObjectKind, TreeId};
pub use ops::{ApplyError, changed_paths, diff, overlapping_paths, paths_overlap};
pub use path::{PathError, RepoPath};
pub use tree::{FileEntry, FileKind, FlatTree, Tree, TreeEntry, TreeError};
