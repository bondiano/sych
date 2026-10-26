//! Model probe for `sych doctor --probe-model PROFILE`: one opt-in paid structured request through the gateway
//! and ledger (roadmap M1.9). It prints the sanitized accounting record; no review result.

use std::path::Path;
use std::process::ExitCode;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;
use sych_core::config::{BillingKind, Enforcement, RunMode, ServerConfig};
use sych_core::model::{DataBlock, ErrorKind, Gateway, Limits, Prompt, Purpose, TaskFailure};
use sych_core::{PolicySnapshotId, SourceSnapshotId, Timestamp, redact, sha256};
use sych_storage::{Partition, Storage, StorageError, TENANT_LOCAL};

use crate::backend::{ResolveError, resolve};
use crate::errors::{EXIT_NO_RESULT, EXIT_PERMISSION, Failure};
use crate::storage::{block_on, store_config};

const PROBE_WORD: &str = "owl-probe";

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Echo {
    echo: String,
}

fn prompt() -> Prompt {
    Prompt {
        template: "probe@1".into(),
        role: "You are a connectivity probe for a code review tool.".into(),
        output_contract: "Reply with only a JSON object {\"echo\": string}.".into(),
        rules: Vec::new(),
        task: "Copy the word from the data block into `echo`.".into(),
        data: vec![DataBlock {
            label: "word".into(),
            content: PROBE_WORD.into(),
        }],
    }
}

#[derive(Serialize)]
struct RouteReport<'a> {
    profile: &'a str,
    provider: &'a str,
    adapter: &'a str,
    model: &'a str,
    upstreams: &'a [String],
    billing: BillingKind,
    enforcement: Enforcement,
    price_revision: Option<&'a str>,
}

/// Runs the probe and prints its accounting record as JSON.
pub fn probe(config: &Path, server: &ServerConfig, name: &str) -> anyhow::Result<ExitCode> {
    let resolved =
        resolve(config, server, name, RunMode::PersonalLocal).map_err(|err| match err {
            ResolveError::Invalid(e) => Failure::invalid(redact(&e)),
            ResolveError::Credentials(e) => Failure::permission(e),
        })?;
    let redactor = server.redaction.redactor().map_err(Failure::invalid)?;
    let store = store_config(Some(config), server)?;
    let outcome = block_on(async {
        let storage = Storage::open(&store).await?;
        let partition = Partition {
            tenant_id: TENANT_LOCAL.into(),
            access_scope: "model-probe".into(),
        };
        let marker = format!("model-probe:{name}");
        let run = storage
            .begin_run(
                &partition,
                &marker,
                &SourceSnapshotId(sha256(marker.as_bytes())),
                &PolicySnapshotId(sha256(marker.as_bytes())),
                Timestamp::now(),
            )
            .await?;
        let gateway = Gateway {
            provider: &resolved.provider,
            ledger: &storage,
            route: resolved.route.clone(),
            price: resolved.price,
            fixed_fee: resolved.fixed_fee,
            tenant_id: TENANT_LOCAL.into(),
            run_id: run.id,
            purpose: Purpose::Analysis,
            limits: Limits::new(&server.budgets, Purpose::Analysis),
            deadline: None,
            now: Timestamp::now,
            redactor: &redactor,
        };
        let outcome = gateway.structured::<Echo>(&prompt(), "probe").await;
        storage
            .fail_run(&run, "model probe; no review result", Timestamp::now())
            .await?;
        Ok::<_, StorageError>((run.id, outcome))
    });
    let (run_id, outcome) = outcome??;
    tracing::info!(
        profile = name,
        model = %resolved.route.model,
        attempts = outcome.attempts.len(),
        valid = outcome.value.is_ok(),
        "model probe finished"
    );
    let r = &resolved.route;
    let report = json!({
        "probe": name,
        "run_id": run_id,
        "route": RouteReport {
            profile: &r.profile,
            provider: &r.provider,
            adapter: &r.adapter,
            model: &r.model,
            upstreams: &resolved.upstreams,
            billing: r.billing,
            enforcement: r.enforcement,
            price_revision: resolved.price_revision.as_deref(),
        },
        "valid": outcome.value.is_ok(),
        "echo_matches": outcome.value.as_ref().is_ok_and(|e| e.echo == PROBE_WORD),
        "failure": outcome.value.as_ref().err(),
        "attempts": outcome.attempts,
    });
    let text = serde_json::to_string_pretty(&report)
        .map_err(|e| Failure::no_result(format!("cannot render probe report: {e}")))?;
    println!("{}", redact(&text));
    // The report is printed either way; a failed call only changes the exit code.
    Ok(match outcome.value {
        Ok(_) => ExitCode::SUCCESS,
        Err(TaskFailure::Provider {
            kind: ErrorKind::Auth,
            ..
        }) => ExitCode::from(EXIT_PERMISSION),
        Err(_) => ExitCode::from(EXIT_NO_RESULT),
    })
}
