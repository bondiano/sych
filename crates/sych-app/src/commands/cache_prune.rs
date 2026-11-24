use std::process::ExitCode;

use sych_adapters::git::delete_local_ref;
use sych_core::Timestamp;
use sych_storage::Storage;

use crate::args::CachePruneArguments;
use crate::config;
use crate::server::mirrors;
use crate::storage::{block_on, local_store_config, store_config};

/// Artifacts, records past retention and unused mirrors; `--dry-run` only lists the plan.
pub fn run(args: &CachePruneArguments) -> anyhow::Result<ExitCode> {
    let dry_run = args.dry_run;
    let path = config::local_path(args.config.as_deref())?;
    let path = path.as_deref();
    let server = config::server(path)?;
    let store = if args.config.is_some() {
        store_config(path, &server)?
    } else {
        local_store_config(path, &server)?
    };
    let retention = server.storage.retention;
    let artifacts = store.artifacts.clone();
    let now = Timestamp::now();
    let (report, records, active, refs) = block_on(async move {
        let storage = Storage::open(&store).await?;
        let report = storage.prune(now, dry_run).await?;
        let records = storage.prune_records(now, &retention, dry_run).await?;
        let active = storage.active_repositories().await?;
        let refs = prune_local_refs(&storage, now, dry_run).await?;
        Ok::<_, sych_storage::StorageError>((report, records, active, refs))
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
        local_refs = refs,
        mirrors = gone.len(),
        dry_run,
        "prune finished"
    );
    Ok(ExitCode::SUCCESS)
}

async fn prune_local_refs(
    storage: &Storage,
    now: Timestamp,
    dry_run: bool,
) -> Result<usize, sych_storage::StorageError> {
    let mut count = 0;
    for head in storage.prunable_local_heads(now).await? {
        if !dry_run {
            if let Err(err) = delete_local_ref(&head.git_dir, &head.oid) {
                tracing::warn!(run = %head.run, error = %err, "cannot prune local Git ref");
                continue;
            }

            storage.forget_local_head(&head).await?;
        }

        println!("LocalRef refs/sych/local/{}", head.oid.hex());
        count += 1;
    }

    Ok(count)
}
