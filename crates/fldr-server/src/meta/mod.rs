//! The metadata store: repos, changesets, refs and derived data.
//!
//! [`MetaStore`] abstracts over Postgres ([`PgMetaStore`], production) and an
//! in-memory fallback ([`MemMetaStore`], local runs without `DATABASE_URL`).
//! M0 only needs liveness and schema introspection for `/health`; the history
//! operations arrive with later milestones (see the trait docs).

use async_trait::async_trait;

pub use mem::MemMetaStore;
pub use pg::PgMetaStore;

mod mem;
mod pg;

/// Which backend a [`MetaStore`] is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MetaKind {
    Postgres,
    Memory,
}

/// A metadata-store failure.
#[derive(Debug, thiserror::Error)]
pub enum MetaError {
    #[error(transparent)]
    Db(#[from] sqlx::Error),
    #[error(transparent)]
    Migrate(#[from] sqlx::migrate::MigrateError),
}

/// Metadata storage backend.
///
/// M0 methods only. Later milestones add, roughly in this order:
///
/// - **M1/M2 – changesets:** `create_changeset(repo, cs, body, paths)`,
///   inserting into `changesets`, `changeset_parents`, `changeset_paths` and
///   `repo_blobs` in one transaction; `get_changeset`; `paths_since(repo,
///   base, head)` for land's overlap check.
/// - **M2 – refs:** `get_ref`, and `move_ref(repo, name, old, new, actor,
///   reason)` as a single-row compare-and-swap plus a `ref_log` insert,
///   returning whether the CAS won (see the comment on `refs` in
///   `migrations/0001_init.sql`).
/// - **M2 – land:** built on `paths_since` and `move_ref`; the server
///   rebases and retries the CAS on disjoint paths, else reports 409.
/// - **M2+ – derived data:** `get_derived(repo, cs, kind, version)` and
///   `put_derived(...)` for the recursive derivation framework.
#[async_trait]
pub trait MetaStore: Send + Sync + 'static {
    /// Round-trip to the backend. Cheap; used by `/health`.
    async fn ping(&self) -> Result<(), MetaError>;

    /// Latest successfully applied schema migration, or `None` if unknown
    /// (no migrations applied, or a backend without a schema).
    async fn schema_version(&self) -> Result<Option<i64>, MetaError>;

    fn kind(&self) -> MetaKind;
}
