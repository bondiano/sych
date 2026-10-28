//! A candidate the verifier leaves open without asking is still searched for what refutes it.

use serde_json::json;
use sych_core::VerificationStatus;
use sych_core::config::Budgets;
use sych_core::model::{ModelReply, ModelRequest, ProviderError};

mod harness;

use harness::*;

fn verdict(status: &str) -> String {
    json!({"verdicts": [{"candidate_id": "cand-1", "verdict": status, "category": "DEFECT",
        "severity": "MEDIUM", "support": "SEMANTIC_ARGUMENT", "title": "t",
        "explanation": "`?? []` keeps it", "anchors": [], "evidence_ids": [],
        "alternatives_checked": [], "suggested_action": "", "validation_expectation": "",
        "limitations": []}], "context_requests": []})
    .to_string()
}

/// Discovery claims `reduce` throws and asks nothing; the verifier decides only once it sees
/// where `reduce` is used.
fn silent(request: &ModelRequest) -> Result<ModelReply, ProviderError> {
    let user = &request.messages[1].content;
    match request.schema_name.as_str() {
        "discovery" | "review" => {
            let (id, line) = first_ref(request, "HEAD");
            let mut found = json!({
                "candidates": [{"category": "DEFECT", "claim": "`reduce` throws on undefined items",
                    "conditions": ["items undefined"], "causal_chain": ["no guard"], "consequence": "throws",
                    "anchors": [{"ref_id": id, "start_line": line, "end_line": line}], "evidence_ids": [],
                    "obligation_ids": [], "proposed_severity": "MEDIUM", "disproof_question": "a fallback?"}],
                "context_requests": [], "unresolved_questions": []
            });
            if request.schema_name == "review" {
                found["assessments"] = json!([supported(request)]);
            }

            Ok(reply(&found.to_string()))
        }
        "candidate_verdicts" if user.contains("answer to: where else is `reduce`") => {
            Ok(reply(&verdict("REJECTED")))
        }
        "candidate_verdicts" => Ok(reply(&verdict("UNRESOLVED"))),
        _ => supported_and_empty(request),
    }
}

#[tokio::test(flavor = "current_thread")]
async fn an_open_candidate_is_searched_for_its_refutation() {
    let (outcome, calls) = run(silent, Budgets::default()).await;
    assert!(
        calls.iter().filter(|c| *c == "candidate_verdicts").count() >= 2,
        "{calls:?}"
    );
    assert!(!outcome.record.answers.is_empty(), "the engine searched");
    assert!(
        outcome
            .candidates
            .iter()
            .all(|c| c.verification_status == VerificationStatus::Rejected),
        "{:?}",
        outcome.candidates
    );
}
