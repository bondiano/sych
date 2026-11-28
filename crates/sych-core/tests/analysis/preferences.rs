//! Discovery and verification contracts for `RuleKind::Preference` (`pref-N`).

use serde_json::json;
use sych_core::analysis::{VerifyOutput, validate_discovery, validate_verdicts};
use sych_core::{Severity, Support, VerificationStatus};

use super::{Reader, anchor, context, discovery, draft};

#[test]
fn discovery_resolves_pref_ids_and_rejects_fabricated_ones() {
    let reader = Reader::new();
    let (context, _) = context(&reader);
    let mut valid = draft(
        &json!([{"ref_id": "ref-2", "start_line": 2, "end_line": 2}]),
        &[],
        &[],
    );
    valid["category"] = json!("NIT");
    valid["proposed_severity"] = json!("LOW");
    valid["preference_ids"] = json!(["pref-1", "pref-1"]);
    let mut fabricated = valid.clone();
    fabricated["claim"] = json!("invented preference");
    fabricated["preference_ids"] = json!(["pref-99"]);

    let output = discovery(json!({
        "candidates": [valid, fabricated],
        "context_requests": [],
        "unresolved_questions": []
    }));
    let found = validate_discovery(output, "k", &context, &[], &["API-PREF"], &reader);
    assert_eq!(found.candidates.len(), 1, "{:?}", found.rejected);
    assert_eq!(found.candidates[0].preference_ids, ["API-PREF"]);
    assert_eq!(found.rejected.len(), 1);
    assert_eq!(
        found.rejected[0].problems,
        ["unknown preference id `pref-99`"]
    );
}

#[test]
fn preference_candidates_require_nit_or_improvement_low_and_preference_support() {
    let reader = Reader::new();
    let (context, _) = context(&reader);
    let mut pref_draft = draft(
        &json!([{"ref_id": "ref-2", "start_line": 2, "end_line": 2}]),
        &[],
        &[],
    );
    pref_draft["category"] = json!("NIT");
    pref_draft["proposed_severity"] = json!("LOW");
    pref_draft["preference_ids"] = json!(["pref-1"]);
    let discovered = validate_discovery(
        discovery(json!({
            "candidates": vec![pref_draft; 4],
            "context_requests": [],
            "unresolved_questions": []
        })),
        "k",
        &context,
        &[],
        &["API-PREF"],
        &reader,
    );
    let mk = |id: &str, category: &str, severity: Severity, support: Support| {
        json!({
            "candidate_id": id,
            "verdict": "CONFIRMED",
            "category": category,
            "severity": severity,
            "support": support,
            "title": "style",
            "explanation": "e",
            "anchors": [anchor("ref-2", 2, 2)],
            "evidence_ids": [],
            "alternatives_checked": ["no wrapper"],
            "suggested_action": "rename",
            "validation_expectation": "matches convention",
            "limitations": [],
            "unverified_external_assumptions": []
        })
    };
    let output: VerifyOutput = serde_json::from_value(json!({
        "verdicts": [
            mk("cand-1", "NIT", Severity::Low, Support::Preference),
            mk("cand-2", "NIT", Severity::Medium, Support::Preference),
            mk("cand-3", "DEFECT", Severity::Low, Support::Preference),
            mk("cand-4", "IMPROVEMENT", Severity::Low, Support::SemanticArgument),
        ],
        "context_requests": []
    }))
    .expect("verdicts");
    let verified = validate_verdicts(
        &output,
        discovered.candidates,
        &context,
        &reader,
        &[],
        &[],
        "unit",
    );
    let statuses: Vec<_> = verified
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
        ]
    );
    assert_eq!(verified.findings.len(), 1);
    assert_eq!(verified.findings[0].rule_id.as_deref(), Some("API-PREF"));
    assert_eq!(verified.findings[0].severity, Severity::Low);
    assert_eq!(verified.findings[0].support, Support::Preference);
}
