use std::time::Duration;

use async_trait::async_trait;
use sqlx::PgPool;
use sqlx::migrate::Migrator;
use sqlx::postgres::PgPoolOptions;

use super::{MetaError, MetaKind, MetaStore};

/// Migrations embedded at compile time from `crates/fldr-server/migrations`.
pub static MIGRATOR: Migrator = sqlx::migrate!("./migrations");

/// Postgres-backed metadata store.
///
/// Queries are built at runtime (`sqlx::query*` functions, not the `query!`
/// macros), so building needs neither a live database nor offline data.
#[derive(Debug, Clone)]
pub struct PgMetaStore {
    pool: PgPool,
}

impl PgMetaStore {
    /// Connect to `url` and apply pending migrations.
    ///
    /// Fails fast (within a few seconds) if the database is unreachable; the
    /// caller decides whether to retry.
    pub async fn connect(url: &str) -> Result<Self, MetaError> {
        let pool = PgPoolOptions::new()
            .max_connections(10)
            .min_connections(0)
            // Bounds both connection establishment and waiting for a free
            // connection, so `/health` never hangs on a dead database.
            .acquire_timeout(Duration::from_secs(5))
            .idle_timeout(Duration::from_secs(300))
            .max_lifetime(Duration::from_secs(30 * 60))
            .connect(url)
            .await?;
        Self::from_pool(pool).await
    }

    /// Wrap an existing pool and apply pending migrations.
    pub async fn from_pool(pool: PgPool) -> Result<Self, MetaError> {
        MIGRATOR.run(&pool).await?;
        Ok(Self { pool })
    }

    pub fn pool(&self) -> &PgPool {
        &self.pool
    }
}

#[async_trait]
impl MetaStore for PgMetaStore {
    async fn ping(&self) -> Result<(), MetaError> {
        sqlx::query("SELECT 1").execute(&self.pool).await?;
        Ok(())
    }

    async fn schema_version(&self) -> Result<Option<i64>, MetaError> {
        let v: Option<i64> =
            sqlx::query_scalar("SELECT max(version) FROM _sqlx_migrations WHERE success")
                .fetch_one(&self.pool)
                .await?;
        Ok(v)
    }

    fn kind(&self) -> MetaKind {
        MetaKind::Postgres
    }
}
