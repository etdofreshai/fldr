//! A conformance suite every [`BlobStore`] implementation must pass.
//!
//! Enabled by the `testing` feature. Call [`conformance`] from a test with a
//! store; it panics on the first violation. Content is salted per call, so the
//! suite can run repeatedly against the same store.

use bytes::Bytes;
use fldr_core::BlobId;

use crate::{BlobStore, StoreError};

/// Exercise the whole [`BlobStore`] contract against `store`.
pub async fn conformance<S: BlobStore>(store: &S) {
    let salt = uuid::Uuid::new_v4();
    let blob = |name: &str| Bytes::from(format!("{salt}:{name}"));

    round_trip(store, blob("round-trip")).await;
    empty_blob(store).await;
    idempotent_put(store, blob("idempotent")).await;
    put_verified(store, blob("verified"), blob("mismatch")).await;
    has(store, blob("has")).await;
    missing(store, [blob("m-a"), blob("m-b"), blob("m-c")]).await;
    check(store).await;
}

async fn round_trip<S: BlobStore>(store: &S, data: Bytes) {
    let id = store.put(data.clone()).await.expect("put");
    assert_eq!(id, BlobId::of(&data), "put returns the sha256 of the data");
    let got = store.get(&id).await.expect("get");
    assert_eq!(got.as_deref(), Some(&data[..]), "get returns what was put");

    let absent = BlobId::of(format!("{data:?}:absent").as_bytes());
    assert_eq!(store.get(&absent).await.expect("get absent"), None);
}

async fn empty_blob<S: BlobStore>(store: &S) {
    let id = store.put(Bytes::new()).await.expect("put empty");
    assert_eq!(id, BlobId::of(b""));
    assert!(store.has(&id).await.expect("has empty"));
    let got = store.get(&id).await.expect("get empty");
    assert_eq!(got.as_deref(), Some(&b""[..]));
}

async fn idempotent_put<S: BlobStore>(store: &S, data: Bytes) {
    let first = store.put(data.clone()).await.expect("first put");
    let second = store.put(data.clone()).await.expect("second put");
    assert_eq!(first, second);
    store
        .put_verified(first, data.clone())
        .await
        .expect("put_verified of existing content");
    let got = store.get(&first).await.expect("get");
    assert_eq!(got.as_deref(), Some(&data[..]));
}

async fn put_verified<S: BlobStore>(store: &S, data: Bytes, other: Bytes) {
    let id = BlobId::of(&data);
    store
        .put_verified(id, data.clone())
        .await
        .expect("put_verified with correct id");
    assert_eq!(
        store.get(&id).await.expect("get").as_deref(),
        Some(&data[..])
    );

    // Claim `other` is `wrong`: must fail and store neither.
    let wrong = BlobId::of(format!("{other:?}:wrong").as_bytes());
    let actual = BlobId::of(&other);
    match store.put_verified(wrong, other).await {
        Err(StoreError::HashMismatch {
            expected,
            actual: got,
        }) => {
            assert_eq!(expected, wrong);
            assert_eq!(got, actual);
        }
        res => panic!("expected HashMismatch, got {res:?}"),
    }
    assert!(!store.has(&actual).await.expect("has actual"));
    assert!(!store.has(&wrong).await.expect("has wrong"));
    assert_eq!(store.get(&actual).await.expect("get actual"), None);
}

async fn has<S: BlobStore>(store: &S, data: Bytes) {
    let id = BlobId::of(&data);
    assert!(!store.has(&id).await.expect("has before put"));
    store.put(data).await.expect("put");
    assert!(store.has(&id).await.expect("has after put"));
}

async fn missing<S: BlobStore>(store: &S, [a, b, c]: [Bytes; 3]) {
    let (a, b, c) = (
        BlobId::of(&a),
        store.put(b).await.expect("put"),
        BlobId::of(&c),
    );
    assert_eq!(store.missing(&[]).await.expect("missing empty"), vec![]);
    let got = store
        .missing(&[c, b, a, c, b, a, c])
        .await
        .expect("missing");
    assert_eq!(
        got,
        vec![c, a],
        "input order, deduplicated, present ids dropped"
    );
}

async fn check<S: BlobStore>(store: &S) {
    let probe = BlobId::of(b"fldr health check\n");
    let before = store.has(&probe).await.expect("has");
    store.check().await.expect("check");
    store.check().await.expect("check twice");
    assert_eq!(
        store.has(&probe).await.expect("has"),
        before,
        "check must not create blobs"
    );
}
