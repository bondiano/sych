//! Finding lineage between revisions (spec §5.6, §18.4): what happens to an open finding of the
//! previous revision. A missing finding is not a fix: `RESOLVED` needs the criterion checked
//! again on the new snapshot by a comparable method; a rename never closes anything by itself.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{
    Applicability, ChangeStatus, Coverage, DiffManifest, FindingCategory, ObligationStatus,
    ReviewResult,
};

/// An open finding of an earlier revision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Prior {
    pub fingerprint: String,
    pub category: FindingCategory,
    pub rule_id: Option<String>,
    pub path: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "decision", rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Decision {
    /// Found again with the same fingerprint.
    Matched,
    /// The criterion was checked again and holds now.
    Resolved {
        basis: String,
        evidence: Vec<String>,
    },
    /// The anchored file is gone from the head (not renamed).
    Obsolete { basis: String },
    /// Renamed or moved, and exactly one finding of the same kind is at the new place: the
    /// problem continues there; nothing is closed.
    Continued { successor: String },
    /// Still open: not re-checked comparably; `reason` says why.
    Open { reason: String },
}

/// The decision for each prior finding against the new `result` and its diff.
pub fn decide(
    prior: &[Prior],
    result: &ReviewResult,
    manifest: &DiffManifest,
) -> Vec<(String, Decision)> {
    prior
        .iter()
        .map(|p| (p.fingerprint.clone(), one(p, result, manifest)))
        .collect()
}

fn one(p: &Prior, result: &ReviewResult, manifest: &DiffManifest) -> Decision {
    if result
        .findings
        .iter()
        .any(|f| f.fingerprint.to_string() == p.fingerprint)
    {
        return Decision::Matched;
    }

    let open = |reason: &str| Decision::Open {
        reason: reason.into(),
    };
    let Some(path) = p.path.as_deref() else {
        return open("no anchor to re-check");
    };
    let change = manifest
        .files
        .iter()
        .find(|f| f.old.as_ref().is_some_and(|o| o.path.as_str() == path));
    match change.map(|f| (f.status, f.new.as_ref())) {
        Some((ChangeStatus::Deleted, _)) => {
            return Decision::Obsolete {
                basis: format!("{path} was deleted"),
            };
        }
        Some((ChangeStatus::Renamed | ChangeStatus::Copied, Some(new)))
            if new.path.as_str() != path =>
        {
            let moved: Vec<_> = result
                .findings
                .iter()
                .filter(|f| {
                    f.category == p.category
                        && f.rule_id == p.rule_id
                        && f.anchors.iter().any(|a| a.path == new.path)
                })
                .collect();
            return match moved.as_slice() {
                [one] => Decision::Continued {
                    successor: one.fingerprint.to_string(),
                },
                _ => open("renamed or moved; not matched again with certainty"),
            };
        }
        _ => {}
    }
    if result.coverage != Coverage::CompleteForScope {
        return open("the new revision was reviewed partially");
    }

    let covered = result
        .scope_manifest
        .units
        .iter()
        .any(|u| u.paths.iter().any(|q| q.as_str() == path));
    if !covered {
        return open("its file was not reviewed again");
    }

    match &p.rule_id {
        Some(rule) => rule_recheck(rule, result),
        None => model_recheck(p, path, result, manifest),
    }
}

/// A rule finding resolves when the rule was assessed again and is supported everywhere.
fn rule_recheck(rule: &str, result: &ReviewResult) -> Decision {
    let open = |reason: &str| Decision::Open {
        reason: reason.into(),
    };
    let checked: Vec<_> = result
        .obligations
        .iter()
        .filter(|o| o.rule_id == rule && o.applicability == Applicability::Applies)
        .collect();
    if checked.is_empty() {
        return open("the rule was not evaluated again");
    }

    if checked
        .iter()
        .all(|o| o.status == ObligationStatus::Supported)
    {
        Decision::Resolved {
            basis: format!("{rule} assessed SUPPORTED on the new snapshot"),
            evidence: checked
                .iter()
                .flat_map(|o| o.evidence_ids.iter().map(ToString::to_string))
                .collect(),
        }
    } else {
        open("the rule is still not supported")
    }
}

/// A model finding is re-checked only when its code changed and the unit was fully reviewed
/// again without the same kind of finding there.
fn model_recheck(
    p: &Prior,
    path: &str,
    result: &ReviewResult,
    manifest: &DiffManifest,
) -> Decision {
    let open = |reason: &str| Decision::Open {
        reason: reason.into(),
    };
    let unchanged = manifest
        .files
        .iter()
        .find(|f| f.new.as_ref().is_some_and(|n| n.path.as_str() == path))
        .is_none();
    let again = result
        .findings
        .iter()
        .any(|f| f.category == p.category && f.anchors.iter().any(|a| a.path.as_str() == path));
    if unchanged {
        open("its code did not change")
    } else if again {
        open("a finding of the same kind is still there")
    } else {
        Decision::Resolved {
            basis: "its code changed and the unit was fully reviewed again".into(),
            evidence: Vec::new(),
        }
    }
}
