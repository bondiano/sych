//! Pinned linters through a scripted stand-in on a real Git snapshot: the empty environment,
//! absolute locations mapped to repository paths, exact anchors and a changed linter config.

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::{env, fs};

use sych_adapters::git::{LocalSnapshot, SnapshotRequest};
use sych_adapters::lint;
use sych_adapters::tools::PinnedTool;
use sych_core::{BackendStatus, LintFacts};

fn git(dir: &Path, args: &[&str]) {
    let out = Command::new("git")
        .current_dir(dir)
        .env_clear()
        .env("PATH", env::var_os("PATH").unwrap_or_default())
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_AUTHOR_NAME", "Fixture")
        .env("GIT_AUTHOR_EMAIL", "fixture@example.invalid")
        .env("GIT_COMMITTER_NAME", "Fixture")
        .env("GIT_COMMITTER_EMAIL", "fixture@example.invalid")
        .args(args)
        .output()
        .expect("git runs");
    assert!(out.status.success(), "git {args:?}");
}

fn commit(dir: &Path, files: &[(&str, &str)], message: &str) {
    for (path, text) in files {
        let path = dir.join(path);
        fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
        fs::write(path, text).expect("write");
    }
    git(dir, &["add", "-A"]);
    git(dir, &["commit", "-q", "-m", message]);
}

/// Answers like actionlint when given `-shellcheck=`, else with SARIF citing line 2 of its
/// last argument through an absolute URI; the message reports the environment it saw.
const STAND_IN: &str = r#"#!/bin/sh
for f in "$@"; do last="$f"; done
case " $* " in
  *" -shellcheck= "*)
    printf '[{"message":"bad","filepath":"%s","line":2,"kind":"expression"}]' "$last" ;;
  *)
    printf '{"version":"2.1.0","runs":[{"tool":{"driver":{"name":"stand-in"}},"results":[{"ruleId":"X1","level":"error","message":{"text":"PATH=[%s] HOME=%s"},"locations":[{"physicalLocation":{"artifactLocation":{"uri":"file://%s/%s"},"region":{"startLine":2}}}]}]}]}' "$PATH" "$HOME" "$PWD" "$last" ;;
esac
"#;

fn collect(base: &[(&str, &str)], head: &[(&str, &str)]) -> Vec<LintFacts> {
    let repo = tempfile::tempdir().expect("repo");
    git(repo.path(), &["init", "-q", "-b", "main"]);
    commit(repo.path(), base, "base");
    commit(repo.path(), head, "head");
    let snapshot = LocalSnapshot::capture(&SnapshotRequest {
        repo: repo.path(),
        base: "HEAD~1",
        head: "HEAD",
        diff_base: None,
        max_blob_bytes: 1 << 20,
    })
    .expect("snapshot");
    let bin = tempfile::tempdir().expect("bin");
    let script: PathBuf = bin.path().join("linter");
    fs::write(&script, STAND_IN).expect("script");
    fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).expect("chmod");
    let resolve = move |_: &'static PinnedTool, _: &str| Ok(script.clone());
    lint::collect(&snapshot, &resolve)
}

#[test]
fn linters_run_on_changed_files_in_an_empty_environment() {
    let base = [
        ("pyproject.toml", "[tool.ruff]\n"),
        ("app/a.py", "x = 1\ny = 2\n"),
        ("web/a.js", "let a;\nlet b;\n"),
        (".github/workflows/ci.yml", "on: push\njobs: {}\n"),
        ("docs/readme.md", "text\n"),
    ];
    let head = [
        ("pyproject.toml", "[tool.ruff]\nselect = []\n"),
        ("app/a.py", "x = 1\ny = 3\n"),
        ("web/a.js", "let a;\nlet c;\n"),
        (
            ".github/workflows/ci.yml",
            "on: pull_request_target\njobs: {}\n",
        ),
        ("docs/readme.md", "more\n"),
    ];
    let facts = collect(&base, &head);
    let tools: Vec<&str> = facts.iter().map(|f| f.producer.name.as_str()).collect();
    assert_eq!(tools, ["ruff", "oxlint", "actionlint", "zizmor"]);
    assert!(facts.iter().all(|f| f.status == BackendStatus::Ready));

    let ruff = &facts[0];
    assert_eq!(ruff.files, ["app/a.py"]);
    let d = &ruff.diagnostics[0];
    assert_eq!((d.path.as_str(), d.line), ("app/a.py", 2));
    let anchor = d.code_ref.as_ref().expect("anchored on head");
    assert_eq!(anchor.start_line, 2);
    assert!(d.message.starts_with("PATH=[] HOME="), "{}", d.message);
    assert!(!d.message.contains(env!("CARGO_MANIFEST_DIR")));
    assert_eq!(ruff.config, ["pyproject.toml"]);
    assert!(
        ruff.limitations
            .iter()
            .any(|l| l.contains("configuration changed"))
    );

    let actionlint = &facts[2];
    assert_eq!(actionlint.diagnostics[0].path, ".github/workflows/ci.yml");
    assert!(actionlint.diagnostics[0].code_ref.is_some());
}

#[test]
fn nothing_to_lint_runs_nothing() {
    let facts = collect(&[("docs/a.md", "a\n")], &[("docs/a.md", "b\n")]);
    assert!(facts.is_empty());
}
