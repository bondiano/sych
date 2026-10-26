//! Advisory publication (roadmap M2.6): the anchor corpus maps findings to exact GitHub
//! positions or keeps them in the summary with the reason, and the summary golden files show
//! full, partial and unavailable results, high findings over the inline limit and nits.

use std::path::{Path, PathBuf};
use std::slice;
use std::{env, fs};

use sych_core::publication::{
    DiffSide, InlineAnchor, MAX_INLINE, Placement, SummaryContext, anchor, inline_body, plan,
    render_summary,
};
use sych_core::{
    Coverage, DiffManifest, ExecutionOutcome, Finding, FindingCategory, GitObjectId, ReviewResult,
    Severity, SourceSide, decode, sha256,
};

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures")
        .join(name)
}

fn manifest() -> DiffManifest {
    serde_json::from_str(
        &fs::read_to_string(fixture("publication/diff-manifest.json")).expect("read"),
    )
    .expect("manifest")
}

fn result() -> ReviewResult {
    decode(&fs::read_to_string(fixture("core/v1/review-result.json")).expect("read"))
        .expect("result")
}

fn finding(side: SourceSide, path: &str, blob: char, lines: (u32, u32)) -> Finding {
    let mut f = result().findings[0].clone();
    let r = &mut f.anchors[0];
    r.side = side;
    r.path = path.parse().expect("path");
    r.blob_oid =
        GitObjectId::new(r.blob_oid.algorithm(), &blob.to_string().repeat(40)).expect("oid");
    (r.start_line, r.end_line) = lines;
    f
}

fn inline(path: &str, line: u32, side: DiffSide, start: Option<u32>) -> Placement {
    Placement::Inline {
        anchor: InlineAnchor {
            commit_id: "b".repeat(40),
            path: path.into(),
            line,
            side,
            start_line: start,
            start_side: start.map(|_| side),
        },
    }
}

fn reason(p: &Placement) -> &str {
    match p {
        Placement::Summary { reason } => reason,
        Placement::Inline { .. } => "inline",
    }
}

#[test]
fn anchors_keep_side_path_and_blob_or_stay_in_the_summary() {
    let m = manifest();
    let head = SourceSide::Head;
    // A range on the head side of a modified file.
    let range = anchor(
        &finding(head, "backend/orders/create.go", 'c', (41, 47)),
        &m,
    );
    assert_eq!(
        range,
        inline("backend/orders/create.go", 47, DiffSide::Right, Some(41))
    );
    // A renamed file with a Unicode name: the head path, right side.
    let renamed = anchor(&finding(head, "src/новое_имя.rs", '2', (11, 11)), &m);
    assert_eq!(
        renamed,
        inline("src/новое_имя.rs", 11, DiffSide::Right, None)
    );
    // The old side of the rename keeps the head path GitHub shows, left side.
    let old = anchor(
        &finding(SourceSide::DiffBase, "src/old_name.rs", '1', (10, 10)),
        &m,
    );
    assert_eq!(old, inline("src/новое_имя.rs", 10, DiffSide::Left, None));
    // A deleted line: only the old path exists.
    let deleted = anchor(
        &finding(SourceSide::DiffBase, "src/gone.rs", '3', (5, 5)),
        &m,
    );
    assert_eq!(deleted, inline("src/gone.rs", 5, DiffSide::Left, None));
    // Three context lines are shown around a hunk; the fourth is not.
    assert!(matches!(
        anchor(
            &finding(head, "backend/orders/create.go", 'c', (50, 50)),
            &m
        ),
        Placement::Inline { .. }
    ));
    let far = anchor(
        &finding(head, "backend/orders/create.go", 'c', (51, 51)),
        &m,
    );
    assert_eq!(reason(&far), "line is outside the diff GitHub shows");
    let other_blob = anchor(
        &finding(head, "backend/orders/create.go", 'e', (41, 41)),
        &m,
    );
    assert_eq!(
        reason(&other_blob),
        "anchor names another blob than the diff"
    );
    let tip = anchor(
        &finding(
            SourceSide::TargetTip,
            "backend/orders/create.go",
            'c',
            (41, 41),
        ),
        &m,
    );
    assert_eq!(reason(&tip), "anchored on the target tip, not in the diff");
    let absent = anchor(&finding(head, "README.md", 'c', (1, 1)), &m);
    assert_eq!(reason(&absent), "file is not in the diff");
}

/// Ten high defects, one already inline, a nit without a rule and an improvement.
fn crowded() -> ReviewResult {
    let mut r = result();
    let base = finding(SourceSide::Head, "backend/orders/create.go", 'c', (42, 42));
    r.findings = (0..10)
        .map(|i| {
            let mut f = base.clone();
            f.fingerprint = sha256(format!("defect-{i}").as_bytes());
            f.severity = if i < 2 {
                Severity::Critical
            } else {
                Severity::High
            };
            f.title = format!("Defect {i}");
            f
        })
        .collect();
    let mut nit = base.clone();
    nit.category = FindingCategory::Nit;
    nit.severity = Severity::Info;
    nit.rule_id = None;
    nit.title = "Name the helper after what it returns".into();
    nit.fingerprint = sha256(b"nit");
    let mut improvement = nit.clone();
    improvement.category = FindingCategory::Improvement;
    improvement.rule_id = Some("STYLE-02".into());
    improvement.title = "Reuse the existing retry helper".into();
    improvement.fingerprint = sha256(b"improvement");
    r.findings.extend([nit, improvement]);
    r
}

#[test]
fn at_most_eight_new_inline_comments_and_optional_findings_stay_in_the_summary() {
    let r = crowded();
    let earlier = sha256(b"defect-9").to_string();
    let planned = plan(&r, &manifest(), slice::from_ref(&earlier), &[], true);
    let inline: Vec<_> = planned
        .iter()
        .filter(|p| matches!(p.placement, Placement::Inline { .. }))
        .collect();
    assert_eq!(
        inline.iter().filter(|p| !p.already_inline).count(),
        MAX_INLINE
    );
    assert!(
        inline.iter().any(|p| p.fingerprint == earlier),
        "an earlier inline does not count"
    );
    assert_eq!(
        planned
            .iter()
            .filter(|p| reason(&p.placement).starts_with("over the"))
            .count(),
        1
    );
    assert_eq!(
        planned
            .iter()
            .filter(|p| reason(&p.placement) == "optional; summary only")
            .count(),
        2
    );
    let incomplete = plan(&r, &manifest(), &[], &[], false);
    assert!(
        incomplete
            .iter()
            .all(|p| !matches!(p.placement, Placement::Inline { .. }))
    );
    assert!(inline_body(&r.findings[0]).starts_with("<!-- sych:inline:sha256:"));
}

fn golden(name: &str, text: &str) {
    let path = fixture(&format!("publication/{name}.md"));
    if env::var_os("SYCH_BLESS").is_some() {
        fs::write(&path, text).expect("write");
    }

    assert_eq!(
        fs::read_to_string(&path).unwrap_or_default(),
        text,
        "{} differs; run with SYCH_BLESS=1",
        path.display()
    );
}

fn ctx() -> SummaryContext<'static> {
    SummaryContext {
        marker: "sych:summary:7",
        head_sha: "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
        base_sha: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        result_url: Some(
            "https://sych.example/v1/runs/01900000-0000-7000-8000-000000000001/result".into(),
        ),
        duration_seconds: Some(42),
    }
}

#[test]
fn summaries_match_their_golden_files() {
    let crowded = crowded();
    let planned = plan(&crowded, &manifest(), &[], &[], true);
    golden(
        "summary-crowded",
        &render_summary(&crowded, &planned, &ctx()),
    );

    let partial = result();
    assert_eq!(partial.coverage, Coverage::Partial);
    golden(
        "summary-partial",
        &render_summary(
            &partial,
            &plan(&partial, &manifest(), &[], &[], true),
            &ctx(),
        ),
    );

    let mut clean = result();
    clean.findings.clear();
    clean.obligations.clear();
    clean.unresolved_questions.clear();
    clean.coverage = Coverage::CompleteForScope;
    golden("summary-complete", &render_summary(&clean, &[], &ctx()));

    let mut unavailable = clean;
    unavailable.coverage = Coverage::Unavailable;
    unavailable.execution_outcome = ExecutionOutcome::InputUnavailable;
    let text = render_summary(&unavailable, &[], &ctx());
    assert!(
        text.contains("**unavailable**") && !text.contains("None confirmed in the reviewed scope")
    );
    golden("summary-unavailable", &text);
}
