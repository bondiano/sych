//! Fix handoff: a reproducible package of a result's own findings, with their
//! requirements, fix criteria, gaps and the consent-first procedure; nothing else is accepted.

use sych_core::handoff::{FORMAT, HandoffError, PROCEDURE, package};
use sych_core::{Coverage, FindingId, ObligationStatus, ReviewResult, SourceSnapshotId, sha256};

fn result() -> ReviewResult {
    let text = include_str!("../../../fixtures/review/violated.bundle.json");
    let bundle: serde_json::Value = serde_json::from_str(text).expect("bundle");
    serde_json::from_value(bundle["review_result"].clone()).expect("result")
}

#[test]
fn a_package_carries_requirements_criteria_gaps_and_the_procedure() {
    let r = result();
    let id = r.findings[0].id;
    let h = package(&r, &[id, id], Some(&r.source_snapshot_id)).expect("package");
    assert_eq!(h.format, FORMAT);
    assert_eq!(h.finding_ids, [id], "once each");
    assert_eq!(h.source_snapshot_id, r.source_snapshot_id);
    let f = &h.findings[0];
    assert_eq!(f.fix_criterion, r.findings[0].validation_expectation);
    assert_eq!(f.requirements.len(), 1);
    assert_eq!(f.requirements[0].rule_id, "EFFECT-01");
    assert_eq!(f.requirements[0].status, ObligationStatus::Violated);
    assert_eq!(h.procedure, PROCEDURE);
    assert!(h.procedure[0].contains("plan") && h.procedure[1].contains("consent"));
    assert_eq!(
        h.gaps.len(),
        r.excluded_or_unreviewed.len(),
        "complete coverage: only the unreviewed paths"
    );
    assert_eq!(
        package(&r, &[id], None).expect("again"),
        h,
        "the same result and ids give the same package"
    );

    let mut partial = r;
    partial.coverage = Coverage::Partial;
    partial.obligations[1].status = ObligationStatus::Unknown {
        reason_code: "MODEL_UNKNOWN".into(),
        next_evidence_needed: "the caller".into(),
    };
    let gaps = package(&partial, &[id], None).expect("package").gaps;
    assert!(gaps.iter().any(|g| g.contains("Partial")), "{gaps:?}");
    assert!(
        gaps.iter().any(|g| g.starts_with("FRONT-01 UNKNOWN")),
        "{gaps:?}"
    );
}

#[test]
fn a_foreign_finding_or_another_snapshot_is_refused() {
    let r = result();
    let foreign = FindingId(uuid::Uuid::from_u128(7));
    assert_eq!(
        package(&r, &[r.findings[0].id, foreign], None),
        Err(HandoffError::Foreign(foreign))
    );
    assert_eq!(package(&r, &[], None), Err(HandoffError::NoFindings));
    let other = SourceSnapshotId(sha256(b"another head"));
    assert!(matches!(
        package(&r, &[r.findings[0].id], Some(&other)),
        Err(HandoffError::SnapshotMismatch { .. })
    ));
}
