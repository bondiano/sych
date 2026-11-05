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

/// The runs of `report` with their stored discoveries, read from the store of `config`.
pub(super) fn frozen(report: &Path, config: &Path) -> anyhow::Result<Vec<Frozen>> {
    let text = fs::read_to_string(report)
        .map_err(|e| Failure::invalid(format!("{}: {e}", report.display())))?;
    let prior: Prior = serde_json::from_str(&text).map_err(|e| {
        Failure::invalid(format!("{}: not a measure report: {e}", report.display()))
    })?;
    let store = load_store_config(Some(config))?;
    block_on(async {
        let storage = Storage::open(&store).await?;
        let mut out = Vec::new();
        for run in prior.runs {
            let Some(measured) = run.measured else {
                continue;
            };
            let stored: Stored = storage
                .load_artifact(TENANT_LOCAL, measured.run_id, "analysis")
                .await?;
            out.push(Frozen {
                id: run.id,
                run: run.run,
                discovered: stored.discovered,
            });
        }
        anyhow::Ok(out)
    })?
}
