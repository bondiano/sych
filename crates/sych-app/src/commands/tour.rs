use std::process::ExitCode;

use crate::args::TourArguments;
use crate::tour;

pub fn run(args: &TourArguments) -> anyhow::Result<ExitCode> {
    tour::run(args)
}
