//! A two-commit Git repository and its pinned linters, each a scripted stand-in.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::{env, fs};

use sych_adapters::git::{LocalSnapshot, SnapshotRequest};
use sych_adapters::lint;
use sych_adapters::semantic::Options;
use sych_adapters::tools::PinnedTool;
use sych_core::LintFacts;
use sych_core::config::Tooling;

pub fn git(dir: &Path, args: &[&str]) {
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

pub fn commit(dir: &Path, files: &[(&str, &str)], message: &str) {
    for (path, text) in files {
        let path = dir.join(path);
        fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
        fs::write(path, text).expect("write");
    }
    git(dir, &["add", "-A"]);
    git(dir, &["commit", "-q", "-m", message]);
}

/// Each pinned tool resolves to `script(name)` in a directory of its own.
pub fn collect_with(
    base: &[(&str, &str)],
    head: &[(&str, &str)],
    script: fn(&str) -> &'static str,
) -> Vec<LintFacts> {
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
    let shebang = format!("#!{}", super::host_sh().display());
    let resolve = move |tool: &'static PinnedTool, _: &str| {
        let path: PathBuf = bin.path().join(&tool.name).join(&tool.name);
        if !path.exists() {
            fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
            super::write_executable(&path, &format!("{shebang}{}", script(&tool.name)));
        }

        Ok(path)
    };
    let state = tempfile::tempdir().expect("state");
    let options = Options {
        resolve: &resolve,
        cache: None,
        tooling: &Tooling::default(),
        state_dir: state.path(),
    };
    lint::collect(&snapshot, &options)
}
