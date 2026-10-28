use std::process::ExitCode;

use crate::args::AskArguments;
use crate::followup;

pub fn run(args: &AskArguments) -> anyhow::Result<ExitCode> {
    followup::run(args)
}
