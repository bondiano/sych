//! The portable report view. Source excerpts and patches are filled from pinned Git objects
//! by the local CLI. The stored review result keeps its own wire contract.

use std::cmp::Reverse;
use std::collections::BTreeMap;
use std::fmt::Write as _;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::architecture::{ArchitectureFacts, PICTURE};
use crate::{
    AlsoFinding, CandidateId, CodeRef, Coverage, Evidence, ExecutionOutcome, Finding,
    FindingCategory, FindingId, Lifecycle, Obligation, ObligationId, ObligationStatus, PathGap,
    PolicySnapshot, Provenance, Recommendation, ResultId, ReviewLanguage, ReviewResult, RunId,
    SchemaVersion, ScopeManifest, Severity, Sha256Digest, SourceSnapshot, Support, Timestamp,
    UsageSummary, VerificationStatus, redact,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ReportView {
    pub schema_version: SchemaVersion,
    pub header: ReportHeader,
    pub findings: Vec<ReportFinding>,
    pub gaps: ReportGaps,
    /// Combined unified patch of applicable suggested fixes on head.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fixes_patch: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub architecture: Vec<ArchitectureFacts>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub obligations: Vec<Obligation>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub policy: Option<ReportPolicy>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub evidence: Vec<Evidence>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum ReportCodeProvider {
    Github,
    Gitlab,
}

/// Provider blob link coordinates when the run is bound to a remote repository.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ReportCodeLinks {
    pub provider: ReportCodeProvider,
    /// Origin and optional base path without trailing slash, e.g. `https://github.com`.
    pub base_url: String,
    /// `owner/repo` or `group/subgroup/project`.
    pub repo: String,
    pub head: String,
    pub base: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ReportHeader {
    pub run_id: RunId,
    pub result_id: ResultId,
    pub review_revision: u64,
    /// Absent after the source snapshot is pruned.
    pub snapshot: Option<SourceSnapshot>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code_links: Option<ReportCodeLinks>,
    pub recommendation: Recommendation,
    pub coverage: Coverage,
    pub execution_outcome: ExecutionOutcome,
    /// Nonzero counts, grouped by severity and category.
    pub counts: Vec<ReportCount>,
    pub usage: UsageSummary,
    #[serde(default)]
    pub language: ReviewLanguage,
    pub created_at: Timestamp,
    pub provenance: Provenance,
    pub scope: ScopeManifest,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ReportCount {
    pub severity: Severity,
    pub category: FindingCategory,
    pub count: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ReportPolicy {
    pub snapshot: PolicySnapshot,
    pub provenance: Vec<ReportPolicyProvenance>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub proposed_files: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub proposed_rules: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub head_errors: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ReportPolicyProvenance {
    pub layer: String,
    pub origin: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha256: Option<Sha256Digest>,
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
    /// Static architecture box-drawing diagram extracted from `explanation` when present.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub picture: Option<String>,
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

/// Redacts secrets and replaces control and bidi override characters with `U+XXXX`.
pub fn neutralize_prose(text: &str) -> String {
    let redacted = redact(text);
    let mut out = String::with_capacity(redacted.len());
    for ch in redacted.replace("\r\n", "\n").chars() {
        if matches!(ch, '\n' | '\t') {
            out.push(ch);
        } else if ch.is_control()
            || matches!(
                ch,
                '\u{061c}'
                    | '\u{200e}'
                    | '\u{200f}'
                    | '\u{2028}'..='\u{202e}'
                    | '\u{2066}'..='\u{2069}'
            )
        {
            let _ = write!(out, "U+{:04X}", u32::from(ch));
        } else {
            out.push(ch);
        }
    }
    out
}

fn prose(text: &str) -> String {
    neutralize_prose(text)
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

fn extract_picture(explanation: &str) -> Option<String> {
    let (_, rest) = explanation.split_once(PICTURE)?;
    let trimmed = rest.trim_end().trim_end_matches('`').trim_end();
    (!trimmed.is_empty()).then(|| prose(trimmed))
}

impl From<&Finding> for ReportFinding {
    fn from(finding: &Finding) -> Self {
        let explanation = prose(&finding.explanation);
        let picture = extract_picture(&finding.explanation);
        Self {
            id: finding.id,
            category: finding.category,
            severity: finding.severity,
            lifecycle: finding.lifecycle,
            verification_status: VerificationStatus::Confirmed,
            support: finding.support,
            title: prose(&finding.title),
            explanation,
            picture,
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

fn default_code_links(snapshot: &SourceSnapshot) -> Option<ReportCodeLinks> {
    let rest = snapshot.repository.strip_prefix("github:")?;
    let repo = match rest.rsplit_once(':') {
        Some((_, full_name)) => full_name,
        None => rest,
    };
    if !repo.contains('/') {
        return None;
    }

    Some(ReportCodeLinks {
        provider: ReportCodeProvider::Github,
        base_url: "https://github.com".into(),
        repo: prose(repo),
        head: snapshot.head.hex().to_owned(),
        base: snapshot.diff_base.hex().to_owned(),
    })
}

fn clean_obligation(ob: &Obligation) -> Obligation {
    let mut cleaned = ob.clone();
    cleaned.rule_id = prose(&ob.rule_id);
    cleaned.question = prose(&ob.question);
    cleaned.acceptance_criterion = prose(&ob.acceptance_criterion);
    cleaned.limitations = prose_list(&ob.limitations);
    cleaned.status = match &ob.status {
        ObligationStatus::Unknown {
            reason_code,
            next_evidence_needed,
        } => ObligationStatus::Unknown {
            reason_code: prose(reason_code),
            next_evidence_needed: prose(next_evidence_needed),
        },
        ObligationStatus::NotApplicable { basis } => ObligationStatus::NotApplicable {
            basis: prose(basis),
        },
        other => other.clone(),
    };
    cleaned
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
        let mut scope = result.scope_manifest.clone();
        for unit in &mut scope.units {
            unit.title = prose(&unit.title);
        }
        for gap in &mut scope.excluded {
            gap.reason = prose(&gap.reason);
        }

        Self {
            schema_version: SchemaVersion,
            header: ReportHeader {
                run_id: result.run_id,
                result_id: result.result_id,
                review_revision: result.review_revision,
                snapshot: snapshot.cloned(),
                code_links: snapshot.and_then(default_code_links),
                recommendation: result.recommendation,
                coverage: result.coverage,
                execution_outcome: result.execution_outcome,
                counts: counts(&result.findings),
                usage: result.usage_summary.clone(),
                language: result.language,
                created_at: result.created_at,
                provenance: Provenance {
                    engine_version: prose(&result.provenance.engine_version),
                    model_adapters: prose_list(&result.provenance.model_adapters),
                },
                scope,
            },
            findings: ordered_findings(&result.findings)
                .into_iter()
                .map(ReportFinding::from)
                .collect(),
            gaps: ReportGaps::from(result),
            fixes_patch: None,
            architecture: Vec::new(),
            obligations: result.obligations.iter().map(clean_obligation).collect(),
            policy: None,
            evidence: Vec::new(),
        }
    }
}
