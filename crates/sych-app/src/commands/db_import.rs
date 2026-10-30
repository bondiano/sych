use std::process::ExitCode;

use sych_storage::Storage;

use crate::args::DbImportArguments;
use crate::storage::{block_on, load_store_config};

pub fn run(args: &DbImportArguments) -> anyhow::Result<ExitCode> {
    let target = load_store_config(Some(&args.config))?;
    block_on(Storage::import_sqlite(&args.source, &target))??;
    Ok(ExitCode::SUCCESS)
}
