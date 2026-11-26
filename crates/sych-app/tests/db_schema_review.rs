//! End-to-end tests for opt-in database and application schema linting inside
//! `sych review` (static SQL BASE vs HEAD, trusted target policy, `block_on`,
//! waivers, local live read-only SQLite DSN and bound CI snapshots).

use std::fs;
use std::path::Path;
use std::process::{Command, Output};

use serde_json::{Value, json};
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use tempfile::TempDir;

fn git(dir: &Path, args: &[&str]) {
    let out = Command::new("git")
        .current_dir(dir)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .args(["-c", "user.name=t", "-c", "user.email=t@example.invalid"])
        .args(["-c", "commit.gpgsign=false"])
        .args(args)
        .output()
        .expect("git");
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

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

fn init_store() -> TempDir {
    let store_dir = TempDir::new().expect("store");
    fs::create_dir(store_dir.path().join(".sych-data")).expect("mkdir .sych-data");
    assert_eq!(
        sych(store_dir.path(), &["db", "migrate"]).status.code(),
        Some(0)
    );
    store_dir
}

fn review_args<'a>(repo: &'a Path, extra: &[&'a str]) -> Vec<&'a str> {
    let mut args = vec![
        "review",
        "--repo",
        repo.to_str().expect("utf8"),
        "--base",
        "target",
        "--head",
        "HEAD",
        "--models",
        "fake",
        "--semantic",
        "off",
    ];
    args.extend_from_slice(extra);
    args
}

fn setup_review_repo(trusted_config: &str, rules_yaml: Option<&str>) -> TempDir {
    let dir = TempDir::new().expect("tempdir");
    let d = dir.path();
    git(d, &["init", "-q", "-b", "main"]);
    fs::create_dir_all(d.join(".sych")).expect("mkdir .sych");
    fs::create_dir_all(d.join("migrations")).expect("mkdir migrations");
    fs::create_dir_all(d.join("prisma")).expect("mkdir prisma");
    fs::write(d.join(".sych/config.yaml"), trusted_config).expect("write config");
    if let Some(rules) = rules_yaml {
        fs::write(d.join(".sych/rules.yaml"), rules).expect("write rules");
    }

    fs::write(
        d.join("migrations/001_users.sql"),
        "CREATE TABLE users (\n  id BIGINT PRIMARY KEY,\n  email TEXT NOT NULL\n);\n",
    )
    .expect("write base sql");
    fs::write(
        d.join("prisma/schema.prisma"),
        "model Users {\n  id BigInt @id\n  email String\n  @@map(\"users\")\n}\n",
    )
    .expect("write base prisma");
    git(d, &["add", "-A"]);
    git(d, &["commit", "-q", "-m", "base"]);
    git(d, &["branch", "target"]);

    fs::write(
        d.join(".sych/config.yaml"),
        "schema_version: 1\nschema_lint:\n  enabled: false\n",
    )
    .expect("write untrusted head config");
    fs::write(
        d.join("migrations/002_orders.sql"),
        "CREATE TABLE orders (\n  id BIGINT PRIMARY KEY,\n  user_id BIGINT NOT NULL REFERENCES users(id),\n  contact_email TEXT,\n  status TEXT NOT NULL\n);\n",
    )
    .expect("write head sql");
    fs::write(
        d.join("prisma/schema.prisma"),
        "enum OrderStatus {\n  PENDING\n  PAID\n}\n\nmodel Users {\n  id BigInt @id\n  email String\n  @@map(\"users\")\n}\n\nmodel Orders {\n  id BigInt @id\n  user_id BigInt\n  contact_email String\n  status OrderStatus\n  @@map(\"orders\")\n}\n",
    )
    .expect("write head prisma");
    git(d, &["add", "-A"]);
    git(d, &["commit", "-q", "-m", "add orders"]);
    dir
}

#[test]
fn review_disabled_by_default_and_enforced_from_trusted_target() {
    let store_dir = init_store();

    let off_repo = setup_review_repo("schema_version: 1\n", None);
    let off_out = sych(
        store_dir.path(),
        &review_args(off_repo.path(), &["--format", "json"]),
    );
    assert!(off_out.status.success());
    let off_report: Value = serde_json::from_slice(&off_out.stdout).expect("off json");
    assert!(off_report.get("db_schema_facts").is_none());
    assert_eq!(
        off_report["review_result"]["findings"]
            .as_array()
            .expect("findings")
            .as_slice(),
        &[] as &[Value]
    );

    let on_repo = setup_review_repo(
        "schema_version: 1\nreview:\n  block_on:\n    - DB-FK-WITHOUT-INDEX\nschema_lint:\n  enabled: true\n  dialect: postgres\n",
        None,
    );
    let on_out = sych(
        store_dir.path(),
        &review_args(
            on_repo.path(),
            &["--exit-policy", "strict", "--format", "json"],
        ),
    );
    assert_eq!(on_out.status.code(), Some(10));
    let report: Value = serde_json::from_slice(&on_out.stdout).expect("on json");
    assert_eq!(
        report["review_result"]["recommendation"],
        "ATTENTION_REQUIRED"
    );
    let findings = report["review_result"]["findings"]
        .as_array()
        .expect("findings");
    let rules: Vec<&str> = findings
        .iter()
        .filter_map(|f| f["rule_id"].as_str())
        .collect();
    assert!(rules.contains(&"DB-FK-WITHOUT-INDEX"), "{rules:?}");
    assert!(
        rules.contains(&"DB-REQUIRED-FIELD-NULLABLE-COLUMN"),
        "{rules:?}"
    );
    assert!(rules.contains(&"DB-ENUM-NOT-ENFORCED"), "{rules:?}");
    let fk = findings
        .iter()
        .find(|f| f["rule_id"] == "DB-FK-WITHOUT-INDEX")
        .expect("fk finding");
    assert_eq!(fk["anchors"][0]["path"], "migrations/002_orders.sql");

    let md_out = sych(
        store_dir.path(),
        &review_args(on_repo.path(), &["--format", "markdown"]),
    );
    assert!(md_out.status.success());
    assert!(String::from_utf8_lossy(&md_out.stdout).contains("## Database schema"));
}

#[test]
fn review_trusted_waiver_clears_blocking_db_rule() {
    let store_dir = init_store();
    let waived_rules = r#"schema_version: 1
rules:
  - id: DB-FK-WITHOUT-INDEX
    version: 1
    title: "Foreign keys must have indexes"
    kind: invariant
    required: false
    severity_on_violation: high
    owner: dba
    applies_to:
      paths: ["migrations/**"]
    statement: Every foreign key has a covering index.
    acceptance:
      evidence_kinds: ["static"]
      criteria: ["Index exists."]
    exceptions:
      - paths: ["migrations/002_orders.sql"]
        reason: "Index created concurrently in follow-up migration"
        owner: dba
        author: lead
        until: "follow-up migration deployed"
"#;
    let waived_repo = setup_review_repo(
        "schema_version: 1\nreview:\n  block_on:\n    - DB-FK-WITHOUT-INDEX\nschema_lint:\n  enabled: true\n  dialect: postgres\n",
        Some(waived_rules),
    );
    let waived_out = sych(
        store_dir.path(),
        &review_args(waived_repo.path(), &["--format", "json"]),
    );
    assert!(
        waived_out.status.success(),
        "waived review failed ({:?}): {}",
        waived_out.status.code(),
        String::from_utf8_lossy(&waived_out.stderr)
    );
    let waived_report: Value = serde_json::from_slice(&waived_out.stdout).expect("waived json");
    assert!(
        waived_report["review_result"].get("blocked_by").is_none(),
        "waived DB-FK-WITHOUT-INDEX must not block: {:?}",
        waived_report["review_result"]["blocked_by"]
    );
    let waived_fk = waived_report["review_result"]["findings"]
        .as_array()
        .expect("findings")
        .iter()
        .find(|f| {
            f["title"]
                .as_str()
                .is_some_and(|t| t.contains("fk-without-index"))
        })
        .expect("waived fk finding kept");
    assert_eq!(waived_fk["category"], "IMPROVEMENT");
    assert_eq!(waived_fk["severity"], "LOW");
    assert!(waived_fk["rule_id"].is_null());
}

#[tokio::test(flavor = "current_thread")]
async fn review_local_live_dsn_and_ci_snapshot() {
    let store_dir = init_store();
    let repo = setup_review_repo("schema_version: 1\n", None);
    let dsn = create_sqlite_db(
        &store_dir.path().join("live.db"),
        &["CREATE TABLE orphan_logs (msg TEXT);"],
    )
    .await;

    let live_out = sych(
        store_dir.path(),
        &review_args(repo.path(), &["--db-dsn", &dsn, "--format", "json"]),
    );
    assert!(
        live_out.status.success(),
        "live review failed ({:?}): {}",
        live_out.status.code(),
        String::from_utf8_lossy(&live_out.stderr)
    );
    let live_report: Value = serde_json::from_slice(&live_out.stdout).expect("live json");
    let live_rules: Vec<&str> = live_report["review_result"]["findings"]
        .as_array()
        .expect("findings")
        .iter()
        .filter_map(|f| f["rule_id"].as_str())
        .collect();
    assert!(
        live_rules.contains(&"DB-TABLE-WITHOUT-PRIMARY-KEY"),
        "{live_rules:?}"
    );

    let snap_file = store_dir.path().join("ci-schema-snapshot.json");
    let snap = snap_file.to_str().expect("utf8");
    assert!(
        sych(
            store_dir.path(),
            &["schema", "snapshot", "--dsn", &dsn, "--output", snap],
        )
        .status
        .success()
    );
    let head_oid = String::from_utf8(
        Command::new("git")
            .current_dir(repo.path())
            .args(["rev-parse", "HEAD"])
            .output()
            .expect("rev-parse")
            .stdout,
    )
    .expect("utf8");
    let manifest_file = store_dir.path().join("sych-ci.json");
    fs::write(
        &manifest_file,
        json!({
            "schema_version": 1,
            "tested_oid": head_oid.trim(),
            "job": "schema-snapshot",
            "toolchain": "sqlite3",
            "target": "local"
        })
        .to_string(),
    )
    .expect("write ci manifest");

    let ci_out = sych(
        store_dir.path(),
        &review_args(
            repo.path(),
            &[
                "--schema-lint",
                "on",
                "--ci-facts",
                snap,
                "--ci-manifest",
                manifest_file.to_str().expect("utf8"),
                "--format",
                "json",
            ],
        ),
    );
    assert!(
        ci_out.status.success(),
        "{}",
        String::from_utf8_lossy(&ci_out.stderr)
    );
    let ci_report: Value = serde_json::from_slice(&ci_out.stdout).expect("ci json");
    assert_eq!(ci_report["db_schema_facts"][0]["source"], "ci_snapshot");
    let ci_rules: Vec<&str> = ci_report["review_result"]["findings"]
        .as_array()
        .expect("findings")
        .iter()
        .filter_map(|f| f["rule_id"].as_str())
        .collect();
    assert!(
        ci_rules.contains(&"DB-TABLE-WITHOUT-PRIMARY-KEY"),
        "{ci_rules:?}"
    );
}
