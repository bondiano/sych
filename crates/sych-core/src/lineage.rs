//! Finding lineage between revisions: what happens to an open finding of the
//! previous revision. A missing finding is not a fix: `RESOLVED` needs the criterion checked
//! again on the new snapshot by a comparable method; a rename never closes anything by itself.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{
    Applicability, BackendStatus, ChangeStatus, Coverage, DiffManifest, FindingCategory,
    Obligation, ObligationStatus, ReviewResult, RunFacts,
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

/// The decision for each prior finding against the new `result`, its diff and its run facts.
pub fn decide(
    prior: &[Prior],
    result: &ReviewResult,
    manifest: &DiffManifest,
    facts: &RunFacts,
) -> Vec<(String, Decision)> {
    prior
        .iter()
        .map(|p| (p.fingerprint.clone(), one(p, result, manifest, facts)))
        .collect()
}

fn one(p: &Prior, result: &ReviewResult, manifest: &DiffManifest, facts: &RunFacts) -> Decision {
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
    if p.path.is_none() {
        if let Some(rule) = p.rule_id.as_deref().filter(|r| r.starts_with("DB-")) {
            if result.coverage != Coverage::CompleteForScope {
                return open("the new revision was reviewed partially");
            }

            return db_recheck(rule, None, result, facts);
        }

        return open("no anchor to re-check");
    }

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
    let Some(rule) = p.rule_id.as_deref() else {
        if !covered {
            return open("its file was not reviewed again");
        }

        return model_recheck(p, path, result, manifest);
    };

    let checked: Vec<_> = result
        .obligations
        .iter()
        .filter(|o| o.rule_id == rule && o.applicability == Applicability::Applies)
        .collect();
    if !checked.is_empty() {
        if !covered {
            return open("its file was not reviewed again");
        }

        return obligation_recheck(rule, &checked);
    }

    if rule.starts_with("SEC-") {
        sec_recheck(rule, path, result, manifest)
    } else if rule.starts_with("ARCH-") {
        arch_recheck(rule, path, result, facts)
    } else if rule.starts_with("DB-") {
        db_recheck(rule, Some(path), result, facts)
    } else if matches!(
        p.category,
        FindingCategory::Nit | FindingCategory::Improvement
    ) {
        if !covered {
            return open("its file was not reviewed again");
        }

        model_recheck(p, path, result, manifest)
    } else if !covered {
        open("its file was not reviewed again")
    } else {
        open("the rule was not evaluated again")
    }
}

/// An obligation rule resolves when the rule was assessed again and is supported everywhere.
fn obligation_recheck(rule: &str, checked: &[&Obligation]) -> Decision {
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
        Decision::Open {
            reason: "the rule is still not supported".into(),
        }
    }
}

fn sec_recheck(rule: &str, path: &str, result: &ReviewResult, manifest: &DiffManifest) -> Decision {
    let open = |reason: &str| Decision::Open {
        reason: reason.into(),
    };
    let ran = result
        .scope_manifest
        .requested_profiles
        .iter()
        .any(|p| p == "security");
    if !ran {
        return open("the security scan did not run");
    }

    if !manifest.omissions.is_empty()
        || result
            .excluded_or_unreviewed
            .iter()
            .any(|g| g.path.as_str() == path)
    {
        return open("its file was not scanned");
    }

    let still_there = result.findings.iter().any(|f| {
        f.rule_id.as_deref() == Some(rule)
            && (f.anchors.iter().any(|a| a.path.as_str() == path) || f.scope == path)
    });
    if still_there {
        open("the rule is still violated")
    } else {
        Decision::Resolved {
            basis: format!("{rule} found no match in added lines on the new snapshot"),
            evidence: Vec::new(),
        }
    }
}

fn project_covers(project: &str, path: &str) -> bool {
    project.is_empty()
        || path == project
        || path
            .strip_prefix(project)
            .is_some_and(|rest| rest.starts_with('/'))
}

fn arch_recheck(rule: &str, path: &str, result: &ReviewResult, facts: &RunFacts) -> Decision {
    let open = |reason: &str| Decision::Open {
        reason: reason.into(),
    };
    let projects: Vec<_> = facts
        .architecture
        .iter()
        .filter(|a| project_covers(&a.project, path))
        .collect();
    if projects.is_empty() {
        return open("the architecture check did not run");
    }

    if projects.iter().any(|a| a.status != BackendStatus::Ready) {
        return open("the architecture check was unavailable");
    }

    let still_there = result.findings.iter().any(|f| {
        f.rule_id.as_deref() == Some(rule) && f.anchors.iter().any(|a| a.path.as_str() == path)
    });
    if still_there {
        open("the architecture check still reports a finding")
    } else {
        Decision::Resolved {
            basis: format!("{rule} checked on the new snapshot"),
            evidence: projects
                .iter()
                .map(|p| p.evidence_id().to_string())
                .collect(),
        }
    }
}

fn db_recheck(rule: &str, path: Option<&str>, result: &ReviewResult, facts: &RunFacts) -> Decision {
    let open = |reason: &str| Decision::Open {
        reason: reason.into(),
    };
    if facts.db_schema.is_empty() {
        return open("schema_lint did not run");
    }

    if facts
        .db_schema
        .iter()
        .any(|d| d.status != BackendStatus::Ready)
    {
        return open("schema_lint was unavailable");
    }

    let short = rule.strip_prefix("DB-").unwrap_or(rule);
    if facts.db_schema.iter().any(|d| {
        d.skipped_rules
            .iter()
            .any(|s| s.eq_ignore_ascii_case(short))
    }) {
        return open("the schema rule was skipped");
    }

    let still_there = result.findings.iter().any(|f| {
        f.rule_id.as_deref() == Some(rule)
            && path.is_none_or(|p| f.anchors.iter().any(|a| a.path.as_str() == p))
    });
    if still_there {
        open("schema_lint still reports a finding")
    } else {
        Decision::Resolved {
            basis: format!("{rule} checked on the new snapshot"),
            evidence: facts
                .db_schema
                .iter()
                .map(|d| d.evidence_id().to_string())
                .collect(),
        }
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
    let again = result.findings.iter().any(|f| {
        f.category == p.category
            && p.rule_id
                .as_ref()
                .is_none_or(|r| f.rule_id.as_ref() == Some(r))
            && f.anchors.iter().any(|a| a.path.as_str() == path)
    });
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
