//! Backend selection and ownership shared by storage and application contract tests.
#![allow(dead_code)]

use std::env;
use std::net::IpAddr;
use std::path::{self, Path};
use std::thread;
use std::time::Duration;

use sqlx::any::{AnyPoolOptions, install_default_drivers};
use sqlx::{AnyPool, Connection, PgConnection};
use sych_storage::{Database, StoreConfig};
use tempfile::TempDir;
use tokio::runtime::Builder;
use tokio::time::timeout;
use url::{Host, Url};

/// Keep this owner alive until every handle using its selected configuration is finished.
pub struct TestDir {
    dir: TempDir,
    config: StoreConfig,
    postgres: Option<(String, String)>,
}

impl TestDir {
    pub async fn new() -> Self {
        let dir = tempfile::tempdir().expect("test directory");
        let admin = env::var_os("SYCH_TEST_POSTGRES_URL")
            .map(|value| value.into_string().expect("UTF-8 PostgreSQL test URL"));
        let (database, postgres) = if let Some(admin) = admin {
            let mut url = Url::parse(&admin).expect("valid disposable PostgreSQL URL");
            assert!(matches!(url.scheme(), "postgres" | "postgresql"));
            let local = match url.host() {
                Some(Host::Domain(host)) => {
                    host == "localhost" || host.parse::<IpAddr>().is_ok_and(|ip| ip.is_loopback())
                }
                Some(Host::Ipv4(ip)) => ip.is_loopback(),
                Some(Host::Ipv6(ip)) => ip.is_loopback(),
                _ => false,
            };
            assert!(local, "PostgreSQL tests require a local disposable cluster");
            assert_eq!(
                url.path(),
                "/postgres",
                "use the disposable postgres admin database"
            );
            assert!(url.fragment().is_none());
            assert!(
                url.query_pairs().all(|(key, _)| matches!(
                    key.as_ref(),
                    "sslmode" | "ssl-mode" | "application_name"
                )),
                "test connection parameters must not redirect the administrator connection"
            );
            let name = format!("sych_test_{}", uuid::Uuid::now_v7().simple());
            let mut connection = PgConnection::connect(&admin)
                .await
                .expect("connect disposable PostgreSQL administrator");
            // The identifier consists only of our fixed prefix and a generated UUID's hex digits.
            sqlx::query(sqlx::AssertSqlSafe(format!(
                "CREATE DATABASE {name} TEMPLATE template0"
            )))
            .execute(&mut connection)
            .await
            .expect("create isolated test database");
            connection.close().await.expect("close test administrator");
            url.set_path(&format!("/{name}"));
            (Database::Postgres(url.to_string()), Some((admin, name)))
        } else {
            (Database::Sqlite(dir.path().join("sych.db")), None)
        };
        let config = StoreConfig {
            database,
            artifacts: dir.path().join("artifacts"),
        };
        Self {
            dir,
            config,
            postgres,
        }
    }

    pub fn path(&self) -> &Path {
        self.dir.path()
    }

    pub fn config(&self) -> &StoreConfig {
        &self.config
    }
}

impl Drop for TestDir {
    fn drop(&mut self) {
        let Some((admin, name)) = self.postgres.take() else {
            return;
        };
        // A fresh runtime/connection can clean up even after the test's runtime has stopped.
        let cleaned = thread::spawn(move || {
            let runtime = Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("cleanup runtime");
            runtime.block_on(async {
                matches!(
                    timeout(Duration::from_secs(10), async {
                        let mut connection = PgConnection::connect(&admin).await?;
                        sqlx::query(sqlx::AssertSqlSafe(format!(
                            "DROP DATABASE {name} WITH (FORCE)"
                        )))
                        .execute(&mut connection)
                        .await?;
                        connection.close().await
                    })
                    .await,
                    Ok(Ok(()))
                )
            })
        })
        .join()
        .unwrap_or(false);
        if !cleaned {
            eprintln!("could not drop an owned disposable PostgreSQL test database");
        }
    }
}

pub async fn inspect(config: &StoreConfig) -> AnyPool {
    install_default_drivers();
    let sqlite = matches!(&config.database, Database::Sqlite(_));
    let url = match &config.database {
        Database::Postgres(url) => url.clone(),
        Database::Sqlite(path) => {
            let absolute = path::absolute(path).expect("absolute test database path");
            let file = Url::from_file_path(absolute).expect("SQLite test file URL");
            format!("sqlite://{}?mode=rw", file.path())
        }
    };
    AnyPoolOptions::new()
        .max_connections(1)
        .after_connect(move |connection, _| {
            Box::pin(async move {
                if sqlite {
                    sqlx::query("PRAGMA foreign_keys = ON")
                        .execute(connection)
                        .await?;
                }

                Ok(())
            })
        })
        .connect(&url)
        .await
        .expect("inspect selected test database")
}
