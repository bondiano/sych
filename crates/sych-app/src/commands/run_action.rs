//! `sych publish`, `sych cancel` and `sych rerun`: server actions taken locally as a configured
//! API client with the role they need.

use std::process::ExitCode;
use std::sync::Arc;

use sych_core::Timestamp;
use sych_core::config::ApiRole;
use sych_storage::Storage;

use crate::args::{ModelSelection, RerunArguments, RunActionArguments, Switch};
use crate::config;
use crate::errors::{EXIT_CANCELLED, Failure};
use crate::server::App;
use crate::server::build::{EngineConfig, caller, connections, engine};
use crate::storage::{block_on, store_config};

/// Queues the summary of a committed run; the server publishes it.
pub fn publish(args: &RunActionArguments) -> anyhow::Result<ExitCode> {
    let server = config::server(Some(&args.config))?;
    let client = caller(&server, &args.actor, ApiRole::Writer)?;
    let store = store_config(Some(&args.config), &server)?;
    let op = block_on(async move {
        let storage = Storage::open(&store).await?;
        storage
            .enqueue_publication(&client.tenant_id, &args.run, Timestamp::now().as_second())
            .await
    })?
    .map_err(|e| Failure::invalid(e.to_string()))?;
    match op {
        Some(op) => println!("publication operation {op} queued"),
        None => println!("this revision is already queued or sent"),
    }
    Ok(ExitCode::SUCCESS)
}

/// Cancels an active run (exit 130); a committed one is reported with its state (exit 0).
pub fn cancel(args: &RunActionArguments) -> anyhow::Result<ExitCode> {
    let server = config::server(Some(&args.config))?;
    let client = caller(&server, &args.actor, ApiRole::Writer)?;
    let store = store_config(Some(&args.config), &server)?;
    let reason = format!("cancelled by {}", client.name);
    let state = block_on(async move {
        let storage = Storage::open(&store).await?;
        storage
            .cancel_run_by_id(&client.tenant_id, &args.run, &reason, Timestamp::now())
            .await
    })??
    .ok_or_else(|| Failure::invalid(format!("run {} not found", args.run)))?;
    println!("run {} is {state}", args.run);
    Ok(if state == "CANCELLED" {
        ExitCode::from(EXIT_CANCELLED)
    } else {
        ExitCode::SUCCESS
    })
}

/// A manual generation from the provider's current state; the server runs it.
pub fn rerun(args: &RerunArguments) -> anyhow::Result<ExitCode> {
    let server = config::server(Some(&args.config))?;
    let client = caller(&server, &args.actor, ApiRole::Writer)?;
    let store = store_config(Some(&args.config), &server)?;
    let conns = connections(&server)?;
    let deep = args.profile == "deep";
    let run = block_on(async {
        let storage = Storage::open(&store).await?;
        let app = Arc::new(App::local(storage, store.artifacts.clone()));
        let engine = engine(
            app,
            conns,
            EngineConfig {
                config: &args.config,
                server: &server,
                store: store.clone(),
                models: ModelSelection::Roles,
                semantic: Switch::On,
            },
        );
        engine
            .manual(args.change, &client.name, false, deep)
            .await
            .map_err(|e| anyhow::anyhow!("{e}"))
    })??;
    println!("run {} queued as generation {}", run.id, run.generation);
    Ok(ExitCode::SUCCESS)
}
