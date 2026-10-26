use std::fs::OpenOptions;
use std::io::{ErrorKind, Write};
use std::process::ExitCode;

use crate::args::ConfigInitArguments;
use crate::config::PERSONAL_EXAMPLE;
use crate::errors::Failure;

/// Writes the personal example; an existing file is never overwritten.
pub fn run(args: &ConfigInitArguments) -> anyhow::Result<ExitCode> {
    debug_assert!(args.personal, "clap requires --personal");
    let output = &args.output;
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(output)
        .and_then(|mut f| f.write_all(PERSONAL_EXAMPLE.as_bytes()))
        .map_err(|err| match err.kind() {
            ErrorKind::AlreadyExists => Failure::invalid(format!(
                "{} exists and was not overwritten",
                output.display()
            )),
            _ => Failure::no_result(format!("cannot write {}: {err}", output.display())),
        })?;
    tracing::info!(path = %output.display(), "personal config written; set the environment variables it names, then run `sych doctor`");
    Ok(ExitCode::SUCCESS)
}
