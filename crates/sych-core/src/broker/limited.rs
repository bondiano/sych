//! What a packet budget cut out of the review, or the snapshot does not show: obligations
//! lose their verdict, discovery spans leave a gap that keeps coverage partial.

use std::cmp::Ordering;
use std::collections::BTreeSet;

use super::{EvidencePacket, REQUIRED_OVER_BUDGET, UNSEEN_GAPS};
use crate::effects::LEDGER_GAPS;
use crate::{Assessment, ContentKind, DiffManifest, ObligationStatus, ReviewPlan, SourceSide};

/// The gap of changed lines no discovery packet shows.
pub const UNSHOWN_CHANGE: &str = "UNSHOWN_CHANGE";

fn unseen(gap: &str) -> bool {
    UNSEEN_GAPS
        .iter()
        .chain(&LEDGER_GAPS)
        .any(|code| gap.starts_with(code))
}

fn rule_ids<'a>(
    plan: &'a ReviewPlan,
    packets: &'a [EvidencePacket],
    keep: impl Fn(&EvidencePacket) -> bool,
) -> BTreeSet<&'a str> {
    packets
        .iter()
        .filter(|p| keep(p))
        .filter_map(|p| p.request.obligation_id)
        .filter_map(|id| plan.obligations.iter().find(|o| o.obligation.id == id))
        .map(|o| o.obligation.rule_id.as_str())
        .collect()
}

/// The assessments of one obligation's chunks become one: a violation in any chunk stands,
/// then an unknown, then support; evidence comes from the deciding chunks.
fn combine(into: &mut Assessment, a: Assessment) {
    let rank = |s: &ObligationStatus| match s {
        ObligationStatus::NotApplicable { .. } => 0,
        ObligationStatus::Supported => 1,
        ObligationStatus::Unknown { .. } => 2,
        ObligationStatus::Violated => 3,
    };
    for limitation in a.limitations {
        if !into.limitations.contains(&limitation) {
            into.limitations.push(limitation);
        }
    }
    match rank(&a.status).cmp(&rank(&into.status)) {
        Ordering::Greater => {
            into.status = a.status;
            into.evidence_ids = a.evidence_ids;
            into.triggering_conditions = a.triggering_conditions;
        }
        Ordering::Equal => {
            for id in a.evidence_ids {
                if !into.evidence_ids.contains(&id) {
                    into.evidence_ids.push(id);
                }
            }
            for case in a.triggering_conditions {
                if !into.triggering_conditions.contains(&case) {
                    into.triggering_conditions.push(case);
                }
            }
        }
        Ordering::Less => {}
    }
}

/// Obligations whose required context did not fit cannot keep a verdict: their assessment
/// becomes `UNKNOWN/CONTEXT_LIMIT`, unless a chunk that did fit shows a violation.
/// One whose changed code the snapshot does not show (`UNSEEN_GAPS`) cannot be `SUPPORTED`:
/// it becomes `UNKNOWN/SEMANTIC_GAP`; a violation shown elsewhere in the change stands.
pub fn context_limited(
    plan: &ReviewPlan,
    packets: &[EvidencePacket],
    assessments: Vec<Assessment>,
) -> Vec<Assessment> {
    let limited = rule_ids(plan, packets, |p| {
        p.excluded.iter().any(|e| e.reason == REQUIRED_OVER_BUDGET)
    });
    let mut merged: Vec<Assessment> = Vec::new();
    for a in assessments {
        match merged.iter_mut().find(|m| m.rule_id == a.rule_id) {
            Some(into) => combine(into, a),
            None => merged.push(a),
        }
    }
    let mut out: Vec<Assessment> = merged
        .into_iter()
        .filter(|a| a.status == ObligationStatus::Violated || !limited.contains(a.rule_id.as_str()))
        .map(|a| unseen_unknown(plan, packets, a))
        .collect();
    let decided: BTreeSet<String> = out.iter().map(|a| a.rule_id.clone()).collect();
    out.extend(
        limited
            .into_iter()
            .filter(|rule_id| !decided.contains(*rule_id))
            .map(|rule_id| Assessment {
                rule_id: rule_id.to_owned(),
                status: ObligationStatus::Unknown {
                    reason_code: "CONTEXT_LIMIT".into(),
                    next_evidence_needed:
                        "split the review unit so its required guard fits the request budget"
                            .into(),
                },
                evidence_ids: Vec::new(),
                limitations: vec![format!(
                    "required context exceeded the packet budget ({REQUIRED_OVER_BUDGET})"
                )],
                triggering_conditions: Vec::new(),
            }),
    );
    out
}

fn unseen_unknown(plan: &ReviewPlan, packets: &[EvidencePacket], a: Assessment) -> Assessment {
    if a.status != ObligationStatus::Supported {
        return a;
    }

    let gaps: Vec<String> = packets
        .iter()
        .filter(|p| {
            p.request.obligation_id.is_some_and(|id| {
                plan.obligations
                    .iter()
                    .any(|o| o.obligation.id == id && o.obligation.rule_id == a.rule_id)
            })
        })
        .flat_map(|p| &p.gaps)
        .filter(|g| unseen(g))
        .cloned()
        .collect();
    if gaps.is_empty() {
        return a;
    }

    Assessment {
        status: ObligationStatus::Unknown {
            reason_code: "SEMANTIC_GAP".into(),
            next_evidence_needed: "analyse the gated code in its build, the macro's expansion, \
                                   the type's Debug impl, the dynamic callee or the changed \
                                   effect wrapper the gap names"
                .into(),
        },
        limitations: gaps,
        ..a
    }
}

/// Required spans of packets without an obligation (discovery) that did not fit, and changed
/// code the snapshot does not show: the change was not read in full, whatever the rules say.
pub fn context_gaps(packets: &[EvidencePacket]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for gap in packets
        .iter()
        .filter(|p| p.request.obligation_id.is_none())
        .flat_map(|p| &p.gaps)
        .filter(|g| g.starts_with(REQUIRED_OVER_BUDGET) || unseen(g))
    {
        if !out.contains(gap) {
            out.push(gap.clone());
        }
    }
    out
}

/// Added lines of a reviewed file that no discovery packet shows, as
/// `UNSHOWN_CHANGE: path:a-b`, one run per hunk: coverage cannot be complete over changed code no
/// call saw. Files the plan already reports (generated, unreviewed, excluded) are left to it.
// shortcut: HEAD side only; a removed span that failed to read is not counted.
pub fn unshown_changes(
    manifest: &DiffManifest,
    plan: &ReviewPlan,
    packets: &[EvidencePacket],
) -> Vec<String> {
    let reported = |path: &str| {
        plan.unreviewed
            .iter()
            .chain(&plan.scope.excluded)
            .any(|g| g.path.as_str() == path)
    };
    let shown: Vec<_> = packets
        .iter()
        .filter(|p| p.request.obligation_id.is_none())
        .flat_map(|p| &p.fragments)
        .map(|f| &f.code_ref)
        .filter(|r| r.side == SourceSide::Head)
        .collect();
    let mut out = Vec::new();
    for file in manifest.files.iter().filter(|f| !f.generated) {
        let Some(new) = (file.new.as_ref())
            .filter(|n| n.content == ContentKind::Text && !reported(n.path.as_str()))
        else {
            continue;
        };
        for hunk in file.hunks.iter().filter(|h| h.new_lines > 0) {
            let lines = hunk.new_start..hunk.new_start + hunk.new_lines;
            let unshown: Vec<u32> = lines
                .filter(|&l| {
                    !shown
                        .iter()
                        .any(|r| r.path == new.path && r.start_line <= l && l <= r.end_line)
                })
                .collect();
            if let (Some(first), Some(last)) = (unshown.first(), unshown.last()) {
                out.push(format!("{UNSHOWN_CHANGE}: {}:{first}-{last}", new.path));
            }
        }
    }
    out
}
