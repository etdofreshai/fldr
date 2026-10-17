use fldr_store::testing::conformance;
use fldr_store::{FsBlobStore, MemBlobStore};

#[tokio::test]
async fn mem_store_conforms() {
    let store = MemBlobStore::new();
    conformance(&store).await;
    conformance(&store).await;
}

#[tokio::test]
async fn fs_store_conforms() {
    let dir = tempfile::tempdir().unwrap();
    let store = FsBlobStore::open(dir.path().join("blobs")).await.unwrap();
    conformance(&store).await;
    conformance(&store).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn fs_store_conforms_on_multi_thread_runtime() {
    let dir = tempfile::tempdir().unwrap();
    let store = FsBlobStore::open(dir.path()).await.unwrap();
    conformance(&store).await;
}
