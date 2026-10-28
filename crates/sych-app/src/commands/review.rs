use std::fs;
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;
use sych_storage::Storage;
use tokio::time;

use crate::args::ReviewArguments;
use crate::errors::Failure;
use crate::output::emit_as;
use crate::server::App;
use crate::server::build::{EngineConfig, connections, engine};
use crate::storage::{block_on, store_config};
use crate::{config, render, review};

pub fn run(args: &ReviewArguments) -> anyhow::Result<ExitCode> {
    if let Some(change) = &args.change {
        return change_review(args, change);
    }

    if args.comparison.base.is_empty() {
        return Err(
            Failure::invalid("--base is required unless --change names a provider change").into(),
        );
    }

    let report = review::run(args, None)?;
    // The result is saved before it is written; `sych show` can still print it.
    emit_as(&report, || render::markdown(&report), &args.out)?;
    let r = &report.review_result;
    tracing::info!(run = %r.run_id, recommendation = ?r.recommendation, coverage = ?r.coverage, "review saved");
    Ok(review::exit_code(args.exit_policy, r))
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
    let path = args
        .config
        .as_deref()
        .ok_or_else(|| Failure::invalid("--change needs --config"))?;
    let server = config::server(Some(path))?;
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
                models: args.models.clone(),
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
    let out = serde_json::to_string_pretty(&result)?;
    match &args.out.output {
        Some(path) => fs::write(path, out + "\n")?,
        None => println!("{out}"),
    }
    Ok(review::exit_code(args.exit_policy, &result))
}
