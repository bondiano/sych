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

mod common;

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
/// last argument (an absolute path) as a file URI; the message reports the environment and arguments
/// it saw.
/// Like Ruff, it writes its report to the file an `--output-file=` argument names.
const STAND_IN: &str = r#"
for f in "$@"; do
  case "$f" in --output-file=*) echo owned > "${f#--output-file=}" ;; esac
  last="$f"
done
case " $* " in
  *" -shellcheck= "*)
    printf '[{"message":"bad","filepath":"%s","line":2,"kind":"expression"}]' "$last" ;;
  *)
    printf '{"version":"2.1.0","runs":[{"tool":{"driver":{"name":"stand-in"}},"results":[{"ruleId":"X1","level":"error","message":{"text":"PATH=[%s] HOME=%s ARGS=[%s]"},"locations":[{"physicalLocation":{"artifactLocation":{"uri":"file://%s"},"region":{"startLine":2}}}]}]}]}' "$PATH" "$HOME" "$*" "$last" ;;
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
    let shebang = format!("#!{}", common::host_sh().display());
    fs::write(&script, shebang + STAND_IN).expect("script");
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
        ("db/q.sql", "select 1;\nselect 2;\n"),
        ("src/a.php", "<?php\n$a = 1;\n"),
        ("R/a.R", "x <- 1\ny <- 2\n"),
        ("src/core.clj", "(ns core)\n(def a 1)\n"),
        (".clj-kondo/config.edn", "{:hooks {:analyze-call {}}}\n"),
        (".github/workflows/ci.yml", "on: push\njobs: {}\n"),
        ("docs/readme.md", "text\n"),
    ];
    let head = [
        ("pyproject.toml", "[tool.ruff]\nselect = []\n"),
        ("app/a.py", "x = 1\ny = 3\n"),
        ("web/a.js", "let a;\nlet c;\n"),
        ("db/q.sql", "select 1;\nselect 3;\n"),
        ("src/a.php", "<?php\n$a = 2;\n"),
        ("R/a.R", "x <- 1\ny <- 3\n"),
        ("src/core.clj", "(ns core)\n(def a 2)\n"),
        (
            ".github/workflows/ci.yml",
            "on: pull_request_target\njobs: {}\n",
        ),
        ("docs/readme.md", "more\n"),
    ];
    let facts = collect(&base, &head);
    let tools: Vec<&str> = facts.iter().map(|f| f.producer.name.as_str()).collect();
    assert_eq!(
        tools,
        [
            "ruff",
            "oxlint",
            "sqruff",
            "mago",
            "jarl",
            "clj-kondo",
            "actionlint",
            "zizmor"
        ]
    );
    assert!(facts.iter().all(|f| f.status == BackendStatus::Ready));

    for (linter, file) in [
        (2, "db/q.sql"),
        (3, "src/a.php"),
        (4, "R/a.R"),
        (5, "src/core.clj"),
    ] {
        let d = &facts[linter].diagnostics[0];
        assert_eq!((d.path.as_str(), d.line), (file, 2));
        assert!(d.code_ref.is_some(), "{file} anchored on head");
    }

    // clj-kondo reads an empty Sych-owned config dir, never the project's hooks.
    let kondo = &facts[5];
    let args = kondo.diagnostics[0].message.as_str();
    assert!(args.contains("--repro --cache false"), "{args}");
    assert!(args.contains("--config-dir /"), "{args}");
    assert!(args.ends_with("--lint src/core.clj]"), "{args}");
    assert!(kondo.config.is_empty());
    assert!(
        kondo
            .limitations
            .iter()
            .any(|l| l.contains(".clj-kondo configuration was not read"))
    );

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

    let actionlint = &facts[6];
    assert_eq!(actionlint.diagnostics[0].path, ".github/workflows/ci.yml");
    assert!(actionlint.diagnostics[0].code_ref.is_some());
}

#[test]
fn a_file_named_like_an_option_stays_a_file() {
    let outside = tempfile::tempdir().expect("outside");
    let canary = outside.path().join("canary.py");
    fs::write(&canary, "kept\n").expect("canary");
    let name = format!("--output-file={}", canary.display());
    let facts = collect(&[("a.py", "x = 1\n")], &[(name.as_str(), "x = 1\ny = 2\n")]);
    assert_eq!(fs::read_to_string(&canary).expect("canary"), "kept\n");
    let ruff = &facts[0];
    assert_eq!(ruff.status, BackendStatus::Ready);
    assert_eq!(ruff.diagnostics[0].path, name);
}

#[test]
fn nothing_to_lint_runs_nothing() {
    let facts = collect(&[("docs/a.md", "a\n")], &[("docs/a.md", "b\n")]);
    assert!(facts.is_empty());
}
