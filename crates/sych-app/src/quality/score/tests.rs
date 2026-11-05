use std::fs;
use std::path::Path;

use super::*;
use crate::quality::Tolerated;
use crate::quality::stages::{Losses, Spread};

fn case(variant: &str, start: u32, end: u32) -> QualityCase {
    serde_yaml_ng::from_str(&format!(
        "schema_version: 1\nid: c\nsplit: development\nlanguage: go\nvariant: {variant}\n\
         profiles: [correctness]\nrationale: r\nexpected:\n  obligations:\n    - {{ rule_id: EFFECT-01, expected_status: violated }}\n  \
         findings:\n    - category: defect\n      semantic_target: t\n      required_condition: c\n      \
         evidence_locations:\n        - {{ path: backend/orders/create.go, start_line: {start}, end_line: {end} }}\n"
    ))
    .expect("case")
}

fn score_of(case: &QualityCase, result: &ReviewResult) -> Measured {
    score(case, result, &remarks(result), &[])
}

fn result() -> ReviewResult {
    let path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/core/v1/review-result.json");
    serde_json::from_str(&fs::read_to_string(path).expect("fixture")).expect("result")
}

#[test]
fn a_finding_matches_a_label_by_overlapping_location_only() {
    let hit = score_of(&case("unsafe", 45, 50), &result());
    assert_eq!((hit.matched_expected, hit.labelled_findings), (1, 1));
    assert_eq!(
        (hit.findings[0].labels.as_slice(), hit.labels.len()),
        (&[0][..], 1)
    );
    assert_eq!(hit.obligations[0].actual, hit.obligations[0].expected);
    let miss = score_of(&case("unsafe", 48, 50), &result());
    assert_eq!((miss.matched_expected, miss.labelled_findings), (0, 0));
    assert_eq!(miss.correctness_findings, 1);
}

#[test]
fn a_nit_label_is_hit_only_by_a_nit_or_improvement() {
    let mut nit = case("safe", 45, 50);
    nit.expected.findings[0].category = "nitpick".into();
    let mut result = result();
    let m = score_of(&nit, &result);
    assert_eq!(
        (m.expected_findings, m.expected_nits, m.matched_nits),
        (0, 1, 0)
    );
    result.findings[0].category = FindingCategory::Nit;
    assert_eq!(score_of(&nit, &result).matched_nits, 1);
}

#[test]
fn a_tolerated_remark_or_an_overrated_nit_is_not_a_false_finding() {
    let mut safe = case("safe", 45, 50);
    safe.expected.findings[0].category = "nitpick".into();
    let run = |case: &QualityCase| {
        let mut totals = Totals::default();
        totals.add(&RunReport {
            id: "c".into(),
            language: "go".into(),
            variant: CaseDisposition::Safe,
            run: 1,
            error: None,
            measured: Some(score_of(case, &result())),
        });
        totals
    };
    let nit = run(&safe);
    assert_eq!((nit.overrated_nits, nit.tolerated_findings), (1, 0));
    assert_eq!(
        (
            nit.clean_runs_with_correctness_finding,
            nit.clean_runs_with_false_finding
        ),
        (1, 0)
    );

    let mut tolerated = case("safe", 1, 2);
    tolerated.expected.findings.clear();
    tolerated.expected.tolerated = safe
        .expected
        .findings
        .drain(..)
        .map(|f| Tolerated {
            semantic_target: f.semantic_target,
            evidence_locations: f.evidence_locations,
        })
        .collect();
    let t = run(&tolerated);
    assert_eq!(
        (t.tolerated_findings, t.clean_runs_with_false_finding),
        (1, 0)
    );

    let mut wrong = case("safe", 1, 2);
    wrong.expected.findings.clear();
    assert_eq!(run(&wrong).clean_runs_with_false_finding, 1);
}

#[test]
fn a_labelled_defect_only_a_dropped_candidate_reached_is_a_verifier_miss() {
    let unsafe_case = case("unsafe", 45, 50);
    let mut result = result();
    let finding = result.findings.remove(0);
    let dropped = [Remark {
        category: FindingCategory::Defect,
        title: "dropped",
        anchors: &finding.anchors,
        status: VerificationStatus::Rejected,
    }];
    let m = score(&unsafe_case, &result, &[], &dropped);
    assert_eq!((m.matched_expected, m.verifier_dropped), (0, 1));
    let kept = score(&unsafe_case, &result, &dropped, &dropped);
    assert_eq!((kept.matched_expected, kept.verifier_dropped), (1, 0));
}

#[test]
fn rates_come_from_runs_and_stay_empty_without_a_base() {
    let mut totals = Totals::default();
    for variant in [CaseDisposition::Safe, CaseDisposition::Unsafe] {
        totals.add(&RunReport {
            id: "c".into(),
            language: "go".into(),
            variant,
            run: 1,
            error: None,
            measured: Some(score_of(&case("unsafe", 45, 50), &result())),
        });
    }
    totals.finish();
    assert_eq!(totals.clean_false_positive_rate, Some(1.0));
    assert_eq!((totals.precision, totals.recall), (Some(1.0), Some(1.0)));
    assert_eq!(totals.unknown_kept_rate, None);
}

#[test]
fn a_missed_defect_counts_at_the_furthest_stage_a_candidate_reached() {
    use VerificationStatus::{Confirmed, Rejected, Unresolved};
    let unsafe_case = case("unsafe", 45, 50);
    let mut result = result();
    let finding = result.findings.remove(0);
    let at = |status| Remark {
        category: FindingCategory::Defect,
        title: "candidate",
        anchors: &finding.anchors,
        status,
    };
    let losses = |candidates: &[Remark<'_>]| score(&unsafe_case, &result, &[], candidates).losses;
    let only = |stage: fn(&mut Losses) -> &mut u32| {
        let mut l = Losses::default();
        *stage(&mut l) = 1;
        l
    };
    assert_eq!(losses(&[]), only(|l| &mut l.not_discovered));
    assert_eq!(losses(&[at(Rejected)]), only(|l| &mut l.verifier_rejected));
    assert_eq!(
        losses(&[at(Rejected), at(Unresolved)]),
        only(|l| &mut l.verifier_unresolved)
    );
    assert_eq!(
        losses(&[at(Unresolved), at(Confirmed)]),
        only(|l| &mut l.confirmed_unmatched)
    );
    let nit = Remark {
        category: FindingCategory::Nit,
        ..at(Rejected)
    };
    assert_eq!(losses(&[nit]), only(|l| &mut l.not_discovered));
    let found = score(&unsafe_case, &result, &[at(Confirmed)], &[at(Rejected)]);
    assert_eq!(found.losses, Losses::default());
}

#[test]
fn an_obligation_unknown_for_its_context_is_counted() {
    let mut result = result();
    assert_eq!(
        score_of(&case("unsafe", 45, 50), &result).context_limited,
        0
    );
    result.obligations[0].status = ObligationStatus::Unknown {
        reason_code: "CONTEXT_LIMIT".into(),
        next_evidence_needed: "a smaller unit".into(),
    };
    assert_eq!(
        score_of(&case("unsafe", 45, 50), &result).context_limited,
        1
    );
}

#[test]
fn the_spread_is_taken_over_the_runs_that_have_a_rate() {
    let totals = |start| {
        let mut t = Totals::default();
        t.add(&RunReport {
            id: "c".into(),
            language: "go".into(),
            variant: CaseDisposition::Unsafe,
            run: 1,
            error: None,
            measured: Some(score_of(&case("unsafe", start, 50), &result())),
        });
        t.finish();
        t
    };
    let spread = serde_json::to_value(Spread::of(&[totals(45), totals(48)])).expect("json");
    assert_eq!(
        spread["recall"],
        serde_json::json!({"min": 0.0, "mean": 0.5, "max": 1.0})
    );
    assert!(spread["unknown_kept_rate"].is_null());
}
