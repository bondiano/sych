//! Where a changed effect went: the verifier is shown HEAD search results for an effect the
//! ledger saw leave a file the candidates point at.

use serde_json::json;
use sych_core::analysis::{AskOperation, effect_asks, validate_discovery};
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
