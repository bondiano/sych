//! Scoped waivers over obligations and repository preferences, and `review.block_on` evaluation.

use std::collections::BTreeMap;

use crate::{
    Evidence, EvidenceId, Finding, FindingCategory, FindingId, Lifecycle, ObligationId,
    ObligationStatus, PlannedObligation, RepoPath, ReviewPlan, Timestamp, Waiver, glob_match,
};

/// Paths a waiver must cover: the violation's anchors, else every path of the obligation's
/// units, so a waiver for one file never lifts the rule for another.
fn waiver_scope(
    planned: &PlannedObligation,
    evidence: &BTreeMap<EvidenceId, &Evidence>,
    plan: &ReviewPlan,
) -> Vec<RepoPath> {
    let o = &planned.obligation;
    let anchors: Vec<RepoPath> = o
        .evidence_ids
        .iter()
        .filter_map(|id| evidence.get(id))
        .flat_map(|ev| ev.code_refs.iter().map(|r| r.path.clone()))
        .collect();
    if matches!(o.status, ObligationStatus::Violated) && !anchors.is_empty() {
        return anchors;
    }

    plan.scope
        .units
        .iter()
        .filter(|u| o.review_unit_ids.contains(&u.id))
        .flat_map(|u| u.paths.iter().cloned())
        .collect()
}

/// Exceptions in force that cover the whole scope become waivers of a VIOLATED or UNKNOWN
/// obligation; expired or partial ones are only noted. The status stays as assessed.
pub(super) fn waive(
    planned: &mut PlannedObligation,
    evidence: &BTreeMap<EvidenceId, &Evidence>,
    plan: &ReviewPlan,
    now: Timestamp,
    unresolved: &mut Vec<String>,
) {
    if !matches!(
        planned.obligation.status,
        ObligationStatus::Violated | ObligationStatus::Unknown { .. }
    ) {
        return;
    }

    let scope = waiver_scope(planned, evidence, plan);
    let o = &mut planned.obligation;
    for e in &planned.exceptions {
        let uncovered = scope
            .iter()
            .find(|p| !e.paths.iter().any(|g| glob_match(g, p.as_str())));
        match (e.expires_at, uncovered) {
            (Some(at), _) if at <= now => o.limitations.push(format!(
                "exception by {} expired at {at}: {}",
                e.owner, e.reason
            )),
            (_, Some(path)) => o.limitations.push(format!(
                "exception by {} does not cover {path}: {}",
                e.owner, e.reason
            )),
            (_, None) => o.waivers.push(Waiver {
                paths: e.paths.clone(),
                reason: e.reason.clone(),
                owner: e.owner.clone(),
                author: e.author.clone(),
                expires_at: e.expires_at,
                until: e.until.clone(),
            }),
        }
    }
    for w in &o.waivers {
        unresolved.push(format!(
            "{}: waived by {} ({}); the status is kept",
            o.rule_id, w.owner, w.reason
        ));
    }
}

/// Open, and not only about obligations that are all waived.
pub(super) fn in_force(f: &Finding, waived: &[ObligationId]) -> bool {
    f.lifecycle == Lifecycle::Open
        && (f.obligation_ids.is_empty() || !f.obligation_ids.iter().all(|id| waived.contains(id)))
}

/// Findings in force of the rules `block_on` names; a waived one blocks nothing, as it raises no
/// recommendation, and a nit or improvement never blocks.
pub(super) fn blocked_by(
    findings: &[Finding],
    obligations: &[PlannedObligation],
    block_on: &[String],
    waived: &[ObligationId],
) -> Vec<FindingId> {
    let matches_block = |rule: &str| block_on.iter().any(|g| glob_match(g, rule));
    findings
        .iter()
        .filter(|f| {
            matches!(
                f.category,
                FindingCategory::Defect | FindingCategory::RuleViolation
            ) && in_force(f, waived)
                && (f.obligation_ids.iter().any(|id| {
                    !waived.contains(id)
                        && obligations
                            .iter()
                            .find(|o| o.obligation.id == *id)
                            .is_some_and(|o| matches_block(&o.obligation.rule_id))
                }) || f.rule_id.as_deref().is_some_and(|r| {
                    matches_block(r)
                        && !f.obligation_ids.iter().any(|id| {
                            waived.contains(id)
                                && obligations
                                    .iter()
                                    .any(|o| o.obligation.id == *id && o.obligation.rule_id == r)
                        })
                }))
        })
        .map(|f| f.id)
        .collect()
}

pub(super) fn waive_preferences(findings: &mut [Finding], plan: &ReviewPlan, now: Timestamp) {
    for finding in findings {
        let Some(rule_id) = finding.rule_id.as_deref() else {
            continue;
        };
        let Some(pref) = plan.preferences.iter().find(|p| p.rule_id == rule_id) else {
            continue;
        };
        let scope: Vec<RepoPath> = if finding.anchors.is_empty() {
            plan.scope
                .units
                .iter()
                .filter(|u| pref.review_unit_ids.contains(&u.id))
                .flat_map(|u| u.paths.iter().cloned())
                .collect()
        } else {
            finding.anchors.iter().map(|a| a.path.clone()).collect()
        };
        for e in &pref.exceptions {
            let uncovered = scope
                .iter()
                .find(|p| !e.paths.iter().any(|g| glob_match(g, p.as_str())));
            let note = match (e.expires_at, uncovered) {
                (Some(at), _) if at <= now => {
                    format!("exception by {} expired at {at}: {}", e.owner, e.reason)
                }
                (_, Some(path)) => format!(
                    "exception by {} does not cover {path}: {}",
                    e.owner, e.reason
                ),
                (_, None) => format!(
                    "{}: waived by {} ({}); the finding is kept",
                    pref.rule_id, e.owner, e.reason
                ),
            };
            if !finding.limitations.contains(&note) {
                finding.limitations.push(note);
            }
        }
    }
}
