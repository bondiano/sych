//! `sych eval compare` (spec §27.5): saved `eval measure` reports of one suite and one config —
//! baselines A, B and C, or repeats — side by side. Totals come from the reports as written;
//! the cases listed are those whose outcome differs between reports or between repeated runs
//! of one report. No model is called.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::errors::Failure;
use crate::output::emit_json;

#[derive(Deserialize)]
struct Input {
    suite: String,
    models: String,
    variant: String,
    runs_per_case: u32,
    /// Absent in reports written before it was recorded; then equal budgets are not proven.
    config_sha256: Option<String>,
    totals: Value,
    runs: Vec<InputRun>,
}

#[derive(Deserialize)]
struct InputRun {
    id: String,
    measured: Option<Outcome>,
}

/// What one run decided; `null` in the output is a run that failed.
#[derive(Deserialize, Serialize, PartialEq, Eq)]
struct Outcome {
    matched_expected: u32,
    correctness_findings: u32,
    labelled_findings: u32,
    unknown_kept: Option<bool>,
}

#[derive(Serialize)]
struct Side {
    report: String,
    variant: String,
    models: String,
    runs_per_case: u32,
    totals: Value,
}

#[derive(Serialize)]
struct Comparison {
    suite: String,
    /// The shared config hash; `null` when a report predates it.
    config_sha256: Option<String>,
    cases: usize,
    reports: Vec<Side>,
    /// Per case, per report (in `reports` order), the outcome of each run.
    differing: BTreeMap<String, Vec<Vec<Option<Outcome>>>>,
}

fn read(path: &Path) -> Result<Input, Failure> {
    let text = fs::read_to_string(path)
        .map_err(|e| Failure::invalid(format!("{}: {e}", path.display())))?;
    serde_json::from_str(&text).map_err(|e| {
        Failure::invalid(format!(
            "{}: not an eval measure report: {e}",
            path.display()
        ))
    })
}

fn compare(inputs: Vec<(String, Input)>) -> Result<Comparison, Failure> {
    let first = &inputs[0].1;
    let ids = |i: &Input| i.runs.iter().map(|r| r.id.clone()).collect::<BTreeSet<_>>();
    let cases = ids(first);
    let mut hashes = BTreeSet::new();
    for (path, input) in &inputs {
        if input.suite != first.suite || ids(input) != cases {
            return Err(Failure::invalid(format!(
                "{path}: another suite or case set than {}; compare reports of the same cases",
                inputs[0].0
            )));
        }

        hashes.insert(input.config_sha256.clone());
    }
    if hashes.iter().flatten().count() > 1 {
        return Err(Failure::invalid(
            "the reports were measured under different configs, so different budgets",
        ));
    }

    let config_sha256 = match hashes.into_iter().collect::<Vec<_>>().as_slice() {
        [Some(hash)] => Some(hash.clone()),
        _ => None,
    };

    let suite = first.suite.clone();
    let mut differing: BTreeMap<String, Vec<Vec<Option<Outcome>>>> = cases
        .iter()
        .map(|id| (id.clone(), inputs.iter().map(|_| Vec::new()).collect()))
        .collect();
    let mut reports = Vec::new();
    for (side, (path, input)) in inputs.into_iter().enumerate() {
        for run in input.runs {
            differing.get_mut(&run.id).expect("same case set")[side].push(run.measured);
        }
        reports.push(Side {
            report: path,
            variant: input.variant,
            models: input.models,
            runs_per_case: input.runs_per_case,
            totals: input.totals,
        });
    }
    differing.retain(|_, sides| {
        let mut runs = sides.iter().flatten();
        let first = runs.next();
        runs.any(|r| Some(r) != first)
    });
    Ok(Comparison {
        suite,
        config_sha256,
        cases: cases.len(),
        reports,
        differing,
    })
}

pub fn run(reports: &[PathBuf], output: Option<&Path>) -> anyhow::Result<ExitCode> {
    let inputs = reports
        .iter()
        .map(|p| Ok((p.display().to_string(), read(p)?)))
        .collect::<Result<Vec<_>, Failure>>()?;
    emit_json(&compare(inputs)?, output)?;
    Ok(ExitCode::SUCCESS)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn report(variant: &str, config: Option<&str>, runs: &[(&str, u32)]) -> (String, Input) {
        let runs: Vec<Value> = runs
            .iter()
            .map(|(id, matched)| {
                serde_json::json!({"id": id, "measured": {"matched_expected": matched,
                    "correctness_findings": 1, "labelled_findings": 1, "unknown_kept": null}})
            })
            .collect();
        let input = serde_json::from_value(serde_json::json!({"suite": "dev", "models": "m",
            "variant": variant, "runs_per_case": 1, "config_sha256": config, "totals": {},
            "runs": runs}))
        .expect("report");
        (variant.into(), input)
    }

    #[test]
    fn lists_cases_that_differ_between_reports_or_repeats() {
        let c = compare(vec![
            report(
                "A",
                Some("h"),
                &[("same", 1), ("differs", 1), ("varies", 0)],
            ),
            report(
                "C",
                Some("h"),
                &[("same", 1), ("differs", 0), ("varies", 0), ("varies", 1)],
            ),
        ])
        .expect("comparable");
        assert_eq!(c.cases, 3);
        assert_eq!(c.config_sha256.as_deref(), Some("h"));
        assert_eq!(
            c.differing.keys().collect::<Vec<_>>(),
            ["differs", "varies"]
        );
        assert_eq!(c.differing["varies"][1].len(), 2);
    }

    #[test]
    fn refuses_other_cases_or_configs_and_keeps_an_unknown_config_unknown() {
        assert!(
            compare(vec![
                report("A", None, &[("x", 1)]),
                report("C", None, &[("y", 1)])
            ])
            .is_err()
        );
        assert!(
            compare(vec![
                report("A", Some("h"), &[("x", 1)]),
                report("C", Some("g"), &[("x", 1)])
            ])
            .is_err()
        );
        let c = compare(vec![
            report("A", Some("h"), &[("x", 1)]),
            report("C", None, &[("x", 1)]),
        ])
        .expect("comparable");
        assert_eq!(c.config_sha256, None);
    }
}
