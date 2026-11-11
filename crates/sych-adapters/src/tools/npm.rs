//! npm packages a Sych-owned lockfile pins, installed by the pinned pnpm.

use std::fs;
use std::path::Path;

use sych_core::sha256;
use tokio::process::Command;

use super::{Artifact, PinnedTool, ToolError};

/// The pnpm that wrote these lockfiles and installs them.
pub(super) const PNPM: &str = "12.14.0";

/// `(tool, package.json, pnpm-lock.yaml)` of each [`super::ArchiveKind::PnpmLock`] tool.
const LOCKS: &[(&str, &str, &str)] = &[(
    "scip-typescript",
    include_str!("npm/scip-typescript/package.json"),
    include_str!("npm/scip-typescript/pnpm-lock.yaml"),
)];

/// The lockfile of `tool`, from the table above.
pub(super) fn lock(tool: &str) -> Option<(&'static str, &'static str)> {
    LOCKS
        .iter()
        .find(|(name, _, _)| *name == tool)
        .map(|(_, manifest, lock)| (*manifest, *lock))
}

/// Checks the lockfile against the pin, then installs it with `--frozen-lockfile`: pnpm
/// fetches every package from the public registry and verifies its integrity, runs no
/// lifecycle scripts or pnpmfile, and reads no user or host npm configuration.
pub(super) async fn pnpm_install(
    pnpm: &Path,
    store: &Path,
    tool: &PinnedTool,
    artifact: &Artifact,
    dir: &Path,
) -> Result<(), ToolError> {
    let (manifest, lock) = lock(&tool.name)
        .ok_or_else(|| ToolError::Unsupported(format!("{} has no lockfile", tool.name)))?;
    let actual = sha256(lock.as_bytes());
    if actual.hex() != artifact.sha256 {
        return Err(ToolError::Checksum {
            url: format!("npm/{}/pnpm-lock.yaml", tool.name),
            expected: artifact.sha256.clone(),
            actual: actual.hex().to_owned(),
        });
    }

    let parent = dir.parent().expect("install dir has a parent");
    fs::create_dir_all(parent)?;
    let staging = tempfile::tempdir_in(parent)?;
    let out = staging.path().join("out");
    let home = staging.path().join("home");
    fs::create_dir_all(&out)?;
    fs::create_dir_all(&home)?;
    fs::write(out.join("package.json"), manifest)?;
    fs::write(out.join("pnpm-lock.yaml"), lock)?;
    let output = Command::new(pnpm)
        .args([
            "install",
            "--frozen-lockfile",
            "--ignore-scripts",
            "--ignore-pnpmfile",
            "--prod",
            "--config.registry=https://registry.npmjs.org/",
            // Copies, so the installed tool shares no inode with the store.
            "--config.package-import-method=copy",
            "--store-dir",
        ])
        .arg(store)
        .current_dir(&out)
        .env_clear()
        .env("HOME", &home)
        .env("PATH", "")
        .output()
        .await?;
    if !output.status.success() {
        return Err(ToolError::Download(format!(
            "pnpm install {}: {}",
            tool.name,
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }

    if !out.join(artifact.binary(tool)).is_file() {
        return Err(ToolError::Download(format!(
            "pnpm install {} did not write {}",
            tool.name,
            artifact.binary(tool)
        )));
    }

    match fs::rename(&out, dir) {
        Err(_) if dir.join(artifact.binary(tool)).is_file() => Ok(()),
        other => Ok(other?),
    }
}
