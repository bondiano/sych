//! Model passes over a real snapshot with a scripted provider (roadmap M1.5–M1.8 harness):
//! the engine keeps its statuses, gaps and accounting whatever the model answers.

use serde_json::{Value, json};
use sych_app::analysis::PassStatus;
use sych_core::VerificationStatus;
use sych_core::broker::OpStatus;
use sych_core::config::Budgets;
use sych_core::model::{
    ChargeState, ErrorKind, ModelReply, ModelRequest, ProviderError, TaskFailure,
};

mod harness;

use harness::*;

#[tokio::test(flavor = "current_thread")]
async fn safe_change_with_empty_discovery_is_assessed_not_skipped() {
    let (outcome, calls) = run(supported_and_empty, Budgets::default()).await;
    // Discovery that also assesses the obligation it shows whole, then the high-risk audit;
    // nothing to verify.
    assert_eq!(calls, ["review", "discovery"]);
    assert_eq!(outcome.assessments.len(), 1);
    assert_eq!(reason(&outcome.assessments[0].status), "SUPPORTED");
    assert!(outcome.candidates.is_empty() && outcome.findings.is_empty());
    assert!(outcome.gaps.is_empty() && !outcome.limit_reached);
    let record = &outcome.record;
    assert_eq!(record.attempts.len(), 2);
    assert!(
        record
            .attempts
            .iter()
            .all(|a| a.charge_state == ChargeState::Settled)
    );
    assert_eq!(
        record.assessments[0].criteria.len(),
        1,
        "obligation → criterion → anchor"
    );
    assert_eq!(record.assessments[0].criteria[0].anchors.len(), 1);
}

#[tokio::test(flavor = "current_thread")]
async fn prose_after_one_repair_fails_the_pass_and_keeps_unknown() {
    let (outcome, calls) = run(prose, Budgets::default()).await;
    // Every pass: initial attempt plus one repair, then failed; nothing is fished out of prose.
    assert_eq!(calls.len(), 4, "{calls:?}");
    assert_eq!(reason(&outcome.assessments[0].status), "MODEL_FAILED");
    assert_eq!(outcome.gaps.len(), 2, "{:?}", outcome.gaps);
    assert!(outcome.record.passes.iter().all(|p| matches!(
        &p.status,
        PassStatus::Failed {
            failure: TaskFailure::Schema { .. }
        }
    )));
}

#[tokio::test(flavor = "current_thread")]
async fn attempt_cap_leaves_later_passes_undone_and_marks_the_limit() {
    let mut budgets = Budgets::default();
    budgets.analysis.max_attempts = Some(1);
    let (outcome, calls) = run(supported_and_empty, budgets).await;
    assert_eq!(
        calls,
        ["review"],
        "the reserve goes to the call that assesses the required obligation"
    );
    assert_eq!(reason(&outcome.assessments[0].status), "SUPPORTED");
    assert!(outcome.limit_reached);
    assert_eq!(outcome.gaps.len(), 1, "{:?}", outcome.gaps);
}

#[tokio::test(flavor = "current_thread")]
async fn dispatched_timeout_keeps_an_unknown_charge() {
    let (outcome, _) = run(timeout, Budgets::default()).await;
    assert_eq!(reason(&outcome.assessments[0].status), "MODEL_FAILED");
    let attempts = &outcome.record.attempts;
    assert!(!attempts.is_empty());
    assert!(
        attempts
            .iter()
            .all(|a| a.charge_state == ChargeState::Unknown && a.charge.is_none())
    );
    let unknown: Value = serde_json::to_value(&outcome.record.passes[0].status).expect("json");
    assert_eq!(unknown["status"], "FAILED");
}

/// Discovery proposes one hypothesis and asks for lines 1-3 of the head file; the verifier
/// must see that answer as a span and rejects the hypothesis.
fn expanding(request: &ModelRequest) -> Result<ModelReply, ProviderError> {
    let user = &request.messages[1].content;
    match request.schema_name.as_str() {
        "obligation_assessment" => supported_and_empty(request),
        "discovery" | "review" => {
            let (id, line) = first_ref(request, "HEAD");
            let mut found = json!({
                "candidates": [{"category": "DEFECT", "claim": "undefined items throw",
                    "conditions": ["items undefined"], "causal_chain": ["no guard"], "consequence": "throws",
                    "anchors": [{"ref_id": id, "start_line": line, "end_line": line}], "evidence_ids": [],
                    "obligation_ids": [], "proposed_severity": "MEDIUM", "disproof_question": "a fallback?"}],
                "context_requests": [{"question": "the whole head function",
                    "operation": {"op": "read_lines", "side": "HEAD", "path": "src/order.ts", "start_line": 1, "end_line": 3}}],
                "unresolved_questions": []
            });
            if request.schema_name == "review" {
                found["assessments"] = json!([supported(request)]);
            }

            Ok(reply(&found.to_string()))
        }
        _ => {
            assert!(
                user.contains("answer to: the whole head function"),
                "verifier sees the answer"
            );
            Ok(reply(
                &json!({"verdicts": [{"candidate_id": "cand-1", "verdict": "REJECTED",
                "category": "DEFECT", "severity": "MEDIUM", "support": "SEMANTIC_ARGUMENT",
                "title": "t", "explanation": "`?? []` keeps it",  "anchors": [], "evidence_ids": [],
                "alternatives_checked": ["nullish fallback"], "suggested_action": "",
                "validation_expectation": "", "limitations": []}], "context_requests": []})
                .to_string(),
            ))
        }
    }
}

fn unsupported(_: &ModelRequest) -> Result<ModelReply, ProviderError> {
    Err(ProviderError {
        kind: ErrorKind::Unsupported,
        dispatched: false,
        message: "no upstream supports the schema".into(),
        retry_after_seconds: None,
    })
}

#[tokio::test(flavor = "current_thread")]
async fn context_request_is_answered_through_the_broker_for_the_verifier() {
    let (outcome, calls) = run(expanding, Budgets::default()).await;
    assert_eq!(
        calls,
        [
            "review",
            "discovery",
            "candidate_verdicts",
            "candidate_verdicts"
        ]
    );
    let answers = &outcome.record.answers;
    assert_eq!(answers.len(), 2, "one per discovery-like pass");
    assert!(
        answers
            .iter()
            .all(|a| a.refs == 1 && a.status == Some(OpStatus::CompleteForQuery))
    );
    assert!(
        outcome
            .candidates
            .iter()
            .all(|c| c.verification_status == VerificationStatus::Rejected)
    );
    assert!(outcome.findings.is_empty());
}

#[tokio::test(flavor = "current_thread")]
async fn unsupported_route_is_a_released_failure_not_an_empty_success() {
    let (outcome, _) = run(unsupported, Budgets::default()).await;
    assert_eq!(reason(&outcome.assessments[0].status), "MODEL_FAILED");
    assert!(
        outcome
            .record
            .attempts
            .iter()
            .all(|a| a.charge_state == ChargeState::Released)
    );
    assert_eq!(outcome.gaps.len(), 2);
}

#[tokio::test(flavor = "current_thread")]
async fn small_units_share_one_discovery_call() {
    let (outcome, calls) = run_on(
        repo(true),
        supported_and_empty,
        Budgets::default(),
        14_000,
        true,
    )
    .await;
    // One review call for both units that also assesses the rule, then the risk audit.
    assert_eq!(calls, ["review", "discovery"]);
    let review = &outcome.record.passes[0];
    assert_eq!(review.packets.len(), 2, "{:?}", review.packets);
    assert_eq!(reason(&outcome.assessments[0].status), "SUPPORTED");
}

#[tokio::test(flavor = "current_thread")]
async fn an_obligation_split_across_calls_keeps_its_own_assessment() {
    // Each unit is its own discovery call, so neither shows the obligation whole.
    let (outcome, calls) =
        run_on(repo(true), supported_and_empty, Budgets::default(), 1, true).await;
    assert_eq!(
        calls,
        [
            "obligation_assessment",
            "discovery",
            "discovery",
            "discovery"
        ]
    );
    assert_eq!(reason(&outcome.assessments[0].status), "SUPPORTED");
}

#[expect(
    clippy::unnecessary_wraps,
    reason = "scripted answers share the provider signature"
)]
fn review_without_assessment(_: &ModelRequest) -> Result<ModelReply, ProviderError> {
    Ok(reply(
        r#"{"assessments": [], "candidates": [], "context_requests": [], "unresolved_questions": []}"#,
    ))
}

#[tokio::test(flavor = "current_thread")]
async fn a_review_that_skips_the_assessment_leaves_it_unknown() {
    let (outcome, calls) = run(review_without_assessment, Budgets::default()).await;
    assert_eq!(calls[0], "review");
    assert_eq!(reason(&outcome.assessments[0].status), "MODEL_FAILED");
    assert_eq!(
        outcome.record.assessments[0].problems,
        ["ob-1 was not assessed"]
    );
}

#[tokio::test(flavor = "current_thread")]
async fn without_the_verifier_candidates_stay_unverified() {
    let (outcome, calls) = run_on(repo(false), expanding, Budgets::default(), 14_000, false).await;
    assert_eq!(calls, ["review", "discovery"], "no verification call");
    assert!(!outcome.candidates.is_empty());
    assert!(
        outcome
            .candidates
            .iter()
            .all(|c| c.verification_status == VerificationStatus::Pending)
    );
    assert!(
        outcome.findings.is_empty(),
        "an unverified candidate is no finding"
    );
}

/// The review asks an open question; the high-risk audit returns one candidate over the cap.
fn questions_and_overflow(request: &ModelRequest) -> Result<ModelReply, ProviderError> {
    let draft = json!({"category": "DEFECT", "claim": "c", "conditions": [], "causal_chain": [],
        "consequence": "x", "anchors": [{"ref_id": "ref-999", "start_line": 1, "end_line": 1}],
        "evidence_ids": [], "obligation_ids": [], "proposed_severity": "LOW", "disproof_question": "q"});
    let mut found = json!({"candidates": [], "context_requests": [],
        "unresolved_questions": ["who calls total()?"]});
    match request.schema_name.as_str() {
        "review" => found["assessments"] = json!([supported(request)]),
        "discovery" => {
            found["candidates"] = json!(vec![draft; 21]);
            found["unresolved_questions"] = json!([]);
        }
        _ => return supported_and_empty(request),
    }
    Ok(reply(&found.to_string()))
}

#[tokio::test(flavor = "current_thread")]
async fn open_questions_and_dropped_candidates_reach_the_result() {
    let (outcome, calls) = run(questions_and_overflow, Budgets::default()).await;
    assert_eq!(calls, ["review", "discovery"]);
    assert!(
        outcome.questions == ["open question: who calls total()?"],
        "{:?}",
        outcome.questions
    );
    assert!(
        outcome
            .gaps
            .iter()
            .any(|g| g.starts_with("CANDIDATE_OVERFLOW: 1 candidates")),
        "{:?}",
        outcome.gaps
    );
    assert!(
        outcome.limit_reached,
        "dropped output makes coverage partial"
    );
    assert!(
        outcome
            .record
            .passes
            .iter()
            .all(|p| matches!(p.status, PassStatus::Done)),
        "a question is not a failed pass"
    );
}

/// The verifier cannot decide from the shown spans, asks for lines 1-3 and decides on the
/// second round, which sees the answer.
fn verifier_asks(request: &ModelRequest) -> Result<ModelReply, ProviderError> {
    if request.schema_name != "candidate_verdicts" {
        return expanding(request);
    }

    let user = &request.messages[1].content;
    let verdict = if user.contains("answer to: the verifier's ask") {
        "REJECTED"
    } else {
        "UNRESOLVED"
    };
    Ok(reply(
        &json!({"verdicts": [{"candidate_id": "cand-1", "verdict": verdict,
        "category": "DEFECT", "severity": "MEDIUM", "support": "SEMANTIC_ARGUMENT",
        "title": "t", "explanation": "e", "anchors": [], "evidence_ids": [],
        "alternatives_checked": [], "suggested_action": "", "validation_expectation": "",
        "limitations": []}],
        "context_requests": [{"question": "the verifier's ask",
            "operation": {"op": "read_lines", "side": "HEAD", "path": "src/order.ts", "start_line": 1, "end_line": 3}}]})
        .to_string(),
    ))
}

#[tokio::test(flavor = "current_thread")]
async fn an_unresolved_candidate_is_verified_again_with_the_verifiers_answers() {
    let (outcome, calls) = run(verifier_asks, Budgets::default()).await;
    assert_eq!(
        calls.iter().filter(|c| *c == "candidate_verdicts").count(),
        4,
        "two rounds per verified pass: {calls:?}"
    );
    assert_eq!(
        outcome.record.answers.len(),
        4,
        "discovery and verifier asks"
    );
    assert!(
        outcome
            .candidates
            .iter()
            .all(|c| c.verification_status == VerificationStatus::Rejected),
        "{:?}",
        outcome.candidates
    );
}

/// The verifier searches BASE for a name nobody uses; the empty answer reaches the second
/// round, which can reject the claim instead of staying unresolved.
fn verifier_finds_no_reader(request: &ModelRequest) -> Result<ModelReply, ProviderError> {
    if request.schema_name != "candidate_verdicts" {
        return expanding(request);
    }

    let user = &request.messages[1].content;
    let verdict = if user.contains("no matches") {
        "REJECTED"
    } else {
        "UNRESOLVED"
    };
    Ok(reply(
        &json!({"verdicts": [{"candidate_id": "cand-1", "verdict": verdict,
        "category": "DEFECT", "severity": "MEDIUM", "support": "SEMANTIC_ARGUMENT",
        "title": "t", "explanation": "e", "anchors": [], "evidence_ids": [],
        "alternatives_checked": [], "suggested_action": "", "validation_expectation": "",
        "limitations": []}],
        "context_requests": [{"question": "readers of the value",
            "operation": {"op": "search_source", "side": "DIFF_BASE", "query": "noSuchReaderAnywhere"}}]})
        .to_string(),
    ))
}

#[tokio::test(flavor = "current_thread")]
async fn an_empty_answer_reaches_the_verifier() {
    let (outcome, _) = run(verifier_finds_no_reader, Budgets::default()).await;
    assert!(
        outcome
            .record
            .answers
            .iter()
            .any(|a| a.refs == 0 && a.status == Some(OpStatus::CompleteForQuery))
    );
    assert!(
        outcome
            .candidates
            .iter()
            .all(|c| c.verification_status == VerificationStatus::Rejected),
        "{:?}",
        outcome.candidates
    );
}

#[tokio::test(flavor = "current_thread")]
async fn a_second_round_over_the_budget_keeps_the_first_verdicts() {
    let mut budgets = Budgets::default();
    budgets.analysis.max_attempts = Some(3);
    let (outcome, calls) = run(verifier_asks, budgets).await;
    assert_eq!(calls, ["review", "discovery", "candidate_verdicts"]);
    assert_eq!(outcome.candidates.len(), 2, "no candidate is lost");
    assert!(
        outcome
            .candidates
            .iter()
            .all(|c| c.verification_status == VerificationStatus::Unresolved)
    );
}

#[tokio::test(flavor = "current_thread")]
async fn parallel_calls_give_the_outcome_of_sequential_ones() {
    let shape = |budgets: Budgets| async move {
        let (outcome, mut calls) = run(verifier_asks, budgets).await;
        calls.sort();
        let passes: Vec<_> = outcome
            .record
            .passes
            .iter()
            .map(|p| (p.template.clone(), p.packets.clone(), p.attempt_ids.len()))
            .collect();
        let candidates: Vec<_> = outcome
            .candidates
            .iter()
            .map(|c| (c.claim.clone(), c.verification_status))
            .collect();
        (calls, passes, candidates, outcome.assessments.len())
    };
    let mut sequential = Budgets::default();
    sequential.analysis.max_parallel_calls = Some(1);
    let mut parallel = Budgets::default();
    parallel.analysis.max_parallel_calls = Some(4);
    assert_eq!(shape(sequential).await, shape(parallel).await);
}
