use std::process::ExitCode;

use crate::args::HandoffArguments;
use crate::handoff;

pub fn run(args: &HandoffArguments) -> anyhow::Result<ExitCode> {
    handoff::run(args)
}
