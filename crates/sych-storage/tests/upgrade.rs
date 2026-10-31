//! Schema upgrades keep old data: a database at v2 seeded from
//! `fixtures/storage/v2-seed.sql` migrates to the current schema with its runs, pins and unknown
//! charges intact, and the rebuilt `runs` keeps its foreign keys consistent.

use std::borrow::Cow;
use std::fs;
use std::path::Path;
use std::str::FromStr;

use sqlx::sqlite::SqliteConnectOptions;
use sqlx::{Connection, Executor, Row, SqliteConnection};
use sych_core::RunId;
use sych_storage::{Database, Storage, StoreConfig};

#[tokio::test(flavor = "current_thread")]
async fn a_v2_database_upgrades_with_its_data() {
    let dir = tempfile::tempdir().expect("tempdir");
    let config = StoreConfig {
        database: Database::Sqlite(dir.path().join("sych.db")),
        artifacts: dir.path().join("artifacts"),
    };
    let options = SqliteConnectOptions::from_str("sqlite:")
        .expect("options")
        .filename(dir.path().join("sych.db"))
        .create_if_missing(true)
        .foreign_keys(true);
    let mut conn = SqliteConnection::connect_with(&options)
        .await
        .expect("connect");
    let mut old = sqlx::migrate!("../../migrations/sqlite");
    old.migrations = Cow::Owned(
        old.migrations
            .iter()
            .filter(|m| m.version <= 2)
            .cloned()
            .collect(),
    );
    old.run(&mut conn).await.expect("v2");
    let seed = fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/storage/v2-seed.sql"),
    )
    .expect("seed");
    conn.execute(sqlx::raw_sql(sqlx::AssertSqlSafe(seed)))
        .await
        .expect("seed");
    conn.close().await.expect("close");

    let applied = Storage::migrate(&config).await.expect("upgrade");
    assert_eq!(applied.first(), Some(&3), "everything after v2 applies");
    let storage = Storage::open(&config).await.expect("open");
    let run = RunId(
        "01a17470-0000-7000-8000-000000000001"
            .parse()
            .expect("uuid"),
    );
    assert_eq!(
        storage.run_state("local", run).await.expect("state"),
        "COMPLETED"
    );

    let mut conn = SqliteConnection::connect_with(&options)
        .await
        .expect("connect");
    let check: Vec<_> = sqlx::query("PRAGMA foreign_key_check")
        .fetch_all(&mut conn)
        .await
        .expect("fk");
    assert!(check.is_empty(), "foreign keys still hold");
    let unknown: i64 = sqlx::query("SELECT count(*) FROM model_attempts WHERE state = 'UNKNOWN'")
        .fetch_one(&mut conn)
        .await
        .expect("attempts")
        .get(0);
    assert_eq!(unknown, 1, "the unknown charge survives");
    let pins: i64 = sqlx::query("SELECT count(*) FROM run_artifacts")
        .fetch_one(&mut conn)
        .await
        .expect("pins")
        .get(0);
    assert_eq!(pins, 1);
}

/// A reconcile decision logged before audit rows named their repository learns it from its
/// publication operation; one whose operation is gone stays unscoped.
#[tokio::test(flavor = "current_thread")]
async fn audit_rows_from_before_their_repository_column_learn_it() {
    let dir = tempfile::tempdir().expect("tempdir");
    let config = StoreConfig {
        database: Database::Sqlite(dir.path().join("sych.db")),
        artifacts: dir.path().join("artifacts"),
    };
    let options = SqliteConnectOptions::from_str("sqlite:")
        .expect("options")
        .filename(dir.path().join("sych.db"))
        .create_if_missing(true)
        .foreign_keys(true);
    let mut conn = SqliteConnection::connect_with(&options)
        .await
        .expect("connect");
    let mut old = sqlx::migrate!("../../migrations/sqlite");
    old.migrations = Cow::Owned(
        old.migrations
            .iter()
            .filter(|m| m.version <= 11)
            .cloned()
            .collect(),
    );
    old.run(&mut conn).await.expect("v11");
    conn.execute(sqlx::raw_sql(
        "INSERT INTO repositories (id, tenant_id, connection_id, provider_instance, external_id,
             full_name) VALUES (41, 'local', 'gh', 'github.com', '42', 'o/r');
         INSERT INTO changes (id, tenant_id, access_scope, change_key, desired_generation,
             repository_id) VALUES (1, 'local', 'github.com/gh/42', 'pr:7', 1, 41);
         INSERT INTO runs (id, tenant_id, access_scope, change_id, generation, state, stage,
             created_at) VALUES ('run', 'local', 'github.com/gh/42', 1, 1, 'QUEUED', 'PREPARE', 0);
         INSERT INTO publication_objects (id, change_id, slot, kind, marker, desired_generation,
             desired_result_id, updated_at) VALUES (51, 1, 'summary', 'SUMMARY', 'm', 1, 'r', 0);
         INSERT INTO publication_operations (id, object_id, run_id, result_id, generation, state,
             next_action_at, created_at) VALUES (61, 51, 'run', 'r', 1, 'UNCERTAIN', 0, 0);
         INSERT INTO audit_events (id, tenant_id, actor, action, target, correlation_id,
             metadata, at) VALUES
             (1, 'local', 'ops', 'PUBLICATION_RECREATE_AUTHORIZED', 'publication_operation:61',
                 'c1', '{}', 0),
             (2, 'local', 'ops', 'PUBLICATION_RECREATE_AUTHORIZED', 'publication_operation:99',
                 'c2', '{}', 0);",
    ))
    .await
    .expect("seed");
    conn.close().await.expect("close");

    let applied = Storage::migrate(&config).await.expect("upgrade");
    assert_eq!(applied.first(), Some(&12));
    let mut conn = SqliteConnection::connect_with(&options)
        .await
        .expect("reconnect");
    let scoped: Vec<(i64, Option<i64>)> =
        sqlx::query_as("SELECT id, repository_id FROM audit_events ORDER BY id")
            .fetch_all(&mut conn)
            .await
            .expect("audit");
    assert_eq!(scoped, vec![(1, Some(41)), (2, None)]);
}
