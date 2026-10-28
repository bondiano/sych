//! Obligation assessment (spec §16.3): the model proposes a status per criterion; the engine
//! accepts SUPPORTED or VIOLATED only when the criteria and their anchors carry it.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::context::{Anchor, Context};
use crate::{
    Applicability, Assessment, CodeRef, EvidenceId, ObligationId, ObligationStatus, PathClass,
    PlannedObligation, SnapshotReader, SourceSide, classify,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ProposedStatus {
    Supported,
    Violated,
    Unknown,
    NotApplicable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum CriterionStatus {
    Met,
    NotMet,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CriterionResult {
    /// `c-N` from the task.
    pub criterion_id: String,
    pub status: CriterionStatus,
    pub anchors: Vec<Anchor>,
    pub evidence_ids: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AssessmentOutput {
    pub obligation_id: String,
    pub applicability: Applicability,
    pub proposed_status: ProposedStatus,
    pub criterion_results: Vec<CriterionResult>,
    pub evidence_ids: Vec<String>,
    /// Short causal explanation, not hidden reasoning.
    pub rationale: String,
    pub limitations: Vec<String>,
    /// What evidence would settle an UNKNOWN.
    pub missing_inputs: Vec<String>,
    /// Checkable basis, only with `NOT_APPLICABLE`.
    pub not_applicable_basis: Option<String>,
}

/// One criterion as the engine accepted it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CriterionRecord {
    pub criterion: String,
    pub status: CriterionStatus,
    pub anchors: Vec<CodeRef>,
    pub evidence_ids: Vec<EvidenceId>,
}

/// The trace from a required obligation to criteria, evidence and limitations.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AssessmentRecord {
    pub obligation_id: ObligationId,
    pub rule_id: String,
    pub proposed_status: Option<ProposedStatus>,
    pub criteria: Vec<CriterionRecord>,
    pub rationale: String,
    pub missing_inputs: Vec<String>,
    /// Why the proposal was not taken as-is.
    pub problems: Vec<String>,
    pub assessment: Assessment,
}

/// An UNKNOWN that names its reason and what would settle it.
pub fn unknown(planned: &PlannedObligation, reason_code: &str, next: &str) -> Assessment {
    Assessment {
        rule_id: planned.obligation.rule_id.clone(),
        status: ObligationStatus::Unknown {
            reason_code: reason_code.into(),
            next_evidence_needed: next.into(),
        },
        evidence_ids: Vec::new(),
        limitations: Vec::new(),
    }
}

/// The finalization for an obligation no model answered: failure, budget or no route.
pub fn not_assessed(planned: &PlannedObligation, reason_code: &str, why: &str) -> AssessmentRecord {
    let mut assessment = unknown(
        planned,
        reason_code,
        &format!(
            "an assessment of: {}",
            planned.obligation.acceptance_criterion
        ),
    );
    assessment.limitations.push(why.to_owned());
    AssessmentRecord {
        obligation_id: planned.obligation.id,
        rule_id: planned.obligation.rule_id.clone(),
        proposed_status: None,
        criteria: Vec::new(),
        rationale: String::new(),
        missing_inputs: Vec::new(),
        problems: vec![why.to_owned()],
        assessment,
    }
}

/// Checks the proposal for `planned`, shown as `ob_id`, against its criteria and the
/// handed-out spans.
pub fn validate(
    output: AssessmentOutput,
    planned: &PlannedObligation,
    ob_id: &str,
    context: &Context,
    reader: &dyn SnapshotReader,
) -> AssessmentRecord {
    // Identity and reference errors break the answer's contract, unlike incomplete criteria.
    let mut invalid = Vec::new();
    let mut problems = Vec::new();
    if output.obligation_id != ob_id {
        invalid.push(format!(
            "answered `{}` instead of {ob_id}",
            output.obligation_id
        ));
    }

    let mut evidence_ids = context.evidence_ids(&output.evidence_ids, &mut invalid);
    let mut criteria = Vec::new();
    for (i, criterion) in planned.criteria.iter().enumerate() {
        let id = format!("c-{}", i + 1);
        // A repeated id is a contradiction the reducer could not see; it stays unknown.
        let mut found = output
            .criterion_results
            .iter()
            .filter(|r| r.criterion_id == id);
        let (Some(result), None) = (found.next(), found.next()) else {
            problems.push(format!("CRITERIA_INCOMPLETE: no single result for {id}"));
            criteria.push(CriterionRecord {
                criterion: criterion.clone(),
                status: CriterionStatus::Unknown,
                anchors: Vec::new(),
                evidence_ids: Vec::new(),
            });
            continue;
        };
        let anchors = context.anchors(&result.anchors, reader, &mut invalid);
        let cited = context.evidence_ids(&result.evidence_ids, &mut invalid);
        let mut status = result.status;
        if status != CriterionStatus::Unknown && anchors.is_empty() {
            problems.push(format!("{id} is {status:?} without an anchor"));
            status = CriterionStatus::Unknown;
        }

        for e in &cited {
            if !evidence_ids.contains(e) {
                evidence_ids.push(*e);
            }
        }
        criteria.push(CriterionRecord {
            criterion: criterion.clone(),
            status,
            anchors,
            evidence_ids: cited,
        });
    }
    let unknown_ids: Vec<&str> = output
        .criterion_results
        .iter()
        .map(|r| r.criterion_id.as_str())
        .filter(|id| {
            id.strip_prefix("c-")
                .and_then(|n| n.parse::<usize>().ok())
                .is_none_or(|n| n == 0 || n > planned.criteria.len())
        })
        .collect();
    if !unknown_ids.is_empty() {
        invalid.push(format!("unknown criterion ids {unknown_ids:?}"));
    }

    // Anchors cite the source spans the prompt showed, so that evidence is cited with them.
    if criteria.iter().any(|c| !c.anchors.is_empty())
        && let Some(source) = context.evidence_id("ev-1")
        && !evidence_ids.contains(&source)
    {
        evidence_ids.push(source);
    }

    let status = if invalid.is_empty() {
        decide(&output, planned, &criteria, &mut problems)
    } else {
        ObligationStatus::Unknown {
            reason_code: "INVALID_REFERENCE".into(),
            next_evidence_needed: format!(
                "an assessment of {ob_id} that cites only the shown spans and evidence"
            ),
        }
    };
    problems.splice(0..0, invalid);
    let mut limitations = output.limitations.clone();
    limitations.extend(problems.iter().map(|p| format!("engine: {p}")));
    AssessmentRecord {
        obligation_id: planned.obligation.id,
        rule_id: planned.obligation.rule_id.clone(),
        proposed_status: Some(output.proposed_status),
        criteria,
        rationale: output.rationale,
        missing_inputs: output.missing_inputs,
        problems,
        assessment: Assessment {
            rule_id: planned.obligation.rule_id.clone(),
            status,
            evidence_ids,
            limitations,
        },
    }
}

/// A violation whose every HEAD anchor of a `NOT_MET` criterion lies in changed code the
/// snapshot does not show (`unseen`, see `broker::UNSEEN_GAPS`) rests on behaviour nobody
/// saw: it becomes `UNKNOWN/SEMANTIC_GAP`. One shown elsewhere in the change stands.
pub fn unseen_violation(mut record: AssessmentRecord, unseen: &[&CodeRef]) -> AssessmentRecord {
    if record.assessment.status != ObligationStatus::Violated {
        return record;
    }

    let inside = |a: &CodeRef| {
        unseen.iter().any(|u| {
            u.side == a.side
                && u.path == a.path
                && u.start_line <= a.end_line
                && a.start_line <= u.end_line
        })
    };
    let head: Vec<&CodeRef> = record
        .criteria
        .iter()
        .filter(|c| c.status == CriterionStatus::NotMet)
        .flat_map(|c| &c.anchors)
        .filter(|a| a.side == SourceSide::Head)
        .collect();
    if head.is_empty() || !head.iter().all(|a| inside(a)) {
        return record;
    }

    let why = "SEMANTIC_GAP: the violation rests only on changed code the snapshot does not show";
    record.problems.push(why.into());
    record.assessment.limitations.push(format!("engine: {why}"));
    record.assessment.status = ObligationStatus::Unknown {
        reason_code: "SEMANTIC_GAP".into(),
        next_evidence_needed: "analyse the gated code in its build, the macro's expansion or \
                               the type's Debug impl"
            .into(),
    };
    record
}

/// The status the criteria allow; anything weaker than the proposal becomes UNKNOWN.
fn decide(
    output: &AssessmentOutput,
    planned: &PlannedObligation,
    criteria: &[CriterionRecord],
    problems: &mut Vec<String>,
) -> ObligationStatus {
    let next = if output.missing_inputs.is_empty() {
        format!("evidence for: {}", planned.obligation.acceptance_criterion)
    } else {
        output.missing_inputs.join("; ")
    };
    let unknown = |reason: &str| ObligationStatus::Unknown {
        reason_code: reason.into(),
        next_evidence_needed: next.clone(),
    };
    let all_met = criteria.iter().all(|c| c.status == CriterionStatus::Met);
    let any_not_met = criteria.iter().any(|c| c.status == CriterionStatus::NotMet);
    let incomplete = problems
        .iter()
        .any(|p| p.starts_with("CRITERIA_INCOMPLETE"));
    match output.proposed_status {
        ProposedStatus::Supported if any_not_met => {
            problems.push("SUPPORTED with a NOT_MET criterion".into());
            unknown("CONTRADICTORY_ASSESSMENT")
        }
        ProposedStatus::Supported if incomplete || !all_met => unknown("CRITERIA_NOT_MET"),
        // A test the author wrote is not the accepted requirement (INV-19), unless the
        // requirement is about the tests themselves.
        ProposedStatus::Supported if !planned.about_tests && test_oracle_only(criteria) => {
            problems.push("every MET criterion rests on test files only".into());
            unknown("TEST_ORACLE_ONLY")
        }
        ProposedStatus::Supported => ObligationStatus::Supported,
        ProposedStatus::Violated if any_not_met => ObligationStatus::Violated,
        ProposedStatus::Violated => {
            problems.push("VIOLATED without a NOT_MET criterion".into());
            unknown("CONTRADICTORY_ASSESSMENT")
        }
        ProposedStatus::NotApplicable => match &output.not_applicable_basis {
            Some(basis) if !basis.trim().is_empty() => ObligationStatus::NotApplicable {
                basis: basis.clone(),
            },
            _ => {
                problems.push("NOT_APPLICABLE without a basis".into());
                unknown("APPLICABILITY_UNKNOWN")
            }
        },
        ProposedStatus::Unknown => unknown(if output.applicability == Applicability::Unknown {
            "APPLICABILITY_UNKNOWN"
        } else {
            "MODEL_UNKNOWN"
        }),
    }
}

fn test_oracle_only(criteria: &[CriterionRecord]) -> bool {
    let anchors: Vec<&CodeRef> = criteria.iter().flat_map(|c| &c.anchors).collect();
    !anchors.is_empty()
        && anchors
            .iter()
            .all(|r| classify(r.path.as_str(), false) == PathClass::Test)
}
