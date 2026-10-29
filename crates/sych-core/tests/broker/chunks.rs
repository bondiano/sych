//! A required guard whose spans do not fit one request is split into chunks; the chunks'
//! assessments merge into one verdict per rule.

use sych_core::broker::{
    EvidencePacket, Limits, Priority, REQUIRED_OVER_BUDGET, context_gaps, context_limited,
};
use sych_core::effects::EffectLedger;
use sych_core::{Assessment, ChangeStatus, DiffManifest, ObligationStatus, SourceSide};

use super::{Reader, facts, limits, manifest, oid, path, run_on};

#[test]
fn required_spans_that_fit_one_by_one_are_split_into_chunks() {
    let reader = Reader::new();
    let two = two_files();
    let none = EffectLedger::default();
    let required = required_tokens(&reader, &two);
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
    let two = two_files();
    let none = EffectLedger::default();
    let required = required_tokens(&reader, &two);
    let smallest = *required.iter().min().expect("spans");
    assert!(required.iter().any(|&t| t > smallest), "one span is larger");
    // The smaller span fits a chunk, the larger one fits none.
    let tight = Limits {
        unit_tokens: smallest,
        request_tokens: smallest,
        ..limits()
    };
    let (plan, packets) = run_on(&two, tight, &[facts(&reader)], &[], &none, &[]);
    let chunks: Vec<_> = packets
        .iter()
        .filter(|p| p.request.obligation_id.is_some())
        .collect();
    let over = |p: &&&EvidencePacket| p.excluded.iter().any(|e| e.reason == REQUIRED_OVER_BUDGET);
    assert!(
        chunks.iter().any(|p| !p.fragments.is_empty() && !over(&p)),
        "a chunk that fit is assessed"
    );
    assert!(
        chunks.iter().any(|p| p.fragments.is_empty() && over(&p)),
        "the over-budget span is on a packet of its own"
    );
    let assessment = |status| Assessment {
        rule_id: "EFFECT-01".into(),
        status,
        evidence_ids: Vec::new(),
        limitations: Vec::new(),
    };
    let violated = context_limited(
        &plan,
        &packets,
        vec![assessment(ObligationStatus::Violated)],
    );
    assert_eq!(violated.len(), 1);
    assert_eq!(violated[0].status, ObligationStatus::Violated);
    let supported = context_limited(
        &plan,
        &packets,
        vec![assessment(ObligationStatus::Supported)],
    );
    assert!(matches!(
        &supported[0].status,
        ObligationStatus::Unknown { reason_code, .. } if reason_code == "CONTEXT_LIMIT"
    ));
}

#[test]
fn merged_spans_stay_within_the_span_line_limit() {
    let reader = Reader::new();
    let none = EffectLedger::default();
    let short = Limits {
        max_span_lines: 3,
        ..limits()
    };
    let (_, packets) = run_on(&manifest(), short, &[facts(&reader)], &[], &none, &[]);
    let fragments: Vec<_> = packets.iter().flat_map(|p| &p.fragments).collect();
    assert!(!fragments.is_empty());
    for f in fragments {
        let r = &f.code_ref;
        assert!(
            r.end_line - r.start_line < 3,
            "{}:{}-{}",
            r.path,
            r.start_line,
            r.end_line
        );
    }
}

#[test]
fn a_run_cap_never_shrinks_requests_below_the_largest_required_span() {
    let reader = Reader::new();
    let two = two_files();
    let largest = *required_tokens(&reader, &two).iter().max().expect("spans");
    let none = EffectLedger::default();
    let (_, packets) = run_on(&two, limits(), &[facts(&reader)], &[], &none, &[]);
    let wide = Limits {
        unit_tokens: largest * 2,
        request_tokens: largest * 2,
        ..limits()
    };
    let fitted = wide.fit_run(&packets, 1).expect("over the run cap");
    assert!(
        fitted.unit_tokens >= largest,
        "{} < {largest}",
        fitted.unit_tokens
    );
    assert!(fitted.request_tokens >= largest);
}

#[test]
fn a_discovery_unit_over_budget_is_split_without_dropping_required_spans() {
    let reader = Reader::new();
    let two = two_files();
    let none = EffectLedger::default();
    let discovery = |packets: &[EvidencePacket]| -> Vec<EvidencePacket> {
        packets
            .iter()
            .filter(|p| p.request.obligation_id.is_none())
            .cloned()
            .collect()
    };
    let (_, whole) = run_on(&two, limits(), &[facts(&reader)], &[], &none, &[]);
    let required: Vec<u32> = discovery(&whole)
        .iter()
        .flat_map(|p| &p.fragments)
        .filter(|f| f.priority == Priority::Required)
        .map(|f| f.tokens)
        .collect();
    let largest = *required.iter().max().expect("spans");
    assert!(largest < required.iter().sum(), "they do not fit together");
    let tight = Limits {
        unit_tokens: largest,
        request_tokens: largest,
        ..limits()
    };
    let (_, packets) = run_on(&two, tight, &[facts(&reader)], &[], &none, &[]);
    let chunks = discovery(&packets);
    assert!(chunks.len() > 1, "{} chunks", chunks.len());
    assert!(chunks.iter().all(|p| p.usage.tokens <= largest));
    assert!(
        context_gaps(&packets).is_empty(),
        "{:?}",
        context_gaps(&packets)
    );
    let shown = chunks
        .iter()
        .flat_map(|p| &p.fragments)
        .filter(|f| f.priority == Priority::Required)
        .count();
    assert_eq!(shown, required.len(), "every required span is shown once");
}

#[test]
fn a_tight_discovery_budget_keeps_the_head_side_of_a_modified_file() {
    let reader = Reader::new();
    let mut modified = manifest();
    let file = &mut modified.files[0];
    file.status = ChangeStatus::Modified;
    file.old = file.new.clone();
    (file.hunks[0].old_start, file.hunks[0].old_lines) = (6, 2);
    let none = EffectLedger::default();
    let (_, whole) = run_on(&modified, limits(), &[facts(&reader)], &[], &none, &[]);
    let required = |p: &EvidencePacket, side| {
        p.fragments
            .iter()
            .filter(|f| f.priority == Priority::Required && f.code_ref.side == side)
            .map(|f| f.tokens)
            .sum::<u32>()
    };
    let (base, head) = (
        required(&whole[0], SourceSide::DiffBase),
        required(&whole[0], SourceSide::Head),
    );
    assert!(base > 0 && head > 0, "both sides are required");
    // Room for one side only: the changed code under review wins over its old version.
    let tight = Limits {
        unit_tokens: head.max(base),
        request_tokens: head.max(base),
        ..limits()
    };
    let (_, packets) = run_on(&modified, tight, &[facts(&reader)], &[], &none, &[]);
    let discovery = &packets[0];
    assert_eq!(required(discovery, SourceSide::Head), head);
    assert!(
        discovery
            .excluded
            .iter()
            .filter(|e| e.reason == REQUIRED_OVER_BUDGET)
            .all(|e| e.code_ref.side == SourceSide::DiffBase),
        "{:?}",
        discovery.excluded
    );
}

/// A second changed file in the same unit: two required spans for the guard.
fn two_files() -> DiffManifest {
    let mut two = manifest();
    let mut tx = two.files[0].clone();
    let new = tx.new.as_mut().expect("added");
    (new.path, new.oid, new.line_count) = (path("src/tx.rs"), oid('e'), Some(4));
    tx.hunks[0].new_start = 2;
    two.files.push(tx);
    two
}

/// Tokens of the guard's required spans when everything fits.
fn required_tokens(reader: &Reader, manifest: &DiffManifest) -> Vec<u32> {
    let none = EffectLedger::default();
    let (_, whole) = run_on(manifest, limits(), &[facts(reader)], &[], &none, &[]);
    whole
        .iter()
        .filter(|p| p.request.obligation_id.is_some())
        .flat_map(|p| &p.fragments)
        .filter(|f| f.priority == Priority::Required)
        .map(|f| f.tokens)
        .collect()
}
