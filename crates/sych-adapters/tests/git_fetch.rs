//! Fetching pinned commits into a bare mirror: exact ids over a local file URL,
//! readable by the snapshot service afterwards; anything but full ids or https/file is refused.

use std::fs;
use std::path::Path;
use std::process::Command;
use std::time::Duration;

use sych_adapters::git::{LocalSnapshot, SnapshotError, SnapshotRequest, fetch_commits};

fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .current_dir(dir)
        .args([
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "-c",
            "commit.gpgsign=false",
        ])
        .args(args)
        .output()
        .expect("git");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout)
        .expect("utf8")
        .trim()
        .to_owned()
}

#[test]
fn pinned_commits_are_fetched_and_snapshot_reads_them() {
    let dir = tempfile::tempdir().expect("tempdir");
    let origin = dir.path().join("origin");
    fs::create_dir(&origin).expect("dir");
    git(&origin, &["init", "-q", "-b", "main"]);
    fs::write(origin.join("a.rs"), "fn a() {}\n").expect("write");
    git(&origin, &["add", "."]);
    git(&origin, &["commit", "-q", "-m", "base"]);
    let base = git(&origin, &["rev-parse", "HEAD"]);
    git(&origin, &["checkout", "-q", "-b", "pr"]);
    fs::write(origin.join("a.rs"), "fn a() { b() }\n").expect("write");
    git(&origin, &["commit", "-q", "-am", "head"]);
    let head = git(&origin, &["rev-parse", "HEAD"]);

    let mirror = dir.path().join("mirror.git");
    let url = format!("file://{}", origin.display());
    fetch_commits(
        &mirror,
        &url,
        None,
        &[&head, &base],
        Duration::from_secs(30),
    )
    .expect("fetch");
    fetch_commits(
        &mirror,
        &url,
        None,
        &[&head, &base],
        Duration::from_secs(30),
    )
    .expect("again");
    let snap = LocalSnapshot::capture(&SnapshotRequest {
        repo: &mirror,
        base: &base,
        head: &head,
        diff_base: None,
        max_blob_bytes: 1 << 20,
    })
    .expect("snapshot");
    assert_eq!(snap.manifest.files.len(), 1);

    for (url, commits) in [
        ("http://example.com/r.git", vec![head.as_str()]),
        (url.as_str(), vec!["main"]),
    ] {
        let refused = fetch_commits(&mirror, url, None, &commits, Duration::from_secs(5));
        assert!(
            matches!(refused, Err(SnapshotError::InvalidInput(_))),
            "{url} {commits:?}"
        );
    }
}
