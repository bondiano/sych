//! Inline comments of earlier revisions against a new result: one whose finding the new
//! revision closes says so, and one whose finding comes back shows its text again.

use std::slice;

use crate::lineage::{Decision, Prior, decide};
use crate::{DiffManifest, ReviewResult};

/// A delivered inline comment of the change: its publication object, the finding it was
/// posted for, and whether it now carries the "no longer reported" note.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Posted {
    pub object_id: i64,
    pub prior: Prior,
    pub noted: bool,
}

/// The objects to rewrite for `result`: a comment whose finding lineage closes (re-checked,
/// deleted or continued elsewhere) gets the note, and a noted one whose finding is found again
/// gets its text back. A finding still open (a partial review, an unreviewed file) is a missing
/// one, not a fix, so its comment stays as it is.
pub fn revisit(posted: &[Posted], result: &ReviewResult, manifest: &DiffManifest) -> Vec<i64> {
    posted
        .iter()
        .filter(
            |p| match &decide(slice::from_ref(&p.prior), result, manifest)[0].1 {
                Decision::Matched => p.noted,
                Decision::Open { .. } => false,
                Decision::Resolved { .. }
                | Decision::Obsolete { .. }
                | Decision::Continued { .. } => !p.noted,
            },
        )
        .map(|p| p.object_id)
        .collect()
}
