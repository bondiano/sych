//! End-to-end tests for the built-in database and application schema linter:
//! standalone `sych schema` CLI and stateless `/v1/schema/*` HTTP API endpoints.

use std::env;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::{Command, Output};

use reqwest::StatusCode;
use serde_json::{Value, json};
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use sych_adapters::db_schema::sql_parse::{SqlFile, parse_sql_files};
use sych_core::db_schema::SqlDialect;
use tempfile::TempDir;

mod fake_github;
mod http;
mod world;

use http::{call, listen};
use world::world_with;

fn sych(cwd: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_sych"))
        .current_dir(cwd)
        .env("XDG_CONFIG_HOME", cwd.join("user-config"))
        .env("XDG_DATA_HOME", cwd.join("user-data"))
        .args(args)
        .output()
        .expect("run sych")
}

async fn create_sqlite_db(path: &Path, sql: &[&'static str]) -> String {
    let opts = SqliteConnectOptions::new()
        .filename(path)
        .create_if_missing(true);
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(opts)
        .await
        .expect("connect sqlite");
    for &stmt in sql {
        sqlx::query(stmt).execute(&pool).await.expect("exec sql");
    }
    pool.close().await;
    format!("sqlite://{}", path.display())
}

#[test]
fn cli_rules_explain_and_manifest_schema() {
    let dir = TempDir::new().expect("tempdir");

    let rules_out = sych(dir.path(), &["schema", "rules", "--format", "json"]);
    assert!(rules_out.status.success());
    let rules: Vec<Value> = serde_json::from_slice(&rules_out.stdout).expect("rules json");
    assert_eq!(rules.len(), 18, "expected all 18 built-in schema rules");

    for query in ["fk-without-index", "DB-FK-WITHOUT-INDEX"] {
        let out = sych(
            dir.path(),
            &["schema", "explain", query, "--format", "json"],
        );
        assert!(out.status.success(), "explain {query} failed");
        let info: Value = serde_json::from_slice(&out.stdout).expect("explain json");
        assert_eq!(info["id"], "fk-without-index");
        assert_eq!(info["review_rule_id"], "DB-FK-WITHOUT-INDEX");
    }

    let schema_out = sych(dir.path(), &["schema", "manifest-schema"]);
    assert!(schema_out.status.success());
    let schema: Value = serde_json::from_slice(&schema_out.stdout).expect("manifest schema");
    assert_eq!(schema["title"], "DbModelManifest");
}

#[tokio::test(flavor = "current_thread")]
async fn cli_snapshot_check_and_sarif() {
    let dir = TempDir::new().expect("tempdir");
    let dsn = create_sqlite_db(
        &dir.path().join("app.db"),
        &[
            "CREATE TABLE users (id INTEGER PRIMARY KEY, email TEXT NOT NULL);",
            "CREATE TABLE events (id INTEGER PRIMARY KEY, user_id INTEGER NOT NULL REFERENCES users(id), active BOOLEAN);",
        ],
    )
    .await;

    let snap_path = dir.path().join("snapshot.json");
    let snap = snap_path.to_str().expect("utf8");
    let snap_out = sych(
        dir.path(),
        &["schema", "snapshot", "--dsn", &dsn, "--output", snap],
    );
    assert!(
        snap_out.status.success(),
        "{}",
        String::from_utf8_lossy(&snap_out.stderr)
    );

    let check_json = sych(
        dir.path(),
        &["schema", "check", "--snapshot", snap, "--format", "json"],
    );
    assert_eq!(check_json.status.code(), Some(1));
    let report: Value = serde_json::from_slice(&check_json.stdout).expect("report json");
    let rule_ids: Vec<&str> = report["findings"]
        .as_array()
        .expect("findings")
        .iter()
        .filter_map(|f| f["rule"].as_str())
        .collect();
    assert!(rule_ids.contains(&"fk-without-index"), "{rule_ids:?}");
    assert!(rule_ids.contains(&"nullable-boolean"), "{rule_ids:?}");

    let check_sarif = sych(
        dir.path(),
        &["schema", "check", "--snapshot", snap, "--format", "sarif"],
    );
    assert_eq!(check_sarif.status.code(), Some(1));
    let sarif: Value = serde_json::from_slice(&check_sarif.stdout).expect("sarif json");
    assert_eq!(sarif["version"], "2.1.0");
    assert!(
        sarif["runs"][0]["results"][0]["locations"][0]["logicalLocations"][0]["fullyQualifiedName"]
            .is_string()
    );
}

#[tokio::test(flavor = "current_thread")]
async fn cli_baseline_preserves_comments_is_idempotent_and_detects_stale() {
    let dir = TempDir::new().expect("tempdir");
    let db_path = dir.path().join("app.db");
    let dsn = create_sqlite_db(
        &db_path,
        &[
            "CREATE TABLE users (id INTEGER PRIMARY KEY, email TEXT NOT NULL);",
            "CREATE TABLE events (id INTEGER PRIMARY KEY, user_id INTEGER NOT NULL REFERENCES users(id));",
        ],
    )
    .await;

    let cfg_path = dir.path().join(".sych/schema-lint.yaml");
    let cfg = cfg_path.to_str().expect("utf8");
    fs::create_dir_all(cfg_path.parent().expect("parent")).expect("mkdir");
    fs::write(&cfg_path, "# keep this comment\nrules: {}\n").expect("write cfg");

    let base_out = sych(
        dir.path(),
        &["schema", "baseline", "--dsn", &dsn, "--config", cfg],
    );
    assert!(base_out.status.success());
    let cfg_after = fs::read_to_string(&cfg_path).expect("read cfg");
    assert!(cfg_after.contains("# keep this comment"), "{cfg_after}");

    let base_again = sych(
        dir.path(),
        &["schema", "baseline", "--dsn", &dsn, "--config", cfg],
    );
    assert!(base_again.status.success());
    assert_eq!(fs::read_to_string(&cfg_path).expect("read cfg2"), cfg_after);

    let clean = sych(
        dir.path(),
        &["schema", "check", "--dsn", &dsn, "--config", cfg],
    );
    assert!(clean.status.success());

    let _ = create_sqlite_db(
        &db_path,
        &["CREATE INDEX idx_events_user_id ON events (user_id);"],
    )
    .await;
    let stale = sych(
        dir.path(),
        &[
            "schema", "check", "--dsn", &dsn, "--config", cfg, "--format", "json",
        ],
    );
    assert_eq!(stale.status.code(), Some(1));
    let stale_report: Value = serde_json::from_slice(&stale.stdout).expect("stale json");
    assert_ne!(
        stale_report["unmatched_exclusions"]
            .as_array()
            .expect("unmatched")
            .as_slice(),
        &[] as &[Value]
    );
}

#[test]
fn cli_extract_builtin_prisma_and_external_adapter() {
    let dir = TempDir::new().expect("tempdir");
    fs::create_dir_all(dir.path().join("prisma")).expect("mkdir");
    fs::write(
        dir.path().join("prisma/schema.prisma"),
        "model User {\n  id Int @id\n  email String @unique\n}\n",
    )
    .expect("write prisma");

    let out = sych(
        dir.path(),
        &[
            "schema",
            "extract",
            "--repo",
            dir.path().to_str().expect("utf8"),
        ],
    );
    assert!(out.status.success());
    let manifest: Value = serde_json::from_slice(&out.stdout).expect("manifest json");
    assert_eq!(manifest["extractor"], "prisma");
    assert_eq!(manifest["tables"][0]["name"], "User");

    let bin_dir = dir.path().join("bin");
    fs::create_dir_all(&bin_dir).expect("mkdir bin");
    let script = bin_dir.join("sych-schema-adapter-mock");
    fs::write(
        &script,
        "#!/bin/sh\necho '{\"schema_version\":1,\"extractor\":\"mock\",\"tables\":[{\"name\":\"widgets\",\"columns\":[{\"name\":\"code\",\"required\":true,\"unique\":true}]}]}'\n",
    )
    .expect("write script");
    let mut perms = fs::metadata(&script).expect("meta").permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&script, perms).expect("chmod");

    let path_env = format!(
        "{}:{}",
        bin_dir.display(),
        env::var("PATH").unwrap_or_default()
    );
    let ext = Command::new(env!("CARGO_BIN_EXE_sych"))
        .current_dir(dir.path())
        .env("PATH", path_env)
        .args([
            "schema",
            "extract",
            "--adapter",
            "mock",
            "--adapter-arg",
            "--flag",
        ])
        .output()
        .expect("run external adapter");
    assert!(
        ext.status.success(),
        "{}",
        String::from_utf8_lossy(&ext.stderr)
    );
    let ext_manifest: Value = serde_json::from_slice(&ext.stdout).expect("ext json");
    assert_eq!(ext_manifest["extractor"], "mock");
    assert_eq!(ext_manifest["tables"][0]["name"], "widgets");
}

#[tokio::test(flavor = "current_thread")]
async fn http_api_schema_rules_and_stateless_check() {
    let w = world_with(None, None).await;
    let url = listen(&w).await;

    assert_eq!(
        call(&url, "GET", "/v1/schema/rules", None, None, &Value::Null)
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        call(&url, "POST", "/v1/schema/check", None, None, &json!({}))
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );

    let (status, rules_body) = call(
        &url,
        "GET",
        "/v1/schema/rules",
        Some("reader"),
        None,
        &Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{rules_body}");
    assert_eq!(rules_body["rules"].as_array().expect("rules").len(), 18);

    let snapshot = parse_sql_files(
        SqlDialect::Postgres,
        &[SqlFile {
            path: "schema.sql".to_owned(),
            content: "CREATE TABLE accounts (id BIGINT PRIMARY KEY, email TEXT);".to_owned(),
        }],
    );
    let req = json!({
        "schema_version": 1,
        "snapshot": snapshot,
        "manifest": {
            "schema_version": 1,
            "extractor": "prisma",
            "tables": [{
                "name": "accounts",
                "columns": [
                    { "name": "email", "required": true, "unique": true }
                ]
            }]
        }
    });
    let (status, check_body) =
        call(&url, "POST", "/v1/schema/check", Some("reader"), None, &req).await;
    assert_eq!(status, StatusCode::OK, "{check_body}");
    let rules: Vec<&str> = check_body["findings"]
        .as_array()
        .expect("findings")
        .iter()
        .filter_map(|f| f["rule"].as_str())
        .collect();
    assert!(
        rules.contains(&"required-field-nullable-column"),
        "{rules:?}"
    );
    assert!(
        rules.contains(&"unique-field-without-unique-index"),
        "{rules:?}"
    );

    let bad_req = json!({
        "schema_version": 1,
        "snapshot": req["snapshot"],
        "config": { "rules": { "nonexistent-rule": { "enabled": true } } }
    });
    let (status, err_body) = call(
        &url,
        "POST",
        "/v1/schema/check",
        Some("reader"),
        None,
        &bad_req,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{err_body}");
    assert_eq!(err_body["error"]["code"], "INVALID_REQUEST");
}
