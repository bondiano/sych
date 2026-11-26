//! The portable report view. Source excerpts and patches are filled from pinned Git objects
//! by the local CLI. The stored review result keeps its own wire contract.

use std::cmp::Reverse;
use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{
    AlsoFinding, CandidateId, CodeRef, Coverage, ExecutionOutcome, Finding, FindingCategory,
    FindingId, Lifecycle, ObligationId, ObligationStatus, PathGap, Recommendation, ResultId,
    ReviewResult, RunId, SchemaVersion, Severity, SourceSnapshot, Support, UsageSummary,
    VerificationStatus, redact,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ReportView {
    pub schema_version: SchemaVersion,
    pub header: ReportHeader,
    pub findings: Vec<ReportFinding>,
    pub gaps: ReportGaps,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ReportHeader {
    pub run_id: RunId,
    pub result_id: ResultId,
    pub review_revision: u64,
    /// Absent after the source snapshot is pruned.
    pub snapshot: Option<SourceSnapshot>,
    pub recommendation: Recommendation,
    pub coverage: Coverage,
    pub execution_outcome: ExecutionOutcome,
    /// Nonzero counts, grouped by severity and category.
    pub counts: Vec<ReportCount>,
    pub usage: UsageSummary,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ReportCount {
    pub severity: Severity,
    pub category: FindingCategory,
    pub count: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ReportFinding {
    pub id: FindingId,
    pub category: FindingCategory,
    pub severity: Severity,
    pub lifecycle: Lifecycle,
    pub verification_status: VerificationStatus,
    pub support: Support,
    pub title: String,
    pub explanation: String,
    pub triggering_conditions: Vec<String>,
    pub consequence: String,
    pub anchors: Vec<ReportAnchor>,
    pub also: Vec<AlsoFinding>,
    pub suggested_action: String,
    pub validation_expectation: String,
    pub limitations: Vec<String>,
    pub rule_id: Option<String>,
    pub remediation: Option<String>,
    /// Unified diff for the suggested fix when its pinned source is available.
    pub suggested_patch: Option<String>,
    /// Why a stored suggestion could not be shown or exported.
    pub fix_note: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ReportAnchor {
    pub reference: CodeRef,
    /// Absent when the source is unavailable.
    pub excerpt: Option<ReportExcerpt>,
    /// Nearby hunks of the reviewed change.
    pub diff: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ReportExcerpt {
    pub first_line: u32,
    /// The owning anchor's inclusive start and end mark the relevant lines in this text.
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ReportGaps {
    pub unknown_obligations: Vec<ReportUnknown>,
    pub paths: Vec<PathGap>,
    pub open_questions: Vec<String>,
    pub unresolved_candidates: Vec<ReportUnresolved>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ReportUnknown {
    pub id: ObligationId,
    pub rule_id: String,
    pub question: String,
    pub required: bool,
    pub reason_code: String,
    pub next_evidence_needed: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ReportUnresolved {
    pub id: CandidateId,
    pub verification_status: VerificationStatus,
    pub limitations: Vec<String>,
}

fn category_order(category: FindingCategory) -> u8 {
    match category {
        FindingCategory::Defect => 0,
        FindingCategory::RuleViolation => 1,
        FindingCategory::Improvement => 2,
        FindingCategory::Nit => 3,
    }
}

/// Stable fix-first order. Findings have passed verification or a deterministic check.
pub fn ordered_findings(findings: &[Finding]) -> Vec<&Finding> {
    let mut ordered: Vec<_> = findings.iter().collect();
    ordered.sort_unstable_by_key(|f| {
        let location = f.anchors.first().map(|a| (&a.path, a.start_line));
        (
            Reverse(f.severity),
            category_order(f.category),
            location.is_none(),
            location,
            f.lifecycle != Lifecycle::Open,
            f.id,
        )
    });
    ordered
}

fn prose(text: &str) -> String {
    redact(text).into_owned()
}

fn prose_list(texts: &[String]) -> Vec<String> {
    texts.iter().map(|text| prose(text)).collect()
}

fn related(finding: &AlsoFinding) -> AlsoFinding {
    AlsoFinding {
        category: finding.category,
        severity: finding.severity,
        title: prose(&finding.title),
        explanation: prose(&finding.explanation),
        triggering_conditions: prose_list(&finding.triggering_conditions),
        consequence: prose(&finding.consequence),
        anchors: finding.anchors.clone(),
    }
}

impl From<&Finding> for ReportFinding {
    fn from(finding: &Finding) -> Self {
        Self {
            id: finding.id,
            category: finding.category,
            severity: finding.severity,
            lifecycle: finding.lifecycle,
            verification_status: VerificationStatus::Confirmed,
            support: finding.support,
            title: prose(&finding.title),
            explanation: prose(&finding.explanation),
            triggering_conditions: prose_list(&finding.triggering_conditions),
            consequence: prose(&finding.consequence),
            anchors: finding
                .anchors
                .iter()
                .map(|reference| ReportAnchor {
                    reference: reference.clone(),
                    excerpt: None,
                    diff: None,
                })
                .collect(),
            also: finding.also.iter().map(related).collect(),
            suggested_action: prose(&finding.suggested_action),
            validation_expectation: prose(&finding.validation_expectation),
            limitations: prose_list(&finding.limitations),
            rule_id: finding.rule_id.as_ref().map(|id| prose(id)),
            remediation: None,
            suggested_patch: None,
            fix_note: None,
        }
    }
}

fn counts(findings: &[Finding]) -> Vec<ReportCount> {
    let mut counts = BTreeMap::new();
    for finding in findings {
        let key = (Reverse(finding.severity), category_order(finding.category));
        let row = counts.entry(key).or_insert(ReportCount {
            severity: finding.severity,
            category: finding.category,
            count: 0,
        });
        row.count += 1;
    }
    counts.into_values().collect()
}

impl From<&ReviewResult> for ReportGaps {
    fn from(result: &ReviewResult) -> Self {
        let unknown_obligations = result
            .obligations
            .iter()
            .filter_map(|obligation| {
                let ObligationStatus::Unknown {
                    reason_code,
                    next_evidence_needed,
                } = &obligation.status
                else {
                    return None;
                };
                Some(ReportUnknown {
                    id: obligation.id,
                    rule_id: prose(&obligation.rule_id),
                    question: prose(&obligation.question),
                    required: obligation.required,
                    reason_code: prose(reason_code),
                    next_evidence_needed: prose(next_evidence_needed),
                })
            })
            .collect();
        Self {
            unknown_obligations,
            paths: result
                .excluded_or_unreviewed
                .iter()
                .map(|gap| PathGap {
                    path: gap.path.clone(),
                    reason: prose(&gap.reason),
                })
                .collect(),
            open_questions: prose_list(&result.unresolved_questions),
            unresolved_candidates: result
                .unresolved_candidates
                .iter()
                .map(|candidate| ReportUnresolved {
                    id: candidate.id,
                    verification_status: candidate.verification_status,
                    limitations: prose_list(&candidate.limitations),
                })
                .collect(),
        }
    }
}

impl ReportView {
    /// Builds the report metadata. The caller fills source material from the pinned snapshot.
    pub fn from_result(result: &ReviewResult, snapshot: Option<&SourceSnapshot>) -> Self {
        Self {
            schema_version: SchemaVersion,
            header: ReportHeader {
                run_id: result.run_id,
                result_id: result.result_id,
                review_revision: result.review_revision,
                snapshot: snapshot.cloned(),
                recommendation: result.recommendation,
                coverage: result.coverage,
                execution_outcome: result.execution_outcome,
                counts: counts(&result.findings),
                usage: result.usage_summary.clone(),
            },
            findings: ordered_findings(&result.findings)
                .into_iter()
                .map(ReportFinding::from)
                .collect(),
            gaps: ReportGaps::from(result),
        }
    }
}
