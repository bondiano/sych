//! Lineage corpus: what an open finding of the previous revision becomes. A
//! missing finding is never a fix by itself; a rename continues the problem or keeps it open.

use std::fs;
use std::path::Path;
use std::slice;

use sych_core::lineage::{Decision, Prior, decide};
use sych_core::{
    Coverage, DiffManifest, Finding, FindingCategory, ObligationStatus, ReviewResult, decode,
    sha256,
};

fn fixture(name: &str) -> String {
    fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures")
            .join(name),
    )
    .expect("fixture")
}

fn manifest() -> DiffManifest {
    serde_json::from_str(&fixture("publication/diff-manifest.json")).expect("manifest")
}

/// A complete revision that reviewed the modified and the renamed file; EFFECT-01 SUPPORTED.
fn revision() -> ReviewResult {
    let mut r: ReviewResult = decode(&fixture("core/v1/review-result.json")).expect("result");
    r.coverage = Coverage::CompleteForScope;
    r.findings.clear();
    r.unresolved_questions.clear();
    let mut unit = r.scope_manifest.units[0].clone();
    unit.paths = vec![
        "backend/orders/create.go".parse().expect("path"),
        "src/новое_имя.rs".parse().expect("path"),
    ];
    r.scope_manifest.units = vec![unit];
    for o in &mut r.obligations {
        o.status = ObligationStatus::Supported;
        o.applicability = sych_core::Applicability::Applies;
    }
    r
}

fn prior(rule: Option<&str>, path: &str) -> Prior {
    Prior {
        fingerprint: sha256(format!("{rule:?}{path}").as_bytes()).to_string(),
        category: if rule.is_some() {
            FindingCategory::RuleViolation
        } else {
            FindingCategory::Defect
        },
        rule_id: rule.map(str::to_owned),
        path: Some(path.into()),
    }
}

fn finding_at(path: &str, fingerprint: &str) -> Finding {
    let mut f = decode::<ReviewResult>(&fixture("core/v1/review-result.json"))
        .expect("result")
        .findings[0]
        .clone();
    f.category = FindingCategory::Defect;
    f.rule_id = None;
    f.anchors[0].path = path.parse().expect("path");
    f.fingerprint = fingerprint.parse().expect("digest");
    f
}

fn one(p: &Prior, r: &ReviewResult) -> Decision {
    decide(slice::from_ref(p), r, &manifest()).remove(0).1
}

fn open_reason(d: &Decision) -> &str {
    match d {
        Decision::Open { reason } => reason,
        other => panic!("expected open, got {other:?}"),
    }
}

#[test]
fn a_rule_finding_resolves_only_on_a_comparable_complete_recheck() {
    let p = prior(Some("EFFECT-01"), "backend/orders/create.go");
    assert!(matches!(one(&p, &revision()), Decision::Resolved { .. }));

    let mut partial = revision();
    partial.coverage = Coverage::Partial;
    assert_eq!(
        open_reason(&one(&p, &partial)),
        "the new revision was reviewed partially"
    );

    let mut removed = revision();
    removed.obligations.retain(|o| o.rule_id != "EFFECT-01");
    assert_eq!(
        open_reason(&one(&p, &removed)),
        "the rule was not evaluated again"
    );

    let mut unknown = revision();
    for o in &mut unknown.obligations {
        o.status = ObligationStatus::Unknown {
            reason_code: "CI_JOB_SKIPPED".into(),
            next_evidence_needed: "tests".into(),
        };
    }
    assert_eq!(
        open_reason(&one(&p, &unknown)),
        "the rule is still not supported"
    );

    let elsewhere = prior(Some("EFFECT-01"), "README.md");
    assert_eq!(
        open_reason(&one(&elsewhere, &revision())),
        "its file was not reviewed again"
    );
}

#[test]
fn renames_continue_or_stay_open_and_deletions_are_obsolete() {
    let p = prior(None, "src/old_name.rs");
    let mut moved = revision();
    moved.findings = vec![finding_at(
        "src/новое_имя.rs",
        &sha256(b"moved").to_string(),
    )];
    assert_eq!(
        one(&p, &moved),
        Decision::Continued {
            successor: sha256(b"moved").to_string()
        }
    );
    let mut ambiguous = moved.clone();
    ambiguous
        .findings
        .push(finding_at("src/новое_имя.rs", &sha256(b"twin").to_string()));
    assert_eq!(
        open_reason(&one(&p, &ambiguous)),
        "renamed or moved; not matched again with certainty"
    );
    assert_eq!(
        open_reason(&one(&p, &revision())),
        "renamed or moved; not matched again with certainty",
        "no successor: nothing closes"
    );

    let gone = prior(None, "src/gone.rs");
    assert!(matches!(one(&gone, &revision()), Decision::Obsolete { .. }));
}

#[test]
fn a_model_finding_needs_changed_code_and_no_finding_of_its_kind_there() {
    let changed = prior(None, "backend/orders/create.go");
    assert!(matches!(
        one(&changed, &revision()),
        Decision::Resolved { .. }
    ));
    let mut again = revision();
    again.findings = vec![finding_at(
        "backend/orders/create.go",
        &sha256(b"other").to_string(),
    )];
    assert_eq!(
        open_reason(&one(&changed, &again)),
        "a finding of the same kind is still there"
    );

    let same = Prior {
        fingerprint: sha256(b"same").to_string(),
        ..changed
    };
    let mut matched = revision();
    matched.findings = vec![finding_at("backend/orders/create.go", &same.fingerprint)];
    assert_eq!(one(&same, &matched), Decision::Matched);
}

/// An earlier revision's inline comment says its finding is gone only when lineage closes it,
/// and gets its text back when the finding returns; a partial review touches nothing.
#[test]
fn an_inline_comment_is_rewritten_only_when_lineage_decides() {
    use sych_core::ReviewLanguage;
    use sych_core::publication::{GONE_MARKER, Posted, gone_body, revisit};

    let posted = |object_id, noted| Posted {
        object_id,
        prior: prior(Some("EFFECT-01"), "backend/orders/create.go"),
        noted,
    };
    let resolved = revision();
    assert_eq!(revisit(&[posted(1, false)], &resolved, &manifest()), [1]);
    let none: [i64; 0] = [];
    assert_eq!(revisit(&[posted(1, true)], &resolved, &manifest()), none);

    let mut partial = revision();
    partial.coverage = Coverage::Partial;
    assert_eq!(
        revisit(&[posted(1, false), posted(2, true)], &partial, &manifest()),
        none
    );

    let mut back = revision();
    let mut again = finding_at(
        "backend/orders/create.go",
        &posted(1, true).prior.fingerprint,
    );
    again.rule_id = Some("EFFECT-01".into());
    back.findings = vec![again];
    assert_eq!(
        revisit(&[posted(1, true), posted(2, false)], &back, &manifest()),
        [1]
    );

    let body = gone_body(
        "sha256:ab",
        "0123456789abcdef",
        "<!-- sych:inline:sha256:ab -->\n**[P1] Lost event**\n\nDetails.",
        ReviewLanguage::En,
    );
    assert!(
        body.starts_with("<!-- sych:inline:sha256:ab -->\n"),
        "{body}"
    );
    assert!(
        body.contains(GONE_MARKER) && body.contains("`0123456789ab`"),
        "{body}"
    );
    assert_eq!(body.matches("<!-- sych:inline:").count(), 1, "{body}");
    assert!(
        body.contains("<summary>Original comment</summary>\n\n**[P1] Lost event**"),
        "{body}"
    );
    let bare = gone_body("sha256:ab", "0123456789abcdef", "", ReviewLanguage::En);
    assert!(!bare.contains("<details>"), "nothing to fold: {bare}");
}
