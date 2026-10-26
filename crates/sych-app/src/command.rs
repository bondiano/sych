//! The command contract: every subcommand runs once and yields its exit code; an error is
//! reported in one place with the code its chain decides.

use std::process::ExitCode;

use crate::errors::exit_code;

/// Runs one subcommand and prints its error chain, line by line, as `sych <name>: …`.
///
/// `name` is the subcommand used in error messages, e.g. `review` or `eval validate`. `Ok`
/// carries non-error outcomes too, such as strict-mode violations (10, 11).
pub fn run_command(name: &str, operation: impl FnOnce() -> anyhow::Result<ExitCode>) -> ExitCode {
    let span = tracing::info_span!("command", name);
    let _entered = span.enter();
    match operation() {
        Ok(code) => code,
        Err(err) => {
            tracing::debug!(error = ?err, "command failed");
            for line in format!("{err:#}").lines() {
                eprintln!("sych {name}: {line}");
            }

            ExitCode::from(exit_code(&err))
        }
    }
}
