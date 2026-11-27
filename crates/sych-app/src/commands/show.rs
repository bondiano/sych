use std::process::ExitCode;

use crate::args::ShowArguments;
use crate::output::{EmitMode, emit_report, validate_report_output};
use crate::storage;

pub fn run(args: &ShowArguments) -> anyhow::Result<ExitCode> {
    validate_report_output(&args.out, EmitMode::Show)?;
    let report = storage::saved_report(args.config.as_deref(), &args.run)?;
    emit_report(
        &report,
        &args.out,
        &args.repo,
        args.config.as_deref(),
        EmitMode::Show,
    )?;
    Ok(ExitCode::SUCCESS)
}
