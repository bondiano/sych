//! Schema and migration validation for the `SQLite` import source and `PostgreSQL` import target.

use futures_util::TryStreamExt;
use sqlx::sqlite::SqliteConnectOptions;
use sqlx::{Any, ConnectOptions, Connection, Row, SqliteConnection, Transaction};

use crate::{MIGRATOR, POSTGRES_MIGRATOR, StorageError};

use super::{Column, Kind, Table};

const TABLES: [&str; 22] = [
    "repositories",
    "changes",
    "runs",
    "artifacts",
    "subscription_fees",
    "inbox_events",
    "audit_events",
    "idempotency_keys",
    "sessions",
    "tour_sessions",
    "tour_exchanges",
    "run_artifacts",
    "model_attempts",
    "jobs",
    "change_events",
    "change_intents",
    "publication_objects",
    "waivers",
    "findings",
    "publication_operations",
    "finding_events",
    "feedback",
];

#[derive(PartialEq, Eq)]
struct Definition {
    position: i64,
    name: String,
    sql_type: String,
    not_null: i64,
    default: Option<String>,
    primary_key: i64,
    hidden: i64,
}

pub(super) async fn validate_integrity(source: &mut SqliteConnection) -> Result<(), StorageError> {
    let rows = sqlx::query("PRAGMA integrity_check")
        .fetch_all(&mut *source)
        .await?;
    if rows.len() != 1 || rows[0].try_get::<&str, _>(0)? != "ok" {
        return Err(StorageError::Corrupt(
            "SQLite import source fails its integrity check".into(),
        ));
    }

    if sqlx::query("PRAGMA foreign_key_check")
        .fetch_optional(&mut *source)
        .await?
        .is_some()
    {
        return Err(StorageError::Corrupt(
            "SQLite import source fails its foreign-key check".into(),
        ));
    }

    Ok(())
}

async fn table_names(source: &mut SqliteConnection) -> Result<Vec<String>, StorageError> {
    let rows = sqlx::query(
        "SELECT name FROM sqlite_schema WHERE type = 'table' ORDER BY name COLLATE BINARY",
    )
    .fetch_all(source)
    .await?;
    rows.iter()
        .map(|row| row.try_get(0).map_err(StorageError::from))
        .collect()
}

async fn definitions(
    connection: &mut SqliteConnection,
    table: &str,
) -> Result<Vec<Definition>, StorageError> {
    let sql = format!("PRAGMA table_xinfo(\"{table}\")");
    let rows = sqlx::query(sqlx::AssertSqlSafe(sql))
        .fetch_all(connection)
        .await?;
    rows.iter()
        .map(|row| {
            Ok(Definition {
                position: row.try_get("cid")?,
                name: row.try_get("name")?,
                sql_type: row.try_get("type")?,
                not_null: row.try_get("notnull")?,
                default: row.try_get("dflt_value")?,
                primary_key: row.try_get("pk")?,
                hidden: row.try_get("hidden")?,
            })
        })
        .collect()
}

pub(super) async fn validate_schema(
    source: &mut SqliteConnection,
) -> Result<Vec<Table>, StorageError> {
    let mut reference = SqliteConnection::connect_with(
        &SqliteConnectOptions::new()
            .in_memory(true)
            .disable_statement_logging(),
    )
    .await?;
    // Only this isolated in-memory reference is migrated, never the source or target.
    MIGRATOR.run(&mut reference).await?;
    let expected_names = table_names(&mut reference).await?;
    let mut known_names = TABLES.to_vec();
    known_names.push("_sqlx_migrations");
    known_names.sort_unstable();
    if expected_names != known_names || table_names(source).await? != expected_names {
        return Err(StorageError::Corrupt(
            "SQLite import source has missing or unknown tables".into(),
        ));
    }

    if definitions(source, "_sqlx_migrations").await?
        != definitions(&mut reference, "_sqlx_migrations").await?
    {
        return Err(StorageError::Corrupt(
            "SQLite import migration metadata has an unknown schema".into(),
        ));
    }

    let mut tables = Vec::with_capacity(TABLES.len());
    for name in TABLES {
        let expected = definitions(&mut reference, name).await?;
        if definitions(source, name).await? != expected {
            return Err(StorageError::Corrupt(format!(
                "SQLite import source table {name} does not match this build"
            )));
        }

        let columns = expected
            .into_iter()
            .map(|column| {
                let kind = match column.sql_type.as_str() {
                    "TEXT" => Kind::Text,
                    "INTEGER" => Kind::Integer,
                    _ => {
                        return Err(StorageError::Corrupt(
                            "SQLite import schema contains an unsupported type".into(),
                        ));
                    }
                };
                Ok(Column {
                    name: column.name,
                    kind,
                    required: column.not_null != 0 || column.primary_key > 0,
                    primary_key: column.primary_key,
                })
            })
            .collect::<Result<Vec<_>, StorageError>>()?;
        tables.push(Table { name, columns });
    }
    reference.close().await?;
    Ok(tables)
}

pub(super) async fn validate_source_migrations(
    source: &mut SqliteConnection,
) -> Result<(), StorageError> {
    let mut rows =
        sqlx::query("SELECT version, success, checksum FROM _sqlx_migrations ORDER BY version")
            .fetch(source);
    for migration in MIGRATOR.iter() {
        let row = rows
            .try_next()
            .await?
            .ok_or_else(|| StorageError::NotMigrated("SQLite import source".into()))?;
        if row.try_get::<i64, _>(0)? != migration.version
            || row.try_get::<i64, _>(1)? != 1
            || row.try_get::<Vec<u8>, _>(2)?.as_slice() != migration.checksum.as_ref()
        {
            return Err(StorageError::Corrupt(
                "SQLite import source migration history does not match this build".into(),
            ));
        }
    }
    if rows.try_next().await?.is_some() {
        return Err(StorageError::NewerSchema("SQLite import source".into()));
    }

    Ok(())
}

pub(super) async fn validate_target_migrations(
    target: &mut Transaction<'_, Any>,
) -> Result<(), StorageError> {
    let mut rows = sqlx::query(
        "SELECT version, CASE WHEN success THEN 1 ELSE 0 END, checksum
         FROM _sqlx_migrations ORDER BY version",
    )
    .fetch(&mut **target);
    for migration in POSTGRES_MIGRATOR.iter() {
        let row = rows
            .try_next()
            .await?
            .ok_or_else(|| StorageError::NotMigrated("PostgreSQL import target".into()))?;
        if row.try_get::<i64, _>(0)? != migration.version
            || row.try_get::<i64, _>(1)? != 1
            || row.try_get::<Vec<u8>, _>(2)?.as_slice() != migration.checksum.as_ref()
        {
            return Err(StorageError::Corrupt(
                "PostgreSQL import target migration history does not match this build".into(),
            ));
        }
    }
    if rows.try_next().await?.is_some() {
        return Err(StorageError::NewerSchema("PostgreSQL import target".into()));
    }

    Ok(())
}
