use std::process::ExitCode;

use sych_core::Timestamp;
use sych_storage::Storage;

use crate::args::CachePruneArguments;
use crate::server::mirrors;
use crate::storage::{block_on, load_store_config};

/// Artifacts, records past retention and unused mirrors; `--dry-run` only lists the plan.
pub fn run(args: &CachePruneArguments) -> anyhow::Result<ExitCode> {
    let dry_run = args.dry_run;
    let store = load_store_config(args.config.as_deref())?;
    let artifacts = store.artifacts.clone();
    let now = Timestamp::now();
    let (report, records, active) = block_on(async move {
        let storage = Storage::open(&store).await?;
        let report = storage.prune(now, dry_run).await?;
        let records = storage.prune_records(now, dry_run).await?;
        let active = storage.active_repositories().await?;
        Ok::<_, sych_storage::StorageError>((report, records, active))
    })??;
    for c in &report.candidates {
        println!("{:?} {} {}", c.reason, c.kind, c.sha256);
    }
    for r in records.iter().filter(|r| r.rows > 0) {
        println!("Records {} {} ({})", r.table, r.rows, r.rule);
    }
    let gone = mirrors::prune(&artifacts, &active, now.as_second(), dry_run)?;
    for mirror in &gone {
        println!("Mirror {}", mirror.display());
    }

    tracing::info!(
        eligible = report.candidates.len(),
        deleted = report.deleted,
        records = records.iter().map(|r| r.rows).sum::<i64>(),
        mirrors = gone.len(),
        dry_run,
        "prune finished"
    );
    Ok(ExitCode::SUCCESS)
}
