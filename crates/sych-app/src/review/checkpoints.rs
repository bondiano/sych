//! The parent checkpoints a successor run may reuse (spec §8.5, §9.6).

use std::collections::BTreeMap;

use sych_core::model::{Checkpoint, reusable};
use sych_core::{RunId, Sha256Digest};
use sych_storage::{CHECKPOINT_ROLE, RunHandle, Storage};

/// The parent's checkpoints in this run's partition; unreadable ones only mean fresh calls.
pub(super) async fn parent(
    storage: &Storage,
    run: &RunHandle,
    parent: Option<RunId>,
) -> BTreeMap<Sha256Digest, Checkpoint> {
    let Some(parent) = parent else {
        return BTreeMap::new();
    };
    match storage
        .load_artifacts(&run.partition, parent, CHECKPOINT_ROLE)
        .await
    {
        Ok(found) => reusable(found),
        Err(e) => {
            tracing::warn!(run = %run.id, %parent, error = %e, "parent checkpoints not read");
            BTreeMap::new()
        }
    }
}
