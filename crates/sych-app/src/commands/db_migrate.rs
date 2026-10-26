use std::process::ExitCode;

use sych_storage::Storage;

use crate::args::StoreArguments;
use crate::storage::{block_on, load_store_config};

pub fn run(args: &StoreArguments) -> anyhow::Result<ExitCode> {
    let store = load_store_config(args.config.as_deref())?;
    let applied = block_on(Storage::migrate(&store))??;
    tracing::info!(database = %store.database.display(), applied = applied.len(), "database is at the current schema");
    Ok(ExitCode::SUCCESS)
}
