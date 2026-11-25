use std::io::{self, IsTerminal};
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;
use sych_storage::Storage;
use tokio::time;

use crate::args::{Comparison, ModelSelection, ReportOutput, ReviewArguments};
use crate::errors::Failure;
use crate::output::{emit, emit_json, emit_report};
use crate::render::ReportFormat;
use crate::server::App;
use crate::server::build::{EngineConfig, connections, engine};
use crate::storage::{block_on, store_config};
use crate::{config, render, render_sarif, review};

mod target;

pub fn run(args: &ReviewArguments) -> anyhow::Result<ExitCode> {
    if let Some(change) = &args.change {
        return change_review(args, change);
    }

    let comparison = target::resolve(&args.comparison, args.target.as_deref())?;
    target::warn_dirty(&comparison.repo);
    let report = review::run_local_cli(request(args, comparison))?;
    let r = &report.review_result;
    // The result is saved before it is written; `sych show` can still print it.
    if terminal_summary(&args.out) {
        emit(&render::summary(r), None)?;
    } else {
        emit_report(&report, &args.out)?;
    }

    tracing::info!(run = %r.run_id, recommendation = ?r.recommendation, coverage = ?r.coverage, "review saved");
    Ok(review::exit_code(args.exit_policy, r))
}

fn terminal_summary(out: &ReportOutput) -> bool {
    out.output.is_none() && out.format.is_none() && io::stdout().is_terminal()
}

/// What a local `sych review` asks of the pipeline: its flags, the rest at their defaults.
fn request(args: &ReviewArguments, comparison: Comparison) -> review::Request {
    review::Request {
        config: args.config.clone(),
        fake_responses: args.fake_responses.clone(),
        policy_ref: args.policy_ref.clone(),
        semantic: args.semantic,
        ci_facts: args.ci_facts.clone(),
        ci_manifest: args.ci_manifest.clone(),
        checks: args.checks.clone(),
        intent_file: args.intent_file.clone(),
        ..review::Request::new(comparison, args.models.clone())
    }
}

/// `github:CONNECTION:OWNER/REPO:NUMBER` → its parts.
fn parse(change: &str) -> Option<(&str, &str, u64)> {
    let rest = change.strip_prefix("github:")?;
    let (connection, rest) = rest.split_once(':')?;
    let (repo, number) = rest.rsplit_once(':')?;
    Some((connection, repo, number.parse().ok()?))
}

/// Reviews a provider change at its current state through the server pipeline, in process;
/// nothing is published.
fn change_review(args: &ReviewArguments, change: &str) -> anyhow::Result<ExitCode> {
    let (connection, repo, number) = parse(change)
        .ok_or_else(|| Failure::invalid("--change is github:CONNECTION:OWNER/REPO:NUMBER"))?;
    // Only the result comes back from the server path; Markdown renders a whole saved run.
    if matches!(args.out.format, Some(ReportFormat::Markdown)) {
        return Err(Failure::invalid(
            "--change writes json or sarif; `sych show --run RUN --format markdown` renders the run",
        )
        .into());
    }

    let path = args
        .config
        .as_deref()
        .ok_or_else(|| Failure::invalid("--change needs --config"))?;
    let server = config::server(Some(path))?;
    let models = ModelSelection::for_review(args.models.as_ref(), server.default_models.as_deref());
    let store = store_config(Some(path), &server)?;
    let conns = connections(&server)?;
    let result = block_on(async {
        let storage = Storage::open(&store).await?;
        let app = Arc::new(App::local(storage, store.artifacts.clone()));
        let engine = engine(
            Arc::clone(&app),
            conns,
            EngineConfig {
                config: path,
                server: &server,
                store: store.clone(),
                models,
                semantic: args.semantic,
            },
        );
        let run = engine
            .request_review(
                connection,
                repo,
                number,
                "cli",
                false,
                false,
                args.checks.clone(),
            )
            .await
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        let tenant = run.partition.tenant_id.clone();
        // A run waiting for CI is released until its deadline; poll until it is terminal.
        loop {
            let (state, ..) = app.storage.run_row(&tenant, &run.id.to_string()).await?;
            if !matches!(state.as_str(), "QUEUED" | "WAITING_EVIDENCE" | "RUNNING") {
                break;
            }

            if !engine.work_once().await? {
                time::sleep(Duration::from_secs(1)).await;
            }
        }
        app.storage
            .load_result(&tenant, run.id)
            .await
            .context("the run ended without a result")
    })??;
    let path = args.out.output.as_deref();
    if terminal_summary(&args.out) {
        emit(&render::summary(&result), None)?;
    } else if matches!(args.out.format, Some(ReportFormat::Sarif)) {
        emit_json(&render_sarif::sarif(&result), path)?;
    } else {
        emit_json(&result, path)?;
    }

    Ok(review::exit_code(args.exit_policy, &result))
}
