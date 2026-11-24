//! Pinning the comparison an analysis reads.

use std::path::Path;

use sych_adapters::git::{LocalSnapshot, SnapshotError, SnapshotRequest};
use sych_core::{ComparisonMode, SnapshotHeadKind, SourceSnapshot};

use crate::args::Comparison;

/// Blobs above this size stay in the manifest unread.
pub const MAX_BLOB_BYTES: u64 = 4 << 20;

pub fn capture(comparison: &Comparison) -> Result<LocalSnapshot, SnapshotError> {
    capture_with_kind(comparison, SnapshotHeadKind::Committed)
}

/// Reopen a saved snapshot from its immutable Git object IDs.
pub fn reopen(saved: &SourceSnapshot, repo: &Path) -> Result<LocalSnapshot, SnapshotError> {
    let snap = capture_with_kind(
        &Comparison {
            repo: repo.to_owned(),
            base: saved.target_tip.hex().to_owned(),
            head: saved.head.hex().to_owned(),
            diff_base: (saved.comparison == ComparisonMode::ExplicitDiffBase)
                .then(|| saved.diff_base.hex().to_owned()),
            fork: false,
        },
        saved.head_kind,
    )?;
    if snap.snapshot.id != saved.id {
        return Err(SnapshotError::InvalidInput(
            "--repo is not the repository this run reviewed".into(),
        ));
    }

    Ok(snap)
}

pub fn capture_with_kind(
    comparison: &Comparison,
    head_kind: SnapshotHeadKind,
) -> Result<LocalSnapshot, SnapshotError> {
    LocalSnapshot::capture_with_kind(
        &SnapshotRequest {
            repo: &comparison.repo,
            base: &comparison.base,
            head: &comparison.head,
            diff_base: comparison.diff_base.as_deref(),
            max_blob_bytes: MAX_BLOB_BYTES,
        },
        head_kind,
    )
}
