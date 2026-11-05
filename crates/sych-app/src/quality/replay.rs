//! `eval measure --replay`: the runs of an earlier report, each with the discoveries its review
//! stored, so a verifier can be judged on exactly the candidates another one saw.

use std::fs;
use std::path::Path;

use serde::Deserialize;
use sych_core::RunId;
use sych_core::analysis::Discovered;
use sych_storage::{Storage, TENANT_LOCAL};

use crate::errors::Failure;
use crate::storage::{block_on, load_store_config};

#[derive(Deserialize)]
struct Prior {
    config_sha256: String,
    runs: Vec<PriorRun>,
}

#[derive(Deserialize)]
struct PriorRun {
    id: String,
    run: u32,
    /// None for a run that failed: it stored nothing to replay.
    measured: Option<PriorMeasured>,
}

#[derive(Deserialize)]
struct PriorMeasured {
    run_id: RunId,
}

/// The part of the stored `analysis` artifact a replay needs.
#[derive(Deserialize)]
struct Stored {
    #[serde(default)]
    discovered: Vec<Discovered>,
}

/// One earlier run of a case and what its passes discovered.
pub(super) struct Frozen {
    pub(super) id: String,
    pub(super) run: u32,
    pub(super) discovered: Vec<Discovered>,
}

/// The runs of `report` with their stored discoveries, read from the store of `config`, and
/// the runs that failed there with nothing to replay. The report must have been measured under
/// the same config (`config_sha256`): its store holds the runs, its budgets bound them.
pub(super) fn frozen(
    report: &Path,
    config: &Path,
    config_sha256: &str,
) -> anyhow::Result<(Vec<Frozen>, Vec<String>)> {
    let text = fs::read_to_string(report)
        .map_err(|e| Failure::invalid(format!("{}: {e}", report.display())))?;
    let prior: Prior = serde_json::from_str(&text).map_err(|e| {
        Failure::invalid(format!("{}: not a measure report: {e}", report.display()))
    })?;
    if prior.config_sha256 != config_sha256 {
        return Err(Failure::invalid(format!(
            "{} was measured under another --config (sha256 {}); replay it with that config",
            report.display(),
            prior.config_sha256
        ))
        .into());
    }

    let store = load_store_config(Some(config))?;
    block_on(async {
        let storage = Storage::open(&store).await?;
        let (mut out, mut failed) = (Vec::new(), Vec::new());
        for run in prior.runs {
            let Some(measured) = run.measured else {
                failed.push(format!(
                    "{} run {}: failed in the replayed report, nothing to replay",
                    run.id, run.run
                ));
                continue;
            };
            let stored: Stored = storage
                .load_artifact(TENANT_LOCAL, measured.run_id, "analysis")
                .await?;
            // Its verification context cannot be rebuilt; replaying it would verify nothing.
            if stored.discovered.iter().any(|d| d.batch.is_empty()) {
                failed.push(format!(
                    "{} run {}: stored before discoveries named their packets",
                    run.id, run.run
                ));
                continue;
            }

            out.push(Frozen {
                id: run.id,
                run: run.run,
                discovered: stored.discovered,
            });
        }
        anyhow::Ok((out, failed))
    })?
}
