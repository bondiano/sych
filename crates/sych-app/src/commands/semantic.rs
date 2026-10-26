use std::process::ExitCode;

use sych_core::config::Tooling;

use crate::args::SemanticArguments;
use crate::output::emit_json;
use crate::{semantic, snapshot};

/// Dependencies are never installed here: that needs a server config with a sandbox.
pub fn run(args: &SemanticArguments) -> anyhow::Result<ExitCode> {
    let snap = snapshot::capture(&args.comparison)?;
    let facts = semantic::collect_facts(&snap, &Tooling::default())?;
    emit_json(&facts, args.output.as_deref())?;
    Ok(ExitCode::SUCCESS)
}
