//! Merges through the engine over a real snapshot with a scripted provider: what several
//! passes, chunks or answers say about one place or one criterion reaches the outcome once.

use serde_json::{Value, json};
use sych_core::Severity;
use sych_core::config::Budgets;
use sych_core::model::{ModelReply, ModelRequest, ProviderError};

mod harness;

use harness::*;

/// A candidate at the first HEAD span, from the risk audit only; the verifier confirms it there.
fn audit_finds_one_per_chunk(request: &ModelRequest) -> Result<ModelReply, ProviderError> {
    let shows_head = request.messages[1]
        .content
        .lines()
        .any(|l| l.starts_with("<<<DATA ") && l.contains(" HEAD "));
    let audit = shows_head && request.messages[0].content.contains("risk auditor");
    let anchors = || {
        let (id, line) = first_ref(request, "HEAD");
        json!([{"ref_id": id, "start_line": line, "end_line": line}])
    };
    match request.schema_name.as_str() {
        "discovery" if audit => Ok(reply(
            &json!({"candidates": [{"category": "DEFECT", "claim": "the guard is gone",
                "conditions": ["items undefined"], "causal_chain": ["no guard"], "consequence": "throws",
                "anchors": anchors(), "evidence_ids": [], "obligation_ids": ["ob-1"],
                "proposed_severity": "MEDIUM", "disproof_question": "a fallback?"}],
                "context_requests": [], "unresolved_questions": []})
            .to_string(),
        )),
        "candidate_verdicts" => Ok(reply(&verdicts(&[("cand-1", "MEDIUM")], &anchors()))),
        _ => supported_and_empty(request),
    }
}

/// Confirmed verdicts with the same anchors, in the given order.
fn verdicts(of: &[(&str, &str)], anchors: &Value) -> String {
    let verdicts: Vec<Value> = of
        .iter()
        .map(|(id, severity)| {
            json!({"candidate_id": id, "verdict": "CONFIRMED", "category": "DEFECT",
                "severity": severity, "support": "SEMANTIC_ARGUMENT", "title": format!("{severity} t"),
                "explanation": "e", "anchors": anchors, "evidence_ids": [],
                "alternatives_checked": [], "suggested_action": "", "validation_expectation": "",
                "limitations": []})
        })
        .collect();
    json!({"verdicts": verdicts, "context_requests": []}).to_string()
}

#[tokio::test(flavor = "current_thread")]
async fn the_risk_findings_of_every_chunk_reach_the_outcome() {
    // Each changed HEAD file is a chunk of its own; the BASE side of `src/order.ts` does not fit.
    let config = "schema_version: 1\ncontext:\n  initial_tokens_per_unit: 90\n  max_tokens_per_request: 90\n";
    let (outcome, _) = run_on(
        repo_with(true, &[(".sych/config.yaml", config)]),
        audit_finds_one_per_chunk,
        Budgets::default(),
        1,
        true,
    )
    .await;
    let mut paths: Vec<&str> = outcome
        .findings
        .iter()
        .flat_map(|f| f.anchors.iter().map(|a| a.path.as_str()))
        .collect();
    paths.sort_unstable();
    paths.dedup();
    assert_eq!(
        paths,
        ["lib/format.ts", "src/order.ts"],
        "{:?}",
        outcome.findings
    );
}

/// Two candidates on one span; the verifier confirms both, at different severities (below HIGH,
/// which needs a static trace).
fn overlapping(request: &ModelRequest, high_first: bool) -> Result<ModelReply, ProviderError> {
    let (id, line) = first_ref(request, "HEAD");
    let anchors = json!([{"ref_id": id, "start_line": line, "end_line": line}]);
    let candidate = |claim: &str| {
        json!({"category": "DEFECT", "claim": claim, "conditions": ["items undefined"],
            "causal_chain": ["no guard"], "consequence": "throws", "anchors": anchors,
            "evidence_ids": [], "obligation_ids": [], "proposed_severity": "MEDIUM",
            "disproof_question": "a fallback?"})
    };
    match request.schema_name.as_str() {
        "discovery" | "review" => {
            let mut found = json!({"candidates": [candidate("throws"), candidate("returns NaN")],
                "context_requests": [], "unresolved_questions": []});
            if request.schema_name == "review" {
                found["assessments"] = json!([supported(request)]);
            }

            Ok(reply(&found.to_string()))
        }
        "candidate_verdicts" if high_first => Ok(reply(&verdicts(
            &[("cand-1", "MEDIUM"), ("cand-2", "LOW")],
            &anchors,
        ))),
        "candidate_verdicts" => Ok(reply(&verdicts(
            &[("cand-1", "LOW"), ("cand-2", "MEDIUM")],
            &anchors,
        ))),
        _ => supported_and_empty(request),
    }
}

#[tokio::test(flavor = "current_thread")]
async fn overlapping_verdicts_keep_the_most_severe_in_either_order() {
    type Answer = fn(&ModelRequest) -> Result<ModelReply, ProviderError>;
    let orders: [Answer; 2] = [|r| overlapping(r, true), |r| overlapping(r, false)];
    for answer in orders {
        let (outcome, _) = run(answer, Budgets::default()).await;
        let severities: Vec<_> = outcome.findings.iter().map(|f| f.severity).collect();
        assert_eq!(severities, [Severity::Medium], "{:?}", outcome.findings);
    }
}

/// The one criterion answered twice, met and then not met.
fn criterion_twice(request: &ModelRequest) -> Result<ModelReply, ProviderError> {
    let mut assessment = supported(request);
    let met = assessment["criterion_results"][0].clone();
    let mut not_met = met.clone();
    not_met["status"] = json!("NOT_MET");
    assessment["criterion_results"] = json!([met, not_met]);
    match request.schema_name.as_str() {
        "obligation_assessment" => Ok(reply(&assessment.to_string())),
        "review" => Ok(reply(
            &json!({"assessments": [assessment], "candidates": [], "context_requests": [],
                "unresolved_questions": []})
            .to_string(),
        )),
        _ => supported_and_empty(request),
    }
}

#[tokio::test(flavor = "current_thread")]
async fn a_repeated_criterion_is_not_settled_by_its_first_answer() {
    // One unit: the combined review assesses it; two units: the standalone assessment.
    for second_unit in [false, true] {
        let (outcome, calls) = run_on(
            repo(second_unit),
            criterion_twice,
            Budgets::default(),
            14_000,
            true,
        )
        .await;
        assert_ne!(
            reason(&outcome.assessments[0].status),
            "SUPPORTED",
            "{calls:?}"
        );
        assert!(
            outcome.record.assessments[0]
                .problems
                .iter()
                .any(|p| p.contains("no single result for c-1")),
            "{calls:?}: {:?}",
            outcome.record.assessments[0].problems
        );
    }
}
