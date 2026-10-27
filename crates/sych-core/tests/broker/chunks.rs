//! A required guard whose spans do not fit one request is split into chunks; the chunks'
//! assessments merge into one verdict per rule.

use sych_core::broker::{Limits, Priority, REQUIRED_OVER_BUDGET, context_limited};
use sych_core::effects::EffectLedger;
use sych_core::{Assessment, ObligationStatus};

use super::{Reader, facts, limits, manifest, oid, path, run, run_on};

#[test]
fn required_spans_that_fit_one_by_one_are_split_into_chunks() {
    let reader = Reader::new();
    // A second changed file in the same unit: two required spans for the guard.
    let mut two = manifest();
    let mut tx = two.files[0].clone();
    let new = tx.new.as_mut().expect("added");
    (new.path, new.oid, new.line_count) = (path("src/tx.rs"), oid('e'), Some(4));
    tx.hunks[0].new_start = 2;
    two.files.push(tx);
    let none = EffectLedger::default();
    let (_, whole) = run_on(&two, limits(), &[facts(&reader)], &[], &none, &[]);
    let required: Vec<u32> = whole
        .iter()
        .filter(|p| p.request.obligation_id.is_some())
        .flat_map(|p| &p.fragments)
        .filter(|f| f.priority == Priority::Required)
        .map(|f| f.tokens)
        .collect();
    assert!(required.len() > 1, "the guard has several required spans");
    let largest = *required.iter().max().expect("spans");
    assert!(largest < required.iter().sum(), "they do not fit together");
    let tight = Limits {
        unit_tokens: largest,
        request_tokens: largest,
        ..limits()
    };
    let (plan, packets) = run_on(&two, tight, &[facts(&reader)], &[], &none, &[]);
    let chunks: Vec<_> = packets
        .iter()
        .filter(|p| p.request.obligation_id.is_some())
        .collect();
    assert!(chunks.len() > 1, "{} chunks", chunks.len());
    assert!(chunks.iter().all(|p| p.usage.tokens <= largest));
    assert!(
        chunks
            .iter()
            .flat_map(|p| &p.excluded)
            .all(|e| e.reason != REQUIRED_OVER_BUDGET)
    );
    let shown: usize = chunks
        .iter()
        .flat_map(|p| &p.fragments)
        .filter(|f| f.priority == Priority::Required)
        .count();
    assert_eq!(shown, required.len(), "every required span is shown once");

    let assessment = |status| Assessment {
        rule_id: "EFFECT-01".into(),
        status,
        evidence_ids: Vec::new(),
        limitations: Vec::new(),
    };
    let both = vec![
        assessment(ObligationStatus::Supported),
        assessment(ObligationStatus::Supported),
    ];
    let merged = context_limited(&plan, &packets, both);
    assert_eq!(merged.len(), 1, "one verdict per rule");
    assert_eq!(merged[0].status, ObligationStatus::Supported);
    let mixed = vec![
        assessment(ObligationStatus::Violated),
        assessment(ObligationStatus::Supported),
    ];
    let merged = context_limited(&plan, &packets, mixed);
    assert_eq!(merged.len(), 1);
    assert_eq!(
        merged[0].status,
        ObligationStatus::Violated,
        "a violated chunk stands"
    );
}

#[test]
fn a_violation_shown_in_a_chunk_stands_beside_an_over_budget_span() {
    let reader = Reader::new();
    let starved = Limits {
        unit_tokens: 10,
        request_tokens: 10,
        ..limits()
    };
    let (plan, packets) = run(starved, &[facts(&reader)]);
    let violated = Assessment {
        rule_id: "EFFECT-01".into(),
        status: ObligationStatus::Violated,
        evidence_ids: Vec::new(),
        limitations: Vec::new(),
    };
    let assessments = context_limited(&plan, &packets, vec![violated]);
    assert_eq!(assessments.len(), 1);
    assert_eq!(assessments[0].status, ObligationStatus::Violated);
}
