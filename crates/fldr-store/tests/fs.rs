use std::path::Path;

use bytes::Bytes;
use fldr_core::BlobId;
use fldr_store::{BlobStore, FsBlobStore, StoreError};
use proptest::prelude::*;

async fn store() -> (tempfile::TempDir, FsBlobStore) {
    let dir = tempfile::tempdir().unwrap();
    let store = FsBlobStore::open(dir.path().join("blobs")).await.unwrap();
    (dir, store)
}

fn tmp_entries(store: &FsBlobStore) -> Vec<std::path::PathBuf> {
    std::fs::read_dir(store.root().join("tmp"))
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect()
}

/// Deterministic incompressible bytes (xorshift), so tests need no RNG crate.
fn noise(len: usize, mut seed: u64) -> Vec<u8> {
    let mut out = Vec::with_capacity(len + 8);
    while out.len() < len {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        out.extend_from_slice(&seed.to_le_bytes());
    }
    out.truncate(len);
    out
}

#[tokio::test]
async fn open_creates_dirs() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("a/b/blobs");
    let store = FsBlobStore::open(&root).await.unwrap();
    assert!(root.is_dir());
    assert!(root.join("tmp").is_dir());
    assert_eq!(store.root(), root);
    // Reopening an existing store is fine.
    FsBlobStore::open(&root).await.unwrap();
}

#[tokio::test]
async fn layout_is_two_level_fanout() {
    let (_dir, store) = store().await;
    let id = store.put(Bytes::from_static(b"hello\n")).await.unwrap();
    let hex = "5891b5b522d5df086d0ff0b110fbd9d21bb4fc7163af34d08286a2e846f6be03";
    assert_eq!(id.to_hex(), hex);
    let expected = store.root().join("58").join("91").join(hex);
    assert_eq!(store.blob_path(&id), expected);
    assert!(expected.is_file());
    // Stored compressed: the file is a zstd frame of the content.
    let raw = std::fs::read(&expected).unwrap();
    assert_eq!(zstd::decode_all(raw.as_slice()).unwrap(), b"hello\n");
    assert!(tmp_entries(&store).is_empty());
}

async fn corrupt_with(f: impl FnOnce(&Path)) {
    let (_dir, store) = store().await;
    let data = Bytes::from(noise(10_000, 7));
    let id = store.put(data).await.unwrap();
    f(&store.blob_path(&id));
    match store.get(&id).await {
        Err(StoreError::Corrupt(bad)) => assert_eq!(bad, id),
        other => panic!("expected Corrupt, got {other:?}"),
    }
    // Existence checks are metadata-only and still see the file.
    assert!(store.has(&id).await.unwrap());
}

#[tokio::test]
async fn flipped_byte_is_corrupt() {
    corrupt_with(|path| {
        let mut raw = std::fs::read(path).unwrap();
        let mid = raw.len() / 2;
        raw[mid] ^= 0x01;
        std::fs::write(path, raw).unwrap();
    })
    .await;
}

#[tokio::test]
async fn truncated_file_is_corrupt() {
    corrupt_with(|path| {
        let raw = std::fs::read(path).unwrap();
        std::fs::write(path, &raw[..raw.len() / 2]).unwrap();
    })
    .await;
}

#[tokio::test]
async fn swapped_content_is_corrupt() {
    // A valid zstd frame of the wrong content must fail the re-hash.
    corrupt_with(|path| {
        std::fs::write(path, zstd::encode_all(&b"other"[..], 3).unwrap()).unwrap();
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn concurrent_puts_succeed_and_leave_no_temp_files() {
    let (_dir, store) = store().await;
    let store = std::sync::Arc::new(store);
    let shared = Bytes::from(noise(200_000, 1));
    let mut tasks = Vec::new();
    for i in 0..32u64 {
        let store = store.clone();
        let shared = shared.clone();
        tasks.push(tokio::spawn(async move {
            let distinct = Bytes::from(noise(100_000, 1000 + i));
            let a = store.put(shared).await.unwrap();
            let b = store.put(distinct.clone()).await.unwrap();
            assert_eq!(b, BlobId::of(&distinct));
            (a, b, distinct)
        }));
    }
    let mut results = Vec::new();
    for task in tasks {
        results.push(task.await.unwrap());
    }
    let shared_id = BlobId::of(&shared);
    for (a, b, distinct) in results {
        assert_eq!(a, shared_id);
        assert_eq!(store.get(&b).await.unwrap().unwrap(), distinct);
    }
    assert_eq!(store.get(&shared_id).await.unwrap().unwrap(), shared);
    assert!(tmp_entries(&store).is_empty(), "leftover temp files");
}

#[tokio::test]
async fn large_blob_round_trips() {
    let (_dir, store) = store().await;
    let data = Bytes::from(noise(5 * 1024 * 1024 + 123, 42));
    let id = store.put(data.clone()).await.unwrap();
    assert_eq!(store.get(&id).await.unwrap().unwrap(), data);
    store.put_verified(id, data).await.unwrap();
}

#[tokio::test]
async fn mismatch_writes_nothing_on_disk() {
    let (_dir, store) = store().await;
    let data = Bytes::from_static(b"data");
    let wrong = BlobId::of(b"not data");
    assert!(matches!(
        store.put_verified(wrong, data.clone()).await,
        Err(StoreError::HashMismatch { .. })
    ));
    assert!(!store.blob_path(&BlobId::of(&data)).exists());
    assert!(!store.blob_path(&wrong).exists());
    assert!(tmp_entries(&store).is_empty());
}

#[tokio::test]
async fn check_leaves_no_files() {
    let (_dir, store) = store().await;
    store.check().await.unwrap();
    assert!(tmp_entries(&store).is_empty());
    let entries: Vec<_> = std::fs::read_dir(store.root()).unwrap().collect();
    assert_eq!(entries.len(), 1, "only tmp/ exists after check");
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(32))]

    #[test]
    fn arbitrary_bytes_round_trip(data in proptest::collection::vec(any::<u8>(), 0..4096)) {
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        rt.block_on(async {
            let (_dir, store) = store().await;
            let data = Bytes::from(data);
            let id = store.put(data.clone()).await.unwrap();
            prop_assert_eq!(id, BlobId::of(&data));
            prop_assert_eq!(store.get(&id).await.unwrap().unwrap(), data);
            Ok(())
        })?;
    }
}
