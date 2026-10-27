//! Deterministic `ResultReducer` (spec §5.7, §8.2): assessments, evidence and findings become
//! obligation statuses, coverage and recommendation. No model verdict is taken as-is.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::{
    Applicability, Candidate, Completeness, Coverage, Evidence, EvidenceId, EvidenceManifest,
    ExecutionOutcome, Finding, FindingCategory, FindingId, Lifecycle, ObligationStatus, PathGap,
    PlannedObligation, PolicySnapshot, Provenance, Recommendation, RepoPath, ResultId, ReviewPlan,
    ReviewResult, RunId, SchemaVersion, Severity, Support, Timestamp, TrustClass, UsageSummary,
    Validity, VerificationStatus, Waiver, derived_uuid, glob_match, sha256,
};

/// A verifier's answer for one rule's obligation. Unknown fields are rejected (spec §5.1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "RawAssessment")]
pub struct Assessment {
    pub rule_id: String,
    #[serde(flatten)]
    pub status: ObligationStatus,
    #[serde(default)]
    pub evidence_ids: Vec<EvidenceId>,
    #[serde(default)]
    pub limitations: Vec<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
enum StatusTag {
    Supported,
    Violated,
    Unknown,
    NotApplicable,
}

/// Flat wire form; serde cannot deny unknown fields through `flatten`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawAssessment {
    rule_id: String,
    status: StatusTag,
    reason_code: Option<String>,
    next_evidence_needed: Option<String>,
    basis: Option<String>,
    #[serde(default)]
    evidence_ids: Vec<EvidenceId>,
    #[serde(default)]
    limitations: Vec<String>,
}

impl TryFrom<RawAssessment> for Assessment {
    type Error = &'static str;

    fn try_from(raw: RawAssessment) -> Result<Self, Self::Error> {
        let status = match (
            raw.status,
            raw.reason_code,
            raw.next_evidence_needed,
            raw.basis,
        ) {
            (StatusTag::Supported, None, None, None) => ObligationStatus::Supported,
            (StatusTag::Violated, None, None, None) => ObligationStatus::Violated,
            (StatusTag::Unknown, Some(reason_code), Some(next_evidence_needed), None) => {
                ObligationStatus::Unknown {
                    reason_code,
                    next_evidence_needed,
                }
            }
            (StatusTag::NotApplicable, None, None, Some(basis)) if !basis.trim().is_empty() => {
                ObligationStatus::NotApplicable { basis }
            }
            _ => {
                return Err("UNKNOWN needs reason_code and next_evidence_needed, \
                            NOT_APPLICABLE needs basis, other statuses take neither");
            }
        };
        Ok(Self {
            rule_id: raw.rule_id,
            status,
            evidence_ids: raw.evidence_ids,
            limitations: raw.limitations,
        })
    }
}

pub struct ReduceInput<'a> {
    pub plan: &'a ReviewPlan,
    pub assessments: &'a [Assessment],
    pub evidence: &'a [Evidence],
    /// Findings that already passed verification.
    pub findings: &'a [Finding],
    pub candidates: &'a [Candidate],
    /// Planned tasks that did not run or failed, e.g. a truncated discovery pass or required
    /// context over budget; any keeps coverage partial.
    pub task_gaps: &'a [String],
    /// Open model questions; reported, but coverage stays as it is.
    pub questions: &'a [String],
    pub outcome: ExecutionOutcome,
    /// Findings and violations at or above it require attention.
    pub attention_threshold: Severity,
    /// Exceptions expiring at or before it are not in force.
    pub now: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reduced {
    pub obligations: Vec<PlannedObligation>,
    pub findings: Vec<Finding>,
    pub coverage: Coverage,
    pub recommendation: Recommendation,
    pub unresolved_questions: Vec<String>,
    pub excluded_or_unreviewed: Vec<PathGap>,
    pub rejected_candidates: Vec<Candidate>,
}

pub const INSUFFICIENT_EVIDENCE: &str = "INSUFFICIENT_EVIDENCE";
/// Default significance threshold until it becomes configurable.
pub const DEFAULT_ATTENTION_THRESHOLD: Severity = Severity::Medium;
const DEFAULT_VIOLATION_SEVERITY: Severity = Severity::Medium;

/// Why the cited evidence cannot carry the claimed status, if it cannot.
fn evidence_problem(
    planned: &PlannedObligation,
    assessment: &Assessment,
    evidence: &BTreeMap<EvidenceId, &Evidence>,
    plan: &ReviewPlan,
) -> Option<String> {
    if matches!(assessment.status, ObligationStatus::Unknown { .. }) {
        return None;
    }

    if assessment.evidence_ids.is_empty() {
        return Some("no evidence cited".into());
    }

    let mut cited = Vec::new();
    for id in &assessment.evidence_ids {
        let Some(ev) = evidence.get(id) else {
            return Some(format!("evidence {id} is not in the run"));
        };
        if ev.validity != Validity::Valid || ev.source_snapshot_id != plan.source_snapshot_id {
            return Some(format!("evidence {id} is not VALID for this snapshot"));
        }

        cited.push(*ev);
    }
    let kinds = &planned.obligation.acceptable_evidence_kinds;
    let acceptable = |ev: &Evidence| kinds.is_empty() || kinds.contains(&ev.kind);
    if !cited.iter().any(|ev| acceptable(ev)) {
        return Some("no cited evidence is of an acceptable kind".into());
    }

    let supported = matches!(assessment.status, ObligationStatus::Supported);
    // Author-written tests or PR text alone do not establish correctness (INV-19).
    if supported
        && !cited.iter().any(|ev| {
            acceptable(ev)
                && ev.completeness == Completeness::CompleteForQuery
                && ev.trust_class != TrustClass::AuthorClaim
        })
    {
        return Some("SUPPORTED needs complete evidence not resting on author claims".into());
    }

    None
}

fn apply(
    planned: &mut PlannedObligation,
    assessment: &Assessment,
    evidence: &BTreeMap<EvidenceId, &Evidence>,
    plan: &ReviewPlan,
) {
    let ob = &mut planned.obligation;
    ob.limitations
        .extend(assessment.limitations.iter().cloned());
    if let Some(problem) = evidence_problem(planned, assessment, evidence, plan) {
        let ob = &mut planned.obligation;
        ob.limitations
            .push(format!("assessment not accepted: {problem}"));
        ob.status = ObligationStatus::Unknown {
            reason_code: INSUFFICIENT_EVIDENCE.into(),
            next_evidence_needed:
                "VALID evidence of an acceptable kind for the acceptance criterion".into(),
        };
        return;
    }

    let ob = &mut planned.obligation;
    if matches!(assessment.status, ObligationStatus::NotApplicable { .. }) {
        ob.applicability = Applicability::DoesNotApply;
    }

    ob.status = assessment.status.clone();
    ob.evidence_ids.clone_from(&assessment.evidence_ids);
}

fn violation_severity(planned: &PlannedObligation) -> Severity {
    planned
        .severity_on_violation
        .unwrap_or(DEFAULT_VIOLATION_SEVERITY)
}

/// A checked violation is never lost for want of a finding (spec §5.7).
fn rule_violation(
    planned: &PlannedObligation,
    evidence: &BTreeMap<EvidenceId, &Evidence>,
) -> Finding {
    let ob = &planned.obligation;
    let key = ob.id.to_string();
    let anchors = ob
        .evidence_ids
        .iter()
        .filter_map(|id| evidence.get(id))
        .flat_map(|ev| ev.code_refs.iter().cloned())
        .collect();
    Finding {
        id: FindingId(derived_uuid(&["finding", &key])),
        category: FindingCategory::RuleViolation,
        severity: violation_severity(planned),
        support: Support::ExplicitRule,
        title: format!("{} violated: {}", ob.rule_id, planned.rule_title),
        explanation: ob.question.clone(),
        triggering_conditions: Vec::new(),
        consequence: format!("acceptance criterion not met: {}", ob.acceptance_criterion),
        anchors,
        evidence_ids: ob.evidence_ids.clone(),
        rule_id: Some(ob.rule_id.clone()),
        obligation_ids: vec![ob.id],
        suggested_action: "Restore the behavior the rule requires.".into(),
        validation_expectation: ob.acceptance_criterion.clone(),
        fingerprint: sha256(format!("{}\0{}\0{key}", ob.rule_id, ob.rule_version).as_bytes()),
        predecessor_finding_id: None,
        lifecycle: Lifecycle::Open,
        scope: ob
            .review_unit_ids
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(","),
        limitations: Vec::new(),
    }
}

/// Every UNKNOWN obligation, diff omission and unverified candidate stays visible.
fn unresolved_questions(
    obligations: &[PlannedObligation],
    plan: &ReviewPlan,
    candidates: &[Candidate],
    unresolved: &mut Vec<String>,
) {
    for o in obligations {
        if let ObligationStatus::Unknown {
            reason_code,
            next_evidence_needed,
        } = &o.obligation.status
        {
            unresolved.push(format!(
                "{}: {reason_code}; next: {next_evidence_needed}",
                o.obligation.rule_id
            ));
        }
    }
    unresolved.extend(plan.omissions.iter().map(|o| format!("diff omission: {o}")));
    unresolved.extend(
        candidates
            .iter()
            .filter(|c| {
                matches!(
                    c.verification_status,
                    VerificationStatus::Pending | VerificationStatus::Unresolved
                )
            })
            .map(|c| format!("unverified candidate: {}", c.claim)),
    );
}

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
fn waive(
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

pub fn reduce(input: &ReduceInput<'_>) -> Reduced {
    let plan = input.plan;
    let evidence: BTreeMap<EvidenceId, &Evidence> =
        input.evidence.iter().map(|e| (e.id, e)).collect();
    let mut obligations = plan.obligations.clone();
    let mut unresolved = Vec::new();
    for assessment in input.assessments {
        match obligations
            .iter_mut()
            .find(|o| o.obligation.rule_id == assessment.rule_id)
        {
            // A deterministic NOT_APPLICABLE is not reopened by a model answer.
            Some(o) if o.obligation.applicability == Applicability::DoesNotApply => unresolved
                .push(format!(
                    "{}: assessment ignored, rule does not apply to this change",
                    assessment.rule_id
                )),
            Some(o) => apply(o, assessment, &evidence, plan),
            None => unresolved.push(format!(
                "{}: assessment for a rule without an obligation ignored",
                assessment.rule_id
            )),
        }
    }

    let mut findings = input.findings.to_vec();
    for o in &obligations {
        let id = o.obligation.id;
        // Only a finding at least as severe stands for the violation; a linked nit does not.
        if matches!(o.obligation.status, ObligationStatus::Violated)
            && !findings
                .iter()
                .any(|f| f.obligation_ids.contains(&id) && f.severity >= violation_severity(o))
        {
            findings.push(rule_violation(o, &evidence));
        }
    }

    for o in &mut obligations {
        waive(o, &evidence, plan, input.now, &mut unresolved);
    }
    let waived: Vec<_> = obligations
        .iter()
        .filter(|o| !o.obligation.waivers.is_empty())
        .map(|o| o.obligation.id)
        .collect();
    unresolved_questions(&obligations, plan, input.candidates, &mut unresolved);
    unresolved.extend(input.task_gaps.iter().cloned());
    unresolved.extend(input.questions.iter().cloned());

    let required_unknown = obligations.iter().any(|o| {
        o.obligation.required && matches!(o.obligation.status, ObligationStatus::Unknown { .. })
    });
    let coverage = match input.outcome {
        ExecutionOutcome::InputUnavailable => Coverage::Unavailable,
        ExecutionOutcome::LimitReached | ExecutionOutcome::RetriesExhausted => Coverage::Partial,
        ExecutionOutcome::PlanExecuted
            if plan.limit_reached
                || required_unknown
                || !input.task_gaps.is_empty()
                || !plan.unreviewed.is_empty()
                || !plan.omissions.is_empty() =>
        {
            Coverage::Partial
        }
        ExecutionOutcome::PlanExecuted => Coverage::CompleteForScope,
    };
    // A waiver lowers the risk decision only; coverage above still counts the UNKNOWN.
    let blocking_unknown = obligations.iter().any(|o| {
        o.obligation.required
            && o.obligation.waivers.is_empty()
            && matches!(o.obligation.status, ObligationStatus::Unknown { .. })
    });
    let significant = findings.iter().any(|f| {
        f.lifecycle == Lifecycle::Open
            && f.severity >= input.attention_threshold
            && (f.obligation_ids.is_empty()
                || !f.obligation_ids.iter().all(|id| waived.contains(id)))
    });
    let recommendation = if significant {
        Recommendation::AttentionRequired
    } else if blocking_unknown || coverage == Coverage::Unavailable {
        Recommendation::NeedsEvidence
    } else {
        Recommendation::NoBlockingFindingsInScope
    };
    Reduced {
        obligations,
        findings,
        coverage,
        recommendation,
        unresolved_questions: unresolved,
        excluded_or_unreviewed: plan
            .scope
            .excluded
            .iter()
            .chain(&plan.unreviewed)
            .cloned()
            .collect(),
        rejected_candidates: input
            .candidates
            .iter()
            .filter(|c| c.verification_status == VerificationStatus::Rejected)
            .cloned()
            .collect(),
    }
}

/// Identity and bookkeeping of the run that produced the result.
pub struct RunInfo {
    pub run_id: RunId,
    pub result_id: ResultId,
    pub review_revision: u64,
    pub parent_result_id: Option<ResultId>,
    pub usage: UsageSummary,
    pub provenance: Provenance,
    pub created_at: Timestamp,
    pub language: crate::ReviewLanguage,
}

/// Assembles the immutable result; the plan's scope is carried unchanged (spec §7.4).
pub fn finalize(
    run: RunInfo,
    plan: &ReviewPlan,
    policy: &PolicySnapshot,
    evidence_manifest: &EvidenceManifest,
    outcome: ExecutionOutcome,
    reduced: Reduced,
) -> ReviewResult {
    ReviewResult {
        schema_version: SchemaVersion,
        result_id: run.result_id,
        run_id: run.run_id,
        review_revision: run.review_revision,
        parent_result_id: run.parent_result_id,
        source_snapshot_id: plan.source_snapshot_id.clone(),
        policy_snapshot_id: policy.id.clone(),
        evidence_manifest_id: evidence_manifest.id,
        scope_manifest: plan.scope.clone(),
        execution_outcome: outcome,
        coverage: reduced.coverage,
        recommendation: reduced.recommendation,
        findings: reduced.findings,
        obligations: reduced
            .obligations
            .into_iter()
            .map(|o| o.obligation)
            .collect(),
        unresolved_questions: reduced.unresolved_questions,
        excluded_or_unreviewed: reduced.excluded_or_unreviewed,
        rejected_candidates: reduced.rejected_candidates,
        usage_summary: run.usage,
        created_at: run.created_at,
        provenance: run.provenance,
        language: run.language,
    }
}
