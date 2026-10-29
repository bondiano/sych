//! Scoring of one reviewed case against its labels, and the totals over runs.

use sych_core::{
    Candidate, CodeRef, Coverage, FindingCategory, ObligationStatus, Recommendation, ReviewResult,
};

use serde::Serialize;

use super::{CaseDisposition, ExpectedFinding, Location, QualityCase};
use crate::eval::status_tag;

#[derive(Serialize)]
struct FindingBrief {
    category: FindingCategory,
    title: String,
    at: Vec<String>,
    labelled: bool,
    /// Indexes into `Measured::labels` this finding overlaps: candidates for `eval judge`.
    /// Nitpick labels are not indexed, though they make `labelled` true: by meaning a
    /// correctness finding on a nit is an overrated nit, never a matched defect.
    labels: Vec<usize>,
}

#[derive(Serialize)]
struct ObligationCheck {
    rule_id: String,
    expected: String,
    actual: String,
}

#[derive(Serialize, Default)]
pub(super) struct Totals {
    runs: u32,
    errors: u32,
    expected_findings: u32,
    matched_expected: u32,
    /// Defect and rule-violation findings; nits and improvements are counted apart.
    correctness_findings: u32,
    labelled_findings: u32,
    clean_runs: u32,
    clean_runs_with_correctness_finding: u32,
    /// Safe runs with a correctness finding that is neither tolerated nor a labelled nit.
    clean_runs_with_false_finding: u32,
    /// Correctness findings on a tolerated location and on a labelled nit.
    tolerated_findings: u32,
    overrated_nits: u32,
    unknown_runs: u32,
    unknown_kept: u32,
    obligation_checks: u32,
    obligation_agreements: u32,
    nits: u32,
    expected_nits: u32,
    matched_nits: u32,
    model_attempts: u32,
    cost_known_micro_usd: u64,
    unknown_cost_attempts: u32,
    /// Labelled defects only a rejected or unresolved candidate reached: verifier misses.
    verifier_dropped: u32,
    /// Runs short of `COMPLETE_FOR_SCOPE`, and paths the reviews left unreviewed.
    partial_coverage_runs: u32,
    unreviewed_paths: u32,
    /// Labelled nitpicks a nit or improvement finding hit.
    nit_recall: Option<f64>,
    /// Labelled of all correctness findings.
    precision: Option<f64>,
    /// Matched of all expected findings.
    recall: Option<f64>,
    clean_false_positive_rate: Option<f64>,
    clean_false_finding_rate: Option<f64>,
    unknown_kept_rate: Option<f64>,
}

fn ratio(part: u32, whole: u32) -> Option<f64> {
    (whole > 0).then(|| f64::from(part) / f64::from(whole))
}

impl Totals {
    pub(super) fn finish(&mut self) {
        self.precision = ratio(self.labelled_findings, self.correctness_findings);
        self.recall = ratio(self.matched_expected, self.expected_findings);
        self.nit_recall = ratio(self.matched_nits, self.expected_nits);
        self.clean_false_positive_rate =
            ratio(self.clean_runs_with_correctness_finding, self.clean_runs);
        self.clean_false_finding_rate = ratio(self.clean_runs_with_false_finding, self.clean_runs);
        self.unknown_kept_rate = ratio(self.unknown_kept, self.unknown_runs);
    }

    pub(super) fn add(&mut self, r: &RunReport) {
        self.runs += 1;
        let Some(m) = &r.measured else {
            self.errors += 1;
            return;
        };
        self.expected_findings += m.expected_findings;
        self.matched_expected += m.matched_expected;
        self.correctness_findings += m.correctness_findings;
        self.labelled_findings += m.labelled_findings;
        if r.variant == CaseDisposition::Safe {
            self.clean_runs += 1;
            self.clean_runs_with_correctness_finding += u32::from(m.correctness_findings > 0);
            self.clean_runs_with_false_finding +=
                u32::from(m.correctness_findings > m.tolerated_findings + m.overrated_nits);
        }

        self.tolerated_findings += m.tolerated_findings;
        self.overrated_nits += m.overrated_nits;

        if let Some(kept) = m.unknown_kept {
            self.unknown_runs += 1;
            self.unknown_kept += u32::from(kept);
        }

        for o in &m.obligations {
            self.obligation_checks += 1;
            self.obligation_agreements += u32::from(o.expected == o.actual);
        }
        self.nits += m.nits;
        self.expected_nits += m.expected_nits;
        self.matched_nits += m.matched_nits;
        self.model_attempts += m.model_attempts;
        self.cost_known_micro_usd += m.cost_known_micro_usd;
        self.unknown_cost_attempts += m.unknown_cost_attempts;
        self.verifier_dropped += m.verifier_dropped;
        self.partial_coverage_runs += u32::from(m.coverage != Coverage::CompleteForScope);
        self.unreviewed_paths += m.unreviewed_paths;
    }
}

#[derive(Serialize)]
pub(super) struct Measured {
    run_id: String,
    recommendation: Recommendation,
    coverage: Coverage,
    expected_findings: u32,
    matched_expected: u32,
    correctness_findings: u32,
    labelled_findings: u32,
    tolerated_findings: u32,
    /// Correctness findings on a location labelled as a nitpick: right place, inflated category.
    overrated_nits: u32,
    nits: u32,
    /// Labelled nitpicks and the ones a nit or improvement finding hit.
    expected_nits: u32,
    matched_nits: u32,
    obligations: Vec<ObligationCheck>,
    /// `unknown` cases: no correctness finding and something left UNKNOWN or partial.
    unknown_kept: Option<bool>,
    /// Labelled defects and rule violations by meaning, in label order: what `eval judge` matches.
    labels: Vec<String>,
    /// `unknown` cases: what the review left open, to judge it against the labelled unknown.
    /// Wider than `unknown_kept`, which keeps its earlier meaning so reports stay comparable:
    /// unresolved questions alone do not make it true.
    left_open: Vec<String>,
    findings: Vec<FindingBrief>,
    model_attempts: u32,
    cost_known_micro_usd: u64,
    unknown_cost_attempts: u32,
    /// Labelled defects a rejected or unresolved candidate hit and no correctness finding did.
    verifier_dropped: u32,
    unreviewed_paths: u32,
}

#[derive(Serialize)]
pub(super) struct RunReport {
    pub(super) id: String,
    pub(super) language: String,
    pub(super) variant: CaseDisposition,
    pub(super) run: u32,
    pub(super) error: Option<String>,
    pub(super) measured: Option<Measured>,
}

fn overlaps(r: &CodeRef, l: &Location) -> bool {
    r.side == l.side && r.path == l.path && r.start_line <= l.end_line && l.start_line <= r.end_line
}

fn touches(f: &Remark<'_>, spots: &[&Location]) -> bool {
    f.anchors
        .iter()
        .any(|a| spots.iter().any(|l| overlaps(a, l)))
}

/// A reported remark: a verified finding, or for baseline A an unverified candidate.
pub(super) struct Remark<'a> {
    category: FindingCategory,
    title: &'a str,
    anchors: &'a [CodeRef],
}

pub(super) fn remarks(result: &ReviewResult) -> Vec<Remark<'_>> {
    result
        .findings
        .iter()
        .map(|f| Remark {
            category: f.category,
            title: &f.title,
            anchors: &f.anchors,
        })
        .collect()
}

pub(super) fn candidate_remarks<'a>(
    candidates: impl IntoIterator<Item = &'a Candidate>,
) -> Vec<Remark<'a>> {
    candidates
        .into_iter()
        .map(|c| Remark {
            category: c.category,
            title: &c.claim,
            anchors: &c.anchors,
        })
        .collect()
}

/// Correctness findings on a labelled nit (overrated) and, of the rest, on a tolerated location.
fn not_false(
    case: &QualityCase,
    nits: &[&ExpectedFinding],
    findings: &[&Remark<'_>],
) -> (usize, usize) {
    let nit_spots: Vec<&Location> = nits.iter().flat_map(|e| &e.evidence_locations).collect();
    // shortcut: tolerance is by overlapping lines, so a wrong claim on a tolerated line is hidden; the owner reads the titles.
    let tolerated_spots: Vec<&Location> = case
        .expected
        .tolerated
        .iter()
        .flat_map(|t| &t.evidence_locations)
        .collect();
    let overrated = findings.iter().filter(|f| touches(f, &nit_spots)).count();
    let tolerated = findings
        .iter()
        .filter(|f| !touches(f, &nit_spots) && touches(f, &tolerated_spots))
        .count();
    (overrated, tolerated)
}

/// Labels some remark's anchor overlaps.
fn hits(labels: &[&ExpectedFinding], found: &[&Remark<'_>]) -> usize {
    labels
        .iter()
        .filter(|e| {
            found.iter().any(|f| {
                f.anchors
                    .iter()
                    .any(|a| e.evidence_locations.iter().any(|l| overlaps(a, l)))
            })
        })
        .count()
}

/// Labelled defects no correctness finding hit and a dropped correctness candidate did.
fn verifier_dropped(
    expected: &[&ExpectedFinding],
    findings: &[&Remark<'_>],
    dropped: &[Remark<'_>],
) -> usize {
    let missed: Vec<_> = expected
        .iter()
        .copied()
        .filter(|e| hits(&[e], findings) == 0)
        .collect();
    let dropped: Vec<_> = dropped
        .iter()
        .filter(|f| {
            matches!(
                f.category,
                FindingCategory::Defect | FindingCategory::RuleViolation
            )
        })
        .collect();
    hits(&missed, &dropped)
}

/// UNKNOWN obligations, unresolved questions and unreviewed paths, as the review stated them.
fn open_questions(result: &ReviewResult) -> Vec<String> {
    let unknown = result.obligations.iter().filter_map(|o| match &o.status {
        ObligationStatus::Unknown {
            reason_code,
            next_evidence_needed,
        } => Some(format!(
            "{} UNKNOWN ({reason_code}): {} — needs {next_evidence_needed}",
            o.rule_id, o.question
        )),
        _ => None,
    });
    let gaps = result
        .excluded_or_unreviewed
        .iter()
        .map(|g| format!("{}: {}", g.path, g.reason));
    unknown
        .chain(result.unresolved_questions.iter().cloned())
        .chain(gaps)
        .collect()
}

/// `dropped`: candidates the verifier rejected or left unresolved; empty without a verifier.
pub(super) fn score(
    case: &QualityCase,
    result: &ReviewResult,
    remarks: &[Remark<'_>],
    dropped: &[Remark<'_>],
) -> Measured {
    let correctness =
        |c: FindingCategory| matches!(c, FindingCategory::Defect | FindingCategory::RuleViolation);
    let labels: Vec<&Location> = case
        .expected
        .findings
        .iter()
        .flat_map(|f| &f.evidence_locations)
        .collect();
    let (expected_nits, expected): (Vec<_>, Vec<_>) = case
        .expected
        .findings
        .iter()
        .partition(|f| f.category == "nitpick");
    let findings: Vec<FindingBrief> = remarks
        .iter()
        .map(|f| FindingBrief {
            category: f.category,
            title: f.title.to_owned(),
            at: f
                .anchors
                .iter()
                .map(|a| format!("{}:{}-{}", a.path, a.start_line, a.end_line))
                .collect(),
            labelled: f
                .anchors
                .iter()
                .any(|a| labels.iter().any(|l| overlaps(a, l))),
            labels: (0..expected.len())
                .filter(|&i| hits(&[expected[i]], &[f]) > 0)
                .collect(),
        })
        .collect();
    let correctness_findings: Vec<_> = remarks.iter().filter(|f| correctness(f.category)).collect();
    let nit_findings: Vec<_> = remarks
        .iter()
        .filter(|f| !correctness(f.category))
        .collect();
    let matched = hits(&expected, &correctness_findings);
    let (overrated_nits, tolerated) = not_false(case, &expected_nits, &correctness_findings);
    let obligations: Vec<ObligationCheck> = case
        .expected
        .obligations
        .iter()
        .map(|o| ObligationCheck {
            rule_id: o.rule_id.clone(),
            expected: o.expected_status.to_uppercase(),
            actual: result
                .obligations
                .iter()
                .find(|r| r.rule_id == o.rule_id)
                .map_or("ABSENT", |r| status_tag(&r.status))
                .into(),
        })
        .collect();
    let left_open = result.coverage != Coverage::CompleteForScope
        || result
            .obligations
            .iter()
            .any(|o| status_tag(&o.status) == "UNKNOWN");
    let count = |n: usize| u32::try_from(n).unwrap_or(u32::MAX);
    let usage = &result.usage_summary;
    Measured {
        run_id: result.run_id.to_string(),
        recommendation: result.recommendation,
        coverage: result.coverage,
        expected_findings: count(expected.len()),
        matched_expected: count(matched),
        correctness_findings: count(correctness_findings.len()),
        labelled_findings: count(
            findings
                .iter()
                .filter(|f| correctness(f.category) && f.labelled)
                .count(),
        ),
        tolerated_findings: count(tolerated),
        overrated_nits: count(overrated_nits),
        nits: count(nit_findings.len()),
        expected_nits: count(expected_nits.len()),
        matched_nits: count(hits(&expected_nits, &nit_findings)),
        obligations,
        unknown_kept: (case.variant == CaseDisposition::Unknown)
            .then_some(correctness_findings.is_empty() && left_open),
        labels: expected
            .iter()
            .map(|e| format!("{} — {}", e.semantic_target, e.required_condition))
            .collect(),
        left_open: if case.variant == CaseDisposition::Unknown {
            open_questions(result)
        } else {
            Vec::new()
        },
        findings,
        model_attempts: usage.model_attempts,
        cost_known_micro_usd: usage.cost.known.0,
        unknown_cost_attempts: usage.cost.unknown_cost_attempts,
        verifier_dropped: count(verifier_dropped(&expected, &correctness_findings, dropped)),
        unreviewed_paths: count(result.excluded_or_unreviewed.len()),
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;

    use super::*;
    use crate::quality::Tolerated;

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
}
