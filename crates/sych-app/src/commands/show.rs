use std::process::ExitCode;

use crate::args::ShowArguments;
use crate::output::emit_as;
use crate::{render, storage};

pub fn run(args: &ShowArguments) -> anyhow::Result<ExitCode> {
    let report = storage::saved_report(args.config.as_deref(), &args.run)?;
    emit_as(&report, || render::markdown(&report), &args.out)?;
    Ok(ExitCode::SUCCESS)
}
