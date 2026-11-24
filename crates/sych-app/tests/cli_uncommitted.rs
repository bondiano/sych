//! The installed CLI reviews staged and working-tree changes through a pinned Git snapshot.

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;
use std::process::{Command, Output};

use serde_json::Value;
use sqlx::SqlitePool;
use sqlx::sqlite::SqliteConnectOptions;
use tempfile::TempDir;

fn git(repo: &Path, args: &[&str]) -> Vec<u8> {
    let out = Command::new("git")
        .current_dir(repo)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .args([
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.invalid",
        ])
        .args(["-c", "commit.gpgsign=false"])
        .args(args)
        .output()
        .expect("git");
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    out.stdout
}

fn sych(data: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_sych"))
        .current_dir(data)
        .env("XDG_CONFIG_HOME", data.join("user-config"))
        .env("XDG_DATA_HOME", data.join("user-data"))
        .args(args)
        .output()
        .expect("sych")
}

fn successful(data: &Path, args: &[&str]) -> Vec<u8> {
    let out = sych(data, args);
    assert!(
        out.status.success(),
        "sych {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    out.stdout
}

fn changed_files(report: &Value) -> BTreeSet<&str> {
    report["diff_manifest"]["files"]
        .as_array()
        .expect("diff manifest")
        .iter()
        .map(|change| change["new"]["path"].as_str().expect("path"))
        .collect()
}

fn commit(report: &Value) -> &str {
    report["snapshot"]["head"]
        .as_str()
        .expect("head")
        .split_once(':')
        .expect("algorithm")
        .1
}

fn run_id(report: &Value) -> &str {
    report["review_result"]["run_id"].as_str().expect("run id")
}

fn prepare_repo(dir: &Path) {
    git(dir, &["init", "-q", "-b", "main"]);
    fs::write(dir.join(".gitignore"), "ignored.txt\n").expect("ignore");
    for path in ["staged.txt", "modified.txt"] {
        fs::write(dir.join(path), "Original\n").expect("initial file");
    }
    git(dir, &["add", "."]);
    git(dir, &["commit", "-qm", "initial"]);
    fs::write(dir.join("staged.txt"), "In index\n").expect("index edit");
    git(dir, &["add", "staged.txt"]);
    fs::write(dir.join("staged.txt"), "After staging\n").expect("worktree edit");
    fs::write(dir.join("modified.txt"), "Not staged\n").expect("worktree edit");
    fs::write(dir.join("untracked.txt"), "New file\n").expect("untracked");
    fs::write(dir.join("ignored.txt"), "Ignored file\n").expect("ignored");
}

#[tokio::test]
async fn review_pins_the_requested_uncommitted_state_without_changing_the_checkout() {
    let repo = TempDir::new().expect("repo");
    let data = TempDir::new().expect("data");
    let dir = repo.path();
    prepare_repo(dir);
    let head = fs::read(dir.join(".git/HEAD")).expect("HEAD before");
    let branches = git(dir, &["for-each-ref", "refs/heads"]);
    let index = fs::read(dir.join(".git/index")).expect("real index");

    fs::create_dir(data.path().join(".sych-data")).expect("local store");
    successful(data.path(), &["db", "migrate"]);
    let repo_arg = dir.to_str().expect("UTF-8 path");
    let review = |mode| {
        let out = sych(
            data.path(),
            &[
                "review",
                "--repo",
                repo_arg,
                "--base",
                "main",
                "--models",
                "fake",
                "--semantic",
                "off",
                mode,
            ],
        );
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(out.status.success(), "{stderr}");
        assert!(stderr.contains("(uncommitted)"), "{stderr}");
        serde_json::from_slice::<Value>(&out.stdout).expect("review JSON")
    };
    let staged = review("--staged");
    assert_eq!(staged["snapshot"]["head_kind"], "STAGED");
    assert_eq!(changed_files(&staged), BTreeSet::from(["staged.txt"]));
    let worktree = review("--worktree");
    assert_eq!(worktree["snapshot"]["head_kind"], "WORKTREE");
    assert_eq!(
        changed_files(&worktree),
        BTreeSet::from(["modified.txt", "staged.txt", "untracked.txt"])
    );

    git(dir, &["gc", "--prune=now"]);
    for (report, contents, label) in [
        (&staged, "In index\n", "staged changes (uncommitted)"),
        (
            &worktree,
            "After staging\n",
            "working tree changes (uncommitted)",
        ),
    ] {
        let oid = commit(report);
        assert_eq!(
            git(dir, &["show", &format!("{oid}:staged.txt")]),
            contents.as_bytes()
        );
        let run = run_id(report);
        let shown = successful(data.path(), &["show", "--run", run, "--format", "markdown"]);
        assert!(String::from_utf8_lossy(&shown).contains(label));
    }

    assert_eq!(
        fs::read(dir.join(".git/index")).expect("index after"),
        index
    );
    assert_eq!(fs::read(dir.join(".git/HEAD")).expect("HEAD after"), head);
    assert_eq!(git(dir, &["for-each-ref", "refs/heads"]), branches);
    for (path, contents) in [
        ("staged.txt", "After staging\n"),
        ("modified.txt", "Not staged\n"),
        ("untracked.txt", "New file\n"),
        ("ignored.txt", "Ignored file\n"),
    ] {
        assert_eq!(
            fs::read_to_string(dir.join(path)).expect("worktree file"),
            contents
        );
    }

    let pins = git(dir, &["for-each-ref", "refs/sych/local"]);
    let rejected = sych(
        data.path(),
        &[
            "review", "--repo", repo_arg, "--base", "missing", "--models", "fake", "--staged",
        ],
    );
    assert_eq!(rejected.status.code(), Some(2));
    assert_eq!(
        git(dir, &["for-each-ref", "refs/sych/local"]),
        pins,
        "failed review releases its pin"
    );
    verify_local_prune(data.path(), dir, &staged, &worktree).await;
}

#[test]
fn review_rejects_conflicting_head_modes() {
    let data = TempDir::new().expect("data");
    for options in [
        &["--staged", "--worktree"][..],
        &["--staged", "--head", "HEAD"],
        &["--worktree", "--change", "github:gh:owner/repo:1"],
    ] {
        let out = sych(
            data.path(),
            &[&["review", "--models", "fake"][..], options].concat(),
        );
        assert_eq!(out.status.code(), Some(2), "{options:?}");
        assert!(String::from_utf8_lossy(&out.stderr).contains("cannot be used with"));
    }
}

async fn verify_local_prune(data: &Path, repo: &Path, expired: &Value, retained: &Value) {
    let run = run_id(expired);
    let db = SqlitePool::connect_with(
        SqliteConnectOptions::new().filename(data.join(".sych-data/sych.db")),
    )
    .await
    .expect("open test database");
    let rows = sqlx::query(
        "UPDATE artifacts SET retain_until = 0 WHERE sha256 IN (
           SELECT sha256 FROM run_artifacts WHERE run_id = $1 AND role = 'source_snapshot'
         )",
    )
    .bind(run)
    .execute(&db)
    .await
    .expect("expire source")
    .rows_affected();
    assert_eq!(rows, 1);
    db.close().await;

    let git_ref = |report| format!("refs/sych/local/{}", commit(report));
    let expired_ref = git_ref(expired);
    let retained_ref = git_ref(retained);
    let refs = || {
        String::from_utf8(git(
            repo,
            &["for-each-ref", "--format=%(refname)", "refs/sych/local"],
        ))
        .expect("refs")
    };
    let dry = successful(data, &["cache", "prune", "--dry-run"]);
    assert!(String::from_utf8_lossy(&dry).contains(&expired_ref));
    assert!(refs().lines().any(|name| name == expired_ref));
    successful(data, &["cache", "prune"]);
    assert_eq!(refs().trim_end(), retained_ref);
    let report = successful(data, &["show", "--run", run]);
    let report: Value = serde_json::from_slice(&report).expect("saved review");
    assert!(report["snapshot"].is_null());
    assert_eq!(report["review_result"], expired["review_result"]);
}
