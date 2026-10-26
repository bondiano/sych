//! Candidate verification (spec §16.3): every candidate ends CONFIRMED, REJECTED or
//! UNRESOLVED, and only a confirmed one with an admissible basis becomes a finding.

use std::cmp::Reverse;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::context::{Anchor, Context};
use super::discovery::ContextAsk;
use crate::{
    Candidate, CodeRef, Finding, FindingCategory, FindingId, Lifecycle, Severity, SnapshotReader,
    Support, VerificationStatus, derived_uuid, sha256,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Verdict {
    Confirmed,
    Rejected,
    Unresolved,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct VerifyOutput {
    pub verdicts: Vec<CandidateVerdict>,
    /// Code a verdict needs and the spans do not show; answered once, then the unresolved
    /// candidates are verified again.
    pub context_requests: Vec<ContextAsk>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CandidateVerdict {
    /// `cand-N` from the task.
    pub candidate_id: String,
    pub verdict: Verdict,
    pub category: FindingCategory,
    pub severity: Severity,
    pub support: Support,
    pub title: String,
    /// One or two sentences; conditions and consequence are the candidate's.
    pub explanation: String,
    /// For `STATIC_TRACE`: one anchor per step from the change to the consequence.
    pub anchors: Vec<Anchor>,
    pub evidence_ids: Vec<String>,
    /// Wrappers, guards, recovery paths or intentional changes that were looked for.
    pub alternatives_checked: Vec<String>,
    pub suggested_action: String,
    pub validation_expectation: String,
    pub limitations: Vec<String>,
}

/// Verified candidates and the findings among them.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Verified {
    pub candidates: Vec<Candidate>,
    pub findings: Vec<Finding>,
}

/// Locations an existing diagnostic already reports; overlap is not novelty.
pub type KnownDiagnostics<'a> = &'a [CodeRef];

fn overlaps(a: &CodeRef, b: &CodeRef) -> bool {
    a.side == b.side && a.path == b.path && a.start_line <= b.end_line && b.start_line <= a.end_line
}

/// Whether `verdict` may confirm a finding; the problem otherwise.
fn admissible(verdict: &CandidateVerdict, anchors: &[CodeRef]) -> Result<(), String> {
    if anchors.is_empty() {
        return Err("no anchor in the shown spans".into());
    }

    let preference = matches!(
        verdict.category,
        FindingCategory::Nit | FindingCategory::Improvement
    );
    match verdict.support {
        Support::ExecutedCounterexample => {
            Err("nothing was executed, so an executed counterexample cannot be the basis".into())
        }
        Support::Preference if !preference => {
            Err("a preference cannot support a defect or rule violation".into())
        }
        _ if preference && verdict.severity > Severity::Low => {
            Err("a nit or improvement is at most LOW".into())
        }
        // A reproducible scenario or a concrete trace, never model agreement (spec §16.3).
        _ if verdict.severity >= Severity::High
            && (verdict.support != Support::StaticTrace || anchors.len() < 2) =>
        {
            Err("HIGH_RISK_WITHOUT_TRACE: HIGH or CRITICAL needs a STATIC_TRACE of at least two anchors".into())
        }
        _ => Ok(()),
    }
}

/// Applies verdicts to `candidates` (shown as `cand-N` in order). A candidate without a
/// verdict stays UNRESOLVED; a rejected nit is dropped, not escalated.
pub fn validate(
    output: &VerifyOutput,
    mut candidates: Vec<Candidate>,
    context: &Context,
    reader: &dyn SnapshotReader,
    known: KnownDiagnostics<'_>,
    scope: &str,
) -> Verified {
    let mut findings: Vec<Finding> = Vec::new();
    for (i, candidate) in candidates.iter_mut().enumerate() {
        let id = format!("cand-{}", i + 1);
        let mut found = output.verdicts.iter().filter(|v| v.candidate_id == id);
        let (Some(verdict), None) = (found.next(), found.next()) else {
            candidate.verification_status = VerificationStatus::Unresolved;
            candidate
                .limitations
                .push(format!("verifier gave no single verdict for {id}"));
            continue;
        };
        let mut problems = Vec::new();
        let anchors = context.anchors(&verdict.anchors, reader, &mut problems);
        let evidence_ids = context.evidence_ids(&verdict.evidence_ids, &mut problems);
        candidate
            .limitations
            .extend(verdict.limitations.iter().cloned());
        candidate.verification_status = match verdict.verdict {
            Verdict::Rejected => VerificationStatus::Rejected,
            Verdict::Unresolved => VerificationStatus::Unresolved,
            // A reference that does not resolve breaks the answer's contract; dropping it
            // and confirming the rest would rewrite the model's proof.
            Verdict::Confirmed if !problems.is_empty() => VerificationStatus::Unresolved,
            Verdict::Confirmed => match admissible(verdict, &anchors) {
                Err(problem) => {
                    problems.push(problem);
                    VerificationStatus::Unresolved
                }
                Ok(()) if anchors.iter().any(|a| known.iter().any(|k| overlaps(a, k))) => {
                    problems.push(
                        "DUPLICATES_EXISTING_DIAGNOSTIC: an imported diagnostic already covers \
                         this location; no comparable baseline shows it is new"
                            .into(),
                    );
                    VerificationStatus::Unresolved
                }
                Ok(()) => VerificationStatus::Confirmed,
            },
        };
        candidate
            .limitations
            .extend(problems.iter().map(|p| format!("engine: {p}")));
        if candidate.verification_status != VerificationStatus::Confirmed {
            continue;
        }

        let finding = finding(candidate, verdict, anchors, evidence_ids, scope);
        if findings
            .iter()
            .any(|f| f.fingerprint == finding.fingerprint)
        {
            candidate
                .limitations
                .push("engine: duplicate of a finding already confirmed in this run".into());
            continue;
        }

        findings.push(finding);
    }
    Verified {
        candidates,
        findings,
    }
}

/// One finding per location: confirmed findings whose anchors overlap are the same defect
/// found by several passes. The most severe is kept and names what it absorbed.
pub fn merge_overlapping(mut findings: Vec<Finding>) -> Vec<Finding> {
    findings.sort_by_key(|f| Reverse(f.severity));
    let mut kept: Vec<Finding> = Vec::new();
    for f in findings {
        let same = kept.iter_mut().find(|k| {
            k.anchors
                .iter()
                .any(|a| f.anchors.iter().any(|b| overlaps(a, b)))
        });
        match same {
            Some(k) => {
                k.limitations
                    .push(format!("merged an overlapping hypothesis: {}", f.title));
                for id in f.obligation_ids {
                    if !k.obligation_ids.contains(&id) {
                        k.obligation_ids.push(id);
                    }
                }

                for id in f.evidence_ids {
                    if !k.evidence_ids.contains(&id) {
                        k.evidence_ids.push(id);
                    }
                }
            }
            None => kept.push(f),
        }
    }
    kept
}

fn finding(
    candidate: &Candidate,
    verdict: &CandidateVerdict,
    anchors: Vec<CodeRef>,
    evidence_ids: Vec<crate::EvidenceId>,
    scope: &str,
) -> Finding {
    let first = &anchors[0];
    // The claim kind and the anchored code itself, not its line number or wording: the same
    // problem keeps its fingerprint when lines above it move.
    let key = format!(
        "{:?}\0{}\0{}\0{}",
        verdict.category,
        first.path,
        first.end_line - first.start_line,
        first.content_sha256
    );
    let fingerprint = sha256(key.as_bytes());
    Finding {
        id: FindingId(derived_uuid(&["finding", &fingerprint.to_string()])),
        category: verdict.category,
        severity: verdict.severity,
        support: verdict.support,
        title: verdict.title.clone(),
        explanation: verdict.explanation.clone(),
        triggering_conditions: candidate.conditions.clone(),
        consequence: candidate.consequences.join("; "),
        anchors,
        evidence_ids: evidence_ids
            .into_iter()
            .chain(candidate.evidence_ids.iter().copied())
            .fold(Vec::new(), |mut all, id| {
                if !all.contains(&id) {
                    all.push(id);
                }

                all
            }),
        rule_id: None,
        obligation_ids: candidate.obligation_ids.clone(),
        suggested_action: verdict.suggested_action.clone(),
        validation_expectation: verdict.validation_expectation.clone(),
        fingerprint,
        predecessor_finding_id: None,
        lifecycle: Lifecycle::Open,
        scope: scope.to_owned(),
        limitations: {
            let mut l = vec![
                "CONFIRMED by the verification policy on the shown spans; not a proof of the \
                 whole program"
                    .to_owned(),
            ];
            l.extend(verdict.limitations.iter().cloned());
            l
        },
    }
}
