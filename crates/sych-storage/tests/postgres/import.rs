use std::fs;
#[cfg(unix)]
use std::os::unix::fs::symlink;

use serde_json::Value;
use sqlx::AnyPool;
use sych_core::{canonical_json, sha256};
use sych_storage::{Storage, StorageError, StoreConfig};

use super::import_support::PgFixture;
use super::import_support::tables::{IDENTITY_TABLES, TABLES, assert_empty, identities, rows};
use super::{Source, inspect};

async fn snapshot(pool: &AnyPool, postgres: bool) -> Vec<Vec<Vec<Value>>> {
    let mut snapshot = Vec::new();
    for table in TABLES {
        snapshot.push(rows(pool, postgres, table).await);
    }
    snapshot
}

async fn assert_refused(source: &Source, target: &StoreConfig, pool: &AnyPool) {
    let before_source = fs::read(source.path()).ok();
    let before_rows = snapshot(pool, true).await;
    let before_ids = identities(pool).await;
    assert!(
        Storage::import_sqlite(source.path(), target).await.is_err(),
        "import must refuse"
    );
    assert_eq!(
        snapshot(pool, true).await,
        before_rows,
        "refusal changes no target rows"
    );
    assert_eq!(
        identities(pool).await,
        before_ids,
        "refusal changes no identities"
    );
    assert_eq!(
        fs::read(source.path()).ok(),
        before_source,
        "source file is untouched"
    );
}

#[tokio::test]
#[ignore = "requires disposable local PostgreSQL 18.6 via SYCH_TEST_POSTGRES_URL; run explicitly"]
async fn imports_all_tables_files_predecessors_nulls_states_and_identities() {
    let source = Source::seeded().await;
    let pg = PgFixture::new(&source.config.artifacts).await;
    let sqlite = inspect(&source.config).await;
    let before_source = fs::read(source.path()).expect("source snapshot bytes");
    let before_file = fs::read(source.result_path()).expect("saved result bytes");
    let expected = snapshot(&sqlite, false).await;
    sqlite.close().await;
    assert_eq!(TABLES.len(), 22);
    assert_eq!(IDENTITY_TABLES.len(), 14);
    assert!(
        expected.iter().all(|rows| !rows.is_empty()),
        "all 22 source tables are populated"
    );

    Storage::import_sqlite(source.path(), &pg.config)
        .await
        .expect("import SQLite history");
    let actual = snapshot(&pg.pool, true).await;
    for ((table, expected), actual) in TABLES.iter().zip(&expected).zip(&actual) {
        assert_eq!(
            actual, expected,
            "{} ordered columns and PK values",
            table.name
        );
        assert_eq!(
            sha256(&canonical_json(actual).expect("target canonical rows")),
            sha256(&canonical_json(expected).expect("source canonical rows")),
            "{} canonical row digest",
            table.name
        );
    }
    let predecessor: i64 = sqlx::query_scalar("SELECT predecessor_id FROM findings WHERE id = 101")
        .fetch_one(&pg.pool)
        .await
        .expect("predecessor restored");
    assert_eq!(
        predecessor, 909,
        "sorted rows encounter the successor before its predecessor"
    );
    for table in IDENTITY_TABLES {
        let max: i64 =
            sqlx::query_scalar(sqlx::AssertSqlSafe(format!("SELECT MAX(id) FROM {table}")))
                .fetch_one(&pg.pool)
                .await
                .expect("imported maximum ID");
        let next: i64 = sqlx::query_scalar("SELECT nextval(pg_get_serial_sequence($1, 'id'))")
            .bind(*table)
            .fetch_one(&pg.pool)
            .await
            .expect("next generated identity");
        assert_eq!(next, max + 1, "{table} identity resumes without collision");
    }
    let storage = Storage::open(&pg.config)
        .await
        .expect("open imported database");
    assert_eq!(
        storage
            .load_result("import-tenant", source.run.id)
            .await
            .expect("saved result"),
        source.result
    );
    assert_eq!(
        before_file,
        canonical_json(&source.result).expect("canonical result bytes")
    );
    assert_eq!(
        fs::read(source.result_path()).expect("result bytes after import"),
        before_file
    );
    assert!(
        matches!(
            storage
                .load_artifact::<Value>("import-tenant", source.run.id, "pruned")
                .await,
            Err(StorageError::ArtifactUnavailable(_))
        ),
        "pruned metadata stays unavailable"
    );
    assert_eq!(
        fs::read(source.path()).expect("source after import"),
        before_source
    );
    drop(storage);
    pg.close().await;
}

#[tokio::test]
#[ignore = "requires disposable local PostgreSQL 18.6 via SYCH_TEST_POSTGRES_URL; run explicitly"]
async fn refuses_populated_and_repeated_targets_without_mutation() {
    let source = Source::seeded().await;
    let pg = PgFixture::new(&source.config.artifacts).await;
    // A table unrelated to runs/artifacts still prevents importing into an occupied target.
    sqlx::query(
        "INSERT INTO tour_exchanges VALUES ('existing', 'tenant', 'tour', 'actor', 'run', '{}', 1)",
    )
    .execute(&pg.pool)
    .await
    .expect("populate otherwise empty target");
    assert_refused(&source, &pg.config, &pg.pool).await;
    sqlx::query("DELETE FROM tour_exchanges")
        .execute(&pg.pool)
        .await
        .expect("remove test occupant");
    Storage::import_sqlite(source.path(), &pg.config)
        .await
        .expect("first import");
    assert_refused(&source, &pg.config, &pg.pool).await;
    pg.close().await;
}

#[tokio::test]
#[ignore = "requires disposable local PostgreSQL 18.6 via SYCH_TEST_POSTGRES_URL; run explicitly"]
async fn requires_postgres_and_explicit_target_migration() {
    let source = Source::seeded().await;
    let before_source = fs::read(source.path()).expect("source bytes");
    assert!(
        Storage::import_sqlite(source.path(), &source.config)
            .await
            .is_err()
    );
    assert_eq!(
        fs::read(source.path()).expect("source still present"),
        before_source
    );
    let pg = PgFixture::unmigrated(&source.config.artifacts).await;
    assert!(
        Storage::import_sqlite(source.path(), &pg.config)
            .await
            .is_err()
    );
    let tables: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM information_schema.tables WHERE table_schema = 'public'",
    )
    .fetch_one(&pg.pool)
    .await
    .expect("unmigrated catalog");
    assert_eq!(tables, 0, "import never migrates the target implicitly");
    assert_eq!(
        fs::read(source.path()).expect("source after refusal"),
        before_source
    );
    pg.close().await;
}

#[tokio::test]
#[ignore = "requires disposable local PostgreSQL 18.6 via SYCH_TEST_POSTGRES_URL; run explicitly"]
async fn refuses_missing_invalid_noncurrent_unknown_and_inconsistent_sources() {
    let dir = tempfile::tempdir().expect("target artifacts");
    let pg = PgFixture::new(dir.path()).await;
    for mutation in [
        "missing",
        "not-sqlite",
        "unmigrated",
        "older",
        "newer",
        "checksum",
        "unknown-table",
        "unknown-column",
        "missing-table",
        "missing-column",
        "foreign-key",
        "integrity",
        "blob-value",
        "real-value",
    ] {
        let source = Source::seeded().await;
        if mutation == "missing" {
            fs::remove_file(source.path()).expect("remove source snapshot");
        } else if mutation == "not-sqlite" {
            fs::write(source.path(), b"not a SQLite database").expect("invalid source file");
        } else {
            let pool = inspect(&source.config).await;
            let sql = match mutation {
                "unmigrated" => "DELETE FROM _sqlx_migrations",
                "older" => "DELETE FROM _sqlx_migrations WHERE version = 10",
                "newer" => {
                    "INSERT INTO _sqlx_migrations (version, description, installed_on, success, checksum, execution_time) SELECT version + 1000, description, installed_on, success, checksum, execution_time FROM _sqlx_migrations WHERE version = 10"
                }
                "checksum" => {
                    "UPDATE _sqlx_migrations SET checksum = zeroblob(length(checksum)) WHERE version = 10"
                }
                "unknown-table" => "CREATE TABLE unknown_extension (id INTEGER PRIMARY KEY)",
                "unknown-column" => "ALTER TABLE inbox_events ADD COLUMN unreviewed TEXT",
                "missing-table" => "DROP TABLE feedback",
                "missing-column" => "ALTER TABLE tour_exchanges DROP COLUMN body",
                "foreign-key" => {
                    "PRAGMA foreign_keys = OFF; UPDATE feedback SET finding_id = 999999; PRAGMA foreign_keys = ON;"
                }
                "integrity" => {
                    "PRAGMA ignore_check_constraints = ON; UPDATE jobs SET state = 'BROKEN'; PRAGMA ignore_check_constraints = OFF;"
                }
                "blob-value" => "UPDATE inbox_events SET normalized = X'00FF'",
                "real-value" => "UPDATE inbox_events SET received_at = 1.5",
                _ => unreachable!("known mutation"),
            };
            sqlx::raw_sql(sql)
                .execute(&pool)
                .await
                .expect("prepare rejected source");
            sqlx::query("PRAGMA wal_checkpoint(TRUNCATE)")
                .execute(&pool)
                .await
                .expect("checkpoint changed source");
            pool.close().await;
        }

        assert_refused(&source, &source.target(&pg), &pg.pool).await;
        assert_empty(&pg.pool).await;
    }
    pg.close().await;
}

#[tokio::test]
#[ignore = "requires disposable local PostgreSQL 18.6 via SYCH_TEST_POSTGRES_URL; run explicitly"]
async fn refuses_missing_corrupt_wrong_size_and_unsafe_artifacts() {
    let dir = tempfile::tempdir().expect("target artifacts");
    let pg = PgFixture::new(dir.path()).await;
    for mutation in [
        "missing",
        "same-size-corruption",
        "wrong-size",
        "unsafe-key",
    ] {
        let source = Source::seeded().await;
        match mutation {
            "missing" => fs::remove_file(source.result_path()).expect("remove result file"),
            "same-size-corruption" => {
                let mut bytes = fs::read(source.result_path()).expect("original result bytes");
                bytes[0] ^= 1;
                fs::write(source.result_path(), bytes).expect("corrupt without changing size");
            }
            "wrong-size" | "unsafe-key" => {
                let pool = inspect(&source.config).await;
                let sql = if mutation == "wrong-size" {
                    "UPDATE artifacts SET size = size + 1 WHERE sha256 = $1"
                } else {
                    let outside = source
                        .config
                        .artifacts
                        .parent()
                        .expect("artifact root parent")
                        .join("outside");
                    fs::write(
                        outside,
                        canonical_json(&source.result).expect("valid outside bytes"),
                    )
                    .expect("unsafe path resolves to intact bytes");
                    "UPDATE artifacts SET store_key = '../outside' WHERE sha256 = $1"
                };
                sqlx::query(sql)
                    .bind(source.digest.to_string())
                    .execute(&pool)
                    .await
                    .expect("unsafe artifact metadata");
                sqlx::query("PRAGMA wal_checkpoint(TRUNCATE)")
                    .execute(&pool)
                    .await
                    .expect("checkpoint artifact metadata");
                pool.close().await;
            }
            _ => unreachable!("known mutation"),
        }
        assert_refused(&source, &source.target(&pg), &pg.pool).await;
        assert_empty(&pg.pool).await;
    }
    pg.close().await;
}

#[cfg(unix)]
#[tokio::test]
#[ignore = "requires disposable local PostgreSQL 18.6 via SYCH_TEST_POSTGRES_URL; run explicitly"]
async fn refuses_artifact_symlinks_escaping_the_configured_root() {
    let dir = tempfile::tempdir().expect("target artifacts");
    let pg = PgFixture::new(dir.path()).await;
    for ancestor in [false, true] {
        let source = Source::seeded().await;
        let outside = tempfile::tempdir().expect("outside configured artifact root");
        let path = source.result_path();
        let link = if ancestor {
            path.parent().expect("artifact prefix").to_path_buf()
        } else {
            path
        };
        let destination = outside.path().join("original");
        fs::rename(&link, &destination).expect("move intact artifact outside root");
        symlink(&destination, &link).expect("escaping symlink");
        // The digest still matches: refusal must be containment, not incidental missing bytes.
        assert_eq!(
            fs::read(source.result_path()).expect("symlink resolves"),
            canonical_json(&source.result).expect("original result")
        );
        assert_refused(&source, &source.target(&pg), &pg.pool).await;
        assert_empty(&pg.pool).await;
    }
    pg.close().await;
}

#[cfg(unix)]
#[tokio::test]
#[ignore = "requires disposable local PostgreSQL 18.6 via SYCH_TEST_POSTGRES_URL; run explicitly"]
async fn permits_artifact_symlinks_resolving_inside_the_configured_root() {
    let source = Source::seeded().await;
    let pg = PgFixture::new(&source.config.artifacts).await;
    let path = source.result_path();
    let destination = source.config.artifacts.join("retained-result");
    fs::rename(&path, &destination).expect("move result within artifact root");
    symlink(&destination, &path).expect("safe in-root symlink");
    Storage::import_sqlite(source.path(), &pg.config)
        .await
        .expect("safe in-root artifact link");
    let storage = Storage::open(&pg.config)
        .await
        .expect("open imported safe link");
    assert_eq!(
        storage
            .load_result("import-tenant", source.run.id)
            .await
            .expect("linked saved result"),
        source.result
    );
    drop(storage);
    pg.close().await;
}

#[tokio::test]
#[ignore = "requires disposable local PostgreSQL 18.6 via SYCH_TEST_POSTGRES_URL; run explicitly"]
async fn late_copy_failure_rolls_back_all_rows_and_all_identity_state() {
    let source = Source::seeded().await;
    let pg = PgFixture::new(&source.config.artifacts).await;
    for (index, table) in IDENTITY_TABLES.iter().enumerate() {
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "ALTER TABLE {table} ALTER COLUMN id RESTART WITH {}",
            8000 + index
        )))
        .execute(&pg.pool)
        .await
        .expect("nondefault pre-import identity");
    }
    let before_ids = identities(&pg.pool).await;
    let guards = TABLES.iter().filter(|table| table.name != "feedback").map(|table| format!(
        "IF NOT EXISTS (SELECT 1 FROM {}) THEN RAISE EXCEPTION 'expected {} before feedback'; END IF;",
        table.name, table.name,
    )).collect::<Vec<_>>().join("\n");
    sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
        r"
        CREATE FUNCTION reject_import_feedback() RETURNS trigger LANGUAGE plpgsql AS $$
        BEGIN
            {guards}
            RAISE EXCEPTION 'injected late-copy failure' USING ERRCODE = 'ZSY01';
        END;
        $$;
        CREATE TRIGGER import_failure BEFORE INSERT ON feedback
            FOR EACH ROW EXECUTE FUNCTION reject_import_feedback();
    "
    )))
    .execute(&pg.pool)
    .await
    .expect("late failure trigger");
    let before_source = fs::read(source.path()).expect("source before failure");
    let before_result = fs::read(source.result_path()).expect("result before failure");
    let error = Storage::import_sqlite(source.path(), &pg.config)
        .await
        .expect_err("late import must fail");
    assert!(
        error.to_string().contains("ZSY01"),
        "must reach final table's distinct SQLSTATE, got {error}"
    );
    assert_empty(&pg.pool).await;
    assert_eq!(
        identities(&pg.pool).await,
        before_ids,
        "all 14 identities roll back exactly"
    );
    assert_eq!(
        fs::read(source.path()).expect("source after failure"),
        before_source
    );
    assert_eq!(
        fs::read(source.result_path()).expect("result after failure"),
        before_result
    );
    sqlx::query("DROP TRIGGER import_failure ON feedback")
        .execute(&pg.pool)
        .await
        .expect("remove test fault");
    Storage::import_sqlite(source.path(), &pg.config)
        .await
        .expect("same snapshot imports after fault removed");
    pg.close().await;
}

#[tokio::test]
#[ignore = "requires disposable local PostgreSQL 18.6 via SYCH_TEST_POSTGRES_URL; run explicitly"]
async fn identity_overflow_after_prior_restarts_rolls_back_rows_and_sequences() {
    let source = Source::seeded().await;
    let sqlite = inspect(&source.config).await;
    sqlx::query("UPDATE feedback SET id = $1")
        .bind(i64::MAX)
        .execute(&sqlite)
        .await
        .expect("last identity cannot advance");
    sqlite.close().await;
    let pg = PgFixture::new(&source.config.artifacts).await;
    for (index, table) in IDENTITY_TABLES.iter().enumerate() {
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "ALTER TABLE {table} ALTER COLUMN id RESTART WITH {}",
            8000 + index
        )))
        .execute(&pg.pool)
        .await
        .expect("nondefault identity state");
    }
    let before = identities(&pg.pool).await;
    assert!(matches!(
        Storage::import_sqlite(source.path(), &pg.config).await,
        Err(StorageError::Corrupt(_))
    ));
    assert_empty(&pg.pool).await;
    assert_eq!(identities(&pg.pool).await, before);
    pg.close().await;
}
