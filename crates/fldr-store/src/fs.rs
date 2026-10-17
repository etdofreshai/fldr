//! Blobs as zstd-compressed files under a root directory.
//!
//! Layout: `<root>/<hex[0..2]>/<hex[2..4]>/<hex>`, plus `<root>/tmp/` for
//! in-flight writes and health-check scratch files.
//!
//! Writes are lock-free and crash-safe on a best-effort basis: content is
//! compressed into a uniquely named temp file under `<root>/tmp/`, fsynced,
//! renamed into place, and then the parent directory is fsynced so the new
//! name survives a crash. Because the rename is atomic and content is
//! immutable, a reader sees either no file or a complete one, and racing
//! writers of the same content can harmlessly rename over each other. A crash
//! can leave stray temp files behind; [`FsBlobStore::open`] removes old ones.

use std::collections::HashSet;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use async_trait::async_trait;
use bytes::Bytes;
use fldr_core::BlobId;

use crate::{BlobStore, StoreError};

/// Payloads at least this large are hashed and (de)compressed on the blocking
/// pool rather than on an async worker thread.
const INLINE_LIMIT: usize = 64 * 1024;

/// Temp files older than this are assumed to belong to a crashed writer.
const STALE_TMP_AGE: Duration = Duration::from_secs(60 * 60);

/// Tuning knobs for [`FsBlobStore`].
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct FsOptions {
    /// zstd compression level for newly written blobs (default 3).
    pub level: i32,
}

impl Default for FsOptions {
    fn default() -> Self {
        Self { level: 3 }
    }
}

impl FsOptions {
    /// Set the zstd compression level.
    pub fn level(mut self, level: i32) -> Self {
        self.level = level;
        self
    }
}

/// Blobs as zstd-compressed files under a root directory.
pub struct FsBlobStore {
    inner: Arc<Inner>,
}

struct Inner {
    root: PathBuf,
    tmp: PathBuf,
    opts: FsOptions,
}

impl FsBlobStore {
    /// Open (creating if needed) a store rooted at `root`. The server passes
    /// `$DATA_DIR/blobs`.
    pub async fn open(root: impl Into<PathBuf>) -> Result<Self, StoreError> {
        Self::with_options(root, FsOptions::default()).await
    }

    /// Like [`FsBlobStore::open`] with explicit [`FsOptions`].
    pub async fn with_options(
        root: impl Into<PathBuf>,
        opts: FsOptions,
    ) -> Result<Self, StoreError> {
        let root = root.into();
        let tmp = root.join("tmp");
        tokio::fs::create_dir_all(&tmp).await?;
        let inner = Arc::new(Inner { root, tmp, opts });
        let sweep = Arc::clone(&inner);
        blocking(move || {
            sweep.remove_stale_tmp();
            Ok(())
        })
        .await?;
        Ok(Self { inner })
    }

    /// The root directory of the store.
    pub fn root(&self) -> &Path {
        &self.inner.root
    }

    /// Where the blob `id` lives (or would live) on disk.
    pub fn blob_path(&self, id: &BlobId) -> PathBuf {
        self.inner.blob_path(id)
    }

    /// Write `data`, already known to hash to `id`, unless it is present.
    async fn store(&self, id: BlobId, data: Bytes) -> Result<(), StoreError> {
        let path = self.blob_path(&id);
        if is_file(&path).await? {
            return Ok(());
        }
        let inner = Arc::clone(&self.inner);
        blocking(move || inner.write_blob(&id, &path, &data)).await
    }
}

impl Inner {
    fn blob_path(&self, id: &BlobId) -> PathBuf {
        let hex = id.to_hex();
        self.root.join(&hex[0..2]).join(&hex[2..4]).join(hex)
    }

    fn tmp_path(&self, label: &str) -> PathBuf {
        self.tmp
            .join(format!("{label}.{}.tmp", uuid::Uuid::new_v4()))
    }

    /// Compress `data` into a temp file and atomically rename it to `path`.
    fn write_blob(&self, id: &BlobId, path: &Path, data: &[u8]) -> Result<(), StoreError> {
        let compressed = zstd::bulk::compress(data, self.opts.level)?;
        let tmp = TempFile(self.tmp_path(&id.to_hex()));
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp.0)?;
        file.write_all(&compressed)?;
        file.sync_all()?;
        drop(file);

        // `path` always has a parent below the root.
        let dir = path.parent().unwrap_or(&self.root);
        let new_dir = !dir.is_dir();
        if new_dir {
            fs::create_dir_all(dir)?;
        }
        fs::rename(&tmp.0, path)?;
        tmp.disarm();

        // Make the rename (and any new fan-out directories) durable.
        sync_dir(dir);
        if new_dir {
            for ancestor in dir.ancestors().skip(1).take(2) {
                sync_dir(ancestor);
            }
        }
        Ok(())
    }

    /// Best-effort removal of temp files left behind by crashed writers.
    fn remove_stale_tmp(&self) {
        let Ok(entries) = fs::read_dir(&self.tmp) else {
            return;
        };
        let now = SystemTime::now();
        for entry in entries.flatten() {
            let stale = entry
                .metadata()
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| now.duration_since(t).ok())
                .is_some_and(|age| age > STALE_TMP_AGE);
            if stale && let Err(err) = fs::remove_file(entry.path()) {
                tracing::warn!(path = %entry.path().display(), %err, "could not remove stale temp file");
            }
        }
    }
}

#[async_trait]
impl BlobStore for FsBlobStore {
    async fn put(&self, data: Bytes) -> Result<BlobId, StoreError> {
        let id = hash(data.clone()).await?;
        self.store(id, data).await?;
        Ok(id)
    }

    async fn put_verified(&self, expected: BlobId, data: Bytes) -> Result<(), StoreError> {
        let actual = hash(data.clone()).await?;
        if actual != expected {
            return Err(StoreError::HashMismatch { expected, actual });
        }
        self.store(actual, data).await
    }

    async fn get(&self, id: &BlobId) -> Result<Option<Bytes>, StoreError> {
        let path = self.blob_path(id);
        let compressed = match tokio::fs::read(&path).await {
            Ok(bytes) => bytes,
            Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(err) => return Err(err.into()),
        };
        let id = *id;
        let inline = compressed.len() < INLINE_LIMIT;
        let decode = move || {
            let data = match zstd::stream::decode_all(compressed.as_slice()) {
                Ok(data) => data,
                Err(err) => {
                    tracing::warn!(blob = %id, %err, "blob does not decompress");
                    return Err(StoreError::Corrupt(id));
                }
            };
            let actual = BlobId::of(&data);
            if actual != id {
                tracing::warn!(blob = %id, %actual, "blob content does not match its id");
                return Err(StoreError::Corrupt(id));
            }
            Ok(Bytes::from(data))
        };
        if inline {
            decode().map(Some)
        } else {
            blocking(decode).await.map(Some)
        }
    }

    async fn has(&self, id: &BlobId) -> Result<bool, StoreError> {
        Ok(is_file(&self.blob_path(id)).await?)
    }

    async fn missing(&self, ids: &[BlobId]) -> Result<Vec<BlobId>, StoreError> {
        let mut seen = HashSet::new();
        let unique: Vec<BlobId> = ids.iter().filter(|id| seen.insert(**id)).copied().collect();
        let inner = Arc::clone(&self.inner);
        blocking(move || {
            let mut missing = Vec::new();
            for id in unique {
                if !is_file_sync(&inner.blob_path(&id))? {
                    missing.push(id);
                }
            }
            Ok(missing)
        })
        .await
    }

    async fn check(&self) -> Result<(), StoreError> {
        let inner = Arc::clone(&self.inner);
        blocking(move || {
            let tmp = TempFile(inner.tmp_path("check"));
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&tmp.0)?;
            file.write_all(b"fldr health check\n")?;
            drop(file);
            fs::remove_file(&tmp.0)?;
            tmp.disarm();
            Ok(())
        })
        .await
    }
}

/// Hash `data`, off the async runtime when it is large.
async fn hash(data: Bytes) -> Result<BlobId, StoreError> {
    if data.len() < INLINE_LIMIT {
        Ok(BlobId::of(&data))
    } else {
        blocking(move || Ok(BlobId::of(&data))).await
    }
}

async fn blocking<T, F>(f: F) -> Result<T, StoreError>
where
    F: FnOnce() -> Result<T, StoreError> + Send + 'static,
    T: Send + 'static,
{
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|err| StoreError::Io(io::Error::other(err)))?
}

async fn is_file(path: &Path) -> io::Result<bool> {
    match tokio::fs::metadata(path).await {
        Ok(meta) => Ok(meta.is_file()),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(err) => Err(err),
    }
}

fn is_file_sync(path: &Path) -> io::Result<bool> {
    match fs::metadata(path) {
        Ok(meta) => Ok(meta.is_file()),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(err) => Err(err),
    }
}

/// fsync a directory so renames and creations inside it are durable. Errors
/// are ignored: some platforms and filesystems cannot fsync directories.
fn sync_dir(dir: &Path) {
    if let Err(err) = File::open(dir).and_then(|d| d.sync_all()) {
        tracing::debug!(dir = %dir.display(), %err, "directory fsync failed");
    }
}

/// A temp file removed on drop unless [`TempFile::disarm`] is called.
struct TempFile(PathBuf);

impl TempFile {
    fn disarm(mut self) {
        self.0 = PathBuf::new();
    }
}

impl Drop for TempFile {
    fn drop(&mut self) {
        if !self.0.as_os_str().is_empty() {
            let _ = fs::remove_file(&self.0);
        }
    }
}
