//! Tour reader smoke: a stored tour is shown, revealed, completed and dismissed
//! through the CLI with no model and no config; a moved head is `STALE` with the run's code,
//! pruned source reads "source unavailable", and the review result never changes.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::Value;
use sych_app::storage::block_on;
use sych_core::analysis::tour_prompt::{DraftStep, TourDraft, assemble};
use sych_core::tour::{Question, QuestionKind, plan};
use sych_core::{ReviewResult, Timestamp};
use sych_storage::{ArtifactKind, Database, Storage, StoreConfig, TENANT_LOCAL, tour_role};
use tempfile::TempDir;

const RULES: &str = r#"schema_version: 1
rules:
  - id: EFFECT-01
    version: 1
    title: Order keeps its event
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

const FAKE: &str = "schema_version: 1
evidence:
  - key: save
    kind: source
    completeness: COMPLETE_FOR_QUERY
    trust_class: SOURCE_OBSERVATION
    code_refs: [{ path: backend/orders/store.go, start_line: 3, end_line: 6 }]
assessments:
  - { rule_id: EFFECT-01, status: VIOLATED, evidence: [save] }
";

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

fn repo() -> TempDir {
    let dir = TempDir::new().expect("tempdir");
    let d = dir.path();
    git(d, &["init", "-q", "-b", "main"]);
    fs::create_dir_all(d.join(".sych")).expect("mkdir");
    fs::create_dir_all(d.join("backend/orders")).expect("mkdir");
    fs::write(d.join(".sych/rules.yaml"), RULES).expect("write");
    let store = d.join("backend/orders/store.go");
    fs::write(
        &store,
        "package orders\n\nfunc Save(o Order) error {\n\treturn tx(o)\n}\n",
    )
    .expect("write");
    git(d, &["add", "-A"]);
    git(d, &["commit", "-q", "-m", "base"]);
    git(d, &["branch", "target"]);
    fs::write(
        &store,
        "package orders\n\nfunc Save(o Order) error {\n\tgo publish(o)\n\treturn nil\n}\n",
    )
    .expect("write");
    git(d, &["commit", "-q", "-am", "head"]);
    dir
}

fn sych(cwd: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_sych"))
        .current_dir(cwd)
        .args(args)
        .output()
        .expect("run sych")
}

fn ok(out: &Output) -> Value {
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).expect("json stdout")
}

fn files(dir: &Path) -> Vec<PathBuf> {
    fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .flat_map(|e| {
            let p = e.path();
            if p.is_dir() { files(&p) } else { vec![p] }
        })
        .collect()
}

/// What `sych tour` would have stored, with a fixed draft instead of a model.
fn store_tour(data: &Path, result: &ReviewResult) -> String {
    let plan = plan(result, 5);
    let draft = TourDraft {
        intent: "publish an order".into(),
        steps: plan
            .stops
            .iter()
            .map(|s| DraftStep {
                stop_id: s.id.clone(),
                title: format!("Stop {}", s.id),
                purpose: "where the event goes".into(),
                explanation: "the publish left the transaction".into(),
                question: Some(Question {
                    kind: QuestionKind::Comprehension,
                    text: "What if the process dies after commit?".into(),
                    reveal: "the event is lost".into(),
                }),
                example_snippets: Vec::new(),
            })
            .collect(),
    };
    let (tour, problems) = assemble(&plan, result, draft, "fixed".into());
    assert!(problems.is_empty(), "{problems:?}");
    let config = StoreConfig {
        database: Database::Sqlite(data.join(".sych-data/sych.db")),
        artifacts: data.join(".sych-data/artifacts"),
    };
    block_on(async {
        let storage = Storage::open(&config).await.expect("open");
        let run = storage
            .run_handle(TENANT_LOCAL, result.run_id)
            .await
            .expect("run");
        storage
            .put_artifact(
                &run,
                &tour_role(tour.tour_id),
                ArtifactKind::Tour,
                &tour,
                Timestamp::now(),
            )
            .await
            .expect("store");
    })
    .expect("runtime");
    plan.stops[0].id.clone()
}

#[test]
fn a_stored_tour_is_read_privately_marked_stale_and_survives_pruned_source() {
    let (repo, data) = (repo(), TempDir::new().expect("tempdir"));
    let (d, r) = (data.path(), repo.path().to_str().expect("utf-8"));
    assert_eq!(sych(d, &["db", "migrate"]).status.code(), Some(0));
    let fake = d.join("fake.yaml");
    fs::write(&fake, FAKE).expect("write");
    let saved = ok(&sych(
        d,
        &[
            "review",
            "--repo",
            r,
            "--base",
            "target",
            "--models",
            "fake",
            "--fake-responses",
            fake.to_str().expect("path"),
        ],
    ));
    let result: ReviewResult =
        serde_json::from_value(saved["review_result"].clone()).expect("result");
    let run = result.run_id.to_string();
    let none = sych(d, &["tour", "show", "--run", &run, "--repo", r]);
    assert_eq!(none.status.code(), Some(2), "no stored tour yet");
    let step = store_tour(d, &result);

    // No --config: no model profile exists, so nothing can be called.
    let shown = ok(&sych(d, &["tour", "show", "--run", &run, "--repo", r]));
    assert_eq!(shown["session"]["state"], "IN_PROGRESS");
    assert_eq!(shown["stale"], false);
    assert_eq!(shown["source_unavailable"], Value::Array(Vec::new()));
    assert_eq!(
        shown["tour"]["steps"][0]["question"]["reveal"], "",
        "not revealed yet"
    );
    let md = sych(
        d,
        &[
            "tour", "show", "--run", &run, "--repo", r, "--format", "markdown",
        ],
    );
    assert!(String::from_utf8_lossy(&md.stdout).contains("\tgo publish(o)\n"));
    let brief = sych(
        d,
        &[
            "tour", "show", "--run", &run, "--repo", r, "--brief", "--format", "markdown",
        ],
    );
    assert!(!String::from_utf8_lossy(&brief.stdout).contains("go publish"));

    let revealed = ok(&sych(
        d,
        &["tour", "reveal", "--run", &run, "--step", &step],
    ));
    assert_eq!(revealed["reveal"], "the event is lost");
    let shown = ok(&sych(d, &["tour", "show", "--run", &run, "--repo", r]));
    assert_eq!(
        shown["tour"]["steps"][0]["question"]["reveal"],
        "the event is lost"
    );
    assert_eq!(shown["session"]["revealed"][0], step.as_str());

    let done = ok(&sych(d, &["tour", "complete", "--run", &run]));
    assert_eq!(done["state"], "COMPLETED");
    assert_eq!(
        sych(d, &["tour", "dismiss", "--run", &run]).status.code(),
        Some(2)
    );
    let after = ok(&sych(d, &["show", "--run", &run]));
    assert_eq!(
        after["review_result"], saved["review_result"],
        "a session changes nothing shared"
    );

    // A new head: the old material stays, marked STALE.
    fs::write(
        repo.path().join("backend/orders/store.go"),
        "package orders\n",
    )
    .expect("write");
    git(repo.path(), &["commit", "-q", "-am", "next"]);
    let stale = sych(
        d,
        &[
            "tour", "show", "--run", &run, "--repo", r, "--format", "markdown",
        ],
    );
    let text = String::from_utf8_lossy(&stale.stdout);
    assert!(
        text.contains("STALE") && text.contains("\tgo publish(o)\n"),
        "{text}"
    );

    // Pruned source: locations stay, the code is "source unavailable".
    for file in files(&d.join(".sych-data/artifacts")) {
        let doc: Value = serde_json::from_slice(&fs::read(&file).expect("read")).expect("json");
        if doc == saved["snapshot"] {
            fs::remove_file(file).expect("remove");
        }
    }
    let gone = ok(&sych(d, &["tour", "show", "--run", &run, "--repo", r]));
    assert!(
        !gone["source_unavailable"]
            .as_array()
            .expect("list")
            .is_empty()
    );
    assert_eq!(gone["session"]["state"], "COMPLETED");
}

#[test]
fn a_run_id_is_checked_and_the_generator_still_needs_its_flags() {
    let dir = TempDir::new().expect("tempdir");
    assert_eq!(
        sych(dir.path(), &["tour", "show", "--run", "nope"])
            .status
            .code(),
        Some(2)
    );
    assert_eq!(
        sych(dir.path(), &["tour"]).status.code(),
        Some(2),
        "--run and --config are required"
    );
}

/// Every file under `dir` with its bytes; `SQLite` rewrites its own files on open, so the
/// database is compared through `show` instead.
fn stored(dir: &Path) -> Vec<(PathBuf, Vec<u8>)> {
    let mut all: Vec<_> = files(dir)
        .into_iter()
        .filter(|p| !p.to_string_lossy().contains("sych.db"))
        .map(|p| {
            let bytes = fs::read(&p).expect("read");
            (p, bytes)
        })
        .collect();
    all.sort();
    all
}

#[test]
fn a_handoff_is_reproducible_consent_first_and_changes_no_file() {
    let (repo, data) = (repo(), TempDir::new().expect("tempdir"));
    let (d, r) = (data.path(), repo.path().to_str().expect("utf-8"));
    assert_eq!(sych(d, &["db", "migrate"]).status.code(), Some(0));
    let fake = d.join("fake.yaml");
    fs::write(&fake, FAKE).expect("write");
    let saved = ok(&sych(
        d,
        &[
            "review",
            "--repo",
            r,
            "--base",
            "target",
            "--models",
            "fake",
            "--fake-responses",
            fake.to_str().expect("path"),
        ],
    ));
    let result: ReviewResult =
        serde_json::from_value(saved["review_result"].clone()).expect("result");
    let (run, id) = (result.run_id.to_string(), result.findings[0].id.to_string());
    let (store_before, repo_before) = (stored(d), stored(repo.path()));
    let shown_before = ok(&sych(d, &["show", "--run", &run]));

    let args = [
        "handoff",
        "--run",
        &run,
        "--finding",
        &id,
        "--format",
        "markdown",
    ];
    let first = sych(d, &args);
    assert_eq!(
        first.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );
    let text = String::from_utf8(first.stdout.clone()).expect("utf-8");
    assert!(
        text.contains("## Procedure") && text.contains("explicit consent"),
        "{text}"
    );
    assert!(
        text.contains("EFFECT-01 v1 (VIOLATED)") && text.contains("**Fix criterion:**"),
        "{text}"
    );
    assert_eq!(sych(d, &args).stdout, first.stdout, "reproducible");
    assert_eq!(
        (stored(d), stored(repo.path())),
        (store_before, repo_before),
        "no artifact or source file changed"
    );
    assert_eq!(
        ok(&sych(d, &["show", "--run", &run])),
        shown_before,
        "the run is unchanged"
    );

    let foreign = sych(
        d,
        &[
            "handoff",
            "--run",
            &run,
            "--finding",
            "0192f000-0000-7000-8000-000000000000",
        ],
    );
    assert_eq!(
        foreign.status.code(),
        Some(2),
        "a foreign finding is refused"
    );
    let other = format!("sha256:{}", "0".repeat(64));
    let moved = sych(
        d,
        &[
            "handoff",
            "--run",
            &run,
            "--finding",
            &id,
            "--snapshot",
            &other,
        ],
    );
    assert_eq!(moved.status.code(), Some(2), "another snapshot is refused");
}
