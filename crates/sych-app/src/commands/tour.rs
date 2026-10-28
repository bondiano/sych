use std::process::ExitCode;

use crate::args::TourArguments;
use crate::{tour, tour_reader};

pub fn run(args: &TourArguments) -> anyhow::Result<ExitCode> {
    match &args.action {
        Some(action) => tour_reader::run(action),
        None => tour::run(args),
    }
}
