//! Verification: which verdicts confirm a finding, and how findings merge.

use serde_json::json;
use sych_core::analysis::{
    Anchor, Context, VerifyOutput, merge_overlapping, validate_discovery, validate_verdicts,
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
           "suggested_action": "restore", "validation_expectation": "returns 0", "limitations": []})
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
    validate_discovery(output, "k", context, &[], reader).candidates
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
         "suggested_action": "", "validation_expectation": "", "limitations": []},
    ], "context_requests": []}))
    .expect("verdicts");
    let verified = validate_verdicts(
        &output,
        candidates(&reader, &context, 6),
        &context,
        &reader,
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
        "unit",
    );
    assert_eq!(
        verified.candidates[0].verification_status,
        VerificationStatus::Unresolved
    );
    assert!(verified.findings.is_empty());
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
        "unit",
    );
    assert!(verified.findings.is_empty());
    for c in &verified.candidates {
        assert_eq!(c.verification_status, VerificationStatus::Unresolved);
        assert!(c.limitations.iter().any(|l| l.contains("unknown")));
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
        "unit",
    );
    // Different first anchors give different fingerprints; only the overlap merges them.
    assert_eq!(verified.findings.len(), 2);
    verified.findings[1].obligation_ids = vec![ObligationId(uuid::Uuid::from_u128(7))];
    let merged = merge_overlapping(verified.findings);
    assert_eq!(merged.len(), 1);
    assert_eq!(merged[0].severity, Severity::High);
    assert!(
        merged[0]
            .limitations
            .iter()
            .any(|l| l.starts_with("merged an overlapping hypothesis"))
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
