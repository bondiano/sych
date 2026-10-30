//! Real `PostgreSQL` fixtures and independent, explicit import comparison metadata.
#![allow(dead_code)]

#[path = "import/tables.rs"]
pub mod tables;

use std::env;
use std::net::IpAddr;
use std::path::{self, Path};

use sqlx::any::{AnyPoolOptions, install_default_drivers};
use sqlx::{AnyPool, Connection, PgConnection};
use sych_core::{ReviewResult, sha256};
use sych_storage::{Database, RunHandle, Storage, StoreConfig};
use url::{Host, Url};

/// Owns one disposable database, never the administrator database supplied by the caller.
/// Keep all Storage handles dropped before calling `close`.
pub struct PgFixture {
    pub config: StoreConfig,
    pub pool: AnyPool,
    admin: PgConnection,
    name: String,
}

impl PgFixture {
    pub async fn new(artifacts: &Path) -> Self {
        let fixture = Self::unmigrated(artifacts).await;
        Storage::migrate(&fixture.config)
            .await
            .expect("migrate disposable PostgreSQL");
        fixture
    }

    pub async fn unmigrated(artifacts: &Path) -> Self {
        let raw = env::var("SYCH_TEST_POSTGRES_URL")
            .expect("set SYCH_TEST_POSTGRES_URL to the disposable local PostgreSQL admin database");
        let mut url = Url::parse(&raw).expect("valid disposable PostgreSQL URL");
        assert!(matches!(url.scheme(), "postgres" | "postgresql"));
        let local = match url.host() {
            Some(Host::Domain(host)) => {
                host == "localhost" || host.parse::<IpAddr>().is_ok_and(|ip| ip.is_loopback())
            }
            Some(Host::Ipv4(ip)) => ip.is_loopback(),
            Some(Host::Ipv6(ip)) => ip.is_loopback(),
            _ => false,
        };
        assert!(
            local,
            "PostgreSQL integration tests require a local disposable cluster"
        );
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
        let mut admin = PgConnection::connect(&raw)
            .await
            .expect("connect disposable administrator");
        let name = format!("sych_test_import_{}", uuid::Uuid::now_v7().simple());
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "CREATE DATABASE {name} TEMPLATE template0"
        )))
        .execute(&mut admin)
        .await
        .expect("create isolated test database");
        url.set_path(&format!("/{name}"));
        let config = StoreConfig {
            database: Database::Postgres(url.to_string()),
            artifacts: artifacts.to_path_buf(),
        };
        let pool = inspect(&config).await;
        Self {
            config,
            pool,
            admin,
            name,
        }
    }

    pub async fn close(mut self) {
        self.pool.close().await;
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "DROP DATABASE {} WITH (FORCE)",
            self.name
        )))
        .execute(&mut self.admin)
        .await
        .expect("drop owned test database");
        self.admin
            .close()
            .await
            .expect("close disposable administrator");
    }
}

pub async fn inspect(config: &StoreConfig) -> AnyPool {
    install_default_drivers();
    let sqlite = matches!(&config.database, Database::Sqlite(_));
    let url = match &config.database {
        Database::Postgres(url) => url.clone(),
        Database::Sqlite(path) => {
            let absolute = path::absolute(path).expect("absolute source path");
            let file = Url::from_file_path(absolute).expect("SQLite file URL");
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
        .expect("inspect owned test database")
}

pub async fn seed_rows(pool: &AnyPool, run: &RunHandle, result: &ReviewResult, now: i64) {
    // Storage creates the completed run and files; direct current-schema values cover every
    // remaining table and states which must survive without interpretation or recovery.
    sqlx::raw_sql(sqlx::AssertSqlSafe(format!(r#"
            INSERT INTO repositories VALUES (41, 'import-tenant', 'github', 'github.com', '42',
                'o/r', 9007199254740993, ' {{ "unicode": "π", "empty": "" }} ', NULL);
            UPDATE changes SET repository_id = 41, external_number = 7, scm_state = 'OPEN',
                head_sha = 'head', base_sha = NULL, base_ref = 'main', fork = 1, synced_at = {T0};
            UPDATE runs SET inputs = '{{"head_sha":"head","publish":false}}',
                inputs_digest = 'input-digest', expected_sources = '[]', lineage_applied = 1,
                input_tokens = 9007199254740993, unknown_obligations = NULL;
            INSERT INTO subscription_fees VALUES ('import-tenant', 'subscription', '2026-10',
                9223372036854775000, -9223372036854775000, {T0});
            INSERT INTO inbox_events VALUES (31, 'github.com', 'github', 'delivery',
                'pull_request', NULL, 'payload-digest', '{{ "unicode": "π" }}', {T0}, 'ACCEPTED');
            INSERT INTO audit_events VALUES (71, 'import-tenant', 'dev', 'review', 'o/r#7',
                'correlation', '{{"line":"first\\nsecond","quote":"it''s"}}', {T0});
            INSERT INTO idempotency_keys VALUES (121, 'import-tenant', 'dev', '/v1/reviews',
                'request-key', 'body-digest', NULL, NULL, {T0}, 1800003600);
            INSERT INTO tour_sessions VALUES ('import-tenant', 'tour', 'dev', '{run_id}',
                'IN_PROGRESS', '["step-1"]', {T0});
            INSERT INTO tour_exchanges VALUES ('exchange', 'import-tenant', 'tour', 'dev',
                '{run_id}', '{{"question":"Why π?","answer":""}}', {T0});
            INSERT INTO run_artifacts VALUES ('{run_id}', 'pruned', 'import-tenant', 'repo:o/r',
                '{pruned}');
            INSERT INTO model_attempts (id, tenant_id, run_id, purpose, kind, template, profile,
                provider, requested_model, resolved_model, generation_id, billing_kind, enforcement,
                day, month, reserved_micro_usd, input_tokens_bound, output_tokens_bound, state,
                charge_micro_usd, reference_micro_usd, input_tokens, output_tokens, reasoning_tokens,
                billing_mismatch, created_at, settled_at, scope)
            VALUES ('attempt-unknown', 'import-tenant', '{run_id}', 'analysis', 'review', 'template',
                'profile', 'subscription', 'requested', NULL, NULL, 'subscription_quota',
                'subscription_observable', '2026-10-30', '2026-10', 9223372036854775000,
                9007199254740993, NULL, 'UNKNOWN', NULL, -9223372036854775000, NULL, NULL, NULL,
                1, {T0}, NULL, NULL);
            INSERT INTO jobs VALUES (21, '{run_id}', 'review', 'analysis', 'CLAIMED', 'worker',
                9007199254740993, 1800000060, 1800003600, 1, 3, {T0}, NULL, {T0}, NULL);
            INSERT INTO change_events VALUES (22, 1, '{run_id}', 'COMPLETED', '{{}}', {T0});
            INSERT INTO change_intents VALUES (23, 1, 'REVIEW', {T0}, 1800000060,
                9007199254740993, 31, 31, {T0}, NULL);
            INSERT INTO publication_objects VALUES (51, 1, 'summary', 'SUMMARY', '<!-- sych -->',
                NULL, 2, '{result_id}', NULL, NULL, 'STALE', {T0}, NULL, NULL);
            INSERT INTO waivers VALUES (81, 'import-tenant', 41, 'rule', '["src/**"]', 'reason',
                'owner', 'author', NULL, 'next-policy', NULL, {T0}, NULL);
            INSERT INTO findings VALUES (909, 1, 'older-fingerprint', 'finding-old', 'BUG',
                NULL, NULL, 'RESOLVED', '{run_id}', '{run_id}', NULL, {T0});
            INSERT INTO findings VALUES (101, 1, 'newer-fingerprint', 'finding-new', 'BUG',
                'rule', 'src/π.rs', 'OPEN', '{run_id}', '{run_id}', 909, {T0});
            INSERT INTO publication_operations VALUES (61, 51, '{run_id}', '{result_id}', 1,
                'PUBLISHING', 'attempt-publishing', 'request-digest', '{{"text":"π"}}', 2,
                'worker', 9007199254740993, 1800000060, NULL, NULL, NULL, 1800003600,
                {T0}, {T0}, NULL);
            INSERT INTO publication_operations VALUES (62, 51, '{run_id}', '{result_id}', 2,
                'UNCERTAIN', 'attempt-uncertain', NULL, NULL, 1, NULL, 0, NULL, NULL,
                '{{"status":502}}', 'response lost', 1800003600, {T0}, {T0}, NULL);
            INSERT INTO finding_events VALUES (111, 101, '{run_id}', NULL, 'OPEN', 'introduced',
                NULL, '[{{"path":"src/π.rs"}}]', {T0});
            INSERT INTO feedback VALUES (91, 'import-tenant', 1, 101, 'dev', 'USEFUL',
                'finding', 'kept exactly, including π and trailing space ', NULL, {T0});
        "#, run_id = run.id, result_id = result.result_id, pruned = sha256(b"pruned"), T0 = now)))
            .execute(pool).await.expect("seed all 21 current-schema tables");
}
