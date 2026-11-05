//! Where a changed effect went: the verifier is shown HEAD search results for an effect the
//! ledger saw leave a file the candidates point at.

use serde_json::json;
use sych_core::analysis::{AskOperation, drop_fixture_claims, effect_asks, validate_discovery};
use sych_core::config::EffectKind;
use sych_core::effects::{EffectMatch, LedgerEntry, Occurrence, Resolution};
use sych_core::{CodeRef, Language, SourceSide, SyntaxKind};

use super::{Reader, context, discovery, draft, path};

fn entry(target: &str, at: &CodeRef, matched: EffectMatch) -> LedgerEntry {
    LedgerEntry {
        effect_id: format!("event:{target}@{}", at.path),
        kind: EffectKind::Event,
        target: target.into(),
        shape: SyntaxKind::RegisteredEffectCall,
        language: Language::Go,
        path: at.path.clone(),
        symbol: Some("Store.Create".into()),
        registry_id: None,
        durable_with_transaction: None,
        contract_refs: Vec::new(),
        base: vec![Occurrence {
            code_ref: at.clone(),
            symbol: Some("Store.Create".into()),
            payload: String::new(),
            guard: None,
            transaction_scope: "n/a".into(),
            lifetime_scope: "caller".into(),
            failure_behavior: "n/a".into(),
            in_change: true,
        }],
        head: Vec::new(),
        matched,
        resolution: Resolution::Resolved,
        notes: Vec::new(),
    }
}

#[test]
fn an_effect_gone_from_a_candidates_file_is_searched_for_on_head() {
    let reader = Reader::new();
    let (context, _) = context(&reader);
    let one = draft(
        &json!([{"ref_id": "ref-2", "start_line": 2, "end_line": 2}]),
        &[],
        &[],
    );
    let candidates = validate_discovery(
        discovery(json!({"candidates": [one], "context_requests": [], "unresolved_questions": []})),
        "k",
        &context,
        &[],
        &reader,
    )
    .candidates;
    let here = candidates[0].anchors[0].clone();
    let mut elsewhere = here.clone();
    elsewhere.path = path("src/unrelated.go");
    let ledger = [
        entry(
            "example.com/shop/orders.Store.AppendEvent",
            &here,
            EffectMatch::Removed,
        ),
        // Named once, though two entries lose it.
        entry("orders.Store.AppendEvent", &here, EffectMatch::Unresolved),
        // The ledger already found its new place.
        entry("Publish", &here, EffectMatch::Moved),
        // No candidate points at that file.
        entry("Notify", &elsewhere, EffectMatch::Removed),
    ];
    let asks = effect_asks(&ledger, &candidates);
    let queries: Vec<_> = asks
        .iter()
        .map(|a| match &a.operation {
            AskOperation::SearchSource { side, query } => (*side, query.as_str()),
            other => panic!("a search, not {other:?}"),
        })
        .collect();
    assert_eq!(queries, [(SourceSide::Head, "AppendEvent")]);
    assert!(
        asks[0]
            .question
            .starts_with("effect ledger: where does `AppendEvent`")
    );
    assert!(effect_asks(&ledger, &[]).is_empty(), "nothing to point at");
}

#[test]
fn a_registered_effect_is_searched_before_a_transactions_own_calls() {
    let reader = Reader::new();
    let (context, _) = context(&reader);
    let one = draft(
        &json!([{"ref_id": "ref-2", "start_line": 2, "end_line": 2}]),
        &[],
        &[],
    );
    let candidates = validate_discovery(
        discovery(json!({"candidates": [one], "context_requests": [], "unresolved_questions": []})),
        "k",
        &context,
        &[],
        &reader,
    )
    .candidates;
    let here = candidates[0].anchors[0].clone();
    let mut ledger: Vec<LedgerEntry> = ["BeginTx", "Commit", "Rollback", "ExecContext"]
        .into_iter()
        .map(|t| entry(t, &here, EffectMatch::Removed))
        .collect();
    let mut outbox = entry("orders.Store.AppendEvent", &here, EffectMatch::Removed);
    outbox.registry_id = Some("order-outbox".into());
    ledger.push(outbox);
    let asks = effect_asks(&ledger, &candidates);
    assert_eq!(asks.len(), 4);
    assert!(
        asks[0].question.contains("`AppendEvent`"),
        "{:?}",
        asks.iter().map(|a| &a.question).collect::<Vec<_>>()
    );
}

#[test]
fn a_candidate_anchored_only_in_fixture_data_is_dropped_with_the_reason() {
    let reader = Reader::new();
    let (context, _) = context(&reader);
    let one = draft(
        &json!([{"ref_id": "ref-2", "start_line": 2, "end_line": 2}]),
        &[],
        &[],
    );
    let found = || {
        validate_discovery(
            discovery(
                json!({"candidates": [one.clone()], "context_requests": [], "unresolved_questions": []}),
            ),
            "k",
            &context,
            &[],
            &reader,
        )
    };
    let kept = drop_fixture_claims(found(), &["eval/quality/**".to_owned()]);
    assert_eq!(
        kept.candidates.len(),
        1,
        "an anchor outside the fixtures keeps it"
    );
    let dropped = drop_fixture_claims(found(), &["src/**".to_owned()]);
    assert!(dropped.candidates.is_empty());
    assert!(dropped.rejected.iter().any(|r| {
        r.problems
            .iter()
            .any(|p| p.contains("review.fixture_paths"))
    }));
    assert_eq!(drop_fixture_claims(found(), &[]).candidates.len(), 1);

    // Every anchor must lie in the fixtures: one in product code keeps it, and none is no claim
    // about fixtures.
    let mut mixed = found();
    let mut fixture = mixed.candidates[0].anchors[0].clone();
    fixture.path = path("eval/quality/case.yaml");
    mixed.candidates[0].anchors.push(fixture);
    let fixtures = ["eval/quality/**".to_owned(), "src/fixtures/**".to_owned()];
    assert_eq!(drop_fixture_claims(mixed, &fixtures).candidates.len(), 1);
    let mut bare = found();
    bare.candidates[0].anchors.clear();
    assert_eq!(
        drop_fixture_claims(bare, &["src/**".to_owned()])
            .candidates
            .len(),
        1
    );
}
