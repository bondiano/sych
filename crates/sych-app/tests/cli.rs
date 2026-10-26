//! Real CLI smoke (roadmap M0.6): review → persisted JSON/Markdown → show/explain, exit codes,
//! failed save and cancellation. Needs `git` in `PATH`; no external API is reachable.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::Value;
use tempfile::TempDir;

const RULES: &str = r#"schema_version: 1
rules:
  - id: EFFECT-01
    version: 1
    title: "Order <img src=x> keeps *event*"
    kind: invariant
    required: true
    severity_on_violation: high
    owner: backend
    applies_to:
      paths: ["backend/orders/**"]
    statement: A stored order publishes order-created.
    acceptance:
      evidence_kinds: ["source"]
      criteria: ["Commit boundary is identified."]
"#;

const BASE: &str = "package orders\n\nfunc Save(o Order) error {\n\treturn tx(o)\n}\n";
const HEAD: &str =
    "package orders\n\nfunc Save(o Order) error {\n\tgo publish(o)\n\treturn nil\n}\n";

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
    assert!(out.status.success(), "git {args:?} failed");
}

/// `target` has the policy and the old file; HEAD changes the file unless `no_op`.
fn repo(no_op: bool) -> TempDir {
    let dir = TempDir::new().expect("tempdir");
    let d = dir.path();
    git(d, &["init", "-q", "-b", "main"]);
    fs::create_dir_all(d.join(".sych")).expect("mkdir");
    fs::create_dir_all(d.join("backend/orders")).expect("mkdir");
    fs::write(d.join(".sych/rules.yaml"), RULES).expect("write");
    fs::write(d.join("backend/orders/store.go"), BASE).expect("write");
    git(d, &["add", "-A"]);
    git(d, &["commit", "-q", "-m", "base"]);
    git(d, &["branch", "target"]);
    if !no_op {
        fs::write(d.join("backend/orders/store.go"), HEAD).expect("write");
        git(d, &["commit", "-q", "-am", "head"]);
    }

    dir
}

/// Fake responses citing lines 3-6 of the changed file with the given status.
fn fixture(dir: &Path, status: &str) -> PathBuf {
    let path = dir.join(format!("{status}.yaml"));
    let text = format!(
        "schema_version: 1
evidence:
  - key: save
    kind: source
    completeness: COMPLETE_FOR_QUERY
    trust_class: SOURCE_OBSERVATION
    code_refs: [{{ path: backend/orders/store.go, start_line: 3, end_line: 6 }}]
assessments:
  - {{ rule_id: EFFECT-01, status: {status}, evidence: [save] }}
  - {{ rule_id: EFFECT-02, status: {status}, evidence: [save] }}
"
    );
    fs::write(&path, text).expect("write fixture");
    path
}

fn sych(cwd: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_sych"))
        .current_dir(cwd)
        .args(args)
        .output()
        .expect("run sych")
}

/// A migrated store in its own working directory (`./.sych-data`).
fn store() -> TempDir {
    let dir = TempDir::new().expect("tempdir");
    assert_eq!(sych(dir.path(), &["db", "migrate"]).status.code(), Some(0));
    dir
}

fn review(cwd: &Path, repo: &Path, extra: &[&str]) -> Output {
    let repo = repo.to_str().expect("utf-8 path");
    let mut args = vec![
        "review", "--repo", repo, "--base", "target", "--models", "fake",
    ];
    args.extend_from_slice(extra);
    sych(cwd, &args)
}

fn json(out: &Output) -> Value {
    serde_json::from_slice(&out.stdout).expect("json stdout")
}

#[test]
fn saved_review_renders_the_same_through_show_and_explains_evidence() {
    let (repo, data) = (repo(false), store());
    let fake = fixture(data.path(), "VIOLATED");
    let out = review(
        data.path(),
        repo.path(),
        &[
            "--fake-responses",
            fake.to_str().expect("path"),
            "--output",
            "out/review.json",
        ],
    );
    assert_eq!(
        out.status.code(),
        Some(0),
        "operational mode never fails on findings"
    );
    let saved: Value =
        serde_json::from_slice(&fs::read(data.path().join("out/review.json")).expect("output"))
            .expect("json");
    let result = &saved["review_result"];
    assert_eq!(result["recommendation"], "ATTENTION_REQUIRED");
    assert_eq!(result["obligations"][0]["status"], "VIOLATED");
    let refs = |e: &Value| e["code_refs"][0]["path"] == "backend/orders/store.go";
    assert!(saved["evidence"].as_array().expect("ev").iter().any(refs));
    let run = result["run_id"].as_str().expect("run id");

    let shown = sych(data.path(), &["show", "--run", run]);
    assert_eq!(
        shown.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&shown.stderr)
    );
    assert_eq!(json(&shown), saved, "show reads exactly what review saved");

    let md = sych(data.path(), &["show", "--run", run, "--format", "markdown"]);
    let md = String::from_utf8(md.stdout).expect("utf-8");
    assert!(md.contains("recommendation: **ATTENTION_REQUIRED**"));
    assert!(md.contains("applicability APPLIES): VIOLATED"));
    assert!(md.contains("&lt;img src=x&gt; keeps \\*event\\*") && !md.contains("<img"));

    let explained = sych(
        data.path(),
        &["explain", "--run", run, "--obligation", "EFFECT-01"],
    );
    assert_eq!(explained.status.code(), Some(0));
    let explained = json(&explained);
    assert_eq!(explained["obligation"]["status"], "VIOLATED");
    assert_eq!(
        explained["evidence"][0]["trust_class"],
        "SOURCE_OBSERVATION"
    );
    assert_eq!(explained["findings"][0]["category"], "RULE_VIOLATION");
}

#[test]
fn strict_exit_policy_ranks_violation_over_incomplete_scope() {
    let (repo, data) = (repo(false), store());
    let violated = fixture(data.path(), "VIOLATED");
    let violated = violated.to_str().expect("path");
    let strict = ["--exit-policy", "strict"];
    let out = review(
        data.path(),
        repo.path(),
        &[&strict[..], &["--fake-responses", violated]].concat(),
    );
    assert_eq!(out.status.code(), Some(10));
    // Without responses the required obligation stays UNKNOWN: incomplete, not a violation.
    let out = review(data.path(), repo.path(), &strict);
    assert_eq!(out.status.code(), Some(11));
    let result = &json(&out)["review_result"];
    assert_eq!(result["recommendation"], "NEEDS_EVIDENCE");
    assert_eq!(result["obligations"][0]["reason_code"], "NOT_ASSESSED");
    assert_eq!(review(data.path(), repo.path(), &[]).status.code(), Some(0));
    let supported = fixture(data.path(), "SUPPORTED");
    let supported = supported.to_str().expect("path");
    let out = review(
        data.path(),
        repo.path(),
        &[&strict[..], &["--fake-responses", supported]].concat(),
    );
    assert_eq!(
        out.status.code(),
        Some(0),
        "happy path passes even in strict mode"
    );
    assert_eq!(
        json(&out)["review_result"]["coverage"],
        "COMPLETE_FOR_SCOPE"
    );
}

#[test]
fn empty_diff_saves_an_explicit_empty_scope() {
    let (repo, data) = (repo(true), store());
    let out = review(
        data.path(),
        repo.path(),
        &["--exit-policy", "strict", "--format", "markdown"],
    );
    assert_eq!(out.status.code(), Some(0));
    let md = String::from_utf8(out.stdout).expect("utf-8");
    assert!(md.contains("coverage: **COMPLETE_FOR_SCOPE**"));
    assert!(md.contains("## Scope (revision 1)\n\n- none"));
}

#[test]
fn invalid_fixture_unmigrated_store_and_failed_save_keep_no_result() {
    let (repo, data) = (repo(false), store());
    let bad = data.path().join("bad.yaml");
    fs::write(&bad, "schema_version: 1\nassessments:\n  - rule_id: EFFECT-01\n    status: SUPPORTED\n    evidence: [missing]\n").expect("write");
    let out = review(
        data.path(),
        repo.path(),
        &["--fake-responses", bad.to_str().expect("path")],
    );
    assert_eq!(out.status.code(), Some(2));

    let empty = TempDir::new().expect("tempdir");
    assert_eq!(
        review(empty.path(), repo.path(), &[]).status.code(),
        Some(3)
    );

    // Artifact bytes cannot be written: the run is FAILED, never COMPLETED.
    fs::remove_dir_all(data.path().join(".sych-data/artifacts")).ok();
    fs::write(data.path().join(".sych-data/artifacts"), "not a directory").expect("write");
    let out = review(data.path(), repo.path(), &[]);
    assert_eq!(out.status.code(), Some(3));
    assert!(String::from_utf8_lossy(&out.stderr).contains("no result was saved"));
    assert!(out.stdout.is_empty());
}

#[cfg(unix)]
#[test]
fn interrupt_cancels_the_run_with_exit_130() {
    use std::process::Stdio;
    use std::thread::sleep;
    use std::time::{Duration, Instant};

    let (repo, data) = (repo(false), store());
    let slow = data.path().join("slow.yaml");
    fs::write(&slow, "schema_version: 1\ndelay_ms: 30000\n").expect("write");
    let child = Command::new(env!("CARGO_BIN_EXE_sych"))
        .current_dir(data.path())
        .args([
            "review",
            "--repo",
            repo.path().to_str().expect("path"),
            "--base",
            "target",
        ])
        .args([
            "--models",
            "fake",
            "--fake-responses",
            slow.to_str().expect("path"),
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn");
    // Wait until the run is recorded, then interrupt it.
    sleep(Duration::from_millis(1500));
    let started = Instant::now();
    let kill = Command::new("kill")
        .args(["-INT", &child.id().to_string()])
        .status()
        .expect("kill");
    assert!(kill.success());
    let out = child.wait_with_output().expect("wait");
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "cancel did not wait for analysis"
    );
    assert_eq!(out.status.code(), Some(130));
    assert!(String::from_utf8_lossy(&out.stderr).contains("cancelled; no result was saved"));
}

#[test]
fn fake_eval_suite_passes_deterministically_without_model_calls() {
    let suite = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../eval/smoke.yaml");
    let suite = suite.to_str().expect("path");
    let cwd = TempDir::new().expect("tempdir");
    let runs: Vec<Output> = (0..2)
        .map(|_| {
            sych(
                cwd.path(),
                &["eval", "run", "--suite", suite, "--models", "fake"],
            )
        })
        .collect();
    assert_eq!(runs[0].status.code(), Some(0));
    let report = json(&runs[0]);
    assert_eq!(
        (report["failed"].as_u64(), report["model_attempts"].as_u64()),
        (Some(0), Some(0))
    );
    assert_eq!(
        runs[0].stdout, runs[1].stdout,
        "same fixtures give the same states"
    );
}

fn head_oid(repo: &Path) -> String {
    let out = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["rev-parse", "HEAD"])
        .output()
        .expect("git");
    String::from_utf8(out.stdout)
        .expect("utf-8")
        .trim()
        .to_owned()
}

#[test]
fn imported_ci_report_is_bound_to_the_tested_head_or_stale() {
    let repo = repo(false);
    let data = store();
    let report = data.path().join("clippy.json");
    fs::write(
        &report,
        concat!(
            r#"{"reason":"compiler-message","message":{"level":"warning","code":{"code":"clippy::needless_return"},"message":"unneeded `return`","spans":[{"file_name":"backend/orders/store.go","line_start":4,"line_end":4,"is_primary":true}]}}"#,
            "\n",
            r#"{"reason":"build-finished","success":true}"#,
            "\n"
        ),
    )
    .expect("write report");
    let run = |tested: &str| {
        let manifest = data.path().join("ci.yaml");
        fs::write(
            &manifest,
            format!("schema_version: 1\njob: local:test\ntested_oid: {tested}\ntoolchain: \"1.94.1\"\ntarget: host\n"),
        )
        .expect("write manifest");
        let out = review(
            data.path(),
            repo.path(),
            &[
                "--ci-facts",
                report.to_str().expect("path"),
                "--ci-manifest",
                manifest.to_str().expect("path"),
            ],
        );
        assert_eq!(
            out.status.code(),
            Some(0),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let saved: Value = serde_json::from_slice(&out.stdout).expect("json");
        let ci = saved["evidence"]
            .as_array()
            .expect("evidence")
            .iter()
            .find(|e| e["binding"]["kind"] == "ci")
            .cloned()
            .expect("ci evidence");
        assert_eq!(
            saved["external_input_manifest"]["inputs"]
                .as_array()
                .map(Vec::len),
            Some(1)
        );
        assert_eq!(
            ci["trust_class"], "AUTHOR_CLAIM",
            "a local import is never authenticated CI"
        );
        ci
    };
    let bound = run(&head_oid(repo.path()));
    assert_eq!(bound["validity"], "VALID");
    assert_eq!(bound["code_refs"][0]["start_line"], 4);
    let stale = run(&"0".repeat(40));
    assert_eq!(stale["validity"], "STALE");
    assert_eq!(stale["code_refs"].as_array().map(Vec::len), Some(0));
}

#[test]
fn development_quality_suite_is_well_labelled_and_anchored() {
    let suite = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../eval/quality/dev");
    let dir = TempDir::new().expect("tempdir");
    let out = sych(
        dir.path(),
        &["eval", "validate", "--suite", suite.to_str().expect("path")],
    );
    let report: Value = serde_json::from_slice(&out.stdout).expect("json report");
    assert_eq!(out.status.code(), Some(0), "{report:#}");
    assert!(
        report["passed"].as_u64() >= Some(24),
        "spec §27.2 asks for at least 24 cases"
    );
    assert_eq!(report["split"], "development");
}

#[test]
fn broken_labels_and_anchors_fail_validation() {
    let suite = TempDir::new().expect("tempdir");
    let case = |id: &str, variant: &str, line: u32| {
        format!(
            "schema_version: 1\nid: {id}\nsplit: development\nlanguage: rust\npair: p\nvariant: {variant}\n\
             profiles: [correctness]\nrationale: r\nbase:\n  src/lib.rs: |\n    fn a() {{}}\nhead:\n  src/lib.rs: |\n    fn b() {{}}\n\
             expected:\n  findings:\n    - category: defect\n      semantic_target: t\n      required_condition: c\n      \
             evidence_locations: [{{ path: src/lib.rs, start_line: {line}, end_line: {line} }}]\n"
        )
    };
    fs::write(
        suite.path().join("bad-line.yaml"),
        case("bad-line", "unsafe", 9),
    )
    .expect("write");
    fs::write(
        suite.path().join("wrong-name.yaml"),
        case("other", "unsafe", 1),
    )
    .expect("write");
    let out = sych(suite.path(), &["eval", "validate", "--suite", "."]);
    assert_eq!(out.status.code(), Some(1));
    let report: Value = serde_json::from_slice(&out.stdout).expect("json");
    let problems = |id: &str| {
        report["cases"]
            .as_array()
            .expect("cases")
            .iter()
            .find(|c| c["id"] == id)
            .map(|c| c["problems"].to_string())
            .unwrap_or_default()
    };
    assert!(
        problems("bad-line").contains("does not resolve"),
        "{report:#}"
    );
    assert!(
        problems("bad-line").contains("needs both safe and unsafe"),
        "{report:#}"
    );
    assert!(
        problems("other").contains("file name must be"),
        "{report:#}"
    );
}

#[test]
fn an_explicit_policy_revision_is_applied_and_recorded() {
    let (repo, data) = (repo(false), store());
    let d = repo.path();
    git(d, &["checkout", "-q", "-b", "policy-next", "target"]);
    let next = RULES.to_owned()
        + "  - id: OTHER-01\n    version: 1\n    title: Other\n    kind: invariant\n    required: false\n    \
           severity_on_violation: low\n    owner: backend\n    statement: Another accepted rule.\n";
    fs::write(d.join(".sych/rules.yaml"), next).expect("write");
    git(d, &["commit", "-q", "-am", "next policy"]);
    git(d, &["checkout", "-q", "main"]);

    let rules = |out: &Output| -> Vec<String> {
        json(out)["review_result"]["obligations"]
            .as_array()
            .expect("obligations")
            .iter()
            .map(|o| o["rule_id"].as_str().expect("id").to_owned())
            .collect()
    };
    let accepted = review(data.path(), d, &[]);
    assert_eq!(rules(&accepted), ["EFFECT-01", "EFFECT-02"], "target tip");
    let explicit = review(data.path(), d, &["--policy-ref", "policy-next"]);
    assert_eq!(explicit.status.code(), Some(0));
    assert_eq!(rules(&explicit), ["EFFECT-01", "OTHER-01", "EFFECT-02"]);
    assert_eq!(
        json(&explicit)["policy"]["source"]["ref_name"],
        "policy-next"
    );
}

#[test]
fn show_and_explain_survive_pruned_source_data() {
    let (repo, data) = (repo(false), store());
    let fake = fixture(data.path(), "VIOLATED");
    let out = review(
        data.path(),
        repo.path(),
        &["--fake-responses", fake.to_str().expect("path")],
    );
    let saved = json(&out);
    let run = saved["review_result"]["run_id"].as_str().expect("run id");
    // Source snapshot and diff expire after 7 days, the result after 30.
    let mut removed = 0;
    for file in files(&data.path().join(".sych-data/artifacts")) {
        let doc: Value = serde_json::from_slice(&fs::read(&file).expect("read")).expect("json");
        if doc == saved["snapshot"] || doc == saved["diff_manifest"] {
            fs::remove_file(file).expect("remove");
            removed += 1;
        }
    }
    assert_eq!(removed, 2);

    let shown = sych(data.path(), &["show", "--run", run]);
    assert_eq!(
        shown.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&shown.stderr)
    );
    let shown = json(&shown);
    assert!(shown["snapshot"].is_null() && shown["diff_manifest"].is_null());
    assert_eq!(shown["review_result"], saved["review_result"]);
    let md = sych(data.path(), &["show", "--run", run, "--format", "markdown"]);
    assert!(String::from_utf8_lossy(&md.stdout).contains("- unavailable: the source snapshot"));
    let explained = sych(
        data.path(),
        &["explain", "--run", run, "--obligation", "EFFECT-01"],
    );
    assert_eq!(explained.status.code(), Some(0));
    assert_eq!(json(&explained)["obligation"]["status"], "VIOLATED");
}

fn files(dir: &Path) -> Vec<PathBuf> {
    fs::read_dir(dir)
        .expect("dir")
        .map(|e| e.expect("entry").path())
        .flat_map(|p| if p.is_dir() { files(&p) } else { vec![p] })
        .collect()
}

#[test]
fn relative_database_url_resolves_next_to_the_config() {
    let (home, cwd_a, cwd_b) = (TempDir::new(), TempDir::new(), TempDir::new());
    let (home, cwd_a, cwd_b) = (home.expect("dir"), cwd_a.expect("dir"), cwd_b.expect("dir"));
    let config = home.path().join("sych.yaml");
    let text =
        "schema_version: 1\nmode: personal\nstorage:\n  database_url_env: SYCH_TEST_DATABASE\n";
    fs::write(&config, text).expect("config");
    for cwd in [cwd_a.path(), cwd_b.path()] {
        let out = Command::new(env!("CARGO_BIN_EXE_sych"))
            .current_dir(cwd)
            .args(["db", "migrate", "--config"])
            .arg(&config)
            .env("SYCH_TEST_DATABASE", "sqlite:relative.db")
            .output()
            .expect("run sych");
        assert_eq!(
            out.status.code(),
            Some(0),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(!cwd.join("relative.db").exists());
    }
    assert!(home.path().join("relative.db").exists());
}
