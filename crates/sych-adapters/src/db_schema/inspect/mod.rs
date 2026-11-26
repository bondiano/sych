//! Read-only live database catalog introspection for PostgreSQL, MySQL and SQLite.

mod mysql;
mod postgres;
mod sqlite;

use std::time::Duration;

use sych_core::db_schema::{DEFAULT_INSPECT_TIMEOUT_SECONDS, DbSchemaSnapshot, SqlDialect};

use super::{DbSchemaError, detect_dialect};

/// Options controlling read-only live database inspection.
#[derive(Debug, Clone)]
pub struct InspectOptions {
    /// Maximum wall-clock duration for connecting and reading all metadata.
    pub timeout: Duration,
    /// Optional schema filter; empty inspects all non-system schemas (or current DB on MySQL).
    pub schemas: Vec<String>,
}

impl Default for InspectOptions {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(u64::from(DEFAULT_INSPECT_TIMEOUT_SECONDS)),
            schemas: Vec::new(),
        }
    }
}

/// Connects to `dsn` in strict read-only mode and returns a normalized [`DbSchemaSnapshot`].
///
/// Credentials in `dsn` are redacted before appearing in any returned error.
pub async fn inspect_dsn(
    dsn: &str,
    options: &InspectOptions,
) -> Result<DbSchemaSnapshot, DbSchemaError> {
    match detect_dialect(dsn)? {
        SqlDialect::Sqlite => sqlite::inspect(dsn, options).await,
        SqlDialect::Postgres => postgres::inspect(dsn, options).await,
        SqlDialect::Mysql => mysql::inspect(dsn, options).await,
    }
}
