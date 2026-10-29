//! `sych eval judge`: rescores a saved `eval measure` report by meaning. `measure` matches a
//! finding to a label when their locations overlap; here the owner's judgments decide which
//! label a correctness finding states, and whether an `unknown` run left open the labelled
//! question. Both scores are shown. A location match nobody judged is listed and does not
//! count by meaning. Nitpicks stay scored by location. No model is called.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;
use std::process::ExitCode;

use serde::{Deserialize, Serialize};
use sych_core::FindingCategory;

use crate::errors::Failure;
use crate::output::emit_json;

#[derive(Deserialize)]
struct Input {
    suite: String,
    runs: Vec<InputRun>,
}

#[derive(Deserialize)]
struct InputRun {
    id: String,
    run: u32,
    measured: Option<InputMeasured>,
}

#[derive(Deserialize)]
struct InputMeasured {
    expected_findings: u32,
    matched_expected: u32,
    correctness_findings: u32,
    labelled_findings: u32,
    unknown_kept: Option<bool>,
    /// Absent in reports written before `eval judge`; such a report cannot be judged.
    #[serde(default)]
    labels: Vec<String>,
    findings: Vec<InputFinding>,
}

#[derive(Deserialize)]
struct InputFinding {
    category: FindingCategory,
    title: String,
    #[serde(default)]
    labels: Vec<usize>,
}

/// One owner judgment: a correctness finding states label `label` (`null`: none), or an
/// `unknown` run did or did not leave open the labelled question.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Judgment {
    case: String,
    run: u32,
    #[serde(default)]
    finding: Option<usize>,
    #[serde(default)]
    label: Option<usize>,
    #[serde(default)]
    unknown_kept: Option<bool>,
}

#[derive(Default)]
struct RunJudgments {
    findings: BTreeMap<usize, Option<usize>>,
    unknown_kept: Option<bool>,
}

#[derive(Serialize, Default, PartialEq, Debug)]
struct Score {
    matched_expected: u32,
    labelled_findings: u32,
    unknown_kept: u32,
    recall: Option<f64>,
    precision: Option<f64>,
    unknown_kept_rate: Option<f64>,
}

#[derive(Serialize)]
struct Judged {
    suite: String,
    runs: u32,
    errors: u32,
    expected_findings: u32,
    correctness_findings: u32,
    unknown_runs: u32,
    by_location: Score,
    by_meaning: Score,
    /// Location matches and kept unknowns without a judgment, as `case#run: …`.
    unjudged: Vec<String>,
}

fn ratio(part: u32, whole: u32) -> Option<f64> {
    (whole > 0).then(|| f64::from(part) / f64::from(whole))
}

fn correctness(c: FindingCategory) -> bool {
    matches!(c, FindingCategory::Defect | FindingCategory::RuleViolation)
}

fn index(
    input: &Input,
    judgments: Vec<Judgment>,
) -> Result<BTreeMap<(String, u32), RunJudgments>, String> {
    let runs: BTreeMap<(&str, u32), &InputMeasured> = input
        .runs
        .iter()
        .filter_map(|r| r.measured.as_ref().map(|m| ((r.id.as_str(), r.run), m)))
        .collect();
    let mut out: BTreeMap<(String, u32), RunJudgments> = BTreeMap::new();
    for j in judgments {
        let at = format!("{}#{}", j.case, j.run);
        let m = *runs
            .get(&(j.case.as_str(), j.run))
            .ok_or_else(|| format!("{at}: no measured run with this case and run"))?;
        let entry = out.entry((j.case, j.run)).or_default();
        match (j.finding, j.unknown_kept) {
            (Some(i), None) => {
                let f = m
                    .findings
                    .get(i)
                    .ok_or_else(|| format!("{at}: no finding {i}"))?;
                if !correctness(f.category) {
                    return Err(format!("{at}: finding {i} is not a correctness finding"));
                }

                if j.label.is_some_and(|l| l >= m.labels.len()) {
                    return Err(format!("{at}: finding {i}: no label {:?}", j.label));
                }

                if entry.findings.insert(i, j.label).is_some() {
                    return Err(format!("{at}: finding {i} is judged twice"));
                }
            }
            (None, Some(kept)) if j.label.is_none() => {
                if m.unknown_kept.is_none() {
                    return Err(format!("{at}: not an unknown case"));
                }

                if entry.unknown_kept.replace(kept).is_some() {
                    return Err(format!("{at}: unknown_kept is judged twice"));
                }
            }
            _ => {
                return Err(format!(
                    "{at}: a judgment names either `finding` (with `label`) or `unknown_kept`"
                ));
            }
        }
    }
    Ok(out)
}

fn judge(input: &Input, judgments: Vec<Judgment>) -> Result<Judged, String> {
    let judged = index(input, judgments)?;
    let none = RunJudgments::default();
    let mut out = Judged {
        suite: input.suite.clone(),
        runs: 0,
        errors: 0,
        expected_findings: 0,
        correctness_findings: 0,
        unknown_runs: 0,
        by_location: Score::default(),
        by_meaning: Score::default(),
        unjudged: Vec::new(),
    };
    for r in &input.runs {
        out.runs += 1;
        let Some(m) = &r.measured else {
            out.errors += 1;
            continue;
        };
        let j = judged.get(&(r.id.clone(), r.run)).unwrap_or(&none);
        let at = format!("{}#{}", r.id, r.run);
        out.expected_findings += m.expected_findings;
        out.correctness_findings += m.correctness_findings;
        out.by_location.matched_expected += m.matched_expected;
        out.by_location.labelled_findings += m.labelled_findings;
        let mut stated = BTreeSet::new();
        for (i, f) in m.findings.iter().enumerate() {
            if !correctness(f.category) {
                continue;
            }

            match j.findings.get(&i) {
                Some(Some(label)) => {
                    stated.insert(*label);
                    out.by_meaning.labelled_findings += 1;
                }
                Some(None) => {}
                None if f.labels.is_empty() => {}
                None => out.unjudged.push(format!("{at}: finding {i}: {}", f.title)),
            }
        }
        out.by_meaning.matched_expected += u32::try_from(stated.len()).unwrap_or(u32::MAX);
        if let Some(kept) = m.unknown_kept {
            out.unknown_runs += 1;
            out.by_location.unknown_kept += u32::from(kept);
            // A kept unknown by location is necessary; the judgment says it is the labelled one.
            match (kept, j.unknown_kept) {
                (true, Some(true)) => out.by_meaning.unknown_kept += 1,
                (true, None) => out.unjudged.push(format!("{at}: unknown kept")),
                _ => {}
            }
        }
    }
    for s in [&mut out.by_location, &mut out.by_meaning] {
        s.recall = ratio(s.matched_expected, out.expected_findings);
        s.precision = ratio(s.labelled_findings, out.correctness_findings);
        s.unknown_kept_rate = ratio(s.unknown_kept, out.unknown_runs);
    }
    Ok(out)
}

pub fn run(report: &Path, judgments: &Path, output: Option<&Path>) -> anyhow::Result<ExitCode> {
    let read = |p: &Path| {
        fs::read_to_string(p).map_err(|e| Failure::invalid(format!("{}: {e}", p.display())))
    };
    let input: Input = serde_json::from_str(&read(report)?).map_err(|e| {
        Failure::invalid(format!(
            "{}: not an eval measure report: {e}",
            report.display()
        ))
    })?;
    let judgments: Vec<Judgment> = serde_yaml_ng::from_str(&read(judgments)?)
        .map_err(|e| Failure::invalid(format!("{}: {e}", judgments.display())))?;
    let judged = judge(&input, judgments).map_err(Failure::invalid)?;
    emit_json(&judged, output)?;
    Ok(ExitCode::SUCCESS)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One unsafe run with a defect on label 0's location, one unknown run that kept it open.
    fn input() -> Input {
        serde_json::from_value(serde_json::json!({"suite": "holdout", "runs": [
            {"id": "u", "run": 1, "measured": {"expected_findings": 1, "matched_expected": 1,
                "correctness_findings": 1, "labelled_findings": 1, "unknown_kept": null,
                "labels": ["lost write — when the batch hangs"],
                "findings": [{"category": "DEFECT", "title": "unrelated claim", "labels": [0]}]}},
            {"id": "k", "run": 1, "measured": {"expected_findings": 0, "matched_expected": 0,
                "correctness_findings": 0, "labelled_findings": 0, "unknown_kept": true,
                "labels": [], "findings": []}},
            {"id": "e", "run": 1, "measured": null}
        ]}))
        .expect("report")
    }

    fn judgments(yaml: &str) -> Vec<Judgment> {
        serde_yaml_ng::from_str(yaml).expect("judgments")
    }

    #[test]
    fn a_location_match_that_states_another_defect_does_not_count_by_meaning() {
        let j = judge(
            &input(),
            judgments("- { case: u, run: 1, finding: 0, label: null }"),
        )
        .expect("judged");
        assert_eq!(
            (
                j.by_location.matched_expected,
                j.by_meaning.matched_expected
            ),
            (1, 0)
        );
        assert_eq!(j.by_meaning.precision, Some(0.0));
        assert_eq!((j.runs, j.errors), (3, 1));
    }

    #[test]
    fn an_unknown_kept_by_location_counts_only_when_judged_the_labelled_one() {
        let other = judge(
            &input(),
            judgments("- { case: k, run: 1, unknown_kept: false }"),
        )
        .expect("judged");
        assert_eq!(
            (
                other.by_location.unknown_kept,
                other.by_meaning.unknown_kept
            ),
            (1, 0)
        );
        let same = judge(
            &input(),
            judgments("- { case: k, run: 1, unknown_kept: true }"),
        )
        .expect("judged");
        assert_eq!(same.by_meaning.unknown_kept_rate, Some(1.0));
    }

    #[test]
    fn unjudged_matches_are_listed_and_do_not_count() {
        let j = judge(&input(), Vec::new()).expect("judged");
        assert_eq!(
            j.unjudged,
            ["u#1: finding 0: unrelated claim", "k#1: unknown kept"]
        );
        assert_eq!(
            (j.by_meaning.matched_expected, j.by_meaning.unknown_kept),
            (0, 0)
        );
    }

    #[test]
    fn refuses_judgments_that_name_nothing_measured_or_twice() {
        for bad in [
            "- { case: x, run: 1, finding: 0 }",
            "- { case: u, run: 1, finding: 1 }",
            "- { case: u, run: 1, finding: 0, label: 1 }",
            "- { case: u, run: 1, unknown_kept: true }",
            "- { case: u, run: 1 }",
            "[{ case: u, run: 1, finding: 0 }, { case: u, run: 1, finding: 0, label: 0 }]",
        ] {
            assert!(judge(&input(), judgments(bad)).is_err(), "{bad}");
        }
    }
}
