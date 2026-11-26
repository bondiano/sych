//! Inline comments of earlier revisions against a new result: one whose finding the new
//! revision closes says so, and one whose finding comes back shows its text again.

use std::slice;

use crate::lineage::{Decision, Prior, decide};
use crate::{DiffManifest, Lifecycle, ReviewResult};

/// A delivered inline comment of the change: its publication object, the finding it was
/// posted for, whether the finding was dismissed, and whether the body now carries the
/// "no longer reported" note.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Posted {
    pub object_id: i64,
    pub prior: Prior,
    pub dismissed: bool,
    pub noted: bool,
}

/// The objects to rewrite for `result`: a comment whose finding lineage closes (re-checked,
/// deleted or continued elsewhere) or the change dismissed gets the note, and a noted one whose
/// finding is found again gets its text back. A finding still open (a partial review, an
/// unreviewed file) is a missing one, not a fix, so its comment stays as it is.
pub fn revisit(posted: &[Posted], result: &ReviewResult, manifest: &DiffManifest) -> Vec<i64> {
    posted
        .iter()
        .filter(|p| gone(p, result, manifest).is_some_and(|gone| gone != p.noted))
        .map(|p| p.object_id)
        .collect()
}

/// Whether `p`'s finding is gone from `result`; `None` while lineage keeps it open.
fn gone(p: &Posted, result: &ReviewResult, manifest: &DiffManifest) -> Option<bool> {
    if p.dismissed {
        return Some(true);
    }

    let dismissed = (result.findings.iter()).any(|f| {
        f.fingerprint.to_string() == p.prior.fingerprint && f.lifecycle == Lifecycle::Dismissed
    });
    if dismissed {
        return Some(true);
    }

    match &decide(slice::from_ref(&p.prior), result, manifest)[0].1 {
        Decision::Matched => Some(false),
        Decision::Open { .. } => None,
        Decision::Resolved { .. } | Decision::Obsolete { .. } | Decision::Continued { .. } => {
            Some(true)
        }
    }
}
