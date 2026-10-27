//! Opening the database: one write-ahead-logged connection per `Storage`, migrations and
//! the write-transaction helper every module uses.

use std::fs;
use std::path::Path;
use std::str::FromStr;
use std::time::Duration;

use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous};
use sqlx::{Row, Sqlite, SqlitePool, Transaction};

use crate::{ArtifactStore, MIGRATOR, Storage, StorageError, StoreConfig};

async fn pool(path: &Path, create: bool) -> Result<SqlitePool, StorageError> {
    let options = SqliteConnectOptions::from_str("sqlite:")?
        .filename(path)
        .create_if_missing(create)
        .journal_mode(SqliteJournalMode::Wal)
        .synchronous(SqliteSynchronous::Full)
        .foreign_keys(true)
        // Another process or pool may hold the write lock; wait instead of failing.
        .busy_timeout(Duration::from_secs(30));
    Ok(SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(options)
        .await?)
}

impl Storage {
    /// Applies pending migrations; the only operation that changes the schema.
    pub async fn migrate(config: &StoreConfig) -> Result<Vec<i64>, StorageError> {
        if let Some(dir) = config.database.parent() {
            fs::create_dir_all(dir)?;
        }

        let pool = pool(&config.database, true).await?;
        let before = applied(&pool).await?;
        MIGRATOR.run(&pool).await?;
        let after = applied(&pool).await?;
        pool.close().await;
        tracing::info!(database = %config.database.display(), applied = after.len() - before.len(), "database migrated");
        Ok(after.into_iter().filter(|v| !before.contains(v)).collect())
    }

    /// Opens a migrated database; never creates or changes the schema.
    pub async fn open(config: &StoreConfig) -> Result<Self, StorageError> {
        let name = config.database.display().to_string();
        if !config.database.exists() {
            return Err(StorageError::NotMigrated(name));
        }

        let pool = pool(&config.database, false).await?;
        let applied = applied(&pool).await?;
        let known: Vec<i64> = MIGRATOR.iter().map(|m| m.version).collect();
        if applied.iter().any(|v| !known.contains(v)) {
            return Err(StorageError::NewerSchema(name));
        }

        if known.iter().any(|v| !applied.contains(v)) {
            return Err(StorageError::NotMigrated(name));
        }

        Ok(Self {
            pool,
            store: ArtifactStore::new(&config.artifacts),
        })
    }

    pub(crate) fn pool(&self) -> &SqlitePool {
        &self.pool
    }

    /// Takes the write lock up front: a deferred transaction that reads and then writes
    /// gets `SQLITE_BUSY` at once when another connection wrote meanwhile, past `busy_timeout`.
    pub(crate) async fn write(&self) -> Result<Transaction<'static, Sqlite>, sqlx::Error> {
        self.pool.begin_with("BEGIN IMMEDIATE").await
    }
}

async fn applied(pool: &SqlitePool) -> Result<Vec<i64>, StorageError> {
    let exists = sqlx::query(
        "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = '_sqlx_migrations'",
    )
    .fetch_optional(pool)
    .await?
    .is_some();
    if !exists {
        return Ok(Vec::new());
    }

    let rows = sqlx::query("SELECT version FROM _sqlx_migrations WHERE success = 1")
        .fetch_all(pool)
        .await?;
    Ok(rows.iter().map(|r| r.get(0)).collect())
}
