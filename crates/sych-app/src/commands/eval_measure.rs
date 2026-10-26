use std::process::ExitCode;

use crate::args::{EvalMeasureArguments, PipelineVariant, Switch};
use crate::quality::{self, MeasureArguments};

pub fn run(args: &EvalMeasureArguments) -> anyhow::Result<ExitCode> {
    let a = args;
    quality::measure(&MeasureArguments {
        suite: &a.suite,
        repos: &a.repo,
        config: &a.config,
        models: &a.models,
        cases: &a.cases,
        runs: a.runs,
        variant: match a.variant {
            PipelineVariant::A => "A",
            PipelineVariant::B => "B",
            PipelineVariant::C => "C",
        },
        risk_audit: a.variant == PipelineVariant::C,
        verify: a.variant != PipelineVariant::A,
        // Baseline A sees the diff with minimal context only.
        semantic: if a.variant == PipelineVariant::A {
            Switch::Off
        } else {
            a.semantic
        },
        output: a.output.as_deref(),
    })
}
