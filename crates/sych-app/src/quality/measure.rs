//! `sych eval measure`: every case of a labelled suite is reviewed with a
//! real model profile through the ordinary review path and its durable ledger, so the run,
//! day and month budgets hold across the whole suite. A finding matches a label when one of
//! its anchors overlaps a labelled evidence location — a proxy for meaning; `eval judge`
//! rescores the report by the owner's judgments. The numbers are measurements; acceptance
//! criteria are the owner's.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use sych_core::config::RunMode;

use serde::Serialize;
use sych_core::analysis::Discovered;
use sych_core::{Candidate, ReviewResult, sha256};
use tempfile::TempDir;

use super::replay::frozen;
use super::score::{RunReport, Totals, candidate_remarks, remarks, score};
use super::stages::Spread;
use super::{QualityCase, materialize, not_given, read_cases, repo_paths};
use crate::args::{ExitPolicy, ModelSelection, Output, ReviewArguments, Switch};
use crate::errors::Failure;
use crate::output::emit_json;
use crate::render::Format;
use crate::review;

pub struct MeasureArguments<'a> {
    pub suite: &'a Path,
    pub repos: &'a [String],
    pub config: &'a Path,
    pub models: &'a ModelSelection,
    /// Case ids to run; all when empty.
    pub cases: &'a [String],
    pub runs: u32,
    /// `A`, `B` or `C`.
    pub variant: &'static str,
    /// Off for baselines A and B; on is variant C, the default pipeline.
    pub risk_audit: bool,
    /// Off only for baseline A: its unverified candidates are scored as findings.
    pub verify: bool,
    pub semantic: Switch,
    /// An earlier report whose runs are verified again on their stored discoveries.
    pub replay: Option<&'a Path>,
    pub output: Option<&'a Path>,
}

#[derive(Serialize)]
struct Report {
    suite: String,
    models: String,
    /// `B` without the high-risk search, `C` with it.
    variant: &'static str,
    semantic: bool,
    runs_per_case: u32,
    /// The report whose discoveries were verified again: only candidate and finding numbers
    /// mean anything then, since nothing was assessed.
    #[serde(skip_serializing_if = "Option::is_none")]
    replay_of: Option<String>,
    /// SHA-256 of the `--config` file: reports compare only under the same budgets.
    config_sha256: String,
    skipped: Vec<String>,
    totals: Totals,
    /// The rates of each run index over the suite: how much a rerun moves them.
    spread: Spread,
    by_language: BTreeMap<String, Totals>,
    runs: Vec<RunReport>,
}

fn review_case(
    case: &QualityCase,
    repos: &BTreeMap<String, PathBuf>,
    args: &MeasureArguments<'_>,
    replay: Option<Vec<Discovered>>,
) -> Result<Option<(ReviewResult, Vec<Candidate>)>, String> {
    let dir = TempDir::new().map_err(|e| e.to_string())?;
    let Some(comparison) = materialize(case, repos, dir.path())? else {
        return Ok(None);
    };
    let ci = super::ci::write(case.ci.as_ref(), &comparison)?;
    let review = ReviewArguments {
        comparison,
        change: None,
        config: Some(args.config.to_path_buf()),
        models: args.models.clone(),
        policy_ref: None,
        fake_responses: None,
        semantic: args.semantic,
        ci_facts: ci.reports.clone(),
        ci_manifest: ci.manifest.clone(),
        exit_policy: ExitPolicy::Operational,
        out: Output {
            format: Format::Json,
            output: None,
        },
        verify: args.verify,
        ci_imported: Vec::new(),
        ci_gaps: Vec::new(),
        checks: case
            .execution
            .as_ref()
            .map_or_else(Vec::new, |x| x.checks.clone()),
        execution: case.execution.as_ref().map(super::CaseExecution::config),
        waivers: Vec::new(),
        run_mode: RunMode::PersonalLocal,
        deep: false,
        parent: None,
        replay,
        risk_audit: args.risk_audit,
    };
    // The configured store: its ledger keeps the money caps across the whole suite.
    review::run_with_candidates(&review, None)
        .map(|(report, candidates)| Some((report.review_result, candidates)))
        .map_err(|e| format!("{e:#}"))
}

/// A review to run: each new one, or each run of the earlier report with what it discovered.
type Job<'c> = (&'c QualityCase, u32, Option<Vec<Discovered>>);

fn jobs<'c>(args: &MeasureArguments<'_>, cases: &'c [QualityCase]) -> anyhow::Result<Vec<Job<'c>>> {
    Ok(match args.replay {
        Some(report) => frozen(report, args.config)?
            .into_iter()
            .filter_map(|f| {
                let case = cases.iter().find(|c| c.id == f.id)?;
                Some((case, f.run, Some(f.discovered)))
            })
            .collect(),
        None => cases
            .iter()
            .flat_map(|c| (1..=args.runs).map(move |run| (c, run, None)))
            .collect(),
    })
}

pub fn measure(args: &MeasureArguments<'_>) -> anyhow::Result<ExitCode> {
    if args.models == &ModelSelection::Fake {
        return Err(Failure::invalid(
            "eval measure needs a real model profile; the fake adapter proves only the harness",
        )
        .into());
    }

    if args.replay.is_some() && !args.verify {
        return Err(Failure::invalid("--replay verifies again; baseline A has no verifier").into());
    }

    let repos = repo_paths(args.repos)?;
    let mut cases = Vec::new();
    for (path, case) in read_cases(args.suite).map_err(Failure::invalid)? {
        let case = case.map_err(|e| Failure::invalid(format!("{}: {e}", path.display())))?;
        if args.cases.is_empty() || args.cases.contains(&case.id) {
            cases.push(case);
        }
    }
    let mut report = Report {
        suite: args.suite.display().to_string(),
        models: match args.models {
            ModelSelection::Profile(name) => name.clone(),
            ModelSelection::Roles => "roles".into(),
            ModelSelection::Fake => "fake".into(),
        },
        variant: args.variant,
        semantic: args.semantic == Switch::On,
        runs_per_case: args.runs,
        replay_of: args.replay.map(|p| p.display().to_string()),
        config_sha256: sha256(
            &fs::read(args.config)
                .map_err(|e| Failure::invalid(format!("{}: {e}", args.config.display())))?,
        )
        .to_string(),
        skipped: Vec::new(),
        totals: Totals::default(),
        spread: Spread::default(),
        by_language: BTreeMap::new(),
        runs: Vec::new(),
    };
    let jobs = jobs(args, &cases)?;
    let runs = jobs.iter().map(|j| j.1).max().unwrap_or(args.runs);
    report.runs_per_case = runs;
    let mut per_run: Vec<Totals> = (0..runs).map(|_| Totals::default()).collect();
    let mut skipped: Vec<&str> = Vec::new();
    for (case, run, replay) in jobs {
        if skipped.contains(&case.id.as_str()) {
            continue;
        }

        let (error, measured) = match review_case(case, &repos, args, replay) {
            Ok(Some((result, candidates))) => {
                let (remarks, candidates) = if args.verify {
                    (remarks(&result), candidate_remarks(&candidates))
                } else {
                    (candidate_remarks(&candidates), Vec::new())
                };
                (None, Some(score(case, &result, &remarks, &candidates)))
            }
            Ok(None) => {
                report
                    .skipped
                    .push(format!("{}: {}", case.id, not_given(case)));
                skipped.push(&case.id);
                continue;
            }
            Err(err) => (Some(err), None),
        };
        let run = RunReport {
            id: case.id.clone(),
            language: case.language.clone(),
            variant: case.variant,
            run,
            error,
            measured,
        };
        tracing::info!(case = %run.id, run = run.run, error = ?run.error, "case reviewed");
        report.totals.add(&run);
        per_run[(run.run - 1) as usize].add(&run);
        report
            .by_language
            .entry(case.language.clone())
            .or_default()
            .add(&run);
        report.runs.push(run);
    }
    report.totals.finish();
    per_run.iter_mut().for_each(Totals::finish);
    report.spread = Spread::of(&per_run);
    report.by_language.values_mut().for_each(Totals::finish);
    emit_json(&report, args.output)?;
    Ok(ExitCode::SUCCESS)
}
