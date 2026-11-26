//! Command-line arguments for `sych eval` subcommands.

use std::path::PathBuf;

use clap::{Subcommand, ValueEnum};

use super::{ModelSelection, Switch};

#[derive(Subcommand)]
pub enum EvalCommands {
    /// Run a fixture suite and compare significant states; fake models make no external calls
    Run(EvalRunArguments),
    /// Check a labelled quality suite: labels are complete and every evidence location and
    /// expected syntax shape resolves; never calls a model
    Validate(EvalValidateArguments),
    /// Review a labelled quality suite with a real model profile and score it against the
    /// labels; makes paid or quota-counted calls within the configured budgets
    Measure(EvalMeasureArguments),
    /// Put saved `eval measure` reports of the same cases and config side by side (baselines
    /// A/B/C, repeats) with the cases whose outcome differs; never calls a model
    Compare(EvalCompareArguments),
    /// Rescore a saved `eval measure` report by the owner's judgments of which label each
    /// correctness finding states; shows location and meaning scores; never calls a model
    Judge(EvalJudgeArguments),
    /// Write the judgments `eval judge` reads by asking a model, once per measured run, which
    /// label each located correctness finding states; quota-counted follow-ups of each run
    AutoJudge(EvalAutoJudgeArguments),
}

#[derive(clap::Args)]
pub struct EvalRunArguments {
    #[arg(long)]
    pub suite: PathBuf,
    /// Only `fake`: the harness suite is deterministic and offline
    #[arg(long, value_parser = ModelSelection::parse)]
    pub models: ModelSelection,
    /// Report path; stdout when omitted
    #[arg(long)]
    pub output: Option<PathBuf>,
}

#[derive(clap::Args)]
pub struct EvalMeasureArguments {
    /// Directory of `<id>.yaml` cases of one split
    #[arg(long)]
    pub suite: PathBuf,
    /// Repository for `repository_fixture` cases, as NAME=PATH; repeatable
    #[arg(long, value_name = "NAME=PATH")]
    pub repo: Vec<String>,
    /// Server config with the model profile, budgets and store
    #[arg(long)]
    pub config: PathBuf,
    /// Model profile of `--config`
    #[arg(long, value_parser = ModelSelection::parse)]
    pub models: ModelSelection,
    /// Only these case ids (repeatable); all cases when omitted
    #[arg(long = "case", value_name = "ID")]
    pub cases: Vec<String>,
    /// Reviews per case, to see the variation of a nondeterministic model
    #[arg(long, default_value_t = 1, value_parser = clap::value_parser!(u32).range(1..=10))]
    pub runs: u32,
    /// An earlier report of this suite and config: each of its runs is verified again on the
    /// candidates that run discovered, with no new search, instead of `--runs` new reviews
    #[arg(long, value_name = "REPORT", conflicts_with = "runs")]
    pub replay: Option<PathBuf>,
    /// `c` is the default pipeline; `b` skips the high-risk search; `a` is one reviewer over
    /// the diff: no semantic facts, no high-risk search, no verifier
    #[arg(long, value_enum, default_value = "c")]
    pub variant: PipelineVariant,
    #[arg(long, value_enum, default_value = "on")]
    pub semantic: Switch,
    #[arg(long)]
    pub output: Option<PathBuf>,
}

#[derive(Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum PipelineVariant {
    A,
    B,
    C,
}

#[derive(clap::Args)]
pub struct EvalCompareArguments {
    /// `eval measure` reports, in the order they are shown
    #[arg(required = true, num_args = 2..)]
    pub reports: Vec<PathBuf>,
    #[arg(long)]
    pub output: Option<PathBuf>,
}

#[derive(clap::Args)]
pub struct EvalJudgeArguments {
    /// `eval measure` report
    pub report: PathBuf,
    /// YAML list of `{case, run, finding, label}` and `{case, run, unknown_kept}` judgments
    #[arg(long)]
    pub judgments: PathBuf,
    #[arg(long)]
    pub output: Option<PathBuf>,
}

#[derive(clap::Args)]
pub struct EvalAutoJudgeArguments {
    /// `eval measure` report
    pub report: PathBuf,
    /// The server config the report was measured with: its store holds the runs
    #[arg(long)]
    pub config: PathBuf,
    /// `roles` or one model profile of `--config`
    #[arg(long, value_parser = ModelSelection::parse)]
    pub models: ModelSelection,
    /// Judgments file for `eval judge --judgments`; stdout when omitted
    #[arg(long)]
    pub output: Option<PathBuf>,
}

#[derive(clap::Args)]
pub struct EvalValidateArguments {
    /// Directory of `<id>.yaml` cases of one split
    #[arg(long)]
    pub suite: PathBuf,
    /// Repository for `repository_fixture` cases, as NAME=PATH; repeatable
    #[arg(long, value_name = "NAME=PATH")]
    pub repo: Vec<String>,
    #[arg(long)]
    pub output: Option<PathBuf>,
}
