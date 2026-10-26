use std::process::ExitCode;

use crate::args::EvalValidateArguments;
use crate::quality;

pub fn run(args: &EvalValidateArguments) -> anyhow::Result<ExitCode> {
    quality::validate(&args.suite, &args.repo, args.output.as_deref())
}
