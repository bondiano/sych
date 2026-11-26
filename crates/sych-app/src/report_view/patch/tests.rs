use std::fs;
use std::path::Path;
use std::process::Command;

use sych_adapters::git::SnapshotRequest;
use sych_core::{ReviewResult, decode};
use tempfile::TempDir;

use super::*;

fn git(dir: &Path, args: &[&str]) {
    let result = Command::new("git")
        .current_dir(dir)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .args([
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.invalid",
        ])
        .args(["-c", "commit.gpgsign=false", "-c", "core.autocrlf=false"])
        .args(args)
        .output()
        .expect("git");
    assert!(
        result.status.success(),
        "{args:?}: {}",
        String::from_utf8_lossy(&result.stderr)
    );
}

fn fixture(path: &str, text: &str) -> (TempDir, LocalSnapshot, RepoPath) {
    let dir = TempDir::new().expect("temp");
    git(dir.path(), &["init", "-q", "-b", "main"]);
    fs::write(dir.path().join(path), text).expect("fixture");
    git(dir.path(), &["add", "--", path]);
    git(dir.path(), &["commit", "-qm", "fixture"]);
    let snap = LocalSnapshot::capture(&SnapshotRequest {
        repo: dir.path(),
        base: "main",
        head: "HEAD",
        diff_base: None,
        max_blob_bytes: 1 << 20,
    })
    .expect("pin");
    (dir, snap, path.parse().expect("path"))
}

fn finding(
    snap: &LocalSnapshot,
    path: &RepoPath,
    start: u32,
    end: u32,
    replacement: &str,
) -> Finding {
    let bytes = include_str!("../../../../../fixtures/core/v1/review-result.json");
    let mut finding = decode::<ReviewResult>(bytes)
        .expect("result")
        .findings
        .remove(0);
    finding.id = FindingId::new_v7();
    finding.lifecycle = Lifecycle::Open;
    finding.suggested_change = Some(SuggestedChange {
        code: snap
            .code_ref(SourceSide::Head, path, start, end)
            .expect("span"),
        replacement: replacement.into(),
    });
    finding
}

#[test]
fn two_fixes_form_one_patch_that_applies_to_crlf_and_unterminated_lines() {
    let filename = "review пример.txt";
    let original =
        "zero\r\none\r\ntwo\r\nthree\r\nfour\r\nfive\r\nsix\r\nseven\r\neight\r\nnine\r\nlast";
    let (dir, snap, path) = fixture(filename, original);
    let a = finding(&snap, &path, 2, 2, "updated\ninserted");
    let b = finding(&snap, &path, 11, 11, "before end\nend");
    let (diff, previews) = fixes(Some(&snap), &[&b, &a], Redactor::builtin());
    assert!(previews.values().all(|preview| preview.patch.is_some()));
    assert_eq!(diff.matches("@@ -").count(), 2, "{diff}");
    assert!(diff.contains("\\ No newline at end of file\n"));
    let patch_path = dir.path().join("fixes.patch");
    fs::write(patch_path, diff).expect("write patch");
    git(dir.path(), &["apply", "--check", "fixes.patch"]);
    git(dir.path(), &["apply", "fixes.patch"]);
    assert_eq!(
        fs::read_to_string(dir.path().join(filename)).expect("applied"),
        "zero\r\nupdated\r\ninserted\r\ntwo\r\nthree\r\nfour\r\nfive\r\nsix\r\nseven\r\neight\r\nnine\r\nbefore end\r\nend"
    );
}

#[test]
fn no_op_is_not_a_patch_and_overlaps_are_excluded_from_the_combination() {
    let (dir, snap, path) = fixture("text.txt", "one\ntwo\nthree\n");
    let identical = finding(&snap, &path, 1, 1, "one");
    let a = finding(&snap, &path, 1, 1, "first");
    let duplicate = finding(&snap, &path, 1, 1, "first");
    let overlap = finding(&snap, &path, 1, 2, "both");
    let adjacent = finding(&snap, &path, 2, 2, "");
    let (diff, previews) = fixes(
        Some(&snap),
        &[&identical, &a, &duplicate, &overlap, &adjacent],
        Redactor::builtin(),
    );
    assert!(previews[&identical.id].patch.is_none());
    assert!(
        previews[&identical.id]
            .note
            .as_deref()
            .expect("no-op note")
            .contains("identical")
    );
    assert!(previews[&duplicate.id].note.is_some());
    assert!(previews[&overlap.id].note.is_some());
    fs::write(dir.path().join("fixes.patch"), diff).expect("write patch");
    git(dir.path(), &["apply", "--check", "fixes.patch"]);
    git(dir.path(), &["apply", "fixes.patch"]);
    assert_eq!(
        fs::read_to_string(dir.path().join("text.txt")).expect("applied"),
        "first\nthree\n"
    );
}

#[test]
fn omissions_keep_the_finding_without_emitting_protected_bytes() {
    let (_dir, snap, path) = fixture("safe.txt", "before\nCUSTOM_SENTINEL\nafter\n");
    let custom = Redactor::new(&[], &[], &["CUSTOM_SENTINEL".into()]).expect("redactor");
    let suggestion = finding(&snap, &path, 1, 1, "changed");
    let bidi = finding(&snap, &path, 1, 1, "hidden\u{202e}suffix");
    for (snap, redactor, findings) in [
        (None, Redactor::builtin(), &[&suggestion][..]),
        (Some(&snap), &custom, &[&suggestion][..]),
        (Some(&snap), Redactor::builtin(), &[&bidi][..]),
    ] {
        let (diff, previews) = fixes(snap, findings, redactor);
        assert_eq!(diff, "");
        assert!(previews[&findings[0].id].patch.is_none());
        assert!(previews[&findings[0].id].note.is_some());
    }
}

#[test]
fn whole_file_masks_protect_context_and_composed_fixes() {
    let content = "begin\none\ntwo\nthree\nfour\nfive\nsix\nseven\nsecret\nend\n";
    let (_dir, snap, path) = fixture("safe.txt", content);
    let a = finding(&snap, &path, 3, 3, "changed");
    let hidden = Redactor::new(&[], &[], &["(?s)begin.*secret".into()]).expect("pattern");
    let (diff, previews) = fixes(Some(&snap), &[&a], &hidden);
    assert_eq!(diff, "");
    assert!(previews[&a.id].patch.is_none());

    let hidden = Redactor::new(&[], &[], &["(?s)marker-one.*marker-two".into()]).expect("pattern");
    let a = finding(&snap, &path, 1, 1, "marker-one");
    let b = finding(&snap, &path, 9, 9, "marker-two");
    let (diff, previews) = fixes(Some(&snap), &[&a, &b], &hidden);
    assert_eq!(diff, "");
    assert!(
        previews
            .values()
            .all(|p| p.patch.is_some() && p.note.is_some())
    );
}
