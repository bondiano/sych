//! The verifier alone over frozen candidates: a replay asks only the verifier, about exactly
//! the candidates an earlier run discovered, each on the context its pass built.

use std::collections::BTreeMap;

use serde_json::json;
use sych_app::analysis::Outcome;
use sych_core::VerificationStatus;
use sych_core::config::Budgets;
use sych_core::model::{ModelReply, ModelRequest, ProviderError};

mod harness;

use harness::*;

fn verdict(status: &str) -> String {
    json!({"verdicts": [{"candidate_id": "cand-1", "verdict": status, "category": "DEFECT",
        "severity": "MEDIUM", "support": "SEMANTIC_ARGUMENT", "title": "t", "explanation": "e",
        "anchors": [], "evidence_ids": [], "alternatives_checked": [], "suggested_action": "",
        "validation_expectation": "", "limitations": []}], "context_requests": []})
    .to_string()
}

/// One candidate on the first HEAD span; the verifier of the first run leaves it open.
fn discovering(request: &ModelRequest) -> Result<ModelReply, ProviderError> {
    match request.schema_name.as_str() {
        "discovery" | "review" => {
            let (id, line) = first_ref(request, "HEAD");
            let mut found = json!({
                "candidates": [{"category": "DEFECT", "claim": "`total` overflows",
                    "conditions": ["large orders"], "causal_chain": ["no check"], "consequence": "wraps",
                    "anchors": [{"ref_id": id, "start_line": line, "end_line": line}], "evidence_ids": [],
                    "obligation_ids": [], "proposed_severity": "MEDIUM", "disproof_question": "a check?"}],
                "context_requests": [], "unresolved_questions": []
            });
            if request.schema_name == "review" {
                found["assessments"] = json!([supported(request)]);
            }

            Ok(reply(&found.to_string()))
        }
        "candidate_verdicts" => Ok(reply(&verdict("UNRESOLVED"))),
        _ => supported_and_empty(request),
    }
}

/// A second verifier that rejects; anything but a verdict would be a new search.
fn rejecting(request: &ModelRequest) -> Result<ModelReply, ProviderError> {
    match request.schema_name.as_str() {
        "candidate_verdicts" => Ok(reply(&verdict("REJECTED"))),
        _ => supported_and_empty(request),
    }
}

#[tokio::test(flavor = "current_thread")]
async fn a_replay_verifies_the_frozen_candidates_and_searches_nothing() {
    let repo = repo(false);
    let budgets = Budgets::default();
    let (first, _, _) = run_reusing(
        repo.path(),
        discovering,
        budgets.clone(),
        (14_000, usize::MAX),
        true,
        BTreeMap::new(),
        None,
    )
    .await;
    let frozen = first.record.discovered.clone();
    assert!(
        !frozen.is_empty() && frozen.iter().all(|d| !d.batch.is_empty()),
        "every discovery names its packets: {frozen:?}"
    );
    assert!(
        first
            .candidates
            .iter()
            .all(|c| c.verification_status == VerificationStatus::Unresolved)
    );

    let (again, calls, _) = run_reusing(
        repo.path(),
        rejecting,
        budgets,
        (14_000, usize::MAX),
        true,
        BTreeMap::new(),
        Some(frozen),
    )
    .await;
    assert!(
        !calls.is_empty() && calls.iter().all(|c| c == "candidate_verdicts"),
        "only the verifier is asked: {calls:?}"
    );
    let claims = |o: &Outcome| {
        o.candidates
            .iter()
            .map(|c| c.claim.clone())
            .collect::<Vec<_>>()
    };
    assert_eq!(claims(&again), claims(&first), "the same candidates");
    assert!(
        again
            .candidates
            .iter()
            .all(|c| c.verification_status == VerificationStatus::Rejected),
        "{:?}",
        again.candidates
    );
}
