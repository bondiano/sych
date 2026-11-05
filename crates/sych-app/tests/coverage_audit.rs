//! The coverage audit in a run: it asks only when the facts call for it, adds what it proposes
//! at shown code as optional obligations, and never makes the run partial or failed.

use std::collections::BTreeMap;

use serde_json::json;
use sych_app::analysis::{Outcome, PassStatus};
use sych_core::config::{Budgets, EffectKind};
use sych_core::effects::{EffectMatch, LedgerEntry, Resolution};
use sych_core::model::{ModelReply, ModelRequest, ProviderError};
use sych_core::{Language, RunFacts, SyntaxKind};

mod harness;

use harness::*;

/// A ledger whose `publish` effect left `src/order.ts`: a reason to audit the change.
fn effect_removed() -> RunFacts {
    let mut facts = RunFacts::default();
    facts.ledger.entries.push(LedgerEntry {
        effect_id: "event:publish@src/order.ts#total".into(),
        kind: EffectKind::Event,
        target: "publish".into(),
        shape: SyntaxKind::RegisteredEffectCall,
        language: Language::Typescript,
        path: "src/order.ts".parse().expect("path"),
        symbol: Some("total".into()),
        registry_id: None,
        durable_with_transaction: None,
        contract_refs: Vec::new(),
        base: Vec::new(),
        head: Vec::new(),
        matched: EffectMatch::Removed,
        resolution: Resolution::Resolved,
        notes: Vec::new(),
    });
    facts
}

/// One proposal at the first HEAD span and one at code it was never shown.
fn proposing(request: &ModelRequest) -> Result<ModelReply, ProviderError> {
    if request.schema_name != "coverage_proposals" {
        return supported_and_empty(request);
    }

    let (id, line) = first_ref(request, "HEAD");
    Ok(reply(
        &json!({"proposals": [
            {"question": "does the consumer of the order event still get it?",
             "anchors": [{"ref_id": id, "start_line": line, "end_line": line}],
             "criterion": "the event is published where the change moved it"},
            {"question": "anything else?",
             "anchors": [{"ref_id": "ref-99", "start_line": 1, "end_line": 1}],
             "criterion": "c"}
        ]})
        .to_string(),
    ))
}

fn failing(request: &ModelRequest) -> Result<ModelReply, ProviderError> {
    if request.schema_name == "coverage_proposals" {
        return timeout(request);
    }

    supported_and_empty(request)
}

async fn audited(
    answer: fn(&ModelRequest) -> Result<ModelReply, ProviderError>,
    facts: RunFacts,
) -> (Outcome, Vec<String>) {
    let repo = repo(false);
    let (outcome, calls, _) = run_with_facts(
        repo.path(),
        answer,
        Budgets::default(),
        (14_000, usize::MAX),
        true,
        BTreeMap::new(),
        None,
        facts,
    )
    .await;
    (outcome, calls)
}

#[tokio::test(flavor = "current_thread")]
async fn an_accepted_proposal_is_assessed_as_an_optional_obligation() {
    let (outcome, calls) = audited(proposing, effect_removed()).await;
    assert_eq!(
        calls.iter().filter(|c| *c == "coverage_proposals").count(),
        1,
        "{calls:?}"
    );
    let [added] = outcome.extra_obligations.as_slice() else {
        panic!("one accepted proposal: {:?}", outcome.extra_obligations);
    };
    assert_eq!(added.obligation.rule_id, "AUDIT-1");
    assert!(!added.obligation.required);
    assert!(
        outcome.assessments.iter().any(|a| a.rule_id == "AUDIT-1"),
        "it got an assessment"
    );
    assert!(
        outcome
            .questions
            .iter()
            .any(|q| q.starts_with("coverage audit proposal refused")),
        "{:?}",
        outcome.questions
    );
    assert!(
        outcome.gaps.iter().all(|g| !g.contains("coverage")),
        "{:?}",
        outcome.gaps
    );
}

#[tokio::test(flavor = "current_thread")]
async fn a_failed_audit_is_an_open_question_not_a_gap() {
    let (outcome, _) = audited(failing, effect_removed()).await;
    assert!(
        outcome
            .questions
            .iter()
            .any(|q| q.starts_with("coverage audit did not run")),
        "{:?}",
        outcome.questions
    );
    assert!(outcome.gaps.iter().all(|g| !g.contains("coverage")));
    assert!(
        !outcome.record.passes.iter().any(|p| {
            p.template.starts_with("coverage_audit")
                && matches!(p.status, PassStatus::Failed { .. })
        }),
        "a skipped optional pass, never a failed one"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn without_a_signal_there_is_no_audit() {
    let (outcome, calls) = audited(proposing, RunFacts::default()).await;
    assert!(!calls.iter().any(|c| c == "coverage_proposals"));
    assert!(outcome.extra_obligations.is_empty());
}
