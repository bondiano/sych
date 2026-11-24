//! Local review defaults through the installed CLI and real Git repositories.

use std::fs;
use std::path::Path;
use std::process::Output;

use serde_json::Value;
use tempfile::TempDir;

use super::{git, json, sych};

fn text_repository() -> TempDir {
    let dir = TempDir::new().expect("repo");
    let cwd = dir.path();
    git(cwd, &["init", "-q", "-b", "main"]);
    fs::write(cwd.join("README.md"), "base\n").expect("base");
    git(cwd, &["add", "README.md"]);
    git(cwd, &["commit", "-qm", "base"]);
    git(cwd, &["checkout", "-qb", "release"]);
    fs::write(cwd.join("README.md"), "release\n").expect("release");
    git(cwd, &["commit", "-qam", "release"]);
    git(cwd, &["checkout", "-qb", "topic", "main"]);
    fs::write(cwd.join("README.md"), "topic\n").expect("topic");
    git(cwd, &["commit", "-qam", "topic"]);
    dir
}

fn reviewed(cwd: &Path, options: &[&str]) -> (Output, Value) {
    let out = sych(cwd, &[&["review"][..], options].concat());
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let report = json(&out);
    (out, report)
}

#[test]
fn review_without_flags_selects_the_default_branch_and_saves_a_run() {
    let repo = text_repository();
    let cwd = repo.path();
    git(
        cwd,
        &["update-ref", "refs/remotes/origin/release", "release"],
    );
    git(
        cwd,
        &[
            "symbolic-ref",
            "refs/remotes/origin/HEAD",
            "refs/remotes/origin/release",
        ],
    );
    fs::write(cwd.join("README.md"), "unstaged\n").expect("dirty worktree");
    let (out, report) = reviewed(cwd, &[]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("No model was selected or called"),
        "{stderr}"
    );
    assert_eq!(
        stderr.matches("reviewing committed changes only").count(),
        1
    );
    assert!(stderr.contains("--staged") && stderr.contains("--worktree"));
    assert_eq!(report["snapshot"]["target_ref"], "refs/remotes/origin/HEAD");
    assert!(cwd.join("user-data/sych/sych.db").is_file());
    assert!(!cwd.join(".sych-data").exists());
    let run = report["review_result"]["run_id"].as_str().expect("run");
    let shown = sych(cwd, &["show", "--run", run]);
    assert!(
        shown.status.success(),
        "{}",
        String::from_utf8_lossy(&shown.stderr)
    );
    assert_eq!(json(&shown), report);

    git(cwd, &["symbolic-ref", "-d", "refs/remotes/origin/HEAD"]);
    let (_, main) = reviewed(cwd, &[]);
    assert_eq!(main["snapshot"]["target_ref"], "refs/heads/main");
    git(cwd, &["branch", "-m", "main", "master"]);
    let (_, master) = reviewed(cwd, &[]);
    assert_eq!(master["snapshot"]["target_ref"], "refs/heads/master");
    git(cwd, &["branch", "-m", "master", "baseline"]);
    let missing = sych(cwd, &["review"]);
    assert_eq!(missing.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&missing.stderr).contains("specify --base"));
}

#[test]
fn review_positional_targets_match_explicit_comparisons() {
    let repo = text_repository();
    let cwd = repo.path();
    let (_, branch) = reviewed(cwd, &["release"]);
    let (_, implicit) = reviewed(cwd, &["--head", "release"]);
    assert_eq!(branch["snapshot"]["id"], implicit["snapshot"]["id"]);

    let (_, two_dot) = reviewed(cwd, &["release..HEAD"]);
    let (_, explicit) = reviewed(cwd, &["--diff-base", "release", "--head", "HEAD"]);
    assert_eq!(two_dot["snapshot"]["id"], explicit["snapshot"]["id"]);
    assert_eq!(
        two_dot["snapshot"]["target_tip"],
        two_dot["snapshot"]["diff_base"]
    );
    let (_, three_dot) = reviewed(cwd, &["release...HEAD"]);
    let (_, explicit) = reviewed(cwd, &["--base", "release", "--head", "HEAD"]);
    assert_eq!(three_dot["snapshot"]["id"], explicit["snapshot"]["id"]);
    assert_ne!(
        two_dot["snapshot"]["diff_base"],
        three_dot["snapshot"]["diff_base"]
    );
    for flag in ["--base", "--head", "--diff-base", "--change"] {
        let out = sych(cwd, &["review", "release", flag, "main"]);
        assert_eq!(out.status.code(), Some(2), "{flag}");
        assert!(
            String::from_utf8_lossy(&out.stderr).contains("cannot be used with"),
            "{flag}"
        );
    }
    for range in ["..HEAD", "main...", "main....HEAD", "main..HEAD..release"] {
        let out = sych(cwd, &["review", range]);
        assert_eq!(out.status.code(), Some(2), "{range}");
        assert!(String::from_utf8_lossy(&out.stderr).contains("expected REF, A..B or A...B"));
    }
}

#[test]
fn review_loads_user_config_and_allows_an_explicit_config_override() {
    let repo = text_repository();
    let cwd = repo.path();
    fs::create_dir_all(cwd.join("user-config/sych")).expect("config directory");
    let config = cwd.join("user-config/sych/sych.yaml");
    let base = "schema_version: 1\nmode: personal\n";
    fs::write(&config, format!("{base}default_models: missing\n")).expect("config");
    let invalid = sych(cwd, &["review"]);
    assert_eq!(invalid.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&invalid.stderr).contains("default_models"));

    fs::write(cwd.join("explicit.yaml"), base).expect("explicit config");
    let (out, _) = reviewed(cwd, &["--config", "explicit.yaml"]);
    assert!(String::from_utf8_lossy(&out.stderr).contains("No model was selected or called"));

    fs::write(&config, format!("{base}default_models: fake\n")).expect("config");
    let (out, _) = reviewed(cwd, &[]);
    assert!(!String::from_utf8_lossy(&out.stderr).contains("No model was selected or called"));
}
