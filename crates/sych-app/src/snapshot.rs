//! Pinning the comparison an analysis reads.

use sych_adapters::git::{LocalSnapshot, SnapshotError, SnapshotRequest};

use crate::args::Comparison;

/// Blobs above this size stay in the manifest unread.
pub const MAX_BLOB_BYTES: u64 = 4 << 20;

pub fn capture(comparison: &Comparison) -> Result<LocalSnapshot, SnapshotError> {
    LocalSnapshot::capture(&SnapshotRequest {
        repo: &comparison.repo,
        base: &comparison.base,
        head: &comparison.head,
        diff_base: comparison.diff_base.as_deref(),
        max_blob_bytes: MAX_BLOB_BYTES,
    })
}
