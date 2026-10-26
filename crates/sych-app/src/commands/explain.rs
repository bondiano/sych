use std::process::ExitCode;

use crate::args::ExplainArguments;
use crate::errors::Failure;
use crate::output::emit_as;
use crate::render::Explanation;
use crate::storage;

pub fn run(args: &ExplainArguments) -> anyhow::Result<ExitCode> {
    let ExplainArguments {
        run,
        obligation,
        config,
        out,
    } = args;
    let report = storage::saved_report(config.as_deref(), run)?;
    let found = report
        .review_result
        .obligations
        .iter()
        .find(|o| o.id.to_string() == *obligation || o.rule_id == *obligation)
        .ok_or_else(|| Failure::invalid(format!("run {run} has no obligation `{obligation}`")))?;
    let explanation = Explanation::new(&report, found);
    emit_as(&explanation, || explanation.markdown(), out)?;
    Ok(ExitCode::SUCCESS)
}
