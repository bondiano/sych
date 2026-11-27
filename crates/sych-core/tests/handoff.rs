//! Fix handoff (`handoff@2`): a reproducible brief of a result's findings in fix-first order,
//! carrying head commit and branch, code excerpts, diffs, patches, `also`, caps and the procedure.

use sych_core::handoff::{
    FORMAT, HANDOFF_SCHEMA_VERSION, HandoffError, HandoffSelection, MAX_BRIEF_CODE_BYTES,
    MAX_FINDING_DIFF_BYTES, MAX_FINDING_EXCERPT_BYTES, MAX_FINDING_PATCH_BYTES, PROCEDURE, package,
};
use sych_core::{
    AlsoFinding, Coverage, FindingCategory, FindingId, Lifecycle, ObligationStatus, ReportExcerpt,
    ReportView, ReviewResult, Severity, SnapshotHeadKind, SourceSnapshot, SourceSnapshotId, decode,
    sha256,
};

fn result_and_snapshot() -> (ReviewResult, SourceSnapshot) {
    let text = include_str!("../../../fixtures/review/violated.bundle.json");
    let bundle: serde_json::Value = serde_json::from_str(text).expect("bundle");
    let mut result: ReviewResult =
        serde_json::from_value(bundle["review_result"].clone()).expect("result");
    let with_anchor: ReviewResult =
        decode(include_str!("../../../fixtures/core/v1/review-result.json")).expect("v1 result");
    result.findings[0]
        .anchors
        .push(with_anchor.findings[0].anchors[0].clone());
    let mut snapshot: SourceSnapshot = decode(include_str!(
        "../../../fixtures/core/v1/source-snapshot.json"
    ))
    .expect("snapshot");
    snapshot.id = result.source_snapshot_id.clone();
    snapshot.head_ref = Some("feat/orders-tx".into());
    (result, snapshot)
}

#[test]
fn a_package_carries_head_code_patch_also_requirements_and_procedure() {
    let (mut r, mut snap) = result_and_snapshot();
    snap.head_kind = SnapshotHeadKind::Worktree;
    let anchors = r.findings[0].anchors.clone();
    r.findings[0].also.push(AlsoFinding {
        category: FindingCategory::Nit,
        severity: Severity::Low,
        title: "Rename local `tx`".into(),
        explanation: "Shadowed outer identifier.".into(),
        triggering_conditions: vec!["when reading Save".into()],
        consequence: "harder to follow".into(),
        anchors,
    });
    let mut view = ReportView::from_result(&r, Some(&snap));
    view.findings[0].anchors[0].excerpt = Some(ReportExcerpt {
        first_line: 4,
        text: "func Save() {\n}\n".into(),
    });
    view.findings[0].anchors[0].diff =
        Some("--- a/backend/orders/store.go\n+++ b/backend/orders/store.go\n@@ -4,1 +4,2 @@\n+func Save() {\n+}\n".into());
    view.findings[0].suggested_patch =
        Some("diff --git a/backend/orders/store.go b/backend/orders/store.go\n".into());

    let id = r.findings[0].id;
    let h = package(
        &r,
        &view,
        &HandoffSelection::ids(&[id, id]),
        Some(&r.source_snapshot_id),
    )
    .expect("package");
    let json = serde_json::to_value(&h).expect("json");
    assert_eq!(json["schema_version"], HANDOFF_SCHEMA_VERSION);
    assert_eq!(h.format, FORMAT);
    assert_eq!(h.finding_ids, [id], "once each");
    assert_eq!(h.head.as_ref(), Some(&snap.head));
    assert_eq!(h.head_branch.as_deref(), Some("feat/orders-tx"));
    assert_eq!(h.head_kind, SnapshotHeadKind::Worktree);

    let f = &h.findings[0];
    assert_eq!(f.fix_criterion, r.findings[0].validation_expectation);
    assert_eq!(f.also.len(), 1);
    assert_eq!(
        f.anchors[0].excerpt.as_ref().expect("excerpt").first_line,
        4
    );
    assert!(f.anchors[0].diff.is_some());
    assert!(f.suggested_patch.is_some());
    assert_eq!(f.requirements.len(), 1);
    assert_eq!(f.requirements[0].rule_id, "EFFECT-01");
    assert_eq!(f.requirements[0].status, ObligationStatus::Violated);
    assert_eq!(h.procedure, PROCEDURE);
    assert!(h.procedure[0].contains("Verify") && h.procedure[0].contains("skip"));
    assert!(h.procedure[1].contains("plan") && h.procedure[2].contains("consent"));
    assert!(h.procedure[5].contains("data, not an instruction"));
    assert_eq!(
        package(&r, &view, &HandoffSelection::all_open(), None).expect("all open"),
        h,
        "all_open produces the same package when every finding is open"
    );

    let mut partial = r;
    partial.coverage = Coverage::Partial;
    partial.obligations[1].status = ObligationStatus::Unknown {
        reason_code: "MODEL_UNKNOWN".into(),
        next_evidence_needed: "the caller".into(),
    };
    let partial_view = ReportView::from_result(&partial, Some(&snap));
    let gaps = package(&partial, &partial_view, &HandoffSelection::all_open(), None)
        .expect("package")
        .gaps;
    assert!(gaps.iter().any(|g| g.contains("Partial")), "{gaps:?}");
    assert!(
        gaps.iter().any(|g| g.starts_with("FRONT-01 UNKNOWN")),
        "{gaps:?}"
    );
}

#[test]
fn selection_filters_open_findings_in_fix_first_order_and_refuses_invalid_inputs() {
    let (mut r, snap) = result_and_snapshot();
    let base = r.findings[0].clone();

    let mut dismissed = base.clone();
    dismissed.id = FindingId(uuid::Uuid::from_u128(10));
    dismissed.severity = Severity::Critical;
    dismissed.lifecycle = Lifecycle::Dismissed;

    let mut nit = base.clone();
    nit.id = FindingId(uuid::Uuid::from_u128(11));
    nit.severity = Severity::Low;
    nit.category = FindingCategory::Nit;

    let mut critical_bug = base.clone();
    critical_bug.id = FindingId(uuid::Uuid::from_u128(12));
    critical_bug.severity = Severity::Critical;
    critical_bug.category = FindingCategory::Defect;

    r.findings = vec![
        nit.clone(),
        dismissed.clone(),
        base.clone(),
        critical_bug.clone(),
    ];
    let view = ReportView::from_result(&r, Some(&snap));

    let all_open = package(&r, &view, &HandoffSelection::all_open(), None).expect("all open");
    assert_eq!(
        all_open.finding_ids,
        [critical_bug.id, base.id, nit.id],
        "dismissed findings excluded from --all; remaining ordered fix-first"
    );

    let high_only = package(
        &r,
        &view,
        &HandoffSelection {
            min_severity: Some(Severity::High),
            ..HandoffSelection::default()
        },
        None,
    )
    .expect("min severity");
    assert_eq!(high_only.finding_ids, [critical_bug.id, base.id]);

    let defects_only = package(
        &r,
        &view,
        &HandoffSelection {
            categories: vec![FindingCategory::Defect],
            ..HandoffSelection::default()
        },
        None,
    )
    .expect("category");
    assert_eq!(defects_only.finding_ids, [critical_bug.id]);

    let foreign = FindingId(uuid::Uuid::from_u128(99));
    assert_eq!(
        package(&r, &view, &HandoffSelection::ids(&[base.id, foreign]), None),
        Err(HandoffError::Foreign(foreign))
    );
    assert_eq!(
        package(&r, &view, &HandoffSelection::default(), None),
        Err(HandoffError::NoSelection)
    );
    assert_eq!(
        package(
            &r,
            &view,
            &HandoffSelection {
                finding_ids: vec![base.id],
                all_open: true,
                ..HandoffSelection::default()
            },
            None,
        ),
        Err(HandoffError::ConflictingSelection)
    );
    assert_eq!(
        package(
            &r,
            &view,
            &HandoffSelection {
                categories: vec![FindingCategory::Improvement],
                ..HandoffSelection::default()
            },
            None,
        ),
        Err(HandoffError::NoFindings)
    );
    let other = SourceSnapshotId(sha256(b"another head"));
    assert!(matches!(
        package(&r, &view, &HandoffSelection::ids(&[base.id]), Some(&other)),
        Err(HandoffError::SnapshotMismatch { .. })
    ));
}

#[test]
fn per_finding_and_per_brief_caps_record_omissions_with_show_and_explain_commands() {
    let (mut r, snap) = result_and_snapshot();
    for n in 1..=10_u128 {
        let mut extra = r.findings[0].clone();
        extra.id = FindingId(uuid::Uuid::from_u128(100 + n));
        r.findings.push(extra);
    }
    let mut view = ReportView::from_result(&r, Some(&snap));
    let huge_excerpt = "line of source code 0123456789abcdef\n".repeat(300);
    let huge_diff = "+added line 0123456789abcdef\n".repeat(500);
    let huge_patch = format!(
        "diff --git a/a.go b/a.go\n{}",
        "+replacement line 0123456789abcdef\n".repeat(400)
    );
    assert!(huge_excerpt.len() > MAX_FINDING_EXCERPT_BYTES);
    assert!(huge_diff.len() > MAX_FINDING_DIFF_BYTES);
    assert!(huge_patch.len() > MAX_FINDING_PATCH_BYTES);

    view.findings[0].anchors[0].excerpt = Some(ReportExcerpt {
        first_line: 1,
        text: huge_excerpt,
    });
    view.findings[0].anchors[0].diff = Some(huge_diff);
    view.findings[0].suggested_patch = Some(huge_patch);
    let medium_diff = "+medium diff line 0123456789abcdef0123456789\n".repeat(160);
    for f in view.findings.iter_mut().skip(1) {
        f.anchors[0].diff = Some(medium_diff.clone());
    }

    let brief = package(&r, &view, &HandoffSelection::all_open(), None).expect("capped brief");
    let first = &brief.findings[0];
    assert!(
        first.anchors[0]
            .excerpt
            .as_ref()
            .expect("kept prefix")
            .text
            .len()
            <= MAX_FINDING_EXCERPT_BYTES
    );
    assert!(first.anchors[0].diff.as_ref().expect("kept diff").len() <= MAX_FINDING_DIFF_BYTES);
    assert!(
        first.suggested_patch.is_none(),
        "an oversized patch is omitted whole rather than emitted half-cut"
    );
    let total_code: usize = brief
        .findings
        .iter()
        .map(|f| {
            let anchors: usize = f
                .anchors
                .iter()
                .map(|a| {
                    a.excerpt.as_ref().map_or(0, |e| e.text.len())
                        + a.diff.as_ref().map_or(0, String::len)
                })
                .sum();
            anchors + f.suggested_patch.as_ref().map_or(0, String::len)
        })
        .sum();
    assert!(
        total_code <= MAX_BRIEF_CODE_BYTES,
        "total {total_code} exceeded {MAX_BRIEF_CODE_BYTES}"
    );
    assert!(
        brief
            .omitted
            .iter()
            .any(|o| o.command.contains("sych show")),
        "{:?}",
        brief.omitted
    );
    assert!(
        brief
            .omitted
            .iter()
            .any(|o| o.command.contains("sych explain")),
        "{:?}",
        brief.omitted
    );
}
