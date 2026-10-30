//! Backend connections, explicit migrations and the shared single-writer transaction.

use std::fs;
use std::io;
use std::path;

use sqlx::any::{AnyConnectOptions, AnyPoolOptions, install_default_drivers};
use sqlx::{Any, AnyPool, Row, Transaction};
use url::Url;

use crate::{
    ArtifactStore, Database, MIGRATOR, POSTGRES_MIGRATOR, Storage, StorageError, StoreConfig,
};

fn connection_options(
    database: &Database,
    create: bool,
) -> Result<AnyConnectOptions, StorageError> {
    match database {
        Database::Sqlite(path) => {
            let file = Url::from_file_path(path::absolute(path)?).map_err(|()| {
                io::Error::new(io::ErrorKind::InvalidInput, "invalid SQLite database path")
            })?;
            let mode = if create { "rwc" } else { "rw" };
            Ok(format!("sqlite://{}?mode={mode}", file.path()).parse()?)
        }
        Database::Postgres(url) => {
            let options: AnyConnectOptions = url.parse().map_err(postgres_connect_error)?;
            if !matches!(options.database_url.scheme(), "postgres" | "postgresql")
                || options.database_url.host_str().is_none_or(str::is_empty)
                || options.database_url.fragment().is_some()
            {
                return Err(
                    sqlx::Error::Configuration("invalid PostgreSQL database URL".into()).into(),
                );
            }

            // SQLx logs unknown parameter values, which may contain credentials.
            for (key, _) in options.database_url.query_pairs() {
                if !(matches!(
                    key.as_ref(),
                    "sslmode"
                        | "ssl-mode"
                        | "sslrootcert"
                        | "ssl-root-cert"
                        | "ssl-ca"
                        | "sslcert"
                        | "ssl-cert"
                        | "sslkey"
                        | "ssl-key"
                        | "statement-cache-capacity"
                        | "host"
                        | "hostaddr"
                        | "port"
                        | "dbname"
                        | "user"
                        | "password"
                        | "application_name"
                        | "options"
                ) || (key.starts_with("options[") && key.ends_with(']')))
                {
                    return Err(sqlx::Error::Configuration(
                        "unsupported PostgreSQL connection parameter".into(),
                    )
                    .into());
                }
            }
            Ok(options)
        }
    }
}

fn postgres_connect_error(error: sqlx::Error) -> sqlx::Error {
    let message = match error {
        sqlx::Error::Database(error) => format!(
            "PostgreSQL connection rejected (SQLSTATE {})",
            error.code().as_deref().unwrap_or("unknown")
        ),
        sqlx::Error::Io(error) => {
            format!("PostgreSQL connection I/O failed ({:?})", error.kind())
        }
        sqlx::Error::Tls(_) => "PostgreSQL TLS connection failed".into(),
        sqlx::Error::Configuration(_) => "invalid PostgreSQL connection options".into(),
        sqlx::Error::PoolTimedOut => "PostgreSQL connection timed out".into(),
        _ => "PostgreSQL connection failed".into(),
    };
    sqlx::Error::Configuration(message.into())
}

async fn pool(database: &Database, create: bool) -> Result<AnyPool, StorageError> {
    install_default_drivers();
    let postgres = matches!(database, Database::Postgres(_));
    let options = connection_options(database, create)?;
    AnyPoolOptions::new()
        .max_connections(1)
        .after_connect(move |connection, _| {
            Box::pin(async move {
                if !postgres {
                    sqlx::raw_sql(
                        "PRAGMA busy_timeout = 30000;
                         PRAGMA journal_mode = WAL;
                         PRAGMA synchronous = FULL;
                         PRAGMA foreign_keys = ON;",
                    )
                    .execute(connection)
                    .await?;
                }

                Ok(())
            })
        })
        .connect_with(options)
        .await
        .map_err(|error| {
            if postgres {
                postgres_connect_error(error)
            } else {
                error
            }
        })
        .map_err(StorageError::from)
}

impl Storage {
    /// Applies pending migrations; the only operation that creates a database file or schema.
    pub async fn migrate(config: &StoreConfig) -> Result<Vec<i64>, StorageError> {
        if let Database::Sqlite(path) = &config.database
            && let Some(dir) = path.parent()
        {
            fs::create_dir_all(dir)?;
        }

        let postgres = matches!(&config.database, Database::Postgres(_));
        let migrator = if postgres {
            &POSTGRES_MIGRATOR
        } else {
            &MIGRATOR
        };
        let pool = pool(&config.database, true).await?;
        let before = applied(&pool, postgres).await?;
        if before.iter().any(|version| {
            !migrator
                .iter()
                .any(|migration| migration.version == *version)
        }) {
            pool.close().await;
            return Err(StorageError::NewerSchema(config.database.to_string()));
        }

        migrator.run(&pool).await?;
        let after = applied(&pool, postgres).await?;
        pool.close().await;
        tracing::info!(database = %config.database, applied = after.len() - before.len(), "database migrated");
        Ok(after.into_iter().filter(|v| !before.contains(v)).collect())
    }

    /// Opens a migrated database; never creates a database file or changes the schema.
    pub async fn open(config: &StoreConfig) -> Result<Self, StorageError> {
        let name = config.database.to_string();
        if let Database::Sqlite(path) = &config.database
            && !path.exists()
        {
            return Err(StorageError::NotMigrated(name));
        }

        let postgres = matches!(&config.database, Database::Postgres(_));
        let pool = pool(&config.database, false).await?;
        let applied = applied(&pool, postgres).await?;
        let migrator = if postgres {
            &POSTGRES_MIGRATOR
        } else {
            &MIGRATOR
        };
        if applied.iter().any(|version| {
            !migrator
                .iter()
                .any(|migration| migration.version == *version)
        }) {
            pool.close().await;
            return Err(StorageError::NewerSchema(name));
        }

        if migrator
            .iter()
            .any(|migration| !applied.contains(&migration.version))
        {
            pool.close().await;
            return Err(StorageError::NotMigrated(name));
        }

        Ok(Self {
            pool,
            store: ArtifactStore::new(&config.artifacts),
            postgres,
        })
    }

    pub(crate) fn pool(&self) -> &AnyPool {
        &self.pool
    }

    /// Serializes writes before business reads, including across independent pools.
    pub(crate) async fn write(&self) -> Result<Transaction<'static, Any>, sqlx::Error> {
        let mut tx = self
            .pool
            .begin_with(if self.postgres {
                "BEGIN ISOLATION LEVEL READ COMMITTED"
            } else {
                "BEGIN IMMEDIATE"
            })
            .await?;
        if self.postgres {
            sqlx::query("SET LOCAL lock_timeout = '30s'")
                .execute(&mut *tx)
                .await?;
            // A separate statement gives business reads a fresh snapshot after the wait.
            sqlx::query("SELECT pg_advisory_xact_lock(1937335144)")
                .execute(&mut *tx)
                .await?;
        }

        Ok(tx)
    }
}

async fn applied(pool: &AnyPool, postgres: bool) -> Result<Vec<i64>, StorageError> {
    let catalog = if postgres {
        "SELECT 1 FROM pg_catalog.pg_class
         WHERE oid = pg_catalog.to_regclass('_sqlx_migrations') AND relkind = 'r'"
    } else {
        "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = '_sqlx_migrations'"
    };
    if sqlx::query(catalog).fetch_optional(pool).await?.is_none() {
        return Ok(Vec::new());
    }

    let versions = if postgres {
        "SELECT version FROM _sqlx_migrations WHERE success = TRUE ORDER BY version"
    } else {
        "SELECT version FROM _sqlx_migrations WHERE success = 1 ORDER BY version"
    };
    let rows = sqlx::query(versions).fetch_all(pool).await?;
    Ok(rows.iter().map(|row| row.get(0)).collect())
}

#[cfg(test)]
mod tests {
    use sqlx::sqlite::SqliteConnectOptions;

    use super::{connection_options, postgres_connect_error};
    use crate::{Database, StoreConfig};

    #[test]
    fn sqlite_url_preserves_reserved_path_characters_without_creating_a_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("storage %?# café.db");
        for (create, mode) in [(false, "rw"), (true, "rwc")] {
            let options = connection_options(&Database::Sqlite(path.clone()), create)
                .expect("SQLite options");
            let native = SqliteConnectOptions::try_from(&options).expect("native options");
            assert_eq!(native.get_filename(), path);
            assert!(
                options
                    .database_url
                    .query_pairs()
                    .any(|(key, value)| key == "mode" && value == mode)
            );
        }
        assert!(!path.exists());
    }

    #[test]
    fn postgres_options_reject_other_drivers_and_unlogged_parameters() {
        for url in [
            "sqlite:///tmp/not-postgres.db",
            "postgres://user:secret@localhost/sych?unknown=secret",
            "postgres://user:secret@localhost/sych#secret",
            "postgres://user:secret@",
        ] {
            let error = connection_options(&Database::Postgres(url.into()), false)
                .expect_err("invalid PostgreSQL options");
            assert!(!error.to_string().contains("secret"));
            assert!(!format!("{error:?}").contains("secret"));
        }
        for scheme in ["postgres", "postgresql"] {
            let url = format!(
                "{scheme}://user:secret@localhost/sych?sslmode=require&options[search_path]=public"
            );
            connection_options(&Database::Postgres(url), false).expect("PostgreSQL options");
        }
    }

    #[test]
    fn postgres_connection_errors_discard_secret_bearing_sources() {
        for error in [
            sqlx::Error::Configuration("postgres://user:secret@localhost/sych".into()),
            sqlx::Error::Protocol("secret".into()),
            sqlx::Error::Tls("secret".into()),
        ] {
            let error = postgres_connect_error(error);
            assert!(!error.to_string().contains("secret"));
            assert!(!format!("{error:?}").contains("secret"));
        }
    }

    #[test]
    fn database_formatting_never_exposes_postgres_connection_details() {
        for url in [
            "postgres://user:secret@private-host/db?password=other-secret",
            "postgresql://user:secret@private-host/db",
        ] {
            let database = Database::Postgres(url.into());
            let displayed = database.to_string();
            let debugged = format!("{database:?}");
            let config = StoreConfig {
                database,
                artifacts: "artifacts".into(),
            };
            let configured = format!("{config:?}");
            for output in [displayed, debugged, configured] {
                for detail in ["user", "secret", "private-host", "/db", "other-secret"] {
                    assert!(!output.contains(detail), "{output}");
                }
            }
        }
    }
}
