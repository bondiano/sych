//! Discovery's expansion rounds over a real snapshot with a scripted provider: an answered ask
//! earns another discovery call that sees the answer and what was proposed, and none without
//! rounds.

use serde_json::json;
use sych_core::VerificationStatus;
use sych_core::analysis::expansion::EXPANSION;
use sych_core::config::Budgets;
use sych_core::model::{ModelReply, ModelRequest, ProviderError};

mod harness;

use harness::*;

/// [`expanding`], but the expansion round sees the answer and what was proposed, and adds a
/// second hypothesis; the verifier checks both.
fn expanding_twice(request: &ModelRequest) -> Result<ModelReply, ProviderError> {
    let (system, user) = (&request.messages[0].content, &request.messages[1].content);
    if request.schema_name != "discovery" || !system.contains("further round") {
        // Only the expansion round is answered here; the rest is `expanding`.
        return if request.schema_name == "candidate_verdicts" {
            Ok(reply(
                &json!({"verdicts": [], "context_requests": []}).to_string(),
            ))
        } else {
            expanding(request)
        };
    }

    assert!(
        user.contains("answer to: the whole head function"),
        "{user}"
    );
    assert!(
        user.contains("- undefined items throw (src/order.ts:"),
        "{user}"
    );
    let (id, line) = first_ref(request, "HEAD");
    Ok(reply(
        &json!({"candidates": [{"category": "DEFECT", "claim": "a negative total is kept",
            "conditions": ["a refund"], "causal_chain": ["no clamp"], "consequence": "negative",
            "anchors": [{"ref_id": id, "start_line": line, "end_line": line}], "evidence_ids": [],
            "obligation_ids": [], "proposed_severity": "LOW", "disproof_question": "a clamp?"}],
            "context_requests": [], "unresolved_questions": []})
        .to_string(),
    ))
}

#[tokio::test(flavor = "current_thread")]
async fn an_answered_ask_earns_an_expansion_round_that_adds_a_hypothesis() {
    let (outcome, calls) = run(expanding_twice, Budgets::default()).await;
    let expansions = outcome
        .record
        .passes
        .iter()
        .filter(|p| p.template == EXPANSION)
        .count();
    assert_eq!(expansions, 1, "{calls:?}");
    let added = outcome
        .candidates
        .iter()
        .find(|c| c.claim == "a negative total is kept")
        .expect("the expansion round's hypothesis");
    // Shown to the verifier, which gave it no verdict.
    assert_eq!(added.verification_status, VerificationStatus::Unresolved);
}

#[tokio::test(flavor = "current_thread")]
async fn without_expansion_rounds_discovery_is_asked_once() {
    let config = "schema_version: 1\ncontext:\n  expansion_rounds: 0\n";
    let (outcome, calls) = run_on(
        repo_with(false, &[(".sych/config.yaml", config)]),
        expanding,
        Budgets::default(),
        14_000,
        true,
    )
    .await;
    assert_eq!(
        calls,
        [
            "review",
            "discovery",
            "candidate_verdicts",
            "candidate_verdicts"
        ]
    );
    assert!(
        outcome.record.answers.iter().all(|a| a.refs == 1),
        "the asks are still answered for the verifier"
    );
}
