//! Integration tests for database schema introspection, static SQL DDL
//! reconstruction, all 9 built-in static ORM extractors, CI artifact import
//! and Git-snapshot review fact collection.

use std::env;
use std::fs;
use std::path::Path;
use std::process::Command;
use std::str::FromStr;

use sqlx::sqlite::SqliteConnectOptions;
use sqlx::{ConnectOptions, Connection, Executor};
use sych_adapters::ci;
use sych_adapters::db_schema::collect::{ReviewCollectOptions, collect_review_facts};
use sych_adapters::db_schema::extract::SourceTree;
use sych_adapters::db_schema::inspect::{InspectOptions, inspect_dsn};
use sych_adapters::db_schema::sql_parse::{SqlFile, discover_sql_targets, parse_sql_files};
use sych_adapters::db_schema::{detect_dialect, redact_dsn};
use sych_adapters::git::{LocalSnapshot, SnapshotRequest};
use sych_core::db_schema::rules::{
    DUPLICATE_INDEX, ENUM_NOT_ENFORCED, FK_WITHOUT_INDEX, MAX_LENGTH_NOT_ENFORCED,
    NULLABLE_BOOLEAN, PLAIN_TABLES, PREFIX_REDUNDANT_INDEX, RELATION_WITHOUT_FOREIGN_KEY,
    REQUIRED_FIELD_NULLABLE_COLUMN, TABLE_WITHOUT_PRIMARY_KEY, UNIQUE_FIELD_WITHOUT_UNIQUE_INDEX,
    evaluate,
};
use sych_core::db_schema::{
    DbColumnType, DbSchemaChange, DbSchemaConfig, DbSchemaRuleConfig, SchemaLintSettings,
    SqlDialect,
};
use sych_core::{BackendStatus, CiFormat, SourceSide};

#[test]
fn dsn_redaction_and_dialect_detection() {
    let pg = redact_dsn(
        "postgres://alice:supersecret@db.internal:5432/app?sslmode=require&password=abc",
    );
    assert!(!pg.contains("supersecret"), "{pg}");
    assert!(!pg.contains("abc"), "{pg}");
    assert!(pg.contains("REDACTED"), "{pg}");
    assert!(pg.contains("db.internal:5432/app"), "{pg}");

    let kv = redact_dsn("host=localhost user=app password=mysecret dbname=prod");
    assert!(!kv.contains("mysecret"), "{kv}");
    assert!(kv.contains("password=REDACTED"), "{kv}");

    assert_eq!(
        detect_dialect("postgres://u:p@h/db").expect("pg"),
        SqlDialect::Postgres
    );
    assert_eq!(
        detect_dialect("postgresql://u:p@h/db").expect("pg2"),
        SqlDialect::Postgres
    );
    assert_eq!(
        detect_dialect("mysql://u:p@h/db").expect("mysql"),
        SqlDialect::Mysql
    );
    assert_eq!(
        detect_dialect("sqlite:///tmp/a.db").expect("sqlite"),
        SqlDialect::Sqlite
    );
    assert_eq!(
        detect_dialect("/tmp/app.sqlite3").expect("sqlite path"),
        SqlDialect::Sqlite
    );
    assert!(detect_dialect("oracle://u:p@h/db").is_err());
}

#[tokio::test]
async fn live_sqlite_introspection_is_read_only_and_detects_all_structural_rules() {
    let dir = tempfile::tempdir().expect("tempdir");
    let db_path = dir.path().join("app.sqlite");
    let dsn = format!("sqlite://{}", db_path.display());

    // Non-existent file must fail without creating it.
    assert!(inspect_dsn(&dsn, &InspectOptions::default()).await.is_err());
    assert!(
        !db_path.exists(),
        "read-only inspector must never create a DB file"
    );

    // Seed SQLite schema covering every structural rule.
    let mut conn = SqliteConnectOptions::from_str(&dsn)
        .expect("opts")
        .create_if_missing(true)
        .connect()
        .await
        .expect("connect");
    conn.execute(
        r"
        CREATE TABLE users (
            id INTEGER PRIMARY KEY,
            email VARCHAR(120) NOT NULL,
            active BOOLEAN,
            role TEXT NOT NULL CHECK (role IN ('admin', 'member')),
            bio TEXT CHECK (length(bio) <= 200)
        );
        CREATE UNIQUE INDEX uq_users_email ON users (email);
        CREATE TABLE events (
            payload TEXT NOT NULL
        );
        CREATE TABLE orders (
            id INTEGER PRIMARY KEY,
            user_id INTEGER NOT NULL REFERENCES users(id),
            code TEXT NOT NULL,
            region TEXT NOT NULL
        );
        CREATE INDEX idx_orders_code_region_1 ON orders (code, region);
        CREATE INDEX idx_orders_code_region_2 ON orders (code, region);
        CREATE INDEX idx_orders_code_prefix ON orders (code);
        CREATE VIEW active_users AS SELECT id, email FROM users WHERE active = 1;
        CREATE TRIGGER tr_orders AFTER INSERT ON orders BEGIN SELECT 1; END;
        ",
    )
    .await
    .expect("seed");
    conn.close().await.expect("close");

    let snapshot = inspect_dsn(&dsn, &InspectOptions::default())
        .await
        .expect("inspect");
    assert_eq!(snapshot.dialect, SqlDialect::Sqlite);
    assert_eq!(snapshot.schemas.len(), 1);
    assert_eq!(snapshot.schemas[0].tables.len(), 3);
    assert_eq!(snapshot.schemas[0].views.len(), 1);
    assert_eq!(snapshot.schemas[0].triggers.len(), 1);

    let mut cfg = DbSchemaConfig::default();
    cfg.rules.insert(
        PLAIN_TABLES.into(),
        DbSchemaRuleConfig {
            enabled: Some(true),
            ..DbSchemaRuleConfig::default()
        },
    );
    let report = evaluate(&snapshot, None, &cfg);
    let rules: Vec<&str> = report.findings.iter().map(|f| f.rule.as_str()).collect();
    assert!(rules.contains(&TABLE_WITHOUT_PRIMARY_KEY), "{rules:?}");
    assert!(rules.contains(&FK_WITHOUT_INDEX), "{rules:?}");
    assert!(rules.contains(&DUPLICATE_INDEX), "{rules:?}");
    assert!(rules.contains(&PREFIX_REDUNDANT_INDEX), "{rules:?}");
    assert!(rules.contains(&NULLABLE_BOOLEAN), "{rules:?}");
    assert!(rules.contains(&PLAIN_TABLES), "{rules:?}");
}

#[tokio::test]
#[ignore = "needs postgres"]
async fn live_postgres_introspection() {
    let dsn = env::var("SYCH_TEST_POSTGRES_DSN").expect("SYCH_TEST_POSTGRES_DSN");
    let snap = inspect_dsn(&dsn, &InspectOptions::default())
        .await
        .expect("pg inspect");
    assert_eq!(snap.dialect, SqlDialect::Postgres);
}

#[tokio::test]
#[ignore = "needs mysql"]
async fn live_mysql_introspection() {
    let dsn = env::var("SYCH_TEST_MYSQL_DSN").expect("SYCH_TEST_MYSQL_DSN");
    let snap = inspect_dsn(&dsn, &InspectOptions::default())
        .await
        .expect("mysql inspect");
    assert_eq!(snap.dialect, SqlDialect::Mysql);
}

#[test]
fn static_sql_reconstructs_pg_mysql_sqlite_and_records_procedural_limitations() {
    let pg_files = vec![
        SqlFile {
            path: "migrations/0001_init.sql".into(),
            content: r"
CREATE TYPE user_role AS ENUM ('admin', 'member');
CREATE TABLE users (
    id BIGSERIAL PRIMARY KEY,
    email VARCHAR(255) NOT NULL UNIQUE,
    role user_role NOT NULL,
    active BOOLEAN
);
CREATE TABLE orders (
    id BIGSERIAL PRIMARY KEY,
    user_id BIGINT NOT NULL REFERENCES users(id)
);
"
            .into(),
        },
        SqlFile {
            path: "migrations/0002_alter.sql".into(),
            content: r"
ALTER TABLE users ALTER COLUMN active SET NOT NULL;
CREATE INDEX idx_orders_user ON orders (user_id);
DO $$ BEGIN RAISE NOTICE 'dynamic migration'; END $$;
"
            .into(),
        },
    ];
    let snap = parse_sql_files(SqlDialect::Postgres, &pg_files);
    assert_eq!(snap.schemas.len(), 1);
    assert_eq!(snap.limitations.len(), 1, "DO block must be recorded");
    assert!(snap.limitations[0].contains("migrations/0002_alter.sql"));
    let users = snap.schemas[0].table("users").expect("users");
    assert!(!users.column("active").expect("active").nullable);
    assert!(matches!(
        &users.column("role").expect("role").data_type,
        DbColumnType::Enum { values, .. } if values == &["admin", "member"]
    ));
    let report = evaluate(&snap, None, &DbSchemaConfig::default());
    assert_eq!(report.findings, Vec::new(), "{report:?}");

    // MySQL with inline KEY, TINYINT(1), ENUM and prefix index.
    let mysql_files = vec![SqlFile {
        path: "db/schema.sql".into(),
        content: r"
CREATE TABLE items (
    id INT AUTO_INCREMENT PRIMARY KEY,
    flag TINYINT(1),
    state ENUM('new', 'done') NOT NULL,
    sku VARCHAR(64) NOT NULL,
    KEY idx_sku_prefix (sku(10))
) ENGINE=InnoDB;
"
        .into(),
    }];
    let msnap = parse_sql_files(SqlDialect::Mysql, &mysql_files);
    let items = msnap.schemas[0].table("items").expect("items");
    assert!(matches!(
        items.column("flag").expect("flag").data_type,
        DbColumnType::Boolean
    ));
    assert_eq!(items.indexes[0].parts[0].prefix_length, Some(10));
}

#[test]
fn static_sql_dogfoods_sych_own_migrations() {
    let repo_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("crates")
        .parent()
        .expect("root");
    let tree = SourceTree::from_dir(repo_root);
    let targets = discover_sql_targets(&tree, &[], None);
    assert!(
        targets
            .iter()
            .any(|t| t.label == "migrations/postgres" && t.dialect == SqlDialect::Postgres),
        "{targets:?}"
    );
    assert!(
        targets
            .iter()
            .any(|t| t.label == "migrations/sqlite" && t.dialect == SqlDialect::Sqlite),
        "{targets:?}"
    );
    for target in &targets {
        if target.label == "migrations/postgres" || target.label == "migrations/sqlite" {
            let snap = parse_sql_files(target.dialect, &target.files);
            assert_eq!(
                snap.limitations,
                Vec::<String>::new(),
                "Sych migrations for {} must parse cleanly: {:?}",
                target.label,
                snap.limitations
            );
            assert!(snap.schemas[0].table("runs").is_some());
            assert!(snap.schemas[0].table("changes").is_some());
        }
    }
}

fn setup_git_schema_repo() -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("tempdir");
    let git = |args: &[&str]| {
        let out = Command::new("git")
            .current_dir(dir.path())
            .env("GIT_AUTHOR_NAME", "test")
            .env("GIT_AUTHOR_EMAIL", "test@example.com")
            .env("GIT_COMMITTER_NAME", "test")
            .env("GIT_COMMITTER_EMAIL", "test@example.com")
            .args(args)
            .output()
            .expect("git");
        assert!(
            out.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    };

    git(&["init", "-b", "main"]);
    fs::create_dir_all(dir.path().join("migrations/postgres")).expect("mkdir");
    fs::create_dir_all(dir.path().join("prisma")).expect("mkdir");
    fs::write(
        dir.path().join("migrations/postgres/0001_init.sql"),
        "CREATE TABLE users (\n    id BIGSERIAL PRIMARY KEY,\n    email VARCHAR(255) NOT NULL UNIQUE\n);\n",
    )
    .expect("write");
    fs::write(
        dir.path().join("prisma/schema.prisma"),
        "model User {\n  id Int @id\n  email String @unique @db.VarChar(255)\n  @@map(\"users\")\n}\n",
    )
    .expect("write");
    git(&["add", "."]);
    git(&["commit", "-m", "base"]);

    git(&["checkout", "-b", "feat"]);
    fs::write(
        dir.path().join("migrations/postgres/0002_orders.sql"),
        "CREATE TABLE orders (\n    id BIGSERIAL PRIMARY KEY,\n    user_id BIGINT REFERENCES users(id),\n    status TEXT NOT NULL\n);\n",
    )
    .expect("write");
    fs::write(
        dir.path().join("prisma/schema.prisma"),
        r#"
enum Status {
  OPEN @map("open")
  CLOSED @map("closed")
}
model User {
  id Int @id
  email String @unique @db.VarChar(255)
  @@map("users")
}
model Order {
  id Int @id
  userId Int @map("user_id")
  status Status
  user User @relation(fields: [userId], references: [id])
  @@map("orders")
}
"#,
    )
    .expect("write");
    git(&["add", "."]);
    git(&["commit", "-m", "head"]);
    dir
}

#[tokio::test]
async fn review_collector_diffs_base_and_head_and_anchors_findings_to_head() {
    let dir = setup_git_schema_repo();
    let snap = LocalSnapshot::capture(&SnapshotRequest {
        repo: dir.path(),
        base: "main",
        head: "feat",
        diff_base: None,
        max_blob_bytes: 1_048_576,
    })
    .expect("snapshot");

    let settings = SchemaLintSettings {
        enabled: true,
        ..SchemaLintSettings::default()
    };
    let config = DbSchemaConfig::default();
    let facts = collect_review_facts(
        &snap,
        &ReviewCollectOptions {
            settings: &settings,
            config: &config,
            ci: &[],
            live_dsn: None,
            live_label: None,
            timeout: ReviewCollectOptions::default_timeout(),
            explicit_manifest: None,
        },
    )
    .await;

    assert_eq!(facts.len(), 1);
    let f = &facts[0];
    assert_eq!(f.status, BackendStatus::Ready);
    assert!(f.base_checked);
    assert_eq!(f.model_extractor.as_deref(), Some("prisma"));
    assert!(
        f.findings
            .iter()
            .all(|item| item.change == Some(DbSchemaChange::New))
    );

    let rules: Vec<&str> = f.findings.iter().map(|i| i.rule.as_str()).collect();
    assert!(rules.contains(&FK_WITHOUT_INDEX), "{rules:?}");
    assert!(rules.contains(&REQUIRED_FIELD_NULLABLE_COLUMN), "{rules:?}");
    assert!(rules.contains(&ENUM_NOT_ENFORCED), "{rules:?}");
    assert!(!rules.contains(&RELATION_WITHOUT_FOREIGN_KEY), "{rules:?}");
    assert!(
        !rules.contains(&UNIQUE_FIELD_WITHOUT_UNIQUE_INDEX),
        "{rules:?}"
    );
    assert!(!rules.contains(&MAX_LENGTH_NOT_ENFORCED), "{rules:?}");

    for item in &f.findings {
        let cr = item.code_ref.as_ref().expect("anchored to HEAD");
        assert_eq!(cr.side, SourceSide::Head);
    }

    let snap_json = serde_json::to_string(&parse_sql_files(
        SqlDialect::Postgres,
        &[SqlFile {
            path: "a.sql".into(),
            content: "CREATE TABLE t (id BIGINT PRIMARY KEY);".into(),
        }],
    ))
    .expect("json");
    let imported = ci::import(&snap, "db-schema.json", &snap_json, None).expect("ci import");
    assert_eq!(imported.format, CiFormat::DbSchemaSnapshot);
    assert!(imported.db_schema_snapshot.is_some());

    let manifest_json = r#"{"schema_version": 1, "tables": [{"name": "t"}]}"#;
    let imported_m = ci::import(&snap, "model.json", manifest_json, None).expect("ci manifest");
    assert_eq!(imported_m.format, CiFormat::DbModelManifest);
    assert!(imported_m.db_model_manifest.is_some());

    // Top-level JSON key detection must not confuse SychManifest or DbModelManifest.
    let sych_manifest_with_tables =
        r#"{"schema_version": 1, "commands": [{"name": "check tables", "exit_code": 0}]}"#;
    let sych_manifest_facts =
        ci::import(&snap, "outcomes.json", sych_manifest_with_tables, None).expect("sych manifest");
    assert_eq!(sych_manifest_facts.format, CiFormat::SychManifest);

    let model_with_dialect_word =
        r#"{"schema_version": 1, "tables": [{"name": "dialect_schemas"}]}"#;
    let dialect_model_facts =
        ci::import(&snap, "models.json", model_with_dialect_word, None).expect("model manifest");
    assert_eq!(dialect_model_facts.format, CiFormat::DbModelManifest);

    // A DbSchemaSnapshot containing a table named `runs` or column `posn` must not be
    // misclassified as SARIF or go-vet JSON.
    let snap_with_runs = serde_json::to_string(&parse_sql_files(
        SqlDialect::Postgres,
        &[SqlFile {
            path: "runs.sql".into(),
            content: "CREATE TABLE runs (id BIGINT PRIMARY KEY, posn INTEGER NOT NULL);".into(),
        }],
    ))
    .expect("json");
    let runs_facts =
        ci::import(&snap, "runs-schema.json", &snap_with_runs, None).expect("runs snapshot");
    assert_eq!(runs_facts.format, CiFormat::DbSchemaSnapshot);
    assert!(runs_facts.db_schema_snapshot.is_some());
}
