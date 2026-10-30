//! Explicit, transactional import of a stopped `SQLite` snapshot into an empty `PostgreSQL` store.

use std::fs;
use std::path::{Path, PathBuf};

use futures_util::TryStreamExt;
use sha2::{Digest, Sha256};
use sqlx::any::AnyRow;
use sqlx::error::DatabaseError;
use sqlx::sqlite::{SqliteConnectOptions, SqliteRow};
use sqlx::{
    Any, ConnectOptions, Connection, Row, SqliteConnection, Transaction, TypeInfo, ValueRef,
};
use sych_core::Sha256Digest;

use crate::{ArtifactStore, Database, Partition, Storage, StorageError, StoreConfig};

mod schema;

use schema::{
    validate_integrity, validate_schema, validate_source_migrations, validate_target_migrations,
};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Text,
    Integer,
}

struct Column {
    name: String,
    kind: Kind,
    required: bool,
    primary_key: i64,
}

struct Table {
    name: &'static str,
    columns: Vec<Column>,
}

impl Table {
    fn column_list(&self) -> String {
        self.columns
            .iter()
            .map(|column| format!("\"{}\"", column.name))
            .collect::<Vec<_>>()
            .join(", ")
    }

    fn select(&self, postgres: bool) -> String {
        let mut keys: Vec<_> = self
            .columns
            .iter()
            .filter(|column| column.primary_key > 0)
            .collect();
        keys.sort_by_key(|column| column.primary_key);
        let order = keys
            .iter()
            .map(|column| match (column.kind, postgres) {
                (Kind::Text, true) => format!("\"{}\" COLLATE \"C\"", column.name),
                (Kind::Text, false) => format!("\"{}\" COLLATE BINARY", column.name),
                (Kind::Integer, _) => format!("\"{}\"", column.name),
            })
            .collect::<Vec<_>>()
            .join(", ");
        format!(
            "SELECT {} FROM \"{}\" ORDER BY {order}",
            self.column_list(),
            self.name
        )
    }

    fn insert(&self) -> String {
        let parameters = (1..=self.columns.len())
            .map(|index| format!("${index}"))
            .collect::<Vec<_>>()
            .join(", ");
        format!(
            "INSERT INTO \"{}\" ({}) VALUES ({parameters})",
            self.name,
            self.column_list()
        )
    }

    fn identity(&self) -> bool {
        self.columns.iter().any(|column| {
            column.name == "id" && column.kind == Kind::Integer && column.primary_key == 1
        })
    }
}

#[derive(Clone, Copy)]
enum Value<'a> {
    Null,
    Text(&'a str),
    Integer(i64),
}

struct Fingerprint {
    rows: u64,
    hash: Sha256,
}

#[derive(PartialEq, Eq)]
struct Report {
    rows: u64,
    digest: Sha256Digest,
}

impl Fingerprint {
    fn new(table: &Table) -> Self {
        let mut hash = Sha256::new();
        hash.update(b"sych-sqlite-import-v1\0");
        hash.update(table.name.as_bytes());
        for column in &table.columns {
            hash.update([0]);
            hash.update(column.name.as_bytes());
        }
        Self { rows: 0, hash }
    }

    fn row(&mut self) -> Result<(), StorageError> {
        self.rows = self
            .rows
            .checked_add(1)
            .ok_or_else(|| StorageError::Corrupt("SQLite import row count overflow".into()))?;
        self.hash.update([b'R']);
        Ok(())
    }

    // Tagged, length-framed values distinguish NULL, empty text, integers and row boundaries.
    fn value(&mut self, value: Value<'_>) {
        match value {
            Value::Null => self.hash.update([0]),
            Value::Text(text) => {
                self.hash.update([1]);
                self.hash.update((text.len() as u64).to_be_bytes());
                self.hash.update(text.as_bytes());
            }
            Value::Integer(integer) => {
                self.hash.update([2]);
                self.hash.update(integer.to_be_bytes());
            }
        }
    }

    fn finish(mut self) -> Report {
        self.hash.update(self.rows.to_be_bytes());
        Report {
            rows: self.rows,
            digest: Sha256Digest::from_bytes(self.hash.finalize().into()),
        }
    }
}

impl Storage {
    /// Imports a current, consistent `SQLite` snapshot without modifying it or its artifacts.
    ///
    /// Writers and artifact GC must remain stopped until the import and cutover finish. The
    /// target must already be migrated, and every application table must be empty. Neither
    /// database is migrated here; failures roll back the `PostgreSQL` import transaction.
    pub async fn import_sqlite(source: &Path, target: &StoreConfig) -> Result<(), StorageError> {
        import(source, target).await.map_err(redact_error)
    }
}

async fn import(source: &Path, target: &StoreConfig) -> Result<(), StorageError> {
    if !matches!(&target.database, Database::Postgres(_)) {
        return Err(StorageError::Conflict(
            "SQLite import requires a PostgreSQL target".into(),
        ));
    }

    if !fs::metadata(source)?.is_file() {
        return Err(StorageError::Corrupt(
            "SQLite import source is not a regular file".into(),
        ));
    }

    let options = SqliteConnectOptions::new()
        .filename(source)
        .read_only(true)
        .create_if_missing(false)
        .foreign_keys(true)
        .disable_statement_logging();
    let mut connection = SqliteConnection::connect_with(&options).await?;
    let mut snapshot = connection.begin().await?;
    validate_integrity(&mut snapshot).await?;
    let tables = validate_schema(&mut snapshot).await?;
    validate_source_migrations(&mut snapshot).await?;

    let storage = Storage::open(target).await?;
    let mut transaction = storage.write().await?;
    let copied = copy(&mut snapshot, &mut transaction, &tables, &storage.store).await;
    let reports = match copied {
        Ok(reports) => reports,
        Err(error) => {
            transaction.rollback().await?;
            return Err(error);
        }
    };
    // All fallible source work happens before the target commit.
    if let Err(error) = snapshot.rollback().await {
        transaction.rollback().await?;
        return Err(error.into());
    }

    if let Err(error) = connection.close().await {
        transaction.rollback().await?;
        return Err(error.into());
    }

    transaction.commit().await?;
    for (table, report) in tables.iter().zip(reports) {
        tracing::info!(
            table = table.name,
            count = report.rows,
            digest = %report.digest,
            "SQLite import committed"
        );
    }
    Ok(())
}

fn sqlite_value<'a>(
    row: &'a SqliteRow,
    index: usize,
    column: &Column,
) -> Result<Value<'a>, StorageError> {
    let raw = row.try_get_raw(index)?;
    if raw.is_null() {
        return if column.required {
            Err(StorageError::Corrupt(
                "SQLite import source contains NULL in a required column".into(),
            ))
        } else {
            Ok(Value::Null)
        };
    }

    match column.kind {
        Kind::Text if raw.type_info().name() == "TEXT" => {
            let text: &str = row.try_get(index)?;
            if text.contains('\0') {
                return Err(StorageError::Corrupt(
                    "SQLite import source contains text unsupported by PostgreSQL".into(),
                ));
            }

            Ok(Value::Text(text))
        }
        Kind::Integer if raw.type_info().name() == "INTEGER" => {
            Ok(Value::Integer(row.try_get(index)?))
        }
        _ => Err(StorageError::Corrupt(
            "SQLite import source contains an unsupported value type".into(),
        )),
    }
}

fn postgres_value<'a>(
    row: &'a AnyRow,
    index: usize,
    column: &Column,
) -> Result<Value<'a>, StorageError> {
    if row.try_get_raw(index)?.is_null() {
        return Ok(Value::Null);
    }

    match column.kind {
        Kind::Text => Ok(Value::Text(row.try_get(index)?)),
        Kind::Integer => Ok(Value::Integer(row.try_get(index)?)),
    }
}

async fn copy(
    source: &mut SqliteConnection,
    target: &mut Transaction<'_, Any>,
    tables: &[Table],
    artifacts: &ArtifactStore,
) -> Result<Vec<Report>, StorageError> {
    validate_target_migrations(target).await?;
    for table in tables {
        let sql = format!("SELECT COUNT(*) FROM \"{}\"", table.name);
        let count: i64 = sqlx::query(sqlx::AssertSqlSafe(sql))
            .fetch_one(&mut **target)
            .await?
            .try_get(0)?;
        if count != 0 {
            return Err(StorageError::Conflict(
                "PostgreSQL import target is not empty".into(),
            ));
        }
    }
    let mut reports = Vec::with_capacity(tables.len());
    let mut artifact_root = None;
    for table in tables {
        let insert_sql = table.insert();
        let mut fingerprint = Fingerprint::new(table);
        let mut rows = sqlx::query(sqlx::AssertSqlSafe(table.select(false))).fetch(&mut *source);
        while let Some(row) = rows.try_next().await? {
            if table.name == "artifacts" {
                validate_artifact(artifacts, &mut artifact_root, &row)?;
            }

            fingerprint.row()?;
            let mut insert = sqlx::query(sqlx::AssertSqlSafe(insert_sql.as_str()));
            for (index, column) in table.columns.iter().enumerate() {
                let value = sqlite_value(&row, index, column)?;
                fingerprint.value(value);
                if table.name == "findings" && column.name == "predecessor_id" {
                    insert = insert.bind(None::<i64>);
                    continue;
                }

                insert = match (value, column.kind) {
                    (Value::Text(text), _) => insert.bind(text),
                    (Value::Integer(integer), _) => insert.bind(integer),
                    (Value::Null, Kind::Text) => insert.bind(None::<String>),
                    (Value::Null, Kind::Integer) => insert.bind(None::<i64>),
                };
            }
            insert.execute(&mut **target).await?;
        }
        drop(rows);
        if table.name == "findings" {
            restore_predecessors(source, target).await?;
        }

        reports.push(fingerprint.finish());
    }
    for table in tables.iter().filter(|table| table.identity()) {
        reset_identity(target, table).await?;
    }
    for (table, expected) in tables.iter().zip(&reports) {
        let mut fingerprint = Fingerprint::new(table);
        let mut rows = sqlx::query(sqlx::AssertSqlSafe(table.select(true))).fetch(&mut **target);
        while let Some(row) = rows.try_next().await? {
            fingerprint.row()?;
            for (index, column) in table.columns.iter().enumerate() {
                fingerprint.value(postgres_value(&row, index, column)?);
            }
        }
        if fingerprint.finish() != *expected {
            return Err(StorageError::Corrupt(format!(
                "SQLite import count or digest differs for table {}",
                table.name
            )));
        }
    }
    Ok(reports)
}

async fn restore_predecessors(
    source: &mut SqliteConnection,
    target: &mut Transaction<'_, Any>,
) -> Result<(), StorageError> {
    let mut rows = sqlx::query(
        "SELECT id, predecessor_id FROM findings WHERE predecessor_id IS NOT NULL ORDER BY id",
    )
    .fetch(source);
    while let Some(row) = rows.try_next().await? {
        sqlx::query("UPDATE findings SET predecessor_id = $1 WHERE id = $2")
            .bind(row.try_get::<i64, _>(1)?)
            .bind(row.try_get::<i64, _>(0)?)
            .execute(&mut **target)
            .await?;
    }
    Ok(())
}

async fn reset_identity(
    target: &mut Transaction<'_, Any>,
    table: &Table,
) -> Result<(), StorageError> {
    let sql = format!("SELECT MAX(id) FROM \"{}\"", table.name);
    let maximum: Option<i64> = sqlx::query(sqlx::AssertSqlSafe(sql))
        .fetch_one(&mut **target)
        .await?
        .try_get(0)?;
    let next = match maximum {
        None => 1,
        Some(maximum) => maximum.checked_add(1).ok_or_else(|| {
            StorageError::Corrupt("SQLite import identity would overflow its range".into())
        })?,
    };
    if next < 1 {
        // SQLite permits negative rowids; retain max+1 without PostgreSQL's default minimum.
        let sql = format!(
            "ALTER TABLE \"{}\" ALTER COLUMN id SET MINVALUE {next}",
            table.name
        );
        sqlx::query(sqlx::AssertSqlSafe(sql))
            .execute(&mut **target)
            .await?;
    }

    let sql = format!(
        "ALTER TABLE \"{}\" ALTER COLUMN id RESTART WITH {next}",
        table.name
    );
    sqlx::query(sqlx::AssertSqlSafe(sql))
        .execute(&mut **target)
        .await?;
    Ok(())
}

fn validate_artifact(
    store: &ArtifactStore,
    root: &mut Option<PathBuf>,
    row: &SqliteRow,
) -> Result<(), StorageError> {
    let digest: Sha256Digest = row.try_get::<&str, _>("sha256")?.parse().map_err(|_| {
        StorageError::Corrupt("SQLite import has an invalid artifact digest".into())
    })?;
    let partition = Partition {
        tenant_id: row.try_get("tenant_id")?,
        access_scope: row.try_get("access_scope")?,
    };
    let key: &str = row.try_get("store_key")?;
    if key != ArtifactStore::key(&partition, &digest) {
        return Err(StorageError::Corrupt(
            "SQLite import has an unsafe or incorrect artifact key".into(),
        ));
    }

    let root = if let Some(root) = root {
        root
    } else {
        let canonical = fs::canonicalize(store.root())?;
        if !fs::metadata(&canonical)?.is_dir() {
            return Err(StorageError::Corrupt(
                "SQLite import artifact root is not a directory".into(),
            ));
        }

        root.insert(canonical)
    };
    let mut path = root.clone();
    for component in Path::new(key).components() {
        path.push(component);
        path = fs::canonicalize(path)?;
        if !path.starts_with(&*root) {
            return Err(StorageError::Corrupt(
                "SQLite import artifact path escapes its configured root".into(),
            ));
        }
    }
    let metadata = fs::metadata(&path)?;
    let size = row.try_get::<i64, _>("size")?;
    if !metadata.is_file() || u64::try_from(size).ok() != Some(metadata.len()) {
        return Err(StorageError::Corrupt(
            "SQLite import artifact path or recorded size is invalid".into(),
        ));
    }

    let bytes = store.get(key, &digest).map_err(|_| {
        StorageError::Corrupt("SQLite import artifact is missing or fails its hash check".into())
    })?;
    if u64::try_from(size).ok() != Some(bytes.len() as u64) {
        return Err(StorageError::Corrupt(
            "SQLite import artifact size changed during validation".into(),
        ));
    }

    Ok(())
}

fn redact_error(error: StorageError) -> StorageError {
    match error {
        // PostgreSQL error details and SQLite decode failures may contain source row values.
        StorageError::Db(error) => {
            let code = error.as_database_error().and_then(DatabaseError::code);
            let message = match code.filter(|code| {
                code.len() == 5
                    && code
                        .bytes()
                        .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit())
            }) {
                Some(code) => format!("SQLite import database operation failed (SQLSTATE {code})"),
                None => "SQLite import database operation failed".into(),
            };
            StorageError::Corrupt(message)
        }
        StorageError::Migrate(_) => {
            StorageError::Corrupt("SQLite import could not construct its trusted schema".into())
        }
        StorageError::Io(error) => StorageError::Corrupt(format!(
            "SQLite import cannot read its source or artifact files ({:?})",
            error.kind()
        )),
        other => other,
    }
}
