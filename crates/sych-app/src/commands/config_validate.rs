use std::process::ExitCode;

use sych_core::redact;

use crate::args::ConfigValidateArguments;
use crate::config;
use crate::errors::Failure;

/// Prints the normalized effective config with hashes and provenance, secrets redacted.
pub fn run(args: &ConfigValidateArguments) -> anyhow::Result<ExitCode> {
    let effective = config::effective(args.config.as_deref(), args.repo.as_deref())?;
    let json = serde_json::to_string_pretty(&effective)
        .map_err(|err| Failure::no_result(format!("cannot render effective config: {err}")))?;
    println!("{}", redact(&json));
    Ok(ExitCode::SUCCESS)
}
