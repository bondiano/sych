use std::process::ExitCode;

use sych_storage::Storage;

use crate::args::StoreArguments;
use crate::storage::{block_on, load_local_store_config, load_store_config};

pub fn run(args: &StoreArguments) -> anyhow::Result<ExitCode> {
    let store = match args.config.as_deref() {
        Some(path) => load_store_config(Some(path))?,
        None => load_local_store_config(None)?,
    };
    let applied = block_on(Storage::migrate(&store))??;
    tracing::info!(database = %store.database, applied = applied.len(), "database is at the current schema");
    Ok(ExitCode::SUCCESS)
}
