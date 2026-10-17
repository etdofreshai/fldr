//! Postgres integration test. Runs only when `FLDR_TEST_DATABASE_URL` points
//! at a disposable database (it creates the fldr schema there).

use fldr_server::meta::{MetaKind, MetaStore, PgMetaStore};

#[tokio::test]
async fn migrations_apply_and_schema_is_v1() {
    let Ok(url) = std::env::var("FLDR_TEST_DATABASE_URL") else {
        eprintln!("skipping Postgres test: FLDR_TEST_DATABASE_URL is not set");
        return;
    };

    let store = PgMetaStore::connect(&url).await.expect("connect + migrate");
    assert_eq!(store.kind(), MetaKind::Postgres);
    store.ping().await.unwrap();
    assert_eq!(store.schema_version().await.unwrap(), Some(1));

    // Re-running migrations is a no-op.
    let again = PgMetaStore::from_pool(store.pool().clone()).await.unwrap();
    assert_eq!(again.schema_version().await.unwrap(), Some(1));

    let tables: Vec<String> = sqlx::query_scalar(
        "SELECT table_name::text FROM information_schema.tables \
         WHERE table_schema = 'public' AND table_name <> '_sqlx_migrations' \
         ORDER BY table_name",
    )
    .fetch_all(store.pool())
    .await
    .unwrap();
    assert_eq!(
        tables,
        [
            "changeset_parents",
            "changeset_paths",
            "changesets",
            "derived",
            "path_acl",
            "ref_log",
            "refs",
            "repo_blobs",
            "repos",
            "tokens",
        ]
    );

    exercise_schema(&store).await;
}

/// Smoke-test constraints and the CAS ref move inside a rolled-back
/// transaction, leaving the database as migrated.
async fn exercise_schema(store: &PgMetaStore) {
    let mut tx = store.pool().begin().await.unwrap();
    let repo: i64 = sqlx::query_scalar(
        "INSERT INTO repos (owner, name, visibility) VALUES ('et', 'demo', 'public') RETURNING id",
    )
    .fetch_one(&mut *tx)
    .await
    .unwrap();

    let a = vec![1u8; 32];
    let b = vec![2u8; 32];
    sqlx::query(
        "INSERT INTO changesets (repo_id, id, p1, gen, body) VALUES ($1, $2, NULL, 1, '{}')",
    )
    .bind(repo)
    .bind(&a)
    .execute(&mut *tx)
    .await
    .unwrap();
    sqlx::query("INSERT INTO changesets (repo_id, id, p1, gen, body) VALUES ($1, $2, $3, 2, '{}')")
        .bind(repo)
        .bind(&b)
        .bind(&a)
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query("INSERT INTO refs (repo_id, name, target) VALUES ($1, 'main', $2)")
        .bind(repo)
        .bind(&a)
        .execute(&mut *tx)
        .await
        .unwrap();

    let cas = "UPDATE refs SET target = $3, updated_at = now() \
               WHERE repo_id = $1 AND name = 'main' AND target = $2";
    let won = sqlx::query(cas)
        .bind(repo)
        .bind(&a)
        .bind(&b)
        .execute(&mut *tx)
        .await
        .unwrap();
    assert_eq!(won.rows_affected(), 1);
    let lost = sqlx::query(cas)
        .bind(repo)
        .bind(&a)
        .bind(&b)
        .execute(&mut *tx)
        .await
        .unwrap();
    assert_eq!(lost.rows_affected(), 0, "stale CAS must not move the ref");

    // A 31-byte id violates the length CHECK.
    sqlx::query("SAVEPOINT bad_id")
        .execute(&mut *tx)
        .await
        .unwrap();
    let bad =
        sqlx::query("INSERT INTO changesets (repo_id, id, gen, body) VALUES ($1, $2, 1, '{}')")
            .bind(repo)
            .bind(vec![3u8; 31])
            .execute(&mut *tx)
            .await;
    assert!(bad.is_err());
    sqlx::query("ROLLBACK TO SAVEPOINT bad_id")
        .execute(&mut *tx)
        .await
        .unwrap();

    tx.rollback().await.unwrap();
}
