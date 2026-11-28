//! Verification: which verdicts confirm a finding, and how findings merge.

use serde_json::json;
use sych_core::analysis::{
    Anchor, AskOperation, Context, VerifyOutput, merge_overlapping, refuting_asks,
    validate_discovery, validate_verdicts,
};
use sych_core::{FindingCategory, ObligationId, Severity, SourceSide, Support, VerificationStatus};

use super::{Reader, anchor, context, discovery, draft};

fn verdict(
    id: &str,
    severity: Severity,
    support: Support,
    anchors: &[Anchor],
) -> serde_json::Value {
    json!({"candidate_id": id, "verdict": "CONFIRMED", "category": "DEFECT", "severity": severity,
           "support": support, "title": "guard removed", "explanation": "e", 
           "anchors": anchors, "evidence_ids": [], "alternatives_checked": ["no wrapper"],
           "suggested_action": "restore", "validation_expectation": "returns 0", "limitations": [],
           "unverified_external_assumptions": []})
}

fn candidates(reader: &Reader, context: &Context, n: usize) -> Vec<sych_core::Candidate> {
    let one = draft(
        &json!([{"ref_id": "ref-2", "start_line": 2, "end_line": 2}]),
        &[],
        &[],
    );
    let output = discovery(
        json!({"candidates": vec![one; n], "context_requests": [], "unresolved_questions": []}),
    );
    validate_discovery(output, "k", context, &[], &[], reader).candidates
}

#[test]
fn verdicts_need_an_admissible_basis() {
    let reader = Reader::new();
    let (context, _) = context(&reader);
    let trace = vec![anchor("ref-1", 2, 4), anchor("ref-2", 2, 2)];
    let output: VerifyOutput = serde_json::from_value(json!({"verdicts": [
        verdict("cand-1", Severity::High, Support::StaticTrace, &trace),
        verdict("cand-2", Severity::High, Support::SemanticArgument, &trace),
        verdict("cand-3", Severity::Medium, Support::ExecutedCounterexample, &trace),
        verdict("cand-4", Severity::Medium, Support::Preference, &trace),
        {"candidate_id": "cand-5", "verdict": "REJECTED", "category": "NIT", "severity": "INFO",
         "support": "PREFERENCE", "title": "t", "explanation": "e",  "anchors": [], "evidence_ids": [], "alternatives_checked": [],
         "suggested_action": "", "validation_expectation": "", "limitations": [], "unverified_external_assumptions": []},
    ], "context_requests": []}))
    .expect("verdicts");
    let verified = validate_verdicts(
        &output,
        candidates(&reader, &context, 6),
        &context,
        &reader,
        &[],
        &[],
        "unit",
    );
    let statuses: Vec<VerificationStatus> = verified
        .candidates
        .iter()
        .map(|c| c.verification_status)
        .collect();
    assert_eq!(
        statuses,
        [
            VerificationStatus::Confirmed,
            VerificationStatus::Unresolved,
            VerificationStatus::Unresolved,
            VerificationStatus::Unresolved,
            VerificationStatus::Rejected,
            VerificationStatus::Unresolved,
        ]
    );
    assert!(
        verified.candidates[1]
            .limitations
            .iter()
            .any(|l| l.contains("HIGH_RISK_WITHOUT_TRACE"))
    );
    assert!(
        verified.candidates[5]
            .limitations
            .iter()
            .any(|l| l.contains("no single verdict"))
    );
    // A round without a verdict is still a round: the next one is numbered 2.
    let silent = &verified.candidates[5].verdicts;
    assert_eq!(
        silent
            .iter()
            .map(|v| (v.round, v.proposed))
            .collect::<Vec<_>>(),
        [(1, VerificationStatus::Pending)]
    );
    let again = validate_verdicts(
        &output,
        verified.candidates.clone(),
        &context,
        &reader,
        &[],
        &[],
        "unit",
    );
    assert_eq!(again.candidates[5].verdicts[1].round, 2);
    // A second round says each limitation once.
    let times = |c: &sych_core::Candidate, text: &str| {
        c.limitations.iter().filter(|l| l.contains(text)).count()
    };
    assert_eq!(times(&again.candidates[5], "no single verdict"), 1);
    assert_eq!(times(&again.candidates[1], "HIGH_RISK_WITHOUT_TRACE"), 1);
    assert_eq!(verified.findings.len(), 1);
    let finding = &verified.findings[0];
    assert_eq!(finding.category, FindingCategory::Defect);
    assert_eq!(finding.support, Support::StaticTrace);
    assert!(finding.limitations[0].contains("not a proof of the whole program"));

    // An imported diagnostic on the same line keeps the candidate unresolved.
    let known = [reader.at(SourceSide::Head, "src/order.ts", 2, 2)];
    let output: VerifyOutput = serde_json::from_value(json!({"verdicts": [
        verdict("cand-1", Severity::Medium, Support::SemanticArgument, &trace)
    ], "context_requests": []}))
    .expect("verdicts");
    let verified = validate_verdicts(
        &output,
        candidates(&reader, &context, 1),
        &context,
        &reader,
        &known,
        &[],
        "unit",
    );
    assert_eq!(
        verified.candidates[0].verification_status,
        VerificationStatus::Unresolved
    );
    assert!(verified.findings.is_empty(), "{:?}", verified.findings);
}

#[test]
fn invented_references_never_confirm() {
    let reader = Reader::new();
    let (context, _) = context(&reader);
    let mut ghost_ev = verdict(
        "cand-2",
        Severity::Medium,
        Support::SemanticArgument,
        &[anchor("ref-1", 2, 4)],
    );
    ghost_ev["evidence_ids"] = json!(["ev-999"]);
    let output: VerifyOutput = serde_json::from_value(json!({"verdicts": [
        verdict("cand-1", Severity::Medium, Support::SemanticArgument,
                &[anchor("ref-1", 2, 4), anchor("ref-999", 1, 1)]),
        ghost_ev,
    ], "context_requests": []}))
    .expect("verdicts");
    let verified = validate_verdicts(
        &output,
        candidates(&reader, &context, 2),
        &context,
        &reader,
        &[],
        &[],
        "unit",
    );
    assert!(verified.findings.is_empty(), "{:?}", verified.findings);
    for c in &verified.candidates {
        assert_eq!(c.verification_status, VerificationStatus::Unresolved);
        assert!(c.limitations.iter().any(|l| l.contains("unknown")));
        let [record] = c.verdicts.as_slice() else {
            panic!("one verdict per round: {:?}", c.verdicts);
        };
        assert_eq!(
            (record.round, record.proposed, record.status),
            (
                1,
                VerificationStatus::Confirmed,
                VerificationStatus::Unresolved
            ),
            "the history keeps the verdict the engine did not accept"
        );
        assert!(record.engine_problems.iter().any(|p| p.contains("unknown")));
        assert_eq!(record.anchors.len(), 1, "only the anchor that resolved");
    }
}

#[test]
fn a_shared_first_anchor_keeps_the_most_severe_in_either_order() {
    let reader = Reader::new();
    let (context, _) = context(&reader);
    let low = |id| {
        verdict(
            id,
            Severity::Low,
            Support::SemanticArgument,
            &[anchor("ref-1", 2, 4)],
        )
    };
    let high = |id| {
        verdict(
            id,
            Severity::High,
            Support::StaticTrace,
            &[anchor("ref-1", 2, 4), anchor("ref-2", 1, 2)],
        )
    };
    for verdicts in [
        [low("cand-1"), high("cand-2")],
        [high("cand-1"), low("cand-2")],
    ] {
        let output: VerifyOutput =
            serde_json::from_value(json!({"verdicts": verdicts, "context_requests": []}))
                .expect("verdicts");
        let verified = validate_verdicts(
            &output,
            candidates(&reader, &context, 2),
            &context,
            &reader,
            &[],
            &[],
            "unit",
        );
        assert_eq!(verified.findings.len(), 2, "both reach the merge");
        assert_eq!(
            verified.findings[0].fingerprint,
            verified.findings[1].fingerprint
        );
        let merged = merge_overlapping(verified.findings);
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].severity, Severity::High);
    }
}

#[test]
fn overlapping_findings_merge_into_the_most_severe() {
    let reader = Reader::new();
    let (context, _) = context(&reader);
    let output: VerifyOutput = serde_json::from_value(json!({"verdicts": [
        verdict("cand-1", Severity::Medium, Support::SemanticArgument, &[anchor("ref-2", 2, 2)]),
        verdict("cand-2", Severity::High, Support::StaticTrace, &[anchor("ref-1", 2, 4), anchor("ref-2", 1, 2)]),
    ], "context_requests": []}))
    .expect("verdicts");
    let mut verified = validate_verdicts(
        &output,
        candidates(&reader, &context, 2),
        &context,
        &reader,
        &[],
        &[],
        "unit",
    );
    // Different first anchors give different fingerprints; only the overlap merges them.
    assert_eq!(verified.findings.len(), 2);
    verified.findings[1].obligation_ids = vec![ObligationId(uuid::Uuid::from_u128(7))];
    let merged = merge_overlapping(verified.findings);
    assert_eq!(merged.len(), 1);
    assert_eq!(merged[0].severity, Severity::High);
    let [also] = merged[0].also.as_slice() else {
        panic!("the absorbed finding is kept: {:?}", merged[0].also);
    };
    assert_eq!(also.severity, Severity::Medium);
    assert_eq!(
        (also.anchors[0].start_line, also.anchors[0].end_line),
        (2, 2),
        "with its own anchors"
    );
    assert_eq!(
        merged[0].obligation_ids,
        [ObligationId(uuid::Uuid::from_u128(7))]
    );
}

#[test]
fn a_useful_nit_needs_no_rule_and_stays_low() {
    let reader = Reader::new();
    let (context, _) = context(&reader);
    let at = [anchor("ref-2", 2, 2)];
    let nit = |id, severity| {
        let mut v = verdict(id, severity, Support::Preference, &at);
        v["category"] = json!("NIT");
        v["title"] = json!("name the empty fallback");
        v
    };
    let output: VerifyOutput = serde_json::from_value(json!({"verdicts": [
        nit("cand-1", Severity::Low),
        nit("cand-2", Severity::Medium),
    ], "context_requests": []}))
    .expect("verdicts");
    let verified = validate_verdicts(
        &output,
        candidates(&reader, &context, 2),
        &context,
        &reader,
        &[],
        &[],
        "unit",
    );
    assert_eq!(verified.findings.len(), 1);
    let finding = &verified.findings[0];
    assert_eq!(
        (finding.category, finding.severity),
        (FindingCategory::Nit, Severity::Low)
    );
    assert!(finding.rule_id.is_none(), "a nit needs no rule");
    assert_ne!(
        verified.candidates[1].verification_status,
        VerificationStatus::Confirmed,
        "a nit is never raised above LOW"
    );
}

/// A change on lines of a shown HEAD span reaches the finding; one on BASE or outside the
/// spans is dropped with the reason, and the finding stands.
#[test]
fn a_suggested_change_needs_lines_of_a_shown_head_span() {
    let reader = Reader::new();
    let (context, _) = context(&reader);
    let trace = vec![anchor("ref-2", 2, 2)];
    for (at, dropped) in [
        (anchor("ref-2", 2, 2), None),
        (anchor("ref-1", 2, 2), Some("BASE lines")),
        (anchor("ref-2", 2, 9), Some("outside ref-2")),
    ] {
        let mut v = verdict(
            "cand-1",
            Severity::Medium,
            Support::SemanticArgument,
            &trace,
        );
        v["suggested_change"] = json!({"anchor": at, "replacement": "  return 0;\n"});
        let output: VerifyOutput =
            serde_json::from_value(json!({"verdicts": [v], "context_requests": []}))
                .expect("verdicts");
        let verified = validate_verdicts(
            &output,
            candidates(&reader, &context, 1),
            &context,
            &reader,
            &[],
            &[],
            "unit",
        );
        let finding = &verified.findings[0];
        match dropped {
            None => {
                let change = finding.suggested_change.as_ref().expect("kept");
                assert_eq!(
                    change.code,
                    reader.at(SourceSide::Head, "src/order.ts", 2, 2)
                );
                assert_eq!(change.replacement, "  return 0;");
            }
            Some(reason) => {
                assert!(finding.suggested_change.is_none(), "{reason}");
                assert!(
                    finding
                        .limitations
                        .iter()
                        .any(|l| l.contains("suggested change dropped") && l.contains(reason)),
                    "{:?}",
                    finding.limitations
                );
            }
        }
    }
}

#[test]
fn open_claims_are_searched_by_the_identifiers_they_name() {
    let reader = Reader::new();
    let (context, _) = context(&reader);
    let mut open = candidates(&reader, &context, 3);
    open[0].claim = "`heartbeat_seconds` of 0 makes `analyze` spin; `a b` is prose".into();
    open[1].claim = "`analyze` again, then `lease_seconds`, `x`, `fetch`".into();
    open[2].claim = "`retry_at` gives up".into();
    let queries: Vec<String> = refuting_asks(&open)
        .into_iter()
        .map(|a| match a.operation {
            AskOperation::SearchSource { query, .. } => query,
            other => panic!("{other:?}"),
        })
        .collect();
    // Two per claim, no repeats, no prose or one-letter names, four in all.
    assert_eq!(
        queries,
        ["heartbeat_seconds", "analyze", "lease_seconds", "fetch"]
    );
}

#[test]
fn unverified_external_assumptions_prevent_confirmation() {
    let reader = Reader::new();
    let (context, _) = context(&reader);
    let trace = vec![anchor("ref-1", 2, 4), anchor("ref-2", 2, 2)];
    let mut external = verdict(
        "cand-1",
        Severity::Medium,
        Support::SemanticArgument,
        &trace,
    );
    external["unverified_external_assumptions"] =
        json!(["uptrace/bun rel:has-many stores FK on parent"]);
    let mut grounded = verdict(
        "cand-2",
        Severity::Medium,
        Support::SemanticArgument,
        &trace,
    );
    grounded["unverified_external_assumptions"] = json!([]);
    let output: VerifyOutput = serde_json::from_value(json!({
        "verdicts": [external, grounded],
        "context_requests": []
    }))
    .expect("verdicts");
    let verified = validate_verdicts(
        &output,
        candidates(&reader, &context, 2),
        &context,
        &reader,
        &[],
        &[],
        "unit",
    );
    assert_eq!(
        verified.candidates[0].verification_status,
        VerificationStatus::Unresolved
    );
    assert!(
        verified.candidates[0]
            .limitations
            .iter()
            .any(|l| l.contains("EXTERNAL_BEHAVIOR_UNVERIFIED")),
        "{:?}",
        verified.candidates[0].limitations
    );
    assert_eq!(
        verified.candidates[1].verification_status,
        VerificationStatus::Confirmed
    );
    assert_eq!(verified.findings.len(), 1);
}

#[test]
fn obligation_linked_findings_populate_rule_id_and_preserve_it_on_overlap_merge() {
    let reader = Reader::new();
    let (context, _) = context(&reader);
    let ob_id = ObligationId(uuid::Uuid::from_u128(7));
    let linked_draft = draft(
        &json!([{"ref_id": "ref-2", "start_line": 2, "end_line": 2}]),
        &[],
        &["ob-1"],
    );
    let defect_draft = draft(
        &json!([{"ref_id": "ref-2", "start_line": 1, "end_line": 2}]),
        &[],
        &[],
    );
    let discovered = validate_discovery(
        discovery(json!({
            "candidates": [linked_draft, defect_draft],
            "context_requests": [],
            "unresolved_questions": []
        })),
        "k",
        &context,
        &[ob_id],
        &[],
        &reader,
    );
    let mut violation_verdict = verdict(
        "cand-1",
        Severity::Medium,
        Support::ExplicitRule,
        &[anchor("ref-2", 2, 2)],
    );
    violation_verdict["category"] = json!("RULE_VIOLATION");
    let defect_verdict = verdict(
        "cand-2",
        Severity::High,
        Support::StaticTrace,
        &[anchor("ref-1", 2, 4), anchor("ref-2", 1, 2)],
    );
    let output: VerifyOutput = serde_json::from_value(json!({
        "verdicts": [violation_verdict, defect_verdict],
        "context_requests": []
    }))
    .expect("verdicts");
    let verified = validate_verdicts(
        &output,
        discovered.candidates,
        &context,
        &reader,
        &[],
        &[(ob_id, "ORDER-01")],
        "unit",
    );
    assert_eq!(verified.findings.len(), 2);
    assert_eq!(verified.findings[0].rule_id.as_deref(), Some("ORDER-01"));
    assert_eq!(verified.findings[1].rule_id, None);

    let merged = merge_overlapping(verified.findings);
    assert_eq!(merged.len(), 1);
    assert_eq!(merged[0].category, FindingCategory::Defect);
    assert_eq!(merged[0].severity, Severity::High);
    assert_eq!(merged[0].rule_id.as_deref(), Some("ORDER-01"));
    assert_eq!(merged[0].obligation_ids, [ob_id]);

    // An absorbed style Nit must not overwrite a Defect's None rule_id.
    let mut defect = merged[0].clone();
    defect.rule_id = None;
    defect.obligation_ids.clear();
    defect.also.clear();
    let mut nit = defect.clone();
    nit.category = FindingCategory::Nit;
    nit.severity = Severity::Low;
    nit.rule_id = Some("PREF-01".into());
    let merged_with_nit = merge_overlapping(vec![defect, nit]);
    assert_eq!(merged_with_nit.len(), 1);
    assert_eq!(merged_with_nit[0].rule_id, None);
}
