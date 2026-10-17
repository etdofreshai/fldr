use async_trait::async_trait;

use super::{MetaError, MetaKind, MetaStore};

/// In-memory metadata store for local runs without `DATABASE_URL`.
///
/// Holds nothing in M0. It reports schema version 1 so local runs look like a
/// freshly migrated database; state is lost on restart.
#[derive(Debug, Default)]
pub struct MemMetaStore {}

impl MemMetaStore {
    /// The schema version this build's migrations produce.
    pub const SCHEMA_VERSION: i64 = 1;

    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl MetaStore for MemMetaStore {
    async fn ping(&self) -> Result<(), MetaError> {
        Ok(())
    }

    async fn schema_version(&self) -> Result<Option<i64>, MetaError> {
        Ok(Some(Self::SCHEMA_VERSION))
    }

    fn kind(&self) -> MetaKind {
        MetaKind::Memory
    }
}
