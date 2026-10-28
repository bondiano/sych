//! The CI report corpus (roadmap M2.3): every supported format normalizes onto a real snapshot
//! with its bounds; malformed, XXE and over-deep reports are refused, not half-read.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use sych_adapters::ci::{CiError, import_as};
use sych_adapters::git::{LocalSnapshot, SnapshotRequest};
use sych_core::{CiFormat, TestStatus};

fn corpus(name: &str) -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/ci")
        .join(name);
    fs::read_to_string(path).expect("fixture")
}

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

/// A head that changes `src/lib.rs` and the test `tests/lib_test.rs`.
fn snapshot(dir: &Path) -> LocalSnapshot {
    git(dir, &["init", "-q", "-b", "main"]);
    fs::create_dir_all(dir.join("src")).expect("dir");
    fs::create_dir_all(dir.join("tests")).expect("dir");
    fs::write(dir.join("src/lib.rs"), "pub fn a() {}\n").expect("write");
    git(dir, &["add", "."]);
    git(dir, &["commit", "-q", "-m", "base"]);
    fs::write(
        dir.join("src/lib.rs"),
        "pub fn a() {}\npub fn b() {}\npub fn c() {}\n",
    )
    .expect("write");
    fs::write(dir.join("tests/lib_test.rs"), "#[test] fn saves() {}\n").expect("write");
    git(dir, &["add", "."]);
    git(dir, &["commit", "-q", "-m", "head"]);
    LocalSnapshot::capture(&SnapshotRequest {
        repo: dir,
        base: "HEAD~1",
        head: "HEAD",
        diff_base: None,
        max_blob_bytes: 1 << 20,
    })
    .expect("snapshot")
}

#[test]
fn every_format_normalizes_with_its_limits() {
    let dir = tempfile::tempdir().expect("tempdir");
    let snap = snapshot(dir.path());
    let import = |name: &str, format| {
        import_as(&snap, name, &corpus(name), None, Some(format), None).expect(name)
    };

    let sarif = import("sarif.json", CiFormat::Sarif);
    assert_eq!(
        sarif.diagnostics.len(),
        1,
        "the dependency location is dropped"
    );
    assert_eq!(
        sarif.diagnostics[0].code.as_deref(),
        Some("clippy::unwrap_used")
    );
    assert!(
        sarif
            .limitations
            .iter()
            .any(|l| l.contains("tool clippy 0.1.94"))
    );
    assert!(
        sarif
            .limitations
            .iter()
            .any(|l| l.contains("outside the repository"))
    );

    let cq = import("code-quality.json", CiFormat::GitlabCodeQuality);
    assert_eq!(cq.diagnostics.len(), 1, "a traversing path is dropped");
    assert_eq!(cq.diagnostics[0].level, "error");

    for (name, format) in [
        ("lcov.info", CiFormat::Lcov),
        ("cobertura.xml", CiFormat::Cobertura),
    ] {
        let cov = import(name, format);
        assert_eq!(cov.coverage.len(), 1, "{name}");
        assert_eq!(cov.coverage[0].path, "src/lib.rs");
        assert!(cov.coverage[0].hit.contains(&1) && !cov.coverage[0].hit.contains(&2));
    }

    let outcomes = import("outcomes.json", CiFormat::SychManifest);
    assert_eq!(
        outcomes.tests[0].status,
        TestStatus::Failed,
        "exit 101 is a failure"
    );
    let assertions: Vec<_> = outcomes
        .tests
        .iter()
        .filter(|t| t.suite == "assertion")
        .map(|t| t.status)
        .collect();
    assert_eq!(
        assertions,
        [TestStatus::Passed, TestStatus::Failed, TestStatus::Skipped],
        "an event order is compared whole; nothing recorded is not a pass"
    );
    assert!(
        outcomes
            .limitations
            .iter()
            .any(|l| l.starts_with("TEST_CHANGED: tests/lib_test.rs")),
        "{:?}",
        outcomes.limitations
    );
}

#[test]
fn malformed_xxe_and_deep_reports_are_refused_or_inert() {
    let dir = tempfile::tempdir().expect("tempdir");
    let snap = snapshot(dir.path());
    let refused = import_as(
        &snap,
        "bad.sarif",
        &corpus("malformed.sarif.json"),
        None,
        Some(CiFormat::Sarif),
        None,
    );
    assert!(
        matches!(refused, Err(CiError::Malformed { .. })),
        "{refused:?}"
    );

    // quick-xml never resolves external entities: the reference fails instead of reading a file.
    let xxe = import_as(
        &snap,
        "x.xml",
        &corpus("junit-xxe.xml"),
        None,
        Some(CiFormat::Junit),
        None,
    );
    if let Ok(facts) = &xxe {
        assert!(
            facts.tests.iter().all(|t| !t.name.contains("root:")),
            "{facts:?}"
        );
    }

    let deep = format!(
        "<testsuites>{}{}</testsuites>",
        "<a>".repeat(200),
        "</a>".repeat(200)
    );
    let deep = import_as(&snap, "deep.xml", &deep, None, Some(CiFormat::Junit), None);
    assert!(matches!(deep, Err(CiError::Malformed { .. })), "{deep:?}");
    let bad_lcov = import_as(&snap, "l", "DA:1,1\n", None, Some(CiFormat::Lcov), None);
    assert!(matches!(bad_lcov, Err(CiError::Malformed { .. })));
}
