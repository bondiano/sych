//! The coverage audit's engine side: when the facts call for it, and which of its proposals
//! become obligations.

use serde_json::json;
use sych_core::analysis::coverage::{CoverageOutput, MAX_PROPOSALS, accept, signals};
use sych_core::{ObligationStatus, PlannedObligation, ReviewPlan, RunFacts};

use super::{Reader, context, snapshot};

fn plan() -> ReviewPlan {
    serde_json::from_value(json!({
        "source_snapshot_id": snapshot(),
        "scope": {"revision": 1, "required_obligation_ids": [], "requested_profiles": [], "excluded": [],
            "units": [
                {"id": "01900000-0000-7000-8000-0000000000a1", "title": "src", "paths": ["src/order.ts", "src/order.test.ts"]},
                {"id": "01900000-0000-7000-8000-0000000000a2", "title": "lib", "paths": ["lib/money.ts"]}
            ]},
        "obligations": [], "unreviewed": [], "omissions": [], "limit_reached": false
    }))
    .expect("plan")
}

fn proposal(ref_id: &str, start: u32, end: u32) -> serde_json::Value {
    json!({"question": "does the consumer still read `total`?",
           "anchors": [{"ref_id": ref_id, "start_line": start, "end_line": end}],
           "criterion": "every reader of `total` handles the new shape"})
}

#[test]
fn only_changed_effects_contracts_schema_or_semantic_links_call_for_an_audit() {
    let plan = plan();
    assert!(signals(&RunFacts::default(), &plan).is_empty());

    let mut linked = plan.clone();
    let mut obligation: PlannedObligation = serde_json::from_value(json!({
        "obligation": {"id": "01900000-0000-7000-8000-0000000000b1", "rule_id": "ORD-1",
            "rule_version": 1, "question": "q", "required": true, "risk_level": "MEDIUM",
            "review_unit_ids": [], "code_refs": [], "applicability": "APPLIES",
            "acceptance_criterion": "c", "acceptable_evidence_kinds": [], "status": "SUPPORTED",
            "limitations": ["applies through a semantic link: `total` is read by lib/money.ts"]},
        "rule_title": "t", "criteria": ["c"], "severity_on_violation": null, "exceptions": []
    }))
    .expect("obligation");
    linked.obligations.push(obligation.clone());
    let named = signals(&RunFacts::default(), &linked);
    assert_eq!(named.len(), 1);
    assert!(named[0].starts_with("ORD-1: applies through a semantic link"));

    obligation.obligation.limitations.clear();
    linked.obligations = vec![obligation];
    assert!(signals(&RunFacts::default(), &linked).is_empty());
}

#[test]
fn a_proposal_becomes_an_optional_obligation_only_at_code_that_is_shown() {
    let reader = Reader::new();
    let (context, _) = context(&reader);
    let plan = plan();
    let mut proposals = vec![
        proposal("ref-2", 1, 2),
        proposal("ref-99", 1, 1),
        json!({"question": "anything else?", "anchors": [], "criterion": "c"}),
    ];
    proposals.extend((0..MAX_PROPOSALS).map(|_| proposal("ref-2", 1, 1)));
    let output: CoverageOutput =
        serde_json::from_value(json!({"proposals": proposals})).expect("output");
    let (accepted, refused) = accept(output, &context, &reader, &plan, &snapshot());

    let first = &accepted[0].obligation;
    assert_eq!(first.rule_id, "AUDIT-1");
    assert!(!first.required, "an audit check never lowers coverage");
    assert!(matches!(first.status, ObligationStatus::Unknown { .. }));
    assert_eq!(first.review_unit_ids, [plan.scope.units[0].id]);
    assert_eq!(first.code_refs.len(), 1);
    assert_eq!(
        accepted[0].criteria,
        ["every reader of `total` handles the new shape"]
    );
    // The unresolved and anchorless ones, and every one past the first three of the answer.
    assert_eq!(accepted.len(), 1);
    assert_eq!(refused.len(), 2 + MAX_PROPOSALS, "{refused:?}");
    assert!(
        refused
            .iter()
            .all(|r| r.starts_with("coverage audit proposal refused"))
    );
    assert_eq!(
        refused.iter().filter(|r| r.contains("more than")).count(),
        MAX_PROPOSALS
    );
}
