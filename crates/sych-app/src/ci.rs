//! CI reports and their manifest read for `sych review --ci-facts`.

use std::fs;
use std::path::Path;

use anyhow::Context;
use sych_adapters::ci;
use sych_adapters::git::LocalSnapshot;
use sych_core::{CiFacts, CiManifest};

use crate::errors::Failure;

/// Reports are read whole; larger ones are rejected rather than truncated.
const MAX_REPORT_BYTES: u64 = 32 << 20;

fn read(path: &Path) -> anyhow::Result<String> {
    let size = fs::metadata(path)
        .map_err(|e| Failure::invalid(format!("{}: {e}", path.display())))?
        .len();
    if size > MAX_REPORT_BYTES {
        return Err(Failure::invalid(format!("{} is larger than 32 MiB", path.display())).into());
    }

    fs::read_to_string(path)
        .map_err(|e| Failure::invalid(format!("{}: {e}", path.display())).into())
}

/// Normalized reports; a malformed report or manifest is an input error, not a silent skip.
#[tracing::instrument(level = "info", skip_all, fields(reports = reports.len()))]
pub fn import(
    snap: &LocalSnapshot,
    reports: &[impl AsRef<Path>],
    manifest: Option<&Path>,
) -> anyhow::Result<Vec<CiFacts>> {
    let manifest: Option<CiManifest> = manifest
        .map(|path| -> anyhow::Result<CiManifest> {
            serde_yaml_ng::from_str(&read(path)?).map_err(|e| {
                Failure::invalid(format!("{}: invalid CI manifest: {e}", path.display())).into()
            })
        })
        .transpose()?;
    if manifest.is_some() && reports.is_empty() {
        return Err(Failure::invalid("--ci-manifest needs at least one --ci-facts report").into());
    }

    reports
        .iter()
        .map(|path| {
            let path = path.as_ref();
            let text = read(path)?;
            ci::import(snap, &path.display().to_string(), &text, manifest.as_ref())
                .map_err(Failure::invalid)
                .with_context(|| format!("cannot import {}", path.display()))
        })
        .collect()
}
