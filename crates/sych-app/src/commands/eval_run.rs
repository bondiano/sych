use std::process::ExitCode;

use crate::args::EvalRunArguments;
use crate::eval;

pub fn run(args: &EvalRunArguments) -> anyhow::Result<ExitCode> {
    eval::run(&args.suite, &args.models, args.output.as_deref())
}
